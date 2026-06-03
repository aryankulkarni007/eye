//! Type and format-specifier mapping: HIR `TypeRef` -> C type strings and
//! printf specifiers.

use hir::core::TypeRef;
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

/// printf format specifier for a value of type `ty`. A pure type -> specifier
/// map, read by the MIR emitter's `print` lowering (one specifier per `{}`).
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
