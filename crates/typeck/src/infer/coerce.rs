//! the tier-2 funnel and its coercion mirror (formerly lowering's `coerce.rs`).
//! every found-meets-expected meeting passes through [`InferCtx::expect`]: it
//! adopts a literal to the expected width, files the array-decay adjustment MIR
//! reads, and reports the cause-specific mismatch. the adoptions reproduce
//! lowering's old tree mutations as stamps; decay is an `adjustments` entry (S2C
//! C4 - lowering no longer injects the cast node).

use ast::UnaryOp;
use hir::core::{Expr, ExprId, Literal, TypeKind, TypeRef};

use crate::{Adjustment, Cause, Expectation, InferObserver};

use super::InferCtx;
use super::ty::*;

impl<'a, O: InferObserver> InferCtx<'a, O> {
    /// the single found-meets-expected funnel - [`Self::infer_expr`]'s tail (the
    /// tier-2 spine, TYPECK.md). with no expectation the value synthesizes
    /// unchanged; with one it is coerced onto the expected type
    /// ([`Self::coerce_to`]) and, when it still cannot satisfy it, the cause's
    /// mismatch is reported. three rules:
    ///
    /// 1. equal / poison / adopted -> the value's settled type.
    /// 2. an array decay applies -> the adjustment is filed (in `coerce_to`),
    ///    the value keeps its own `&[T; N]`.
    /// 3. a mismatch at an atomic `Arg`/`Field`/`Return` site -> the matching
    ///    `TypeError`. a transparent container (`if`/`match`/block) delegates to
    ///    the branch/arm consistency checks; the other causes are adopt-only and
    ///    own their mismatch elsewhere (the let-init check, the consistency checks).
    pub(crate) fn expect(
        &mut self,
        id: ExprId,
        found: Option<TypeRef>,
        expected: Expectation,
    ) -> Option<TypeRef> {
        let Expectation::HasType(exp, cause) = expected else {
            return found;
        };
        // an `Error` expectation never constrains (poison discipline: no cascade).
        if type_ref_contains_error(exp, self.types) {
            return found;
        }
        // adopt a literal / divergent value to the expected width, re-type a
        // value-position container onto it, coerce an array literal, file decay.
        self.coerce_to(exp, id);
        let Some(found) = self.ty_of(id) else {
            // nothing to type (a `Missing` child): adopt the expectation so a
            // parent reads a concrete slot rather than a hole.
            return Some(exp);
        };
        // a `()` (void) value is the completeness sweep's diagnostic, not a
        // mismatch; `Error` poisons silently.
        if matches!(self.types.lookup(found), TypeKind::Unit)
            || type_ref_contains_error(found, self.types)
        {
            return Some(found);
        }
        // a transparent container delegates its mismatch to the branch/arm
        // consistency checks (which compare against its settled type, restamped
        // just above); a block delegates to its tail (it carried the expectation).
        if matches!(
            self.body.exprs[id],
            Expr::If { .. } | Expr::Match { .. } | Expr::Block(_)
        ) {
            return Some(found);
        }
        match cause {
            Cause::Arg { .. } | Cause::Field { .. } | Cause::Return { .. } => {
                if !self.cause_assignable(&cause, exp, found) {
                    self.emit_mismatch(id, exp, found, &cause);
                }
            }
            // adopt-only: the mismatch (if any) is owned by a separate judgment
            // (the let-init check, the branch/arm consistency checks).
            Cause::LetDecl | Cause::IfBranch | Cause::MatchArm => {}
        }
        Some(found)
    }

    /// coerce one expression onto `expected` (the funnel's adoption step): adopt
    /// a literal / divergent value to the expected width, re-type a
    /// value-position `if`/`match` onto it (MIR reads the hoist temp from this),
    /// coerce an array literal element-wise, and file an array-reference decay.
    /// the branches / arms / elements were already given the expectation during
    /// the downward walk; this re-types the container or leaf node itself. a
    /// block needs no re-type - its value is its tail's, already coerced.
    fn coerce_to(&mut self, expected: TypeRef, id: ExprId) {
        if matches!(self.body.exprs[id], Expr::If { .. } | Expr::Match { .. }) {
            self.restamp_value_node(id, expected);
            return;
        }
        self.coerce_array_literal(expected, id);
        self.adopt_int_literal(expected, id);
        self.adopt_float_literal(expected, id);
        self.adopt_divergent(expected, id);
        self.record_decay(expected, id);
    }

    /// whether `found` satisfies `expected` at this cause's site, using that
    /// site's assignability policy (preserved from the pre-spine per-site
    /// checks). an argument or field accepts the pointer escapes
    /// (`site_assignable`: the `&[T; N]` decay, any pointer widening into the
    /// untyped `ptr`); a return is stricter - the decay and the safe `&T -> T*`
    /// widening, but no `ptr` (void*) widening.
    fn cause_assignable(&self, cause: &Cause, expected: TypeRef, found: TypeRef) -> bool {
        match cause {
            Cause::Arg { .. } | Cause::Field { .. } => site_assignable(expected, found, self.types),
            Cause::Return { .. } => {
                types_compatible(found, expected, self.types)
                    || array_ref_decays_to(expected, found, self.types)
                    || ref_widens_to_ptr(expected, found, self.types)
            }
            // adopt-only causes never reach this check.
            Cause::LetDecl | Cause::IfBranch | Cause::MatchArm => true,
        }
    }

    /// report the cause-specific mismatch `TypeError` for an atomic value that
    /// could not satisfy its expectation (the funnel's third rule). the
    /// adopt-only causes never reach here.
    fn emit_mismatch(&mut self, id: ExprId, expected: TypeRef, found: TypeRef, cause: &Cause) {
        let expected = self.types.display(expected).to_string();
        let found = self.types.display(found).to_string();
        let err: hir::core::TypeError = match cause {
            Cause::Arg { index, decl } => hir::core::TypeError::ArgTypeMismatch {
                index: *index,
                expected,
                found,
                decl: decl.clone(),
            },
            Cause::Field { name, decl } => hir::core::TypeError::StructFieldTypeMismatch {
                field: name.clone(),
                expected,
                found,
                decl: decl.clone(),
            },
            Cause::Return { decl } => hir::core::TypeError::ReturnTypeMismatch {
                expected,
                found,
                decl: decl.clone(),
            },
            Cause::LetDecl | Cause::IfBranch | Cause::MatchArm => return,
        };
        self.emit_at(id, None, err);
    }

    /// re-type an `if`/`match` to the coercion-site's expected type so MIR reads
    /// the hoist temp from it - but only when the node actually yields a value.
    /// a unit `if`/`match` (every branch value-less) is left as `()` so the
    /// completeness sweep rejects it; a never one is left `!` (it diverges, MIR
    /// lowers it in place). without this guard a coercion site would fabricate a
    /// temp of the expected type and silently miscompile the void binding
    /// (`let int32* p = if c { malloc(); } else { NULL; };`).
    fn restamp_value_node(&mut self, id: ExprId, expected: TypeRef) {
        match self.ty_of(id) {
            Some(t) if t != self.types.unit_ty() && t != self.types.never_ty() => {
                self.record(id, expected);
            }
            _ => {}
        }
    }

    /// record an array-reference *decay* (`coerce.rs` `maybe_decay` mirror): a
    /// `&[T; N]` value meeting a `&T` / `string` expectation is read through a
    /// pointer cast. the decay site keeps its own `&[T; N]` type; only the read
    /// is adjusted, so MIR emits `(target)<value>` exactly as lowering's former
    /// cast node did. recorded after the literal mirrors so `ty_of` reads the
    /// pre-coercion type (a string literal stays `&[uint8; N]`, never re-typed).
    fn record_decay(&mut self, expected: TypeRef, id: ExprId) {
        if let Some(found) = self.ty_of(id)
            && array_ref_decays_to(expected, found, self.types)
        {
            self.results
                .adjustments
                .insert(id.into(), Adjustment::Decay(expected));
        }
    }

    /// a value-position divergent expression (a `Never`-typed `loop`/`return`/
    /// `break`/`continue`) never produces a value: MIR lowers it as a statement
    /// and yields poison `0` in its place. re-type it from `!` to the expected
    /// type at the coercion site so MIR types the poison temp concretely (a bare
    /// `!` temp would emit `void _t`). the value is never read, so the adopted
    /// type is purely a placeholder for the unreachable slot.
    fn adopt_divergent(&mut self, expected: TypeRef, id: ExprId) {
        if (self.ty_of(id).is_none() || self.is_never(id))
            && matches!(
                self.body.exprs[id],
                Expr::Loop { .. } | Expr::Return(_) | Expr::Break | Expr::Continue
            )
        {
            self.record(id, expected);
        }
    }

    fn coerce_array_literal(&mut self, declared: TypeRef, id: ExprId) {
        let &TypeKind::Array {
            elem,
            len: declared_len,
        } = self.types.lookup(declared)
        else {
            return;
        };
        if !matches!(
            self.body.exprs[id],
            Expr::ArrayLit(_) | Expr::ArrayRepeat { .. }
        ) {
            return;
        }
        let Some(lit_len) = self.ty_of(id).and_then(|t| match self.types.lookup(t) {
            &TypeKind::Array { len, .. } => Some(len),
            _ => None,
        }) else {
            return;
        };
        if lit_len != declared_len {
            return;
        }
        self.record(id, declared);
        let children: Vec<ExprId> = match &self.body.exprs[id] {
            Expr::ArrayLit(elems) => elems.to_vec(),
            Expr::ArrayRepeat { value, .. } => vec![*value],
            _ => return,
        };
        for (i, child) in children.into_iter().enumerate() {
            // the element was given no expectation during the walk (the array
            // literal's element type is only known here); coerce it onto the
            // declared element type now.
            self.coerce_to(elem, child);
            // L4 (S3): per-element value judgment against the declared element
            // type (`[1, true, "x"]` against `[int32; 3]`). runs after the
            // coercion so an adopted literal / decayed ref is not falsely
            // flagged; the same `site_assignable` the arg/field judgments use.
            if let Some(found) = self.ty_of(child)
                && !site_assignable(elem, found, self.types)
            {
                let expected = self.types.display(elem).to_string();
                let got = self.types.display(found).to_string();
                self.emit_at(
                    child,
                    None,
                    hir::core::TypeError::ArrayElementTypeMismatch {
                        index: i,
                        expected,
                        found: got,
                    },
                );
            }
        }
    }

    /// whether an expectation imposes an array type. when it does, the array
    /// literal's elements are judged against the declared element type by
    /// [`Self::coerce_array_literal`] at the funnel, so the synth-time
    /// homogeneity check is skipped (each bad element reported once).
    pub(crate) fn expects_array(&self, expected: &Expectation) -> bool {
        matches!(
            expected,
            Expectation::HasType(t, _) if matches!(self.types.lookup(*t), TypeKind::Array { .. })
        )
    }

    /// an array literal has a single element type: every element must agree
    /// with the first. this is the no-declaration counterpart to
    /// [`Self::coerce_array_literal`]'s vs-declared judgment - it catches a
    /// heterogeneous literal in a position with no expected type
    /// (`let xs = [1, "two"]`), which would otherwise synthesize `[int32; 2]`
    /// and silently accept the mismatched element.
    pub(crate) fn check_array_homogeneous(&mut self, elems: &[ExprId], first_ty: TypeRef) {
        for (i, &child) in elems.iter().enumerate().skip(1) {
            if let Some(found) = self.ty_of(child)
                && !site_assignable(first_ty, found, self.types)
            {
                let expected = self.types.display(first_ty).to_string();
                let got = self.types.display(found).to_string();
                self.emit_at(
                    child,
                    None,
                    hir::core::TypeError::ArrayElementTypeMismatch {
                        index: i,
                        expected,
                        found: got,
                    },
                );
            }
        }
    }

    fn adopt_int_literal(&mut self, expected: TypeRef, id: ExprId) {
        match self.types.lookup(expected) {
            TypeKind::Path(name) if is_int_type_name(name) => {}
            _ => return,
        }
        match self.body.exprs[id] {
            Expr::Literal(Literal::Int(_)) => self.record(id, expected),
            Expr::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                if matches!(self.body.exprs[operand], Expr::Literal(Literal::Int(_))) {
                    self.record(operand, expected);
                    self.record(id, expected);
                }
            }
            _ => {}
        }
    }

    /// F3 (S3): a bare float literal defaults to `float64` (`literal_type`);
    /// against a `float32` expectation it adopts the narrower width, so the
    /// expression type agrees with its binding/slot instead of carrying a
    /// latent `float64` the C assignment silently narrows. the int-literal
    /// analogue of `adopt_int_literal`.
    fn adopt_float_literal(&mut self, expected: TypeRef, id: ExprId) {
        match self.types.lookup(expected) {
            TypeKind::Path(name) if is_float_type_name(name) => {}
            _ => return,
        }
        if matches!(self.body.exprs[id], Expr::Literal(Literal::Float(_))) {
            self.record(id, expected);
        }
    }
}
