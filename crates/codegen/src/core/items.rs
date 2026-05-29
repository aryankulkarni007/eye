//! Top-level declaration codegen: structs, enums, functions, and the function
//! body driver.

use super::CGen;
use super::types::CType;
use hir::core::{Body, Enum, Expr, Function, Struct, Union};

impl<'a> CGen<'a> {
    pub(super) fn gen_struct(&mut self, struct_def: &Struct) {
        self.output.push_str("typedef struct {\n");
        self.indent_level += 1;

        for &field_id in &struct_def.fields {
            let field = &self.hir.fields[field_id];
            self.push_indent();
            emitln!(self, "{} {};", CType::new(&field.ty), field.name);
        }

        self.indent_level -= 1;
        emit!(self, "}} {};\n\n", struct_def.name);
    }

    // Same shape as `gen_struct`, emitting `union` for overlapping storage.
    pub(super) fn gen_union(&mut self, union_def: &Union) {
        self.output.push_str("typedef union {\n");
        self.indent_level += 1;

        for &field_id in &union_def.fields {
            let field = &self.hir.fields[field_id];
            self.push_indent();
            emitln!(self, "{} {};", CType::new(&field.ty), field.name);
        }

        self.indent_level -= 1;
        emit!(self, "}} {};\n\n", union_def.name);
    }

    pub(super) fn gen_enum(&mut self, enum_def: &Enum) {
        self.output.push_str("typedef enum {\n");
        self.indent_level += 1;
        for variant in &enum_def.variants {
            self.push_indent();
            emitln!(self, "{},", variant.name);
        }
        self.indent_level -= 1;
        emit!(self, "}} {};\n\n", enum_def.name);
    }

    pub(super) fn gen_function(&mut self, r#fn: &Function) {
        // `_matchN`/`_ifN` temp names are local to each C function.
        self.value_temps.clear();
        self.temp_counter = 0;

        // An extern fn is a bare prototype: signature then `;`, no body. The
        // linker binds the symbol (libc for the v0.4 alloc/IO seam).
        if r#fn.is_extern {
            match &r#fn.ret {
                Some(ret) => emit!(self, "{} {}(", CType::new(ret), r#fn.name),
                None => emit!(self, "void {}(", r#fn.name),
            };
            self.comma_sep(r#fn.params.iter(), |this, param| {
                emit!(this, "{}", CType::new(&param.ty));
            });
            self.output.push_str(");\n");
            return;
        }

        if r#fn.name == "main" {
            emit!(self, "int {}(", r#fn.name);
        } else {
            match &r#fn.ret {
                Some(ret) => emit!(self, "{} {}(", CType::new(ret), r#fn.name),
                None => emit!(self, "void {}(", r#fn.name),
            }
        };

        self.comma_sep(r#fn.params.iter(), |this, param| {
            emit!(this, "{} {}", CType::new(&param.ty), param.name);
        });
        self.output.push_str(") {\n");
        self.indent_level += 1;

        if let Some(body_id) = r#fn.body {
            let body = &self.hir.bodies[body_id];
            self.gen_body(body);

            if let Some(tail_expr_idx) = body.tail {
                let returns_value = r#fn.name != "main" && r#fn.ret.is_some();
                if returns_value {
                    // The tail is the implicit return value. A value-position
                    // `match`/`if` must be hoisted into its temp before the
                    // `return`, same as a `let` initializer, so the return reads
                    // the temp instead of an unhoisted form.
                    self.hoist_values(tail_expr_idx, body);
                    self.push_indent();
                    self.output.push_str("return ");
                    self.gen_expr(tail_expr_idx, body);
                    self.output.push_str(";\n");
                } else if let Expr::Match { scrut, .. } = &body.exprs[tail_expr_idx] {
                    // Tail value is discarded (`main` / void fn): the match runs
                    // for effect, so emit a bare statement-position `switch`.
                    self.hoist_values(*scrut, body);
                    self.gen_match(tail_expr_idx, body, None);
                } else if let Expr::If {
                    cond,
                    then_branch,
                    else_branch,
                } = &body.exprs[tail_expr_idx]
                {
                    // Tail value is discarded: emit a C `if` statement, not a
                    // ternary (which an else-less or statement-bodied chain
                    // cannot form).
                    let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                    self.hoist_values(cond, body);
                    self.push_indent();
                    self.gen_if_statement(cond, then_branch, else_branch, body, None);
                    self.output.push('\n');
                } else {
                    self.push_indent();
                    self.gen_expr(tail_expr_idx, body);
                    self.output.push_str(";\n");
                }
            }
        }

        if r#fn.name == "main" {
            self.push_indent();
            self.output.push_str("return 0;\n");
        }

        self.indent_level -= 1;
        self.output.push_str("}\n\n");
    }

    pub fn gen_body(&mut self, body: &Body) {
        for &stmt_idx in &body.block {
            let stmt = &body.stmts[stmt_idx];
            self.gen_stmt(stmt, body);
        }
    }
}
