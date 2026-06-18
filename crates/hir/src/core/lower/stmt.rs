//! block and statement lowering.

use ast::AstNode;
use rustc_hash::FxHashSet;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::const_eval::{ScopedConsts, fold_with_map};
use super::types::lower_type_ref;
use crate::core::{
    Block, BlockId, LocalConst, Pat, PatternError, ResolveError, Stmt, StmtId, StructPatBinding,
    Text, TypeRef, fx_set,
};

impl<'a> LoweringCtx<'a> {
    pub(super) fn lower_block(&mut self, block: ast::Block) -> BlockId {
        // stmts must lower before the tail expression: the parser already
        // ensures `block.stmts()` and `block.tail_expr()` are disjoint
        // (the abandoned-marker form in the block parser puts a bare expr
        // in the tail slot only when no `;` follows). locals defined by
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
            ast::Stmt::LetStmt(l) if l.pat().is_some() => {
                // `let Point { x, y } = p` - struct destructure. a distinct path
                // from a `type name` binding: it makes n field bindings, not one,
                // and skips the single-binding checks (decay / explicit-type /
                // array-init / match-result), which assume one type and one name.
                self.lower_let_destructure(l, ptr)
            }
            ast::Stmt::LetStmt(l) => {
                let name: Text = self.text(l.name());
                let ty = l.ty().map(|t| {
                    let consts = ScopedConsts {
                        scopes: &self.scopes,
                        local_consts: &self.body.local_consts,
                        globals: self.const_values,
                    };
                    lower_type_ref(&t, &mut self.diagnostics, &consts, self.types)
                });
                // an untyped `let x = init` is no longer rejected here: typeck
                // infers x's type from the initializer (let-from-init) and only
                // rejects (T025) when the init has no value to infer from.
                // R012: the annotation's type names must be declared.
                if let Some(t) = ty {
                    self.check_type_names(t, ptr);
                }
                let mutable = matches!(l.kind(), Some(ast::LetKind::Mut));
                let init = l.value().map(|e| self.lower_expr(&e));
                // the initializer's coercion against the declared type
                // (`&[T; N]` decay, array-literal/integer-literal typing) and the
                // let-init judgments (array-init-length, explicit-init type) all
                // live in the typeck pass (S2C); lowering no longer types it.

                // same-scope redeclaration is an error (R015); shadowing
                // needs a nested block scope. the binding is still defined
                // afterwards (newest wins) so later uses resolve.
                if self.scopes.declared_in_current(&name) {
                    self.emit(ptr, ResolveError::DuplicateLocal { name: name.clone() });
                }
                let (pat_id, local_id) = self.alloc_bind_pat(name.clone(), ty, mutable, ptr);
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
            ast::Stmt::ConstDef(c) => {
                // block-scope `const TYPE NAME = expr;`: fold the initializer
                // now, against the consts visible at this point (top-level
                // consts plus enclosing local consts - a strictly-earlier
                // declaration, so no cycle is possible). the folded value lives
                // in `body.local_consts`; references inline it, so the
                // statement itself emits nothing in MIR.
                let name: Text = self.text(c.name());
                let (ty, value) = {
                    let consts = ScopedConsts {
                        scopes: &self.scopes,
                        local_consts: &self.body.local_consts,
                        globals: self.const_values,
                    };
                    // a missing type or initializer was already diagnosed by
                    // the parser; fall back to poison without re-reporting.
                    let ty = match c.ty() {
                        Some(t) => {
                            lower_type_ref(&t, &mut self.diagnostics, &consts, self.types)
                        }
                        None => self.types.error_type(),
                    };
                    let value = c
                        .value()
                        .and_then(|e| fold_with_map(&e, &consts, &mut self.diagnostics));
                    (ty, value)
                };
                // R012: the declared type's names must be declared types.
                self.check_type_names(ty, ptr);
                // same-scope redeclaration (R015) applies to local consts too.
                if self.scopes.declared_in_current(&name) {
                    self.emit(ptr, ResolveError::DuplicateLocal { name: name.clone() });
                }
                let id = self.body.local_consts.alloc(LocalConst {
                    name: name.clone(),
                    ty,
                    value,
                });
                self.scopes.define_const(name, id);
                self.alloc_stmt(Stmt::Const(id), ptr)
            }
        }
    }

    /// lower a `let Point { x, y } = p` / `let Point { x: px } = p` struct
    /// destructure. binds one local per field (the field name, or the rename),
    /// typed by the struct field; exhaustive - every field must be bound. the
    /// resulting `Stmt::Let` carries a `Pat::Struct`; MIR expands it into one
    /// field-projection `Let` per binding.
    fn lower_let_destructure(&mut self, l: &ast::LetStmt, ptr: SyntaxNodePtr) -> StmtId {
        let mutable = matches!(l.kind(), Some(ast::LetKind::Mut));
        let init = l.value().map(|e| self.lower_expr(&e));

        let sp = l.pat().expect("caller checked l.pat() is Some");
        let ty_name: Text = self.text(sp.ty().and_then(|n| n.name()));
        let struct_id = self.hir.items.structs.get(&ty_name).copied();
        if struct_id.is_none() {
            self.emit(
                ptr,
                PatternError::DestructureNotAStruct {
                    ty: ty_name.clone(),
                },
            );
        }

        let mut bindings: ThinVec<StructPatBinding> = ThinVec::new();
        let field_count = sp.field_list().map(|fl| fl.fields().count()).unwrap_or(0);
        let mut seen: FxHashSet<Text> = fx_set(field_count);
        if let Some(fl) = sp.field_list() {
            for pf in fl.fields() {
                let field_name: Text = self.text(pf.name());
                let binding_name: Text = match pf.binding() {
                    Some(b) => self.text(b.name()),
                    None => field_name.clone(),
                };
                // resolve the field's type fully before any `&mut self` call.
                let field_ty: Option<TypeRef> = struct_id
                    .and_then(|sid| self.hir.structs[sid].field_index.get(&field_name).copied())
                    .map(|fid| self.hir.fields[fid].ty);
                if struct_id.is_some() && field_ty.is_none() {
                    self.emit(
                        ptr,
                        PatternError::DestructureUnknownField {
                            ty: ty_name.clone(),
                            field: field_name.clone(),
                        },
                    );
                }
                if !seen.insert(field_name.clone()) {
                    self.emit(
                        ptr,
                        PatternError::DestructureDuplicateField {
                            field: field_name.clone(),
                        },
                    );
                }

                // same-scope redeclaration (R015): catches a destructure
                // binding colliding with an earlier binding, including a
                // rename collision inside one destructure (`{ x: a, y: a }`,
                // which the duplicate-*field* check cannot see).
                if self.scopes.declared_in_current(&binding_name) {
                    self.emit(
                        ptr,
                        ResolveError::DuplicateLocal {
                            name: binding_name.clone(),
                        },
                    );
                }
                let (_pat_id, local_id) =
                    self.alloc_bind_pat(binding_name.clone(), field_ty, mutable, ptr);
                self.scopes.define(binding_name, local_id);
                bindings.push(StructPatBinding {
                    field: field_name,
                    local: local_id,
                });
            }
        }

        // exhaustiveness: every field of the struct must be bound (no `..`/ignore).
        if let Some(sid) = struct_id {
            let missing: Vec<Text> = self.hir.structs[sid]
                .fields
                .iter()
                .map(|&fid| self.hir.fields[fid].name.clone())
                .filter(|f| !seen.contains(f))
                .collect();
            if !missing.is_empty() {
                self.emit(
                    ptr,
                    PatternError::DestructureNonExhaustive {
                        ty: ty_name.clone(),
                        missing,
                    },
                );
            }
        }

        let pat_id = self.alloc_pat(
            Pat::Struct {
                ty: ty_name,
                fields: bindings,
            },
            ptr,
        );
        self.alloc_stmt(
            Stmt::Let {
                pat: pat_id,
                ty: None,
                init,
                mutable,
            },
            ptr,
        )
    }
}
