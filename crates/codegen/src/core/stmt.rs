//! Statement codegen. Handles the value-position match hoist for `let`
//! initializers and the statement-position match `switch`.

use super::CGen;
use super::types::CDeclarator;
use hir::core::{Body, Expr, Pat, Stmt};

impl<'a> CGen<'a> {
    pub(super) fn gen_stmt(&mut self, stmt: &Stmt, body: &Body) {
        match stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                mutable,
            } => {
                // Hoist value-position matches in the initializer so their
                // `_matchN` temps are declared and filled before this line.
                if let Some(expr_idx) = init {
                    self.hoist_matches(*expr_idx, body);
                }

                self.push_indent();

                if !*mutable {
                    self.output.push_str("const ");
                }

                let pat_node = &body.pats[*pat];
                let local_idx = match pat_node {
                    Pat::Bind(id) => *id,
                    // syntactically impossible: only Bind comes from let-pat
                    // lowering. Variant/Wildcard live in match arms; Missing
                    // means broken syntax. Skip instead of emitting invalid C.
                    Pat::Missing | Pat::Variant { .. } | Pat::Wildcard => return,
                };
                let var_name = body.locals[local_idx].name.clone();
                // `c_declarator` keeps an array's `[N]` next to the name
                // (`int xs[4]`); for every other type it is `<type> <name>`.
                match ty {
                    Some(t) => emit!(self, "{}", CDeclarator::new(t, &var_name)),
                    // FIXME: change once we have type inference
                    None => emit!(self, "/* EXPLICT TYPE MISSING */ {}", var_name),
                }

                if let Some(expr_idx) = init {
                    self.output.push_str(" = ");
                    self.gen_expr(*expr_idx, body);
                }

                self.output.push_str(";\n");
            }
            Stmt::Expr(expr_idx) => match &body.exprs[*expr_idx] {
                Expr::If { cond, .. } => {
                    // A match in the condition is value-position; hoist it.
                    self.hoist_matches(*cond, body);
                    self.push_indent();
                    self.gen_expr(*expr_idx, body);
                    self.output.push('\n');
                }
                Expr::Match { scrut, .. } => {
                    // Statement-position match: a direct `switch`, no temp and
                    // no trailing `;`. Hoist any match nested in the scrutinee.
                    self.hoist_matches(*scrut, body);
                    self.gen_match(*expr_idx, body, None);
                }
                _ => {
                    self.hoist_matches(*expr_idx, body);
                    self.push_indent();
                    self.gen_expr(*expr_idx, body);
                    self.output.push_str(";\n");
                }
            },
        }
    }
}
