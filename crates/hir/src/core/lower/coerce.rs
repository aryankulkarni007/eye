//! The single coercion point (CLEAK fix order step 3).
//!
//! Every site where lowering knows the expected type of an expression funnels
//! through [`LoweringCtx::coerce`]: `let` initializers, call arguments,
//! explicit `return` values, the function tail, struct-literal fields, and
//! (recursively) array-literal elements. Before this module existed, decay
//! and array-literal re-typing were each duplicated at four sites and missing
//! at the last two - which is exactly where the L1/L2 C-leaks lived.
//!
//! `coerce` performs three context-directed adjustments, in order:
//! 1. Array-literal re-typing: a literal's elements default to `int32`; the
//!    expected array type wins, recursively, with the full `coerce` applied
//!    to every element so decay and literal typing work inside aggregates.
//! 2. Integer-literal typing: a bare (possibly negated) integer literal
//!    adopts the expected integer type. The value-vs-type range check (CLEAK
//!    M1) runs once for *all* literals - coerced or defaulted - in the
//!    post-lowering sweep [`LoweringCtx::check_int_literal_ranges`].
//! 3. Decay: a `&[T; N]` value meeting a `&T` / `T*` / `string` expectation
//!    is wrapped in a pointer cast (the kernel string story, HORIZON0 C3).

use ast::UnaryOp;
use rustc_hash::FxHashSet;

use super::LoweringCtx;
use crate::core::{Expr, ExprId, Literal, Text, TypeError, TypeInterner, TypeKind, TypeRef};

/// The value range of a primitive integer type, as `(negative magnitude
/// bound, positive bound)`: a literal `N` must satisfy `N <= max`, a negated
/// literal `-N` must satisfy `N <= neg`. `usize`/`isize` use 64-bit ranges:
/// the C backend maps them to `size_t`/`ptrdiff_t` and every supported target
/// (64-bit macOS/Linux) is LP64. A 32-bit target would need these bounds to
/// come from a target description instead. `None` for any non-integer name.
fn int_type_range(name: &str) -> Option<(u128, u128)> {
    Some(match name {
        "int8" => (1 << 7, (1 << 7) - 1),
        "int16" => (1 << 15, (1 << 15) - 1),
        "int32" => (1 << 31, (1 << 31) - 1),
        "int64" | "isize" => (1 << 63, (1 << 63) - 1),
        "uint8" => (0, (1 << 8) - 1),
        "uint16" => (0, (1 << 16) - 1),
        "uint32" => (0, (1 << 32) - 1),
        "uint64" | "usize" => (0, u64::MAX as u128),
        _ => return None,
    })
}

impl<'a> LoweringCtx<'a> {
    /// Coerce an expression toward a locally-known expected type. Returns the
    /// (possibly rewrapped) expression to use in place of `expr_id`. A pairing
    /// no rule applies to returns the expression unchanged - this is a
    /// coercion point, not a type *checker*; mismatches are diagnosed by the
    /// per-site checks and, eventually, the typeck pass.
    pub(super) fn coerce(&mut self, expected: &TypeRef, expr_id: ExprId) -> ExprId {
        self.coerce_array_literal(expected, expr_id);
        self.adopt_int_literal_type(expected, expr_id);
        self.maybe_decay(expected, expr_id)
    }

    /// Re-type an array literal - and recursively every nested element - onto
    /// a declared array type. A literal's elements default integer literals
    /// to `int32`; the declared type wins (C converts the constants inside
    /// the brace initializer). Each level is length-guarded: a literal whose
    /// length disagrees with the declared length keeps its own type so the
    /// existing length diagnostic fires rather than the wrapper being
    /// reshaped around the wrong element count. Elements go through the full
    /// [`coerce`](Self::coerce), so a decay inside an aggregate
    /// (`let [string; 2] xs = ["a", "b"]`, CLEAK L2) rewraps the element and
    /// is written back into the literal.
    fn coerce_array_literal(&mut self, declared: &TypeRef, init_id: ExprId) {
        let (elem, declared_len) = {
            let types = &self.types;
            match types.lookup(*declared) {
                &TypeKind::Array { elem, len } => (elem, len),
                _ => return,
            }
        };
        if !matches!(
            self.body.exprs[init_id],
            Expr::ArrayLit(_) | Expr::ArrayRepeat { .. }
        ) {
            return;
        }
        let lit_len = match self.body.expr_types.get(init_id.into()).copied() {
            Some(ty) => {
                let types = &self.types;
                match types.lookup(ty) {
                    &TypeKind::Array { len, .. } => len,
                    _ => return,
                }
            }
            None => return,
        };
        if lit_len != declared_len {
            return;
        }
        self.body.expr_types.insert(init_id.into(), *declared);
        // Elements to coerce against the declared element type: every element
        // of a literal, or the single repeated value of `[value; N]`.
        // Collected first to release the borrow on `exprs`; written back only
        // when an element was rewrapped (a decay cast).
        let children: Vec<ExprId> = match &self.body.exprs[init_id] {
            Expr::ArrayLit(elems) => elems.to_vec(),
            Expr::ArrayRepeat { value, .. } => vec![*value],
            _ => return,
        };
        let coerced: Vec<ExprId> = children.iter().map(|&e| self.coerce(&elem, e)).collect();
        if coerced != children {
            match &mut self.body.exprs[init_id] {
                Expr::ArrayLit(elems) => *elems = coerced.into_iter().collect(),
                Expr::ArrayRepeat { value, .. } => *value = coerced[0],
                _ => {}
            }
        }
    }

    /// An integer literal adopts the expected integer type: `let int64 x = 5`
    /// records the literal as `int64`, so the MIR temp and any printf spec
    /// derived from it carry the right width. `-N` lowers to
    /// `Unary(Neg, N)`; both the negation and the literal adopt the type (the
    /// unary's type follows its operand everywhere else). Whether the value
    /// actually fits is judged later by
    /// [`check_int_literal_ranges`](Self::check_int_literal_ranges) - adopting
    /// unconditionally means the declared type is the one the value is judged
    /// against (`let int8 x = 300` reports the `int8` range, not `int32`'s).
    fn adopt_int_literal_type(&mut self, expected: &TypeRef, expr_id: ExprId) {
        {
            let types = &self.types;
            match types.lookup(*expected) {
                TypeKind::Path(name) if int_type_range(name).is_some() => {}
                _ => return,
            }
        }
        match self.body.exprs[expr_id] {
            Expr::Literal(Literal::Int(_)) => {
                self.body.expr_types.insert(expr_id.into(), *expected);
            }
            Expr::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                if matches!(self.body.exprs[operand], Expr::Literal(Literal::Int(_))) {
                    self.body.expr_types.insert(operand.into(), *expected);
                    self.body.expr_types.insert(expr_id.into(), *expected);
                }
            }
            _ => {}
        }
    }

    /// Post-lowering sweep (CLEAK M1): every integer literal's value must fit
    /// the integer type it ended up with - the type a `coerce` site gave it,
    /// or the `int32` literal default. Before this check `let int32 x =
    /// 5000000000;` built successfully and stored 705032704 (clang only warns
    /// on the truncation). Run once per body after all coercion sites, so the
    /// judgment exists in exactly one place. A literal that is the operand of
    /// a negation is checked against the negative bound. Literals with no
    /// recorded type (the synthesized `len`-fold cast operand) are skipped.
    pub(super) fn check_int_literal_ranges(&mut self) {
        let neg_operands: FxHashSet<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(_, e)| match e {
                &Expr::Unary {
                    op: UnaryOp::Neg,
                    operand,
                } => Some(operand),
                _ => None,
            })
            .collect();
        let lits: Vec<(ExprId, u128)> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| match e {
                Expr::Literal(Literal::Int(v)) => Some((id, *v)),
                _ => None,
            })
            .collect();
        for (id, v) in lits {
            let Some(ty) = self.body.expr_types.get(id.into()).copied() else {
                continue;
            };
            let name: Text = {
                let types = &self.types;
                match types.lookup(ty) {
                    TypeKind::Path(n) => n.clone(),
                    _ => continue,
                }
            };
            let Some((neg_mag, max)) = int_type_range(&name) else {
                continue;
            };
            let negated = neg_operands.contains(&id);
            let limit = if negated { neg_mag } else { max };
            if v <= limit {
                continue;
            }
            let Some(ptr) = self.body.source_map.expr.get(id.into()).cloned() else {
                continue;
            };
            let value = if negated {
                format!("-{v}")
            } else {
                v.to_string()
            };
            let min = if neg_mag == 0 {
                "0".to_string()
            } else {
                format!("-{neg_mag}")
            };
            self.emit(
                ptr,
                TypeError::IntLiteralOutOfRange {
                    value,
                    ty: name,
                    min,
                    max: max.to_string(),
                },
            );
        }
    }

    /// Insert an array-reference *decay* when a `&[T; N]` value meets a context
    /// expecting a pointer-to-element (`&T` / `T*` / `string`): wrap the value in
    /// a cast to the expected type. The decay is lowered as a plain pointer cast
    /// because the array wrapper places its `data` at offset 0, so
    /// `(T*)wrapper_ptr` is the element pointer. This is the kernel string story
    /// (`&[uint8; N]` -> `&uint8`/`string`, HORIZON0 C3): length is known at the
    /// literal, erased at the boundary. The cast's type *is* the expected type,
    /// so the normal type check then passes (no symmetric `types_compatible`
    /// relaxation, which would mask real mismatches). A non-decay pairing returns
    /// the expression unchanged.
    fn maybe_decay(&mut self, declared: &TypeRef, expr_id: ExprId) -> ExprId {
        let Some(found) = self.body.expr_types.get(expr_id.into()).cloned() else {
            return expr_id;
        };
        if !Self::array_ref_decays_to(*declared, found, &self.types) {
            return expr_id;
        }
        let Some(ptr) = self.body.source_map.expr.get(expr_id.into()).cloned() else {
            return expr_id;
        };
        self.alloc_expr_with_type(
            Expr::Cast {
                operand: expr_id,
                ty: *declared,
            },
            ptr,
            *declared,
        )
    }

    /// Whether a `&[T; N]` (`found`) decays to `declared`: `declared` is `&T`/`T*`
    /// with the same element type, or `string` (the byte-pointer view of a
    /// `&[uint8; N]`).
    fn array_ref_decays_to(declared: TypeRef, found: TypeRef, types: &TypeInterner) -> bool {
        let &TypeKind::Ref(arr) = types.lookup(found) else {
            return false;
        };
        let &TypeKind::Array { elem, .. } = types.lookup(arr) else {
            return false;
        };
        match types.lookup(declared) {
            TypeKind::Path(n) if n == "string" => {
                matches!(types.lookup(elem), TypeKind::Path(e) if e == "uint8")
            }
            TypeKind::Ref(t) | TypeKind::Ptr(t) => *t == elem,
            _ => false,
        }
    }
}
