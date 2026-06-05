//! Function-body lowering (pass 3 per-function driver).

use ast::AstNode;
use rustc_hash::FxHashMap;
use syntax::SyntaxNodePtr;

use super::LoweringCtx;
use crate::core::{BodyId, ConstValue, FnId, HIR, Text};

pub(super) fn lower_fn_body(
    hir: &mut HIR,
    fn_id: FnId,
    fn_ast: &ast::FnDef,
    const_values: &FxHashMap<Text, ConstValue>,
) -> BodyId {
    let mut ctx = LoweringCtx::new(hir, const_values);

    // Return type for checking explicit `return` statements. `main` is an
    // ordinary function here: the C entry point (`int main` + `return 0`) is a
    // backend concern emitted as a shim, not a language rule, so a bare void
    // `main()` has return type `None` like any other void function.
    ctx.fn_ret = hir.functions[fn_id].ret;

    if let Some(block) = fn_ast.body() {
        // lower_block will push its own scope. We need parameters to be
        // visible inside that scope, so push a scope first, add params,
        // then lower_block will push another scope.
        ctx.scopes.push();
        if let Some(param_list) = fn_ast.param_list() {
            for (idx, param_ast) in param_list.params().enumerate() {
                let name: Text = LoweringCtx::text(param_ast.name());
                let ty = hir.functions[fn_id].params.get(idx).map(|p| p.ty);
                let ptr = SyntaxNodePtr::new(param_ast.syntax());
                let (_pat_id, local_id) = ctx.alloc_bind_pat(
                    name.clone(),
                    ty,
                    // Parameters are mutable: there is no `mut`-parameter syntax
                    // yet, so a default-immutable param would reject in-body
                    // reassignment with no way to opt out. Revisit when the
                    // grammar grows a `mut` parameter marker.
                    true,
                    ptr,
                );
                ctx.scopes.define(name, local_id);
            }
        }

        let block_ptr = SyntaxNodePtr::new(block.syntax());
        ctx.fn_block_ptr = Some(block_ptr);
        let block_id = ctx.lower_block(block);
        let lowered_block = &ctx.body.blocks[block_id];
        ctx.body.block = lowered_block.stmts.clone();
        ctx.body.tail = lowered_block.tail;
        ctx.scopes.pop();
    }

    // Post-lowering type checks (body fully built). Return-type enforcement
    // runs first: it re-records the declared return type onto a value-position
    // tail match, which the per-arm consistency pass then reads as the result
    // type.
    let ret = hir.functions[fn_id].ret;
    // A `&[T; N]` tail decays to a `&T`/`string` return (the implicit return):
    // `pick() -> string { "hi" }`. Decay before enforcement so the cast's type
    // matches the declared return.
    if let (Some(ret_ty), Some(tail)) = (ret.as_ref(), ctx.body.tail) {
        ctx.body.tail = Some(ctx.maybe_decay(ret_ty, tail));
    }
    ctx.enforce_fn_return_type(ret.as_ref());
    ctx.check_value_position_match_arms(ret.is_none());

    let (body, diagnostics) = ctx.finish();
    hir.diagnostics.extend(diagnostics);
    hir.bodies.alloc(body)
}
