//! Module-level item signatures and the module item scope.
//!
//! Collected in pass 1 before any body is walked, so forward references
//! resolve. Bodies live elsewhere (see [`Body`]); a [`Function`] only points
//! at its [`BodyId`].

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::*;

#[derive(Debug)]
pub struct Struct {
    pub name: Text,
    pub fields: SmallVec<[FieldId; 4]>,
    pub field_index: FxHashMap<Text, FieldId>,
}

/// A top-level compile-time constant value (`const int32 MAX = 100;`). A const
/// is a *value*, not storage: it has no guaranteed address (`&const` is
/// illegal), and a reference to it inlines its folded [`ConstValue`] rather than
/// reading a C symbol. The initializer is a bounded const-expr folded in pass
/// 1.5 ([`lower::const_eval`]); `value` is `None` only when the fold failed (a
/// diagnostic was already emitted), so downstream lowering treats that as poison.
#[derive(Debug)]
pub struct Const {
    pub name: Text,
    /// The declared type (always explicit at the floor - no inference).
    pub ty: TypeRef,
    pub value: Option<ConstValue>,
}

/// A global: addressable static storage declared with a top-level `let`/`mut`.
/// Unlike a [`Const`] (a value with no address), a global has an address, so a
/// reference reads/writes a named C symbol rather than inlining. The initializer
/// is const-folded (scalar-only floor) to seed the C static initializer.
#[derive(Debug)]
pub struct Global {
    pub name: Text,
    pub ty: TypeRef,
    /// `let` is read-only, `mut` is writable. Assigning a `let` global is a `T`
    /// diagnostic (immutable-by-default), like a `let` local.
    pub mutable: bool,
    pub value: Option<ConstValue>,
}

/// The folded scalar value of a [`Const`]. Scalar-only at the floor (aggregate
/// const values are deferred). Integers fold in `i128` so a negated const
/// (`const int32 N = -5`) keeps its sign; floats fold in `f64`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Char(char),
}

/// A union - overlapping storage. Structurally identical to [`Struct`]; the
/// difference is the C emit (`union` not `struct`) and that a value occupies
/// exactly one field at a time.
#[derive(Debug)]
pub struct Union {
    pub name: Text,
    pub fields: SmallVec<[FieldId; 4]>,
    pub field_index: FxHashMap<Text, FieldId>,
}

#[derive(Debug)]
pub struct Enum {
    pub name: Text,
    pub variants: SmallVec<[Variant; 4]>,
    pub variant_index: FxHashMap<Text, u32>,
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
    pub params: SmallVec<[Param; 4]>,
    pub ret: Option<TypeRef>,
    /// Body lives in its own arena keyed by [`FnId`] on [`HIR`].
    pub body: Option<BodyId>,
    /// `true` for a signature declared in an `extern` block: no body, emitted
    /// as a bare C prototype and resolved by the linker.
    pub is_extern: bool,
    /// `true` for a variadic extern signature (`printf(string fmt, ...)`).
    /// A C-ABI marker only: calls may pass extra trailing arguments, the
    /// prototype gains `...`, and Eye has no varargs access of its own. The
    /// parser rejects `...` outside an `extern` block, so a defined function
    /// is never variadic.
    pub variadic: bool,
    /// The function-pointer type `(ParamTys) -> RetTy`, computed once after
    /// collection so expression lowering clones it O(1) instead of rebuilding
    /// from every param's TypeRef tree.
    pub fn_type: Option<TypeRef>,
}

#[derive(Debug)]
pub struct Param {
    pub name: Text,
    pub ty: TypeRef,
}

/// An opaque FFI type declared as `type Name;` in an `extern` block: a named
/// C type whose layout Eye never sees. Usable only behind a pointer or
/// reference (`FILE*`, `&FILE`); codegen emits a forward typedef
/// (`typedef struct Name Name;`) and no definition, so a value-position use
/// is an incomplete-type error in the C backend (an HIR-side diagnostic
/// waits on the typeck split).
#[derive(Debug)]
pub struct OpaqueType {
    pub name: Text,
}

#[derive(Debug, Default)]
pub struct ItemScope {
    pub functions: FxHashMap<Text, FnId>,
    pub structs: FxHashMap<Text, StructId>,
    pub unions: FxHashMap<Text, UnionId>,
    pub enums: FxHashMap<Text, EnumId>,
    pub consts: FxHashMap<Text, ConstId>,
    pub globals: FxHashMap<Text, GlobalId>,
    pub opaques: FxHashMap<Text, OpaqueId>,
    /// Flat variant-name index across every enum. Lets a bare variant name
    /// resolve to its enum + index without an expected-type hint. Two enums
    /// sharing a variant name is a hard error at decl time (recorded as a
    /// diagnostic), so a successful lookup here is always unambiguous.
    pub variants: FxHashMap<Text, (EnumId, u32)>,
}
