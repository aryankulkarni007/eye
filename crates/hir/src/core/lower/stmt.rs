//! Block and statement lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::lower_type_ref;
use crate::core::{Block, BlockId, ExprId, Local, Pat, Stmt, StmtId, Text, TypeRef};

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
            self.diag(
                ptr,
                format!(
                    "array initializer length mismatch: declared length {declared_len}, initializer has {init_len} element(s)"
                ),
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
            self.diag(
                ptr,
                format!(
                    "let initializer type mismatch: expected {}, got {}",
                    display_type_ref(expected),
                    display_type_ref(&actual)
                ),
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

fn display_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Path(name) => name.to_string(),
        TypeRef::Ref(inner) => format!("&{}", display_type_ref(inner)),
        TypeRef::Ptr(inner) => format!("{}*", display_type_ref(inner)),
        TypeRef::Array { elem, len } => format!("[{}; {}]", display_type_ref(elem), len),
        TypeRef::Error => "<error>".to_string(),
    }
}
