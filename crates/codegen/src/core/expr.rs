//! Expression codegen. A value-position `match`/`if` is emitted here only as a
//! reference to its hoisted `_matchN`/`_ifN` temp (see [`super::matches`]); the
//! statement form of `if` lives in [`CGen::gen_if_statement`].

use super::CGen;
use super::types::CType;
use hir::core::{BlockId, Body, Expr, ExprId, Literal, Resolution, TypeRef};

impl<'a> CGen<'a> {
    /// Render an `if` as a C `if`/`else if`/`else` statement chain - the only
    /// form `if` ever takes (there is no ternary). In statement position pass
    /// `temp = None` and the value is discarded. In value position the `if` is
    /// hoisted ahead of its enclosing statement (see [`super::matches`]) with
    /// `temp = Some(name)`, and each branch's tail value is assigned into
    /// `name`, mirroring `gen_match`. An `else { if ... }` block (the parser's
    /// `else if` desugaring) is flattened back to `else if`, recursing through
    /// this same renderer with the same `temp`.
    pub(super) fn gen_if_statement(
        &mut self,
        cond: ExprId,
        then_branch: BlockId,
        else_branch: Option<BlockId>,
        body: &Body,
        temp: Option<&str>,
    ) {
        self.output.push_str("if (");
        self.gen_expr(cond, body);
        self.output.push_str(") {\n");
        self.indent_level += 1;

        let then_block = &body.blocks[then_branch];
        for &stmt_idx in &then_block.stmts {
            let stmt = &body.stmts[stmt_idx];
            self.gen_stmt(stmt, body);
        }
        if let Some(tail) = then_block.tail {
            self.push_indent();
            if let Some(name) = temp {
                emit!(self, "{} = ", name);
            }
            self.gen_expr(tail, body);
            self.output.push_str(";\n");
        }

        self.indent_level -= 1;
        self.push_indent();
        self.output.push('}');

        let Some(else_id) = else_branch else {
            return;
        };
        let else_block = &body.blocks[else_id];

        // Flatten a desugared `else { if ... }` back to `else if`, recursing
        // through this renderer (with the same `temp`) so a chained if never
        // picks up braces.
        if else_block.stmts.is_empty()
            && let Some(tail) = else_block.tail
            && let Expr::If {
                cond,
                then_branch,
                else_branch,
            } = &body.exprs[tail]
        {
            self.output.push_str(" else ");
            self.gen_if_statement(*cond, *then_branch, *else_branch, body, temp);
            return;
        }

        self.output.push_str(" else {\n");
        self.indent_level += 1;
        for &stmt_idx in &else_block.stmts {
            let stmt = &body.stmts[stmt_idx];
            self.gen_stmt(stmt, body);
        }
        if let Some(tail) = else_block.tail {
            self.push_indent();
            if let Some(name) = temp {
                emit!(self, "{} = ", name);
            }
            self.gen_expr(tail, body);
            self.output.push_str(";\n");
        }
        self.indent_level -= 1;
        self.push_indent();
        self.output.push('}');
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
                // A value array is a compound literal of its wrapper struct:
                // `(__eye_arr_T_N){{ a, b, c }}` - the outer brace is the
                // struct, the inner initializes its `data[N]`. Valid in both
                // initializer and rvalue position, so by-value passing and
                // returning an array literal work.
                if let Some(TypeRef::Array { elem, len }) = self.get_expr_type(expr_idx, body) {
                    emit!(
                        self,
                        "({}){{{{ ",
                        super::arrays::array_wrapper_name(&elem, len)
                    );
                    self.comma_sep(elems.iter().copied(), |this, e| this.gen_expr(e, body));
                    self.output.push_str(" }}");
                } else {
                    // Element type unknown: fall back to a bare brace list (only
                    // valid in a declaration initializer).
                    self.output.push('{');
                    self.comma_sep(elems.iter().copied(), |this, e| this.gen_expr(e, body));
                    self.output.push('}');
                }
            }
            Expr::Index { base, index } => {
                // Arrays are wrapper structs, so indexing reaches through the
                // `data` field. A value array uses `.data[i]`; a reference or
                // pointer to an array uses `->data[i]`; a raw pointer indexes
                // directly.
                let base_ty = self.get_expr_type(*base, body);
                self.gen_expr(*base, body);
                match base_ty {
                    Some(TypeRef::Array { .. }) => self.output.push_str(".data["),
                    Some(TypeRef::Ref(inner) | TypeRef::Ptr(inner))
                        if matches!(*inner, TypeRef::Array { .. }) =>
                    {
                        self.output.push_str("->data[")
                    }
                    _ => self.output.push('['),
                }
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
            Expr::Assign { op, lhs, rhs } => {
                self.gen_expr(*lhs, body);
                emit!(self, " {} ", op);
                self.gen_expr(*rhs, body);
            }
            Expr::If { .. } => {
                // Value-position `if`. Like a value-position match, it was
                // hoisted into a `_ifN` temp by `hoist_values` before the
                // enclosing statement, so here we only reference that temp. A
                // statement-position `if` never reaches this arm - `gen_stmt`
                // and the discarded-tail path in `items` call `gen_if_statement`
                // directly. A miss means the hoist walk didn't cover this spot.
                match self.value_temps.get(&expr_idx) {
                    Some(name) => {
                        let name = name.clone();
                        self.output.push_str(&name);
                    }
                    None => self.output.push_str("/* UNHOISTED IF */"),
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
                // `hoist_values` before the enclosing statement was emitted, so
                // here we only reference that temp. A miss means the match sits
                // in a context the hoist walk doesn't cover; emit a visible
                // marker rather than dropping it.
                match self.value_temps.get(&expr_idx) {
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
