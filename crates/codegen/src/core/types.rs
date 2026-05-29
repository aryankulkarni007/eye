//! Type and format-specifier mapping: HIR `TypeRef` -> C type strings and
//! printf specifiers.

use super::CGen;
use hir::core::{Body, Expr, ExprId, Resolution, TypeRef};
use std::fmt;

pub(super) struct CType<'a> {
    ty: &'a TypeRef,
}

impl<'a> CType<'a> {
    pub(super) fn new(ty: &'a TypeRef) -> Self {
        Self { ty }
    }
}

impl fmt::Display for CType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty {
            TypeRef::Path(name) => match name.as_str() {
                "int8" => f.write_str("int8_t"),
                "int16" => f.write_str("int16_t"),
                "int32" => f.write_str("int32_t"),
                "int64" => f.write_str("int64_t"),
                "uint8" => f.write_str("uint8_t"),
                "uint16" => f.write_str("uint16_t"),
                "uint32" => f.write_str("uint32_t"),
                "uint64" => f.write_str("uint64_t"),
                // pointer-width integers: the libc/FFI seam (malloc, memcpy,
                // strlen, sizeof, indexing) traffics in size_t/ptrdiff_t.
                // Width is platform-defined, so these map to the C library's
                // pointer-width types rather than fixed-width integers.
                "usize" => f.write_str("size_t"),
                "isize" => f.write_str("ptrdiff_t"),
                "float32" => f.write_str("float"),
                "float64" => f.write_str("double"),
                "bool" => f.write_str("bool"),
                "char" => f.write_str("char"),
                "string" => f.write_str("const char*"),
                "ptr" => f.write_str("void*"),
                other => f.write_str(other),
            },
            TypeRef::Ref(inner) | TypeRef::Ptr(inner) => write!(f, "{}*", CType::new(inner)),
            // An array is a value: it renders as its struct-wrap typedef name
            // (see `arrays`). The wrapper makes copy, by-value passing, return,
            // and multi-dimensional nesting all work as plain C struct values.
            TypeRef::Array { elem, len } => {
                f.write_str(&super::arrays::array_wrapper_name(elem, *len))
            }
            TypeRef::Error => f.write_str("void* /* ERROR TY */"),
        }
    }
}

pub(super) struct CDeclarator<'a> {
    ty: &'a TypeRef,
    name: &'a str,
}

impl<'a> CDeclarator<'a> {
    pub(super) fn new(ty: &'a TypeRef, name: &'a str) -> Self {
        Self { ty, name }
    }
}

impl fmt::Display for CDeclarator<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every type, arrays included, is now a plain C value: `<type> <name>`.
        // Arrays carry their `[N]` inside the wrapper struct, not the
        // declarator, so there is no special nesting here anymore.
        write!(f, "{} {}", CType::new(self.ty), self.name)
    }
}

impl<'a> CGen<'a> {
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
                    TypeRef::Path(n) => n,
                    TypeRef::Ref(inner) | TypeRef::Ptr(inner) => match inner.as_ref() {
                        TypeRef::Path(n) => n,
                        _ => return None,
                    },
                    _ => return None,
                };

                // Field-typed lookup spans both products (structs) and unions
                // - they share the field arena, so a union member resolves the
                // same way.
                let field_id = self
                    .hir
                    .items
                    .structs
                    .get(struct_name)
                    .and_then(|&id| self.hir.structs[id].field_index.get(name).copied())
                    .or_else(|| {
                        self.hir
                            .items
                            .unions
                            .get(struct_name)
                            .and_then(|&id| self.hir.unions[id].field_index.get(name).copied())
                    });

                field_id.map(|id| self.hir.fields[id].ty.clone())
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
            Expr::Index { base, .. } => {
                // base[i] has the element/pointee type of the base.
                let base_ty = self.get_expr_type(*base, body)?;
                match base_ty {
                    TypeRef::Array { elem, .. } => Some(*elem),
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
                // int8/int16 default-promote to int, so %d is correct.
                "int8" | "int16" | "int32" => "%d",
                "int64" => "%lld",
                // uint8/uint16 default-promote to int; %u reads the same small
                // positive value. uint32 is unsigned int.
                "uint8" | "uint16" | "uint32" => "%u",
                "uint64" => "%llu",
                // C99 length modifiers: %zu for size_t, %td for ptrdiff_t.
                "usize" => "%zu",
                "isize" => "%td",
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
            // refs/pointers and decayed arrays print as addresses.
            TypeRef::Ref(_) | TypeRef::Ptr(_) | TypeRef::Array { .. } => "%p",
            TypeRef::Error => "%d",
        }
    }
}
