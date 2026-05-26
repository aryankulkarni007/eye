//! Expression codegen. A value-position match is emitted here only as a
//! reference to its hoisted `_matchN` temp (see [`super::matches`]).

use super::CGen;
use super::types::CType;
use hir::core::{Body, Expr, ExprId, Literal, Resolution, TypeRef};

impl<'a> CGen<'a> {
    /// True if `expr` is an `if` that codegen renders as a C `if` statement
    /// rather than a `(cond ? a : b)` ternary. The ternary form is taken only
    /// when the if has an else branch and its then-block is a single tail
    /// value with no statements - so anything else is statement-shaped. Used
    /// to flatten `else { if }` into `else if` (a ternary cannot follow a bare
    /// `else`, so those fall back to braces).
    fn if_is_statement_shaped(&self, expr_idx: ExprId, body: &Body) -> bool {
        let Expr::If {
            then_branch,
            else_branch,
            ..
        } = &body.exprs[expr_idx]
        else {
            return false;
        };
        let then_block = &body.blocks[*then_branch];
        !(else_branch.is_some() && then_block.stmts.is_empty() && then_block.tail.is_some())
    }

    pub(super) fn gen_expr(&mut self, expr_idx: ExprId, body: &Body) {
        match &body.exprs[expr_idx] {
            Expr::Missing => self.output.push_str("/* MISSING EXPR */"),
            Expr::Literal(literal) => match literal {
                Literal::Int(val) => self.output.push_str(&val.to_string()),
                Literal::Float(val) => self.output.push_str(val.as_str()),
                Literal::String(val) => emit!(self, "\"{}\"", val),
                Literal::Bool(val) => self.output.push_str(if *val { "true" } else { "false" }),
                Literal::Char(val) => emit!(self, "'{}'", val),
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
                emit!(self, "({}){{ ", CType::new(ty));
                self.comma_sep(fields.iter(), |this, field| {
                    emit!(this, ".{} = ", field.name);
                    this.gen_expr(field.value, body);
                });
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
                self.comma_sep(args.iter().copied(), |this, arg| this.gen_expr(arg, body));
                self.output.push(')');
            }
            Expr::ArrayLit(elems) => {
                // `{a, b, c}` - a C brace-enclosed initializer list. The
                // supported array path uses this in declaration initializers.
                self.output.push('{');
                self.comma_sep(elems.iter().copied(), |this, e| this.gen_expr(e, body));
                self.output.push('}');
            }
            Expr::Index { base, index } => {
                self.gen_expr(*base, body);
                self.output.push('[');
                self.gen_expr(*index, body);
                self.output.push(']');
            }
            Expr::Field { base, name } => {
                self.gen_expr(*base, body);
                let mut is_pointer_like = false;
                if let Some(base_ty) = self.get_expr_type(*base, body) {
                    is_pointer_like = matches!(base_ty, TypeRef::Ref(_) | TypeRef::Ptr(_));
                }

                if is_pointer_like {
                    emit!(self, "->{}", name);
                } else {
                    emit!(self, ".{}", name);
                }
            }
            Expr::Binary { op, lhs, rhs } => {
                self.output.push('(');
                self.gen_expr(*lhs, body);
                emit!(self, " {} ", op);
                self.gen_expr(*rhs, body);
                self.output.push(')');
            }
            Expr::Unary { op, operand } => {
                emit!(self, "{}", op);
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
                        let else_block = &body.blocks[*else_id];

                        // Flatten a desugared `else { if ... }` (the parser's
                        // `else if` form) back to `else if (...)` when the
                        // chained if renders as a statement. The braces-wrapped
                        // fallback handles value-shaped (ternary) inner ifs,
                        // which cannot follow a bare `else`.
                        if else_block.stmts.is_empty()
                            && let Some(tail) = else_block.tail
                            && self.if_is_statement_shaped(tail, body)
                        {
                            self.output.push_str(" else ");
                            self.gen_expr(tail, body);
                        } else {
                            self.output.push_str(" else {\n");
                            self.indent_level += 1;
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
            Expr::Cast { operand, ty } => {
                emit!(self, "({})", CType::new(ty));
                self.gen_expr(*operand, body);
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
