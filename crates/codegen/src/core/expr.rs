//! Expression codegen. A value-position match is emitted here only as a
//! reference to its hoisted `_matchN` temp (see [`super::matches`]).

use super::CGen;
use hir::core::{Body, Expr, ExprId, Literal, Resolution, TypeRef};

impl<'a> CGen<'a> {
    pub(super) fn gen_expr(&mut self, expr_idx: ExprId, body: &Body) {
        match &body.exprs[expr_idx] {
            Expr::Missing => self.output.push_str("/* MISSING EXPR */"),
            Expr::Literal(literal) => match literal {
                Literal::Int(val) => self.output.push_str(&val.to_string()),
                Literal::Float(val) => self.output.push_str(val.as_str()),
                Literal::String(val) => self.output.push_str(&format!("\"{}\"", val)),
                Literal::Bool(val) => self.output.push_str(if *val { "true" } else { "false" }),
                Literal::Char(val) => self.output.push_str(&format!("'{}'", val)),
            },
            Expr::Path(resolution) => match resolution {
                Resolution::Local(id) => self.output.push_str(&body.locals[*id].name),
                Resolution::Fn(id) => self.output.push_str(&self.hir.functions[*id].name),
                Resolution::Struct(id) => self.output.push_str(&self.hir.structs[*id].name),
                Resolution::Unresolved(name) => self.output.push_str(name.as_str()),
                Resolution::Variant { enum_id, idx } => {
                    let variant = &self.hir.enums[*enum_id].variants[*idx as usize];
                    self.output.push_str(&variant.name);
                }
                Resolution::Enum(_) => {
                    // HIR lowering converts bare enum-name in expr position to
                    // a diagnostic + Expr::Missing, so codegen never sees this.
                    unreachable!("Resolution::Enum reached codegen; HIR should have rejected it");
                }
            },
            Expr::StructLit { ty, fields } => {
                let ty_str = self.map_type_ref(ty);
                self.output.push_str(&format!("({}){{ ", ty_str));
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.output.push_str(&format!(".{} = ", field.name));
                    self.gen_expr(field.value, body);
                }
                self.output.push_str(" }");
            }
            Expr::Call { callee, args } => {
                if let Expr::Path(Resolution::Unresolved(name)) = &body.exprs[*callee]
                    && name == "print"
                {
                    self.gen_print(args, body);
                    return;
                }
                self.gen_expr(*callee, body);
                self.output.push('(');
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.output.push_str(", ");
                    }
                    self.gen_expr(*arg, body);
                }
                self.output.push(')');
            }
            Expr::Field { base, name } => {
                self.gen_expr(*base, body);
                let mut is_pointer_like = false;
                if let Some(base_ty) = self.get_expr_type(*base, body) {
                    is_pointer_like = matches!(base_ty, TypeRef::Ref(_) | TypeRef::Ptr(_));
                }

                if is_pointer_like {
                    self.output.push_str(&format!("->{}", name));
                } else {
                    self.output.push_str(&format!(".{}", name));
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                self.output.push('(');
                self.gen_expr(*lhs, body);
                self.output.push_str(&format!(" {} ", op));
                self.gen_expr(*rhs, body);
                self.output.push(')');
            }
            Expr::Unary { op, operand } => {
                self.output.push_str(&format!("{}", op));
                self.gen_expr(*operand, body);
            }
            Expr::Block(block_id) => {
                let block = &body.blocks[*block_id];

                self.output.push_str("{\n");
                self.indent_level += 1;

                for &stmt_idx in &block.stmts {
                    let stmt = &body.stmts[stmt_idx];
                    self.gen_stmt(stmt, body);
                }

                if let Some(tail_expr_idx) = block.tail {
                    self.push_indent();
                    self.gen_expr(tail_expr_idx, body);
                    self.output.push_str(";\n");
                }

                self.indent_level -= 1;
                self.push_indent();
                self.output.push('}');
            }
            Expr::Assign { lhs, rhs } => {
                self.gen_expr(*lhs, body);
                self.output.push_str(" = ");
                self.gen_expr(*rhs, body);
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let then_block = &body.blocks[*then_branch];

                // If it has an else branch AND contains no inner statements,
                // it's an expression-based ternary assignment context!
                if let Some(_else_id) = else_branch
                    && then_block.stmts.is_empty()
                    && let Some(_then_tail) = then_block.tail
                {
                    self.output.push('(');
                    self.gen_expr(*cond, body);
                    self.output.push_str(" ? ");
                    self.gen_expr(then_block.tail.unwrap(), body);
                    self.output.push_str(" : ");

                    let else_block = &body.blocks[else_branch.unwrap()];
                    if let Some(else_tail) = else_block.tail {
                        self.gen_expr(else_tail, body);
                    } else {
                        self.output.push('0');
                    }
                    self.output.push(')');
                } else {
                    // conditional control statement block
                    self.output.push_str("if (");
                    self.gen_expr(*cond, body);
                    self.output.push_str(") {\n");
                    self.indent_level += 1;

                    for &stmt_idx in &then_block.stmts {
                        let stmt = &body.stmts[stmt_idx];
                        self.gen_stmt(stmt, body);
                    }
                    if let Some(tail) = then_block.tail {
                        self.push_indent();
                        self.gen_expr(tail, body);
                        self.output.push_str(";\n");
                    }

                    self.indent_level -= 1;
                    self.push_indent();
                    self.output.push('}');

                    if let Some(else_id) = else_branch {
                        self.output.push_str(" else {\n");
                        self.indent_level += 1;
                        let else_block = &body.blocks[*else_id];
                        for &stmt_idx in &else_block.stmts {
                            let stmt = &body.stmts[stmt_idx];
                            self.gen_stmt(stmt, body);
                        }
                        if let Some(tail) = else_block.tail {
                            self.push_indent();
                            self.gen_expr(tail, body);
                            self.output.push_str(";\n");
                        }
                        self.indent_level -= 1;
                        self.push_indent();
                        self.output.push('}');
                    }
                }
            }
            Expr::Loop { body: block_id } => {
                self.output.push_str("while (true) {\n");
                self.indent_level += 1;

                let block = &body.blocks[*block_id];
                for &stmt_idx in &block.stmts {
                    let stmt = &body.stmts[stmt_idx];
                    self.gen_stmt(stmt, body);
                }
                if let Some(tail) = block.tail {
                    self.push_indent();
                    self.gen_expr(tail, body);
                    self.output.push_str(";\n");
                }
                self.indent_level -= 1;
                self.push_indent();
                self.output.push('}');
            }
            Expr::Break => self.output.push_str("break"),
            Expr::Continue => self.output.push_str("continue"),
            Expr::Ref { operand } => {
                self.output.push('&');
                self.gen_expr(*operand, body);
            }
            Expr::Deref { operand } => {
                self.output.push('(');
                self.output.push('*');
                self.gen_expr(*operand, body);
                self.output.push(')');
            }
            Expr::Match { .. } => {
                // Value-position match. It was hoisted into a `_matchN` temp by
                // `hoist_matches` before the enclosing statement was emitted, so
                // here we only reference that temp. A miss means the match sits
                // in a context the hoist walk doesn't cover (e.g. a ternary
                // branch); emit a visible marker rather than dropping it.
                match self.match_temps.get(&expr_idx) {
                    Some(name) => {
                        let name = name.clone();
                        self.output.push_str(&name);
                    }
                    None => self.output.push_str("/* UNHOISTED MATCH */"),
                }
            }
        }
    }
}
