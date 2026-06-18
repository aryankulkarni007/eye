//! function-body lowering (pass 3 per-function driver).

use ast::AstNode;
use diagnostics::Sink;
use rustc_hash::FxHashMap;
use syntax::{StringTable, SyntaxNodePtr};

use super::LoweringCtx;
use crate::core::{Body, ConstValue, FnId, HIR, HirError, Text};

/// one lowered function body, independent of any `HIR` arena. its `TypeRef`
/// handles resolve through the shared scope interner (`scope.types`) lowering
/// interned into - no per-body interner is carried (S6).
pub(super) struct FnLowerOut {
    pub body: Body,
    pub diagnostics: Sink<HirError>,
}

/// lower one function body against an immutable item scope, interning any
/// body-local types into the scope's shared interner (`scope.types`, `&self`
/// interning - no clone, no take/restore).
pub(super) fn lower_fn_with(
    scope: &HIR,
    fn_id: FnId,
    fn_ast: &ast::FnDef,
    const_values: &FxHashMap<Text, ConstValue>,
    interner: &dyn StringTable,
) -> FnLowerOut {
    let mut ctx = LoweringCtx::new(scope, &scope.types, const_values, interner);

    if let Some(block) = fn_ast.body() {
        // lower_block will push its own scope. we need parameters to be
        // visible inside that scope, so push a scope first, add params,
        // then lower_block will push another scope.
        ctx.scopes.push();
        if let Some(param_list) = fn_ast.param_list() {
            for (idx, param_ast) in param_list.params().enumerate() {
                let name: Text = ctx.text(param_ast.name());
                let ty = scope.functions[fn_id].params.get(idx).map(|p| p.ty);
                let ptr = SyntaxNodePtr::new(param_ast.syntax());
                let (_pat_id, local_id) = ctx.alloc_bind_pat(
                    name.clone(),
                    ty,
                    // parameters are mutable: there is no `mut`-parameter syntax
                    // yet, so a default-immutable param would reject in-body
                    // reassignment with no way to opt out. revisit when the
                    // grammar grows a `mut` parameter marker.
                    true,
                    ptr,
                );
                ctx.scopes.define(name, local_id);
            }
        }

        let block_ptr = SyntaxNodePtr::new(block.syntax());
        ctx.body.fn_block_ptr = Some(block_ptr);
        let block_id = ctx.lower_block(block);
        let lowered_block = &ctx.body.blocks[block_id];
        ctx.body.block = lowered_block.stmts.clone();
        ctx.body.tail = lowered_block.tail;
        ctx.scopes.pop();
    }

    // lowering no longer types the tail expression (S2C C5): the typeck pass
    // coerces the implicit-return tail against the declared return type
    // (decay/array-literal/integer-literal) and enforces the return-type
    // diagnostics (`enforce_return_type` / `check_explicit_return`).
    let (body, diagnostics) = ctx.finish();
    FnLowerOut { body, diagnostics }
}
