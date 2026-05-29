//! Statement codegen. Handles the value-position `match`/`if` hoist for `let`
//! initializers and the statement-position `if` chain and match `switch`.

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
                // Hoist value-position `match`/`if` in the initializer so their
                // `_matchN`/`_ifN` temps are declared and filled before this line.
                if let Some(expr_idx) = init {
                    self.hoist_values(*expr_idx, body);
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
                    None => emit!(self, "/* EXPLICIT TYPE MISSING */ {}", var_name),
                }

                if let Some(expr_idx) = init {
                    self.output.push_str(" = ");
                    self.gen_expr(*expr_idx, body);
                }

                self.output.push_str(";\n");
            }
            Stmt::Expr(expr_idx) => match &body.exprs[*expr_idx] {
                Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                    // Statement position: the value is discarded, so this is
                    // always a C `if` statement, never a temp. A value-position
                    // `match`/`if` in the condition is hoisted first.
                    self.hoist_values(cond, body);
                    self.push_indent();
                    self.gen_if_statement(cond, then_branch, else_branch, body, None);
                    self.output.push('\n');
                }
                Expr::Match { scrut, .. } => {
                    // Statement-position match: a direct `switch`, no temp and
                    // no trailing `;`. Hoist any match nested in the scrutinee.
                    self.hoist_values(*scrut, body);
                    self.gen_match(*expr_idx, body, None);
                }
                _ => {
                    self.hoist_values(*expr_idx, body);
                    self.push_indent();
                    self.gen_expr(*expr_idx, body);
                    self.output.push_str(";\n");
                }
            },
        }
    }
}
