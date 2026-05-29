//! Function-body lowering (pass 3 per-function driver).

use ast::AstNode;
use syntax::SyntaxNodePtr;

use super::LoweringCtx;
use crate::core::{BodyId, FnId, HIR, Local, Pat, Text};

pub(super) fn lower_fn_body(hir: &mut HIR, fn_id: FnId, fn_ast: &ast::FnDef) -> BodyId {
    let mut ctx = LoweringCtx::new(hir);

    if let Some(block) = fn_ast.body() {
        // lower_block will push its own scope. We need parameters to be
        // visible inside that scope, so push a scope first, add params,
        // then lower_block will push another scope.
        ctx.scopes.push();
        if let Some(param_list) = fn_ast.param_list() {
            for (idx, param_ast) in param_list.params().enumerate() {
                let name: Text = LoweringCtx::text(param_ast.name());
                let ty = hir.functions[fn_id].params.get(idx).map(|p| p.ty.clone());
                let ptr = SyntaxNodePtr::new(param_ast.syntax());
                let pat_id = ctx.alloc_pat(Pat::Missing, ptr);
                let local_id = ctx.body.locals.alloc(Local {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable: false,
                    pat: pat_id,
                });
                ctx.body.pats[pat_id] = Pat::Bind(local_id);
                ctx.scopes.define(name, local_id);
            }
        }

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
    let ret = hir.functions[fn_id].ret.clone();
    ctx.enforce_fn_return_type(ret.as_ref());
    ctx.check_value_position_match_arms(ret.is_none());
    ctx.check_unhoisted_matches();

    let (body, diagnostics) = ctx.finish();
    hir.diagnostics.extend(diagnostics);
    hir.bodies.alloc(body)
}
