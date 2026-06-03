//! Block and statement lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::lower_type_ref;
use crate::core::{Block, BlockId, ExprId, Local, Pat, Stmt, StmtId, Text, TypeError, TypeRef};

impl<'a> LoweringCtx<'a> {
    pub(super) fn lower_block(&mut self, block: ast::Block) -> BlockId {
        // Stmts must lower before the tail expression: the parser already
        // ensures `block.stmts()` and `block.tail_expr()` are disjoint
        // (the abandoned-marker form in the block parser puts a bare Expr
        // in the tail slot only when no `;` follows). Locals defined by
        // those stmts have to be in scope when the tail - typically a
        // `loop { ... }` or `if { ... }` body - references them.
        let ptr = SyntaxNodePtr::new(block.syntax());
        let mut stmts = ThinVec::new();
        let mut tail = None;

        self.scopes.push();

        for s in block.stmts() {
            stmts.push(self.lower_stmt(&s));
        }

        if let Some(tail_expr) = block.tail_expr() {
            tail = Some(self.lower_expr(&tail_expr));
        }

        self.scopes.pop();

        self.alloc_block(Block { stmts, tail }, ptr)
    }

    pub(super) fn lower_stmt(&mut self, stmt: &ast::Stmt) -> StmtId {
        let ptr = SyntaxNodePtr::new(stmt.syntax());
        match stmt {
            ast::Stmt::LetStmt(l) => {
                let name: Text = Self::text(l.name());
                let ty = l.ty().map(|t| lower_type_ref(&t, &mut self.diagnostics));
                let mutable = matches!(l.kind(), Some(ast::LetKind::Mut));
                let init = l.value().map(|e| self.lower_expr(&e));
                self.check_array_init_len(ptr, ty.as_ref(), init);
                self.check_explicit_let_init_type(ptr, ty.as_ref(), init);
                self.record_match_result_override(ty.as_ref(), init);
                self.record_array_init_override(ty.as_ref(), init);

                // pat <-> local back-reference: allocate Pat::Missing first so
                // the local can point at a valid PatId, then patch the pat to
                // Bind(local_id).
                let pat_id = self.alloc_pat(Pat::Missing, ptr);
                let local_id = self.body.locals.alloc(Local {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable,
                    pat: pat_id,
                });
                self.body.pats[pat_id] = Pat::Bind(local_id);
                self.scopes.define(name, local_id);

                self.alloc_stmt(
                    Stmt::Let {
                        pat: pat_id,
                        ty,
                        init,
                        mutable,
                    },
                    ptr,
                )
            }
            ast::Stmt::ExprStmt(e) => {
                let expr = self.lower_required_expr(e.expr(), ptr);
                self.alloc_stmt(Stmt::Expr(expr), ptr)
            }
        }
    }

    fn check_array_init_len(
        &mut self,
        ptr: SyntaxNodePtr,
        ty: Option<&TypeRef>,
        init: Option<ExprId>,
    ) {
        let Some(TypeRef::Array {
            len: declared_len, ..
        }) = ty
        else {
            return;
        };
        let Some(init_id) = init else {
            return;
        };
        let Some(TypeRef::Array { len: init_len, .. }) = self.body.expr_types.get(init_id) else {
            return;
        };
        if declared_len != init_len {
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::ArrayInitLenMismatch {
                    declared: *declared_len,
                    found: *init_len,
                },
            );
        }
    }

    fn check_explicit_let_init_type(
        &mut self,
        ptr: SyntaxNodePtr,
        ty: Option<&TypeRef>,
        init: Option<ExprId>,
    ) {
        let Some(expected) = ty else {
            return;
        };
        let Some(init_id) = init else {
            return;
        };
        if !matches!(self.body.exprs[init_id], crate::core::Expr::Call { .. }) {
            return;
        }
        let Some(actual) = self.body.expr_types.get(init_id).cloned() else {
            return;
        };
        if type_ref_contains_error(expected) || type_ref_contains_error(&actual) {
            return;
        }
        if matches!(
            (expected, &actual),
            (
                TypeRef::Array {
                    len: expected_len,
                    ..
                },
                TypeRef::Array {
                    len: actual_len, ..
                }
            ) if expected_len != actual_len
        ) {
            return;
        }
        if actual != *expected {
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::LetTypeMismatch {
                    expected: expected.clone(),
                    got: actual.clone(),
                },
            );
        }
    }

    /// When a value-position `match` is bound to an explicitly typed `let`, the
    /// binding type is authoritative. Re-record it as the match's result type
    /// so codegen declares the hoist temp with the binding's type, not the
    /// first arm's (e.g. `let int64 x = match` -> `int64_t _matchN`). Cross-arm
    /// consistency is checked once, later, by `check_value_position_match_arms`.
    /// No-op for non-match inits or untyped lets (those anchor on the
    /// provisional first-known-arm type recorded by `lower_match_expr`).
    fn record_match_result_override(&mut self, declared: Option<&TypeRef>, init: Option<ExprId>) {
        let Some(match_id) = init else {
            return;
        };
        let Some(declared) = declared else {
            return;
        };
        if !matches!(self.body.exprs[match_id], crate::core::Expr::Match { .. }) {
            return;
        }
        self.body.expr_types.insert(match_id, declared.clone());
    }

    /// A literal array initializer takes its element type from its elements,
    /// which default integer literals to `int32`. When the `let` declares the
    /// array type (e.g. `[usize; 2] = [100, 200]`), that type wins: re-type the
    /// literal onto the declared type so its wrapper matches the binding.
    fn record_array_init_override(&mut self, declared: Option<&TypeRef>, init: Option<ExprId>) {
        let (Some(declared), Some(init_id)) = (declared, init) else {
            return;
        };
        self.coerce_array_literal_type(declared, init_id);
    }

    /// Re-type an array literal - and recursively every nested array literal -
    /// onto a declared array type so each level's wrapper matches it. Without
    /// the recursion only the outer literal is re-typed, leaving inner literals
    /// at their `int32` default (e.g. `[[usize; 2]; 2] = [[1, 0], [0, 1]]`
    /// would emit a `usize` outer wrapper holding `int32` inner wrappers - a C
    /// type error). C converts the element constants inside the brace
    /// initializer. Each level is length-guarded: a literal whose length
    /// disagrees with the declared length keeps its own type so the existing
    /// length diagnostic still fires rather than the wrapper being reshaped
    /// around the wrong element count. Shared by the `let`, return, and
    /// call-argument coercion sites, all of which face the same nesting.
    pub(super) fn coerce_array_literal_type(&mut self, declared: &TypeRef, init_id: ExprId) {
        let TypeRef::Array {
            elem,
            len: declared_len,
        } = declared
        else {
            return;
        };
        if !matches!(self.body.exprs[init_id], crate::core::Expr::ArrayLit(_)) {
            return;
        }
        let Some(TypeRef::Array { len: lit_len, .. }) = self.body.expr_types.get(init_id) else {
            return;
        };
        if lit_len != declared_len {
            return;
        }
        let crate::core::Expr::ArrayLit(elems) = &self.body.exprs[init_id] else {
            return;
        };
        let elems: ThinVec<ExprId> = elems.clone();
        self.body.expr_types.insert(init_id, declared.clone());
        for e in elems {
            self.coerce_array_literal_type(elem, e);
        }
    }

    /// Cross-arm result-type consistency for every value-position `match` in the
    /// body, run once after the body is fully lowered. A value-position match is
    /// any `Expr::Match` that is not the direct expression of a statement
    /// (`Stmt::Expr`): statement-position matches run their arms for effect and
    /// have no result type (MATCH.md), so they are excluded. The result type is
    /// the match's recorded type - a `let`/return-type override when present
    /// (see `record_match_result_override` / `enforce_fn_return_type`), else the
    /// provisional first-known-arm type. Every arm whose body type is known must
    /// be compatible with it; unknown arm types are left alone (no cascade until
    /// inference exists).
    pub(super) fn check_value_position_match_arms(&mut self, tail_value_discarded: bool) {
        // Statement-position matches: the direct expr of a `Stmt::Expr`.
        let mut stmt_pos: Vec<ExprId> = self
            .body
            .stmts
            .iter()
            .filter_map(|(_, stmt)| match stmt {
                Stmt::Expr(id)
                    if matches!(self.body.exprs[*id], crate::core::Expr::Match { .. }) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();

        // A tail match whose value is discarded (void/`main` body, no declared
        // return) runs for effect like a statement-position match - codegen
        // emits it as a bare `switch` - so it has no result type either.
        if tail_value_discarded
            && let Some(tail) = self.body.tail
            && matches!(self.body.exprs[tail], crate::core::Expr::Match { .. })
        {
            stmt_pos.push(tail);
        }

        let value_matches: Vec<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, expr)| matches!(expr, crate::core::Expr::Match { .. }).then_some(id))
            .filter(|id| !stmt_pos.contains(id))
            .collect();

        for match_id in value_matches {
            let Some(result_ty) = self.body.expr_types.get(match_id).cloned() else {
                continue;
            };
            let arm_bodies: Vec<ExprId> = match &self.body.exprs[match_id] {
                crate::core::Expr::Match { arms, .. } => arms.iter().map(|a| a.body).collect(),
                _ => continue,
            };
            for body_id in arm_bodies {
                let Some(arm_ty) = self.body.expr_types.get(body_id).cloned() else {
                    continue;
                };
                if !types_compatible(&arm_ty, &result_ty) {
                    let Some(arm_ptr) = self.body.source_map.expr.get(body_id).cloned() else {
                        continue;
                    };
                    self.emit(
                        arm_ptr,
                        TypeError::MatchArmTypeMismatch {
                            expected: result_ty.clone(),
                            found: arm_ty.clone(),
                        },
                    );
                }
            }
        }
    }

    /// The function body's tail expression must produce the declared return
    /// type. General HIR check over any tail expression. When the tail is a
    /// value-position `match`, the return type is authoritative: re-record it
    /// as the match's result type (drives the codegen hoist temp and the
    /// per-arm check in `check_value_position_match_arms`) and defer to that
    /// per-arm reporting instead of emitting a whole-match diagnostic. Lenient
    /// via `types_compatible` (Error; and integer-family-tolerant) to avoid
    /// false positives before inference exists.
    pub(super) fn enforce_fn_return_type(&mut self, ret: Option<&TypeRef>) {
        let Some(ret) = ret else {
            return;
        };
        let Some(tail) = self.body.tail else {
            return;
        };
        if matches!(self.body.exprs[tail], crate::core::Expr::Match { .. }) {
            self.body.expr_types.insert(tail, ret.clone());
            return;
        }
        // An array-literal tail defaults its elements to int32; the declared
        // array return type wins, same as a `let` initializer. Coerce only when
        // the lengths agree so a wrong-length literal still hits the mismatch
        // diagnostic below.
        if matches!(self.body.exprs[tail], crate::core::Expr::ArrayLit(_))
            && let TypeRef::Array { len: ret_len, .. } = ret
            && let Some(TypeRef::Array { len: lit_len, .. }) = self.body.expr_types.get(tail)
            && lit_len == ret_len
        {
            self.coerce_array_literal_type(ret, tail);
            return;
        }
        let Some(actual) = self.body.expr_types.get(tail).cloned() else {
            return;
        };
        if !types_compatible(&actual, ret) {
            let Some(ptr) = self.body.source_map.expr.get(tail).cloned() else {
                return;
            };
            self.emit(
                ptr,
                TypeError::ReturnTypeMismatch {
                    expected: ret.clone(),
                    found: actual.clone(),
                },
            );
        }
    }
}

fn type_ref_contains_error(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Path(_) => false,
        TypeRef::Ref(inner) | TypeRef::Ptr(inner) => type_ref_contains_error(inner),
        TypeRef::Array { elem, .. } => type_ref_contains_error(elem),
        TypeRef::Error => true,
    }
}

/// Compatibility test for value-position match arm types. Two types are
/// compatible when they are equal, when either carries an `Error` (don't
/// cascade follow-on diagnostics), or when both are integer-family scalars.
/// The integer leniency is required because integer literals are always typed
/// `int32` today, so a wider explicit binding (e.g. `int64`) would otherwise
/// spuriously reject integer-literal arms.
fn types_compatible(a: &TypeRef, b: &TypeRef) -> bool {
    if type_ref_contains_error(a) || type_ref_contains_error(b) {
        return true;
    }
    if is_integer_path(a) && is_integer_path(b) {
        return true;
    }
    a == b
}

fn is_integer_path(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Path(name)
            if matches!(
                name.as_str(),
                "int8" | "int16" | "int32" | "int64"
                    | "uint8" | "uint16" | "uint32" | "uint64"
                    | "usize" | "isize"
            )
    )
}
