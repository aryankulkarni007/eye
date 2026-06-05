//! Block and statement lowering.

use ast::AstNode;
use rustc_hash::FxHashSet;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::const_eval::{ScopedConsts, fold_with_map};
use super::types::lower_type_ref;
use crate::core::{
    fx_set, Block, BlockId, ExprId, LocalConst, Pat, PatternError, Stmt, StmtId,
    StructPatBinding, Text, TypeError, TypeInterner, TypeKind, TypeRef, VisitTypeRef,
};

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
            ast::Stmt::LetStmt(l) if l.pat().is_some() => {
                // `let Point { x, y } = p` - struct destructure. A distinct path
                // from a `type name` binding: it makes N field bindings, not one,
                // and skips the single-binding checks (decay / explicit-type /
                // array-init / match-result), which assume one type and one name.
                self.lower_let_destructure(l, ptr)
            }
            ast::Stmt::LetStmt(l) => {
                let name: Text = Self::text(l.name());
                let ty = l
                    .ty()
                    .map(|t| {
                        let consts = ScopedConsts {
                            scopes: &self.scopes,
                            local_consts: &self.body.local_consts,
                            globals: self.const_values,
                        };
                        lower_type_ref(&t, &mut self.diagnostics, &consts, &mut self.hir.types.borrow_mut())
                    });
                // Type inference is on hiatus, so a binding needs an explicit
                // type. Without one it would reach codegen as an `Error` type
                // (`void* /* ERROR TY */`); reject it cleanly here instead.
                if ty.is_none() {
                    self.emit(ptr, TypeError::MissingTypeAnnotation { name: name.clone() });
                }
                let mutable = matches!(l.kind(), Some(ast::LetKind::Mut));
                let init = l.value().map(|e| self.lower_expr(&e));
                // A `&[T; N]` initializer decays to a `&T`/`string` binding
                // (HORIZON0 C3): `let string s = "hi"`. Insert the decay before
                // the type check so the cast's type matches the declaration.
                let init = match (ty.as_ref(), init) {
                    (Some(declared), Some(id)) => Some(self.maybe_decay(declared, id)),
                    _ => init,
                };
                self.check_array_init_len(ptr, ty.as_ref(), init);
                self.check_explicit_let_init_type(ptr, ty.as_ref(), init);
                self.record_match_result_override(ty.as_ref(), init);
                self.record_array_init_override(ty.as_ref(), init);

                let (pat_id, local_id) =
                    self.alloc_bind_pat(name.clone(), ty, mutable, ptr);
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
                // Block-scope `const TYPE NAME = expr;`: fold the initializer
                // now, against the consts visible at this point (top-level
                // consts plus enclosing local consts - a strictly-earlier
                // declaration, so no cycle is possible). The folded value lives
                // in `body.local_consts`; references inline it, so the
                // statement itself emits nothing in MIR.
                let name: Text = Self::text(c.name());
                let (ty, value) = {
                    let consts = ScopedConsts {
                        scopes: &self.scopes,
                        local_consts: &self.body.local_consts,
                        globals: self.const_values,
                    };
                    // A missing type or initializer was already diagnosed by
                    // the parser; fall back to poison without re-reporting.
                    let ty = match c.ty() {
                        Some(t) => lower_type_ref(&t, &mut self.diagnostics, &consts, &mut self.hir.types.borrow_mut()),
                        None => self.hir.types.borrow_mut().error_type(),
                    };
                    let value = c
                        .value()
                        .and_then(|e| fold_with_map(&e, &consts, &mut self.diagnostics));
                    (ty, value)
                };
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

    /// Lower a `let Point { x, y } = p` / `let Point { x: px } = p` struct
    /// destructure. Binds one local per field (the field name, or the rename),
    /// typed by the struct field; exhaustive - every field must be bound. The
    /// resulting `Stmt::Let` carries a `Pat::Struct`; MIR expands it into one
    /// field-projection `Let` per binding.
    fn lower_let_destructure(&mut self, l: &ast::LetStmt, ptr: SyntaxNodePtr) -> StmtId {
        let mutable = matches!(l.kind(), Some(ast::LetKind::Mut));
        let init = l.value().map(|e| self.lower_expr(&e));

        let sp = l.pat().expect("caller checked l.pat() is Some");
        let ty_name: Text = Self::text(sp.ty().and_then(|n| n.name()));
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
                let field_name: Text = Self::text(pf.name());
                let binding_name: Text = match pf.binding() {
                    Some(b) => Self::text(b.name()),
                    None => field_name.clone(),
                };
                // Resolve the field's type fully before any `&mut self` call.
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

                let (_pat_id, local_id) =
                    self.alloc_bind_pat(binding_name.clone(), field_ty, mutable, ptr);
                self.scopes.define(binding_name, local_id);
                bindings.push(StructPatBinding {
                    field: field_name,
                    local: local_id,
                });
            }
        }

        // Exhaustiveness: every field of the struct must be bound (no `..`/ignore).
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

    fn check_array_init_len(
        &mut self,
        ptr: SyntaxNodePtr,
        ty: Option<&TypeRef>,
        init: Option<ExprId>,
    ) {
        let Some(declared_len) = ty.copied().and_then(|t| {
            let types = self.hir.types.borrow();
            match types.lookup(t) {
                &TypeKind::Array { len, .. } => Some(len),
                _ => None,
            }
        }) else {
            return;
        };
        let Some(init_id) = init else {
            return;
        };
        let Some(init_len) = self.body.expr_types.get(init_id).copied().and_then(|t| {
            let types = self.hir.types.borrow();
            match types.lookup(t) {
                &TypeKind::Array { len, .. } => Some(len),
                _ => None,
            }
        }) else {
            return;
        };
        if declared_len != init_len {
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::ArrayInitLenMismatch {
                    declared: declared_len,
                    found: init_len,
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
        let Some(&expected) = ty else {
            return;
        };
        let Some(init_id) = init else {
            return;
        };
        // An `if` used as a value must yield a value on every path; an else-less
        // or void-branch `if` would leave the binding uninitialized (a C read of
        // an uninitialized local).
        if self.yields_no_value(init_id) {
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::VoidValueInValuePosition,
            );
            return;
        }
        if !matches!(self.body.exprs[init_id], crate::core::Expr::Call { .. }) {
            return;
        }
        let Some(actual) = self.body.expr_types.get(init_id).copied() else {
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::VoidValueInValuePosition,
            );
            return;
        };
        {
            let types = self.hir.types.borrow();
            if type_ref_contains_error(expected, &types) || type_ref_contains_error(actual, &types) {
                return;
            }
            if let (TypeKind::Array { len: exp_len, .. }, TypeKind::Array { len: act_len, .. }) =
                (types.lookup(expected), types.lookup(actual))
                && exp_len != act_len {
                    return;
                }
        }
        if actual != expected {
            let expected_str = self.hir.types.borrow().display(expected).to_string();
            let got_str = self.hir.types.borrow().display(actual).to_string();
            self.emit(
                self.expr_ptr(init_id, ptr),
                TypeError::LetTypeMismatch {
                    expected: expected_str,
                    got: got_str,
                },
            );
        }
    }

    /// True when an expression provably yields no value on some control path, so
    /// it must not sit in a value-consuming position (`let` init, `return`, the
    /// function tail). Today the proven case is an `if` with no `else` (directly,
    /// or nested as another branch's tail): when the condition is false it falls
    /// through with no value, leaving the consumer's storage uninitialized.
    /// Conservative: anything it cannot prove valueless returns `false`, so a
    /// diverging branch (`{ return; }`) and the inference-hiatus `None` types
    /// never cause a false rejection. The check fires only at value-consuming
    /// sites, so a discarded `if` (a statement, or a loop-body tail) is never
    /// reached.
    fn yields_no_value(&self, id: ExprId) -> bool {
        let (then_block, else_block) = match &self.body.exprs[id] {
            crate::core::Expr::If { then_branch, else_branch, .. } => (*then_branch, *else_branch),
            _ => return false,
        };
        match else_block {
            None => true,
            Some(eb) => self.block_yields_no_value(then_block) || self.block_yields_no_value(eb),
        }
    }

    /// A block provably yields no value only when its tail itself provably
    /// yields no value (a nested else-less `if`). A block with **no** tail is
    /// *not* proven valueless: it may diverge (a trailing `return` / `break` /
    /// `continue` never falls through), which is legal in value position
    /// (`let x = if c { return; } else { v };`). Returning `false` there keeps
    /// the check free of false positives; the residual void-branch leak
    /// (`else { foo(); }`) is left for the typeck pass.
    fn block_yields_no_value(&self, block: crate::core::BlockId) -> bool {
        match self.body.blocks[block].tail {
            None => false,
            Some(tail) => self.yields_no_value(tail),
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
        self.body.expr_types.insert(match_id, *declared);
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
        let (elem, declared_len) = {
            let types = self.hir.types.borrow();
            match types.lookup(*declared) {
                &TypeKind::Array { elem, len } => (elem, len),
                _ => return,
            }
        };
        if !matches!(
            self.body.exprs[init_id],
            crate::core::Expr::ArrayLit(_) | crate::core::Expr::ArrayRepeat { .. }
        ) {
            return;
        }
        let lit_len = match self.body.expr_types.get(init_id).copied() {
            Some(ty) => {
                let types = self.hir.types.borrow();
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
        self.body.expr_types.insert(init_id, *declared);
        // Elements to re-type against the declared element type: every element
        // of a literal, or the single repeated value of `[value; N]`. Collected
        // first to release the borrow on `exprs` before the recursive call.
        let children: Vec<ExprId> = match &self.body.exprs[init_id] {
            crate::core::Expr::ArrayLit(elems) => elems.to_vec(),
            crate::core::Expr::ArrayRepeat { value, .. } => vec![*value],
            _ => return,
        };
        for e in children {
            self.coerce_array_literal_type(&elem, e);
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
            let Some(result_ty) = self.body.expr_types.get(match_id).copied() else {
                continue;
            };
            let arm_bodies: Vec<ExprId> = match &self.body.exprs[match_id] {
                crate::core::Expr::Match { arms, .. } => arms.iter().map(|a| a.body).collect(),
                _ => continue,
            };
            let types = self.hir.types.borrow();
            for body_id in arm_bodies {
                let Some(arm_ty) = self.body.expr_types.get(body_id).copied() else {
                    continue;
                };
                if !types_compatible(arm_ty, result_ty, &types) {
                    let Some(arm_ptr) = self.body.source_map.expr.get(body_id).cloned() else {
                        continue;
                    };
                    let expected = types.display(result_ty).to_string();
                    let found = types.display(arm_ty).to_string();
                    self.emit(
                        arm_ptr,
                        TypeError::MatchArmTypeMismatch {
                            expected,
                            found,
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
        let Some(&ret) = ret else {
            return;
        };
        let Some(tail) = self.body.tail else {
            // No tail expression and no explicit `return val;` statement means
            // the function body never produces a value despite its declaration.
            let has_return = self.body.stmts.iter().any(|(_, s)| match s {
                Stmt::Expr(e) => matches!(self.body.exprs[*e], crate::core::Expr::Return(Some(_))),
                _ => false,
            });
            if !has_return
                && let Some(ptr) = self.fn_block_ptr {
                    let expected = self.hir.types.borrow().display(ret).to_string();
                    self.emit(ptr, TypeError::ReturnMissingValue { expected });
                }
            return;
        };
        // A tail else-less / void-branch `if` yields no value for the return.
        if self.yields_no_value(tail) {
            if let Some(ptr) = self.body.source_map.expr.get(tail).cloned() {
                self.emit(ptr, TypeError::VoidValueInValuePosition);
            }
            return;
        }
        if matches!(self.body.exprs[tail], crate::core::Expr::Match { .. }) {
            self.body.expr_types.insert(tail, ret);
            return;
        }
        // An array-literal tail defaults its elements to int32; the declared
        // array return type wins, same as a `let` initializer. Coerce only when
        // the lengths agree so a wrong-length literal still hits the mismatch
        // diagnostic below.
        if matches!(self.body.exprs[tail], crate::core::Expr::ArrayLit(_)) {
            let types = self.hir.types.borrow();
            let ret_len = match types.lookup(ret) {
                TypeKind::Array { len, .. } => *len,
                _ => { drop(types); return; }
            };
            let lit_len = match self.body.expr_types.get(tail).copied() {
                Some(t) => match types.lookup(t) {
                    TypeKind::Array { len, .. } => *len,
                    _ => { drop(types); return; }
                },
                None => { drop(types); return; }
            };
            if lit_len == ret_len {
                drop(types);
                self.coerce_array_literal_type(&ret, tail);
                return;
            }
        }
        let Some(actual) = self.body.expr_types.get(tail).copied() else {
            return;
        };
        {
            let types = self.hir.types.borrow();
            if !types_compatible(actual, ret, &types) {
                let Some(ptr) = self.body.source_map.expr.get(tail).cloned() else {
                    return;
                };
                let expected = types.display(ret).to_string();
                let found = types.display(actual).to_string();
                self.emit(
                    ptr,
                    TypeError::ReturnTypeMismatch {
                        expected,
                        found,
                    },
                );
            }
        }
    }

    /// Check an explicit `return expr?;` against the enclosing function's
    /// effective return type ([`LoweringCtx::fn_ret`]). Covers the three
    /// return-arity errors clang would otherwise reject: a value in a void
    /// function, a missing value in a typed function, and a value of the wrong
    /// type. `ret_ptr` anchors the arity diagnostics on the whole `return`;
    /// a type mismatch anchors on the returned value instead. Leniency matches
    /// the tail check ([`enforce_fn_return_type`]) via `types_compatible`.
    pub(super) fn check_explicit_return(&mut self, value: Option<ExprId>, ret_ptr: SyntaxNodePtr) {
        match (self.fn_ret, value) {
            (None, None) => {}
            (None, Some(_)) => self.emit(ret_ptr, TypeError::ReturnValueInVoid),
            (Some(expected), None) => {
                let expected_str = self.hir.types.borrow().display(expected).to_string();
                self.emit(ret_ptr, TypeError::ReturnMissingValue { expected: expected_str })
            }
            (Some(ret), Some(val)) => {
                // A returned else-less / void-branch `if` yields no value.
                if self.yields_no_value(val) {
                    self.emit(
                        self.body.source_map.expr.get(val).cloned().unwrap_or(ret_ptr),
                        TypeError::VoidValueInValuePosition,
                    );
                    return;
                }
                // An array-literal return defaults its elements to int32; the
                // declared array return type wins when the lengths agree, same
                // as the tail and a `let` initializer. A wrong length still
                // falls through to the mismatch check below.
                if matches!(self.body.exprs[val], crate::core::Expr::ArrayLit(_)) {
                    let types = self.hir.types.borrow();
                    let ret_len = match types.lookup(ret) {
                        TypeKind::Array { len, .. } => *len,
                        _ => { drop(types); return; }
                    };
                    let lit_len = match self.body.expr_types.get(val).copied() {
                        Some(t) => match types.lookup(t) {
                            TypeKind::Array { len, .. } => *len,
                            _ => { drop(types); return; }
                        },
                        None => { drop(types); return; }
                    };
                    if lit_len == ret_len {
                        drop(types);
                        self.coerce_array_literal_type(&ret, val);
                        return;
                    }
                }
                let Some(actual) = self.body.expr_types.get(val).copied() else {
                    self.emit(
                        self.body.source_map.expr.get(val).cloned().unwrap_or(ret_ptr),
                        TypeError::VoidValueInValuePosition,
                    );
                    return;
                };
                {
                    let types = self.hir.types.borrow();
                    if !types_compatible(actual, ret, &types) {
                        let ptr = self
                            .body
                            .source_map
                            .expr
                            .get(val)
                            .cloned()
                            .unwrap_or(ret_ptr);
                        let expected = types.display(ret).to_string();
                        let found = types.display(actual).to_string();
                        self.emit(
                            ptr,
                            TypeError::ReturnTypeMismatch {
                                expected,
                                found,
                            },
                        );
                    }
                }
            }
        }
    }
}

struct ContainsError(bool);

impl VisitTypeRef for ContainsError {
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
        let is_error = matches!(types.lookup(ty), TypeKind::Error);
        if is_error {
            self.0 = true;
        }
        !is_error
    }
}

fn type_ref_contains_error(ty: TypeRef, types: &TypeInterner) -> bool {
    let mut v = ContainsError(false);
    types.walk(ty, &mut v);
    v.0
}

/// Compatibility test for value-position match arm types. Two types are
/// compatible when they are equal, when either carries an `Error` (don't
/// cascade follow-on diagnostics), or when both are integer-family scalars.
/// The integer leniency is required because integer literals are always typed
/// `int32` today, so a wider explicit binding (e.g. `int64`) would otherwise
/// spuriously reject integer-literal arms.
fn types_compatible(a: TypeRef, b: TypeRef, types: &TypeInterner) -> bool {
    if type_ref_contains_error(a, types) || type_ref_contains_error(b, types) {
        return true;
    }
    if is_integer_path(a, types) && is_integer_path(b, types) {
        return true;
    }
    a == b
}

fn is_integer_path(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Path(name)
            if matches!(
                name.as_str(),
                "int8" | "int16" | "int32" | "int64"
                    | "uint8" | "uint16" | "uint32" | "uint64"
                    | "usize" | "isize"
            )
    )
}
