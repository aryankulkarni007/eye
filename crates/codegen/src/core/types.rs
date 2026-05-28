//! Type and format-specifier mapping: HIR `TypeRef` -> C type strings and
//! printf specifiers.

use super::CGen;
use hir::core::{Body, Expr, ExprId, Resolution, TypeRef};

impl<'a> CGen<'a> {
    pub(super) fn map_type_ref(&self, ty: &TypeRef) -> String {
        match ty {
            TypeRef::Path(name) => match name.as_str() {
                "int32" => "int32_t".to_string(),
                "float32" => "float".to_string(),
                "float64" => "double".to_string(),
                "bool" => "bool".to_string(),
                "char" => "char".to_string(),
                "string" => "const char*".to_string(), // string literal base
                other => other.to_string(),
            },
            TypeRef::Ref(inner) => format!("{}*", self.map_type_ref(inner)),
            TypeRef::Ptr(inner) => format!("{}*", self.map_type_ref(inner)),
            TypeRef::Error => "void* /* ERROR TY */".to_string(),
        }
    }

    pub(super) fn get_expr_type(&self, expr_idx: ExprId, body: &Body) -> Option<TypeRef> {
        // check the explicit type map
        if let Some(ty) = body.expr_types.get(expr_idx) {
            return Some(ty.clone());
        }

        // then try to derive it from the expression itself
        match &body.exprs[expr_idx] {
            Expr::Path(Resolution::Local(local_id)) => {
                // the local should have its type from lowering
                body.locals[*local_id].ty.clone()
            }
            Expr::Field { base, name } => {
                let parent_ty = self.get_expr_type(*base, body)?;

                let struct_name = match &parent_ty {
                    TypeRef::Path(n) => n.clone(),
                    TypeRef::Ref(inner) | TypeRef::Ptr(inner) => match inner.as_ref() {
                        TypeRef::Path(n) => n.clone(),
                        _ => return None,
                    },
                    _ => return None,
                };

                let struct_def = self
                    .hir
                    .structs
                    .iter()
                    .find(|(_, s)| s.name == struct_name)
                    .map(|(_, s)| s)?;

                for &field_id in &struct_def.fields {
                    let field = &self.hir.fields[field_id];
                    if field.name == *name {
                        return Some(field.ty.clone());
                    }
                }
                None
            }
            Expr::Ref { operand } => {
                // &expr has type Ref(inner_type)
                let inner = self.get_expr_type(*operand, body)?;
                Some(TypeRef::Ref(Box::new(inner)))
            }
            Expr::Deref { operand } => {
                // *expr has the inner type
                let op_ty = self.get_expr_type(*operand, body)?;
                match op_ty {
                    TypeRef::Ref(inner) | TypeRef::Ptr(inner) => Some(*inner),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn spec_for_type(ty: &TypeRef) -> &'static str {
        match ty {
            TypeRef::Path(name) => match name.as_str() {
                "int32" => "%d",
                // printf promotes float to double for variadics, so a single
                // `%f` covers both surface types.
                "float32" | "float64" => "%f",
                "bool" => "%d",
                "char" => "%c",
                "string" => "%s",
                // Unknown nominal type (likely a struct): no sensible printf
                // representation, but we still emit *something* so codegen
                // does not silently drop the placeholder.
                _ => "%d",
            },
            TypeRef::Ref(_) | TypeRef::Ptr(_) => "%p",
            TypeRef::Error => "%d",
        }
    }
}
