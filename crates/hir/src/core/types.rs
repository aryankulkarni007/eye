//! Type references as they exist at HIR time.
//!
//! Stays *unresolved* at HIR time: just a name. Type inference / resolution
//! runs in a later pass and produces real `Ty` ids. Builtins (`int32`, `bool`)
//! are still recognized here as a convenience.

use super::*;

// TODO: store types in arena instead of Box
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeRef {
    Path(Text),
    Ref(Box<TypeRef>), // &T
    Ptr(Box<TypeRef>), // *T
    // [T; N] fixed-size array. `len` is a concrete element count parsed from an
    // integer literal (no const-expr yet).
    Array { elem: Box<TypeRef>, len: u64 },
    Error,
}
