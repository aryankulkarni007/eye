//! [`LoweringCtx`] lifecycle, allocation, name resolution, and field lookup.

use diagnostics::Sink;
use smol_str::SmolStr;
use syntax::{StringTable, SyntaxNodePtr, SyntaxToken};

use rustc_hash::FxHashMap;

use super::LoweringCtx;
use crate::core::{
    Block, BlockId, Body, ConstValue, Expr, ExprId, HIR, HirError, Local, LocalId, Pat, PatId,
    Resolution, Stmt, StmtId, Text, TypeRef,
};

impl<'a> LoweringCtx<'a> {
    pub fn new(
        hir: &'a HIR,
        types: &'a crate::core::TypeInterner,
        const_values: &'a FxHashMap<Text, ConstValue>,
        interner: &'a dyn StringTable,
    ) -> Self {
        Self {
            hir,
            types,
            body: Body::default(),
            scopes: super::Scopes::new(),
            diagnostics: Sink::new(),
            const_values,
            interner,
        }
    }

    /// emit a diagnostic anchored at `ptr`. the pointer is stored as-is; the
    /// renderer resolves it against the parse tree and trims trivia off the
    /// span centrally, so every diagnostic - from any emit path - is tight
    /// without each site repeating the trim.
    pub(super) fn emit(&mut self, ptr: SyntaxNodePtr, err: impl Into<HirError>) {
        self.diagnostics.emit(ptr, err.into());
    }

    /// the source pointer of an already-lowered expression, for an emit site
    /// that wants to anchor on a child sub-expression (e.g. a `let`
    /// initializer) rather than the enclosing node. falls back to `default`
    /// when the expression has no recorded pointer.
    pub(super) fn expr_ptr(&self, id: ExprId, default: SyntaxNodePtr) -> SyntaxNodePtr {
        self.body
            .source_map
            .expr
            .get(id.into())
            .cloned()
            .unwrap_or(default)
    }

    pub(super) fn text(&self, token: Option<SyntaxToken>) -> Text {
        token
            .map(|t| {
                self.interner
                    .get(t.text())
                    .unwrap_or_else(|| SmolStr::from(t.text()))
            })
            .unwrap_or_default()
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

    pub(super) fn alloc_expr(&mut self, expr: Expr, ptr: SyntaxNodePtr) -> ExprId {
        let id = self.body.exprs.alloc(expr);
        self.body.source_map.expr.insert(id.into(), ptr);
        id
    }

    pub(super) fn alloc_stmt(&mut self, stmt: Stmt, ptr: SyntaxNodePtr) -> StmtId {
        let id = self.body.stmts.alloc(stmt);
        self.body.source_map.stmt.insert(id.into(), ptr);
        id
    }

    pub(super) fn alloc_pat(&mut self, pat: Pat, ptr: SyntaxNodePtr) -> PatId {
        let id = self.body.pats.alloc(pat);
        self.body.source_map.pat.insert(id.into(), ptr);
        id
    }

    /// create a circular `Pat::Bind(local) <-> Local { pat }`
    /// reference atomically. allocates `Pat::Missing` first (so the local has a
    /// valid patid), then patches it to `Bind(local_id)`. previously each caller
    /// hand-rolled the two-step sequence, creating a window where `pats[pat_id]`
    /// reads as `Missing`. this helper keeps the window internal.
    /// to revert: inline the body and return to the fragile two-step pattern.
    pub(super) fn alloc_bind_pat(
        &mut self,
        name: Text,
        ty: Option<TypeRef>,
        mutable: bool,
        ptr: SyntaxNodePtr,
    ) -> (PatId, LocalId) {
        let pat_id = self.alloc_pat(Pat::Missing, ptr);
        let local_id = self.body.locals.alloc(Local {
            name,
            ty,
            mutable,
            pat: pat_id,
        });
        self.body.pats[pat_id] = Pat::Bind(local_id);
        (pat_id, local_id)
    }

    pub(super) fn alloc_block(&mut self, block: Block, ptr: SyntaxNodePtr) -> BlockId {
        let id = self.body.blocks.alloc(block);
        self.body.block_source_map.insert(id.into(), ptr);
        id
    }

    pub(super) fn finish(self) -> (Body, Sink<HirError>) {
        (self.body, self.diagnostics)
    }

    /// R012 for body-position type annotations (`let` types, cast targets,
    /// local `const` types): every `Path` name in the type must be a declared
    /// type. bodies lower after item collection, so unlike item signatures
    /// (validated post-collect by `collect::validate_type_names`) this can
    /// check eagerly.
    pub(super) fn check_type_names(&mut self, ty: TypeRef, ptr: SyntaxNodePtr) {
        // `ty` may be body-local (interned during this body's lowering), so
        // resolve through the working interner, not the frozen scope one.
        let names = {
            let types = &self.types;
            super::types::unknown_type_names(ty, types, self.hir)
        };
        for name in names {
            self.emit(ptr, crate::core::ResolveError::UnknownTypeName { name });
        }
    }

    /// resolve a `NameRef`. lexical scopes first, then module-level values,
    /// then types, then enum variants (flat across every enum). unknown
    /// names produce [`Resolution::Unresolved`] so later diagnostics still
    /// have the original text.
    pub(super) fn resolve(&self, name: &Text) -> Resolution {
        match self.scopes.lookup(name) {
            Some(super::scopes::Binding::Local(id)) => return Resolution::Local(id),
            Some(super::scopes::Binding::Const(id)) => return Resolution::LocalConst(id),
            None => {}
        }
        if let Some(&id) = self.hir.items.consts.get(name) {
            return Resolution::Const(id);
        }
        if let Some(&id) = self.hir.items.globals.get(name) {
            return Resolution::Global(id);
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

}
