//! `match` lowering (Strategy A) and the shared value hoist. Statement-position
//! matches become a direct `switch`; value-position `match` and `if` are hoisted
//! into `_matchN`/`_ifN` temps declared ahead of their enclosing statement, then
//! referenced at the use site. `if` shares this path so it is never a ternary.

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

    /// Hoist every value-position `match` and `if` reachable from `expr_idx`
    /// into a `_matchN`/`_ifN` temp, declared and filled ahead of the enclosing
    /// statement, so the use site only references the temp. Both control forms
    /// share one mechanism and one counter; an `if` is never a ternary.
    pub(super) fn hoist_values(&mut self, expr_idx: ExprId, body: &Body) {
        let ids = self.collect_value_ids(expr_idx, body);
        for id in ids {
            match &body.exprs[id] {
                Expr::Match { .. } => {
                    let name = format!("_match{}", self.temp_counter);
                    self.temp_counter += 1;

                    // The match type is the first arm body's type (recorded in
                    // HIR). When absent (e.g. a call-typed arm), fall back to
                    // int32 with a visible note - documented v0.3 limitation,
                    // never `void*`.
                    self.push_indent();
                    match body.expr_types.get(id) {
                        Some(ty) => emitln!(self, "{} {};", CType::new(ty), name),
                        None => emitln!(self, "int32_t /* match temp type unknown */ {};", name),
                    }
                    self.gen_match(id, body, Some(&name));
                    self.value_temps.insert(id, name);
                }
                Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);

                    // The condition is value-position too; hoist any nested
                    // value forms there first so they are declared and filled
                    // before this `if` reads them.
                    self.hoist_values(cond, body);

                    let name = format!("_if{}", self.temp_counter);
                    self.temp_counter += 1;

                    self.push_indent();
                    match body.expr_types.get(id) {
                        Some(ty) => emitln!(self, "{} {};", CType::new(ty), name),
                        None => emitln!(self, "int32_t /* if temp type unknown */ {};", name),
                    }
                    self.push_indent();
                    self.gen_if_statement(cond, then_branch, else_branch, body, Some(&name));
                    self.output.push('\n');
                    self.value_temps.insert(id, name);
                }
                _ => unreachable!("collect_value_ids yields only Match and If"),
            }
        }
    }

    fn collect_value_ids(&self, expr_idx: ExprId, body: &Body) -> SmallVec<[ExprId; 4]> {
        let mut ids = SmallVec::new();
        self.collect_value_ids_rec(expr_idx, body, &mut ids);
        ids
    }

    fn collect_value_ids_rec(
        &self,
        expr_idx: ExprId,
        body: &Body,
        out: &mut SmallVec<[ExprId; 4]>,
    ) {
        let expr = &body.exprs[expr_idx];
        match expr {
            Expr::Match { scrut, .. } => {
                self.collect_value_ids_rec(*scrut, body, out);
                out.push(expr_idx);
            }
            // An `if` is hoisted as a unit; its condition and branch interiors
            // are handled when `hoist_values` emits it, so do not recurse here.
            Expr::If { .. } => out.push(expr_idx),
            // Block boundaries: do NOT recurse into these.
            Expr::Loop { .. } | Expr::Block(_) => {}
            _ => expr.for_each_child_expr(|child| self.collect_value_ids_rec(child, body, out)),
        }
    }
}
