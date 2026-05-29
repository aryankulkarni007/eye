//! Top-level declaration codegen: structs, enums, functions, and the function
//! body driver.

use super::CGen;
use hir::core::{Body, Enum, Function, Struct, Union};

impl<'a> CGen<'a> {
    pub(super) fn gen_struct(&mut self, struct_def: &Struct) {
        self.output.push_str("typedef struct {\n");
        self.indent_level += 1;

        for &field_id in &struct_def.fields {
            let field = &self.hir.fields[field_id];
            self.push_indent();
            let ty_str = self.map_type_ref(&field.ty);
            self.output
                .push_str(&format!("{} {};\n", ty_str, field.name));
        }

        self.indent_level -= 1;
        self.output
            .push_str(&format!("}} {};\n\n", struct_def.name));
    }

    // Same shape as `gen_struct`, emitting `union` for overlapping storage.
    pub(super) fn gen_union(&mut self, union_def: &Union) {
        self.output.push_str("typedef union {\n");
        self.indent_level += 1;

        for &field_id in &union_def.fields {
            let field = &self.hir.fields[field_id];
            self.push_indent();
            let ty_str = self.map_type_ref(&field.ty);
            self.output
                .push_str(&format!("{} {};\n", ty_str, field.name));
        }

        self.indent_level -= 1;
        self.output.push_str(&format!("}} {};\n\n", union_def.name));
    }

    pub(super) fn gen_enum(&mut self, enum_def: &Enum) {
        self.output.push_str("typedef enum {\n");
        self.indent_level += 1;
        for variant in &enum_def.variants {
            self.push_indent();
            self.output.push_str(&format!("{},\n", variant.name));
        }
        self.indent_level -= 1;
        self.output.push_str(&format!("}} {};\n\n", enum_def.name));
    }

    pub(super) fn gen_function(&mut self, r#fn: &Function) {
        // `_matchN` temp names are local to each C function.
        self.match_temps.clear();
        self.match_counter = 0;

        // An extern fn is a bare prototype: signature then `;`, no body. The
        // linker binds the symbol (libc for the v0.4 alloc/IO seam).
        if r#fn.is_extern {
            let ret_type = r#fn
                .ret
                .as_ref()
                .map_or("void".to_string(), |t| self.map_type_ref(t));
            self.output
                .push_str(&format!("{} {}(", ret_type, r#fn.name));
            for (i, param) in r#fn.params.iter().enumerate() {
                if i > 0 {
                    self.output.push_str(", ");
                }
                self.output.push_str(&self.map_type_ref(&param.ty));
            }
            self.output.push_str(");\n");
            return;
        }

        let ret_type = if r#fn.name == "main" {
            "int".to_string()
        } else {
            r#fn.ret
                .as_ref()
                .map_or("void".to_string(), |t| self.map_type_ref(t))
        };

        self.output
            .push_str(&format!("{} {}(", ret_type, r#fn.name));

        for (i, param) in r#fn.params.iter().enumerate() {
            if i > 0 {
                self.output.push_str(", ");
            }
            let p_ty = self.map_type_ref(&param.ty);
            self.output.push_str(&format!("{} {}", p_ty, param.name));
        }
        self.output.push_str(") {\n");
        self.indent_level += 1;

        if let Some(body_id) = r#fn.body {
            let body = &self.hir.bodies[body_id];
            self.gen_body(body);

            if let Some(tail_expr_idx) = body.tail {
                self.push_indent();
                if r#fn.name != "main" && r#fn.ret.is_some() {
                    self.output.push_str("return ");
                }
                self.gen_expr(tail_expr_idx, body);
                self.output.push_str(";\n");
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
