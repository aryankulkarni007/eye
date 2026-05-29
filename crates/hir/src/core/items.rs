//! Module-level item signatures and the module item scope.
//!
//! Collected in pass 1 before any body is walked, so forward references
//! resolve. Bodies live elsewhere (see [`Body`]); a [`Function`] only points
//! at its [`BodyId`].

use rustc_hash::FxHashMap;

use super::*;

#[derive(Debug)]
pub struct Struct {
    pub name: Text,
    pub fields: Vec<FieldId>,
}

/// A union - overlapping storage. Structurally identical to [`Struct`]; the
/// difference is the C emit (`union` not `struct`) and that a value occupies
/// exactly one field at a time.
#[derive(Debug)]
pub struct Union {
    pub name: Text,
    pub fields: Vec<FieldId>,
}

#[derive(Debug)]
pub struct Enum {
    pub name: Text,
    pub variants: Vec<Variant>,
}

#[derive(Debug)]
pub struct Variant {
    pub name: Text,
}

#[derive(Debug)]
pub struct Field {
    pub name: Text,
    pub ty: TypeRef,
}

#[derive(Debug)]
pub struct Function {
    pub name: Text,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    /// Body lives in its own arena keyed by [`FnId`] on [`HIR`].
    pub body: Option<BodyId>,
    /// `true` for a signature declared in an `extern` block: no body, emitted
    /// as a bare C prototype and resolved by the linker.
    pub is_extern: bool,
}

#[derive(Debug)]
pub struct Param {
    pub name: Text,
    pub ty: TypeRef,
}

#[derive(Debug, Default)]
pub struct ItemScope {
    pub functions: FxHashMap<Text, FnId>,
    pub structs: FxHashMap<Text, StructId>,
    pub unions: FxHashMap<Text, UnionId>,
    pub enums: FxHashMap<Text, EnumId>,
    /// Flat variant-name index across every enum. Lets a bare variant name
    /// resolve to its enum + index without an expected-type hint. Two enums
    /// sharing a variant name is a hard error at decl time (recorded as a
    /// diagnostic), so a successful lookup here is always unambiguous.
    pub variants: FxHashMap<Text, (EnumId, u32)>,
}
