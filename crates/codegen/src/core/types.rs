//! type and format-specifier mapping: HIR `TypeRef` -> c type strings and
//! printf specifiers.

use hir::core::{TypeInterner, TypeKind, TypeRef};
use std::fmt;

pub(super) struct CType<'a> {
    ty: TypeRef,
    types: &'a TypeInterner,
}

impl<'a> CType<'a> {
    pub(super) fn new(ty: TypeRef, types: &'a TypeInterner) -> Self {
        Self { ty, types }
    }
}

impl fmt::Display for CType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.types.lookup(self.ty) {
            TypeKind::Path(name) => match name.as_str() {
                "int8" => f.write_str("int8_t"),
                "int16" => f.write_str("int16_t"),
                "int32" => f.write_str("int32_t"),
                "int64" => f.write_str("int64_t"),
                "uint8" => f.write_str("uint8_t"),
                "uint16" => f.write_str("uint16_t"),
                "uint32" => f.write_str("uint32_t"),
                "uint64" => f.write_str("uint64_t"),
                "usize" => f.write_str("size_t"),
                "isize" => f.write_str("ptrdiff_t"),
                "float32" => f.write_str("float"),
                "float64" => f.write_str("double"),
                "bool" => f.write_str("bool"),
                "char" => f.write_str("char"),
                "string" => f.write_str("const char*"),
                other => f.write_str(other),
            },
            TypeKind::RawPtr => f.write_str("void*"),
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => {
                write!(f, "{}*", CType::new(*inner, self.types))
            }
            TypeKind::Array { elem, len } => {
                f.write_str(&super::arrays::array_wrapper_name(*elem, *len, self.types))
            }
            TypeKind::Fn {
                params,
                ret,
                variadic,
            } => f.write_str(&super::arrays::fn_typedef_name(
                params, *ret, *variadic, self.types,
            )),
            // `()` and `!` carry no value; a clean program never gives a temp
            // either type (value-position unit is rejected, never coerces away),
            // so this only renders in a declaration position that is itself
            // unreachable. `void` is the honest c spelling.
            TypeKind::Unit | TypeKind::Never => f.write_str("void"),
            TypeKind::Error => f.write_str("void* /* ERROR TY */"),
        }
    }
}

pub(super) struct CDeclarator<'a> {
    ty: TypeRef,
    name: &'a str,
    types: &'a TypeInterner,
}

impl<'a> CDeclarator<'a> {
    pub(super) fn new(ty: TypeRef, name: &'a str, types: &'a TypeInterner) -> Self {
        Self { ty, name, types }
    }
}

impl fmt::Display for CDeclarator<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", CType::new(self.ty, self.types), self.name)
    }
}

/// printf format specifier for a value of type `ty`. a pure type -> specifier
/// map, read by the MIR emitter's `print` lowering (one specifier per `{}`).
pub(super) fn spec_for_type(ty: TypeRef, types: &TypeInterner) -> &'static str {
    match types.lookup(ty) {
        TypeKind::Path(name) => match name.as_str() {
            "int8" | "int16" | "int32" => "%d",
            "int64" => "%lld",
            "uint8" | "uint16" | "uint32" => "%u",
            "uint64" => "%llu",
            "usize" => "%zu",
            "isize" => "%td",
            "float32" | "float64" => "%f",
            "bool" => "%d",
            "char" => "%c",
            "string" => "%s",
            // the remaining `_` names are enums (c `int`) - a struct/union/
            // array argument is rejected before lowering.
            _ => "%d",
        },
        // `ptr` is c `void*`; printing it as `%d` would be a varargs type
        // mismatch (UB).
        TypeKind::RawPtr => "%p",
        TypeKind::Ref(inner)
            if matches!(
                types.lookup(*inner),
                TypeKind::Array { elem, .. } if matches!(types.lookup(*elem), TypeKind::Path(n) if n == "uint8")
            ) =>
        {
            "%s"
        }
        TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::Array { .. } | TypeKind::Fn { .. } => "%p",
        // unreachable: a unit/never value is never a `println` argument (rejected
        // as a void value before lowering).
        TypeKind::Unit | TypeKind::Never | TypeKind::Error => "%d",
    }
}
