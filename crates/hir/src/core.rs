//! HIR: AST -> name-resolved, desugared, arena-allocated IR.
//!
//! Two layers per crate:
//! - **ItemTree**: module-level signatures (structs, enums, fn headers). One
//!   per file. Forward references work because all items are collected before
//!   any body is lowered.
//! - **Body**: per-function expression/statement/pattern arenas plus a source
//!   map back to syntax pointers. Per-fn so editing one fn body invalidates
//!   only that body, not the whole crate.
//!
//! The module is split by concern:
//! - [`ids`]: typed arena-index aliases.
//! - [`items`]: module-level item signatures + the [`ItemScope`].
//! - [`types`]: [`TypeRef`], the HIR-time (unresolved) type representation.
//! - [`body`]: the per-fn body IR ([`Body`], [`Expr`], [`Stmt`], [`Pat`], ...).
//! - [`lower`]: the lowering logic and entry point [`lower_source_file`],
//!   split into `lower/{scopes,ctx,types,collect,fn_body,stmt,pat,expr}`.
//!
//! This file holds only the top-level [`HIR`] aggregate and re-exports every
//! submodule so the public path stays `hir::core::*`.

mod body;
mod errors;
mod ids;
mod items;
mod lower;
mod typed_arena;
mod typegraph;
mod types;

#[cfg(test)]
mod tests;

pub use body::*;
pub use errors::*;
pub use ids::*;
pub use items::*;
pub use lower::*;
pub use typegraph::*;
pub use types::*;

use diagnostics::Sink;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use smol_str::SmolStr;

pub use typed_arena::TypedArena;

pub type Text = SmolStr;

/// Decode a string-literal's source spelling (without surrounding quotes) into
/// its byte sequence, expanding escapes. This is the single source of truth for
/// a string's byte content: its length is the literal's `N` in `&[uint8; N]` and
/// its bytes seed the backing static. The *stored* `Literal::String` keeps the
/// raw spelling (the `print` / format paths re-emit it as a C string literal and
/// let C decode), so this decoder feeds only `N` and the byte-array initializer.
///
/// Recognized escapes: `\n \t \r \0 \\ \" \'`. An unrecognized escape keeps both
/// bytes (the backslash and the char), matching the lenient front end. A `\0`
/// embeds a NUL, which truncates `strlen` / `%s` on the C-string backing - an
/// accepted limit of the C-backed representation.
///
/// ```
/// # use hir::core::decode_string_literal;
/// assert_eq!(decode_string_literal("hello"), b"hello");
/// assert_eq!(decode_string_literal("a\\nb"), b"a\nb");
/// assert_eq!(decode_string_literal("tab\\there"), b"tab\there");
/// assert_eq!(decode_string_literal("\\0\\x"), b"\0\\x");
/// assert_eq!(decode_string_literal(""), b"");
/// ```
pub fn decode_string_literal(raw: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut chars = raw.chars();
    let mut buf = [0u8; 4];
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push(b'\n'),
                Some('t') => out.push(b'\t'),
                Some('r') => out.push(b'\r'),
                Some('0') => out.push(0),
                Some('\\') => out.push(b'\\'),
                Some('"') => out.push(b'"'),
                Some('\'') => out.push(b'\''),
                Some(other) => {
                    out.push(b'\\');
                    out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => out.push(b'\\'),
            }
        } else {
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// Decode a char literal's inner text (between the quotes) to a single `char`,
/// expanding the same escapes as [`decode_string_literal`]. Multi-char bodies
/// take the first decoded char; an empty body falls back to NUL, matching the
/// lenient front end.
///
/// ```
/// # use hir::core::decode_char_literal;
/// assert_eq!(decode_char_literal("\\n"), '\n');
/// assert_eq!(decode_char_literal("\\\\"), '\\');
/// assert_eq!(decode_char_literal("A"), 'A');
/// assert_eq!(decode_char_literal(""), '\0');
/// assert_eq!(decode_char_literal("\\'"), '\'');
/// ```
pub fn decode_char_literal(inner: &str) -> char {
    let mut chars = inner.chars();
    match chars.next() {
        Some('\\') => match chars.next() {
            Some('n') => '\n',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('0') => '\0',
            Some('\\') => '\\',
            Some('"') => '"',
            Some('\'') => '\'',
            // unrecognized escape: keep the escaped char, matching the lenient
            // string decoder.
            Some(other) => other,
            None => '\\',
        },
        Some(c) => c,
        None => '\0',
    }
}

/// Create an [`FxHashMap`] with the given initial capacity. Avoids the resize
/// overhead that `Default` incurs when the eventual size is known.
pub fn fx_map<K, V>(capacity: usize) -> FxHashMap<K, V> {
    FxHashMap::with_capacity_and_hasher(capacity, FxBuildHasher)
}

/// Create an [`FxHashSet`] with the given initial capacity.
pub fn fx_set<V>(capacity: usize) -> FxHashSet<V> {
    FxHashSet::with_capacity_and_hasher(capacity, FxBuildHasher)
}

/// Top-level lowered module. Items live in flat arenas; bodies are keyed by
/// [`FnId`] through [`Function::body`].
///
/// EXPERIMENTAL(typed-arena): Arena fields use [`TypedArena<T, XId>`] so every
/// index carries its element type at the type level and the compiler refuses
/// to mix up `StructId` with `FnId`.  Every `hir.structs[id]` and
/// `arena.alloc(value)` site is unchanged because [`Index<StructId>`] and
/// [`TypedArena::alloc`] work through the wrapper.
#[derive(Debug, Default)]
pub struct HIR {
    pub structs: TypedArena<Struct, StructId>,
    pub unions: TypedArena<Union, UnionId>,
    pub enums: TypedArena<Enum, EnumId>,
    pub consts: TypedArena<Const, ConstId>,
    pub globals: TypedArena<Global, GlobalId>,
    pub opaques: TypedArena<OpaqueType, OpaqueId>,
    pub fields: TypedArena<Field, FieldId>,
    pub functions: TypedArena<Function, FnId>,
    pub bodies: TypedArena<Body, BodyId>,
    /// Module-level scope. Both namespaces flat for v0.1 since structs + fns
    /// don't collide (struct names start uppercase by convention, but the
    /// resolver treats them in one map until the language says otherwise).
    pub items: ItemScope,
    /// Diagnostics produced during lowering. Non-empty means the input had
    /// semantic issues even if the parser was happy.
    pub diagnostics: Sink<HirError>,
    /// Interned type representations. Every [`TypeRef`] handle in this HIR
    /// is valid in this interner.
    ///
    /// Plain (no `RefCell`): collection interns through `&mut HIR`, and body
    /// lowering owns a working interner inside [`lower::LoweringCtx`] (taken
    /// from here and restored by the whole-file wrapper, or cloned from the
    /// frozen scope by the per-fn query path). After lowering completes the
    /// interner is read-only, which is what the salsa query layer requires
    /// (`Send + Sync` query results, no interior mutability).
    pub types: TypeInterner,
}
