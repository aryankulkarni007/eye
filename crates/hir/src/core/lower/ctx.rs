//! [`LoweringCtx`] lifecycle, allocation, name resolution, and field lookup.

use diagnostics::Sink;
use smol_str::SmolStr;
use syntax::{SyntaxNodePtr, SyntaxToken};

use super::LoweringCtx;
use crate::core::{
    Block, BlockId, Body, Expr, ExprId, HIR, HirError, Pat, PatId, Resolution, Stmt, StmtId, Text,
    TypeRef,
};

impl<'a> LoweringCtx<'a> {
    pub fn new(hir: &'a HIR) -> Self {
        Self {
            hir,
            body: Body::default(),
            scopes: super::Scopes::new(),
            diagnostics: Sink::new(),
        }
    }

    /// Emit a diagnostic anchored at `ptr`. The pointer is stored as-is; the
    /// renderer resolves it against the parse tree and trims trivia off the
    /// span centrally, so every diagnostic - from any emit path - is tight
    /// without each site repeating the trim.
    pub(super) fn emit(&mut self, ptr: SyntaxNodePtr, err: impl Into<HirError>) {
        self.diagnostics.emit(ptr, err.into());
    }

    /// The source pointer of an already-lowered expression, for an emit site
    /// that wants to anchor on a child sub-expression (e.g. a `let`
    /// initializer) rather than the enclosing node. Falls back to `default`
    /// when the expression has no recorded pointer.
    pub(super) fn expr_ptr(&self, id: ExprId, default: SyntaxNodePtr) -> SyntaxNodePtr {
        self.body
            .source_map
            .expr
            .get(id)
            .cloned()
            .unwrap_or(default)
    }

    pub(super) fn text(token: Option<SyntaxToken>) -> Text {
        token.map(|t| SmolStr::from(t.text())).unwrap_or_default()
    }

    pub(super) fn missing_expr(&mut self, ptr: SyntaxNodePtr) -> ExprId {
        self.alloc_expr(Expr::Missing, ptr)
    }

    pub(super) fn lower_required_expr(
        &mut self,
        expr: Option<ast::Expr>,
        ptr: SyntaxNodePtr,
    ) -> ExprId {
        expr.map(|e| self.lower_expr(&e))
            .unwrap_or_else(|| self.missing_expr(ptr))
    }

    #[allow(dead_code)]
    pub(super) fn alloc_expr_with_type(
        &mut self,
        expr: Expr,
        ptr: SyntaxNodePtr,
        ty: TypeRef,
    ) -> ExprId {
        let id = self.body.exprs.alloc(expr);
        self.body.source_map.expr.insert(id, ptr);
        self.body.expr_types.insert(id, ty);
        id
    }

    pub(super) fn alloc_expr(&mut self, expr: Expr, ptr: SyntaxNodePtr) -> ExprId {
        let id = self.body.exprs.alloc(expr);
        self.body.source_map.expr.insert(id, ptr);
        id
    }

    pub(super) fn alloc_stmt(&mut self, stmt: Stmt, ptr: SyntaxNodePtr) -> StmtId {
        let id = self.body.stmts.alloc(stmt);
        self.body.source_map.stmt.insert(id, ptr);
        id
    }

    pub(super) fn alloc_pat(&mut self, pat: Pat, ptr: SyntaxNodePtr) -> PatId {
        let id = self.body.pats.alloc(pat);
        self.body.source_map.pat.insert(id, ptr);
        id
    }

    pub(super) fn alloc_block(&mut self, block: Block, ptr: SyntaxNodePtr) -> BlockId {
        let id = self.body.blocks.alloc(block);
        self.body.block_source_map.insert(id, ptr);
        id
    }

    pub(super) fn finish(self) -> (Body, Sink<HirError>) {
        (self.body, self.diagnostics)
    }

    /// Resolve a `NameRef`. Lexical scopes first, then module-level values,
    /// then types, then enum variants (flat across every enum). Unknown
    /// names produce [`Resolution::Unresolved`] so later diagnostics still
    /// have the original text.
    pub(super) fn resolve(&self, name: &Text) -> Resolution {
        if let Some(id) = self.scopes.lookup(name) {
            return Resolution::Local(id);
        }
        if let Some(&id) = self.hir.items.functions.get(name) {
            return Resolution::Fn(id);
        }
        if let Some(&id) = self.hir.items.structs.get(name) {
            return Resolution::Struct(id);
        }
        if let Some(&id) = self.hir.items.enums.get(name) {
            return Resolution::Enum(id);
        }
        if let Some(&(enum_id, idx)) = self.hir.items.variants.get(name) {
            return Resolution::Variant { enum_id, idx };
        }
        Resolution::Unresolved(name.clone())
    }

    pub(super) fn block_tail_type(&self, block_id: BlockId) -> Option<TypeRef> {
        let block = &self.body.blocks[block_id];
        block
            .tail
            .and_then(|expr_id| self.body.expr_types.get(expr_id).cloned())
    }

    /// Look up the type of a struct or union field given the receiver type and
    /// field name.
    pub(super) fn lookup_field_type(&self, struct_ty: &TypeRef, field_name: &Text) -> TypeRef {
        match struct_ty {
            TypeRef::Path(name) => {
                // Structs and unions share the field arena, so a member of
                // either resolves the same way - check both namespaces.
                let field_id = self
                    .hir
                    .items
                    .structs
                    .get(name)
                    .and_then(|&id| self.hir.structs[id].field_index.get(field_name).copied())
                    .or_else(|| {
                        self.hir.items.unions.get(name).and_then(|&id| {
                            self.hir.unions[id].field_index.get(field_name).copied()
                        })
                    });
                if let Some(field_id) = field_id {
                    return self.hir.fields[field_id].ty.clone();
                }
                TypeRef::Error
            }
            TypeRef::Ref(inner) | TypeRef::Ptr(inner) => {
                // NOTE: auto-deref: look through one level of indirection
                self.lookup_field_type(inner, field_name)
            }
            // arrays have no named fields
            TypeRef::Array { .. } | TypeRef::Error => TypeRef::Error,
        }
    }
}
