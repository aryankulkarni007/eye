//! `match` lowering (Strategy A). Statement-position matches become a direct
//! `switch`; value-position matches are hoisted into `_matchN` temps declared
//! ahead of their enclosing statement, then referenced at the use site.

use super::CGen;
use super::types::CType;
use hir::core::{Body, Expr, ExprId, Pat};
use smallvec::SmallVec;

impl<'a> CGen<'a> {
    /// Emit a `switch` for a match. With `temp = Some(name)` each arm assigns
    /// the match value into `name` (value-position, hoisted). With `temp =
    /// None` the arm bodies run for effect only (statement-position).
    pub(super) fn gen_match(&mut self, match_id: ExprId, body: &Body, temp: Option<&str>) {
        let (scrut, arms) = match &body.exprs[match_id] {
            Expr::Match { scrut, arms } => (*scrut, arms),
            _ => unreachable!("gen_match called on a non-match expression"),
        };

        self.push_indent();
        self.output.push_str("switch (");
        self.gen_expr(scrut, body);
        self.output.push_str(") {\n");
        self.indent_level += 1;

        for arm in arms {
            self.push_indent();
            match &body.pats[arm.pat] {
                Pat::Wildcard => self.output.push_str("default:\n"),
                Pat::Variant { enum_id, idx } => {
                    let label = &self.hir.enums[*enum_id].variants[*idx as usize].name;
                    emitln!(self, "case {}:", label);
                }
                // HIR guarantees only Variant/Wildcard survive in arms on a
                // clean lowering, and codegen only runs when hir.diagnostics is
                // empty. Degrade rather than panic - mirrors Stmt::Let.
                Pat::Bind(_) | Pat::Missing => {
                    self.output.push_str("/* INVALID PATTERN */\n");
                    self.push_indent();
                    self.output.push_str("break;\n");
                    continue;
                }
            }
            self.indent_level += 1;
            self.gen_match_arm_body(arm.body, body, temp);
            self.push_indent();
            self.output.push_str("break;\n");
            self.indent_level -= 1;
        }

        self.indent_level -= 1;
        self.push_indent();
        self.output.push_str("}\n");
    }

    /// Emit one arm body. A block body becomes a braced group; a simple
    /// expression is emitted inline. `temp` prefixes the value with an
    /// assignment when the match is in value position.
    fn gen_match_arm_body(&mut self, body_expr: ExprId, body: &Body, temp: Option<&str>) {
        if let Expr::Block(block_id) = &body.exprs[body_expr] {
            let block = &body.blocks[*block_id];
            self.push_indent();
            self.output.push_str("{\n");
            self.indent_level += 1;
            for &stmt_idx in &block.stmts {
                let stmt = &body.stmts[stmt_idx];
                self.gen_stmt(stmt, body);
            }
            // The tail is the block's value. With no tail the block is void:
            // emit no assignment so we never produce `_matchN = ;`.
            if let Some(tail) = block.tail {
                self.push_indent();
                if let Some(name) = temp {
                    emit!(self, "{} = ", name);
                }
                self.gen_expr(tail, body);
                self.output.push_str(";\n");
            }
            self.indent_level -= 1;
            self.push_indent();
            self.output.push_str("}\n");
        } else {
            self.push_indent();
            if let Some(name) = temp {
                emit!(self, "{} = ", name);
            }
            self.gen_expr(body_expr, body);
            self.output.push_str(";\n");
        }
    }

    pub(super) fn hoist_matches(&mut self, expr_idx: ExprId, body: &Body) {
        let match_ids = self.collect_match_ids(expr_idx, body);
        for mid in match_ids {
            let name = format!("_match{}", self.match_counter);
            self.match_counter += 1;

            // The match type is the first arm body's type (recorded in HIR).
            // When absent (e.g. a call-typed arm), fall back to int32 with a
            // visible note - documented v0.3 limitation, never `void*`.
            self.push_indent();
            match body.expr_types.get(mid) {
                Some(ty) => emitln!(self, "{} {};", CType::new(ty), name),
                None => emitln!(self, "int32_t /* match temp type unknown */ {};", name),
            }
            self.gen_match(mid, body, Some(&name));
            self.match_temps.insert(mid, name);
        }
    }

    fn collect_match_ids(&self, expr_idx: ExprId, body: &Body) -> SmallVec<[ExprId; 4]> {
        let mut ids = SmallVec::new();
        self.collect_match_ids_rec(expr_idx, body, &mut ids);
        ids
    }

    fn collect_match_ids_rec(
        &self,
        expr_idx: ExprId,
        body: &Body,
        out: &mut SmallVec<[ExprId; 4]>,
    ) {
        let expr = &body.exprs[expr_idx];
        match expr {
            Expr::Match { scrut, .. } => {
                self.collect_match_ids_rec(*scrut, body, out);
                out.push(expr_idx);
            }
            // Block boundaries: do NOT recurse into these
            Expr::If { .. } | Expr::Loop { .. } | Expr::Block(_) => {}
            _ => expr.for_each_child_expr(|child| self.collect_match_ids_rec(child, body, out)),
        }
    }
}
