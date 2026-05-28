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
//! - [`lower`]: the lowering logic and entry point [`lower_source_file`].
//!
//! This file holds only the top-level [`HIR`] aggregate and re-exports every
//! submodule so the public path stays `hir::core::*`.

mod body;
mod ids;
mod items;
mod lower;
mod types;

#[cfg(test)]
mod tests;

pub use body::*;
pub use ids::*;
pub use items::*;
pub use lower::*;
pub use types::*;

use la_arena::Arena;
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

pub type Text = SmolStr;

/// Top-level lowered module. Items live in flat arenas; bodies are keyed by
/// [`FnId`] through [`Function::body`].
#[derive(Debug, Default)]
pub struct HIR {
    pub structs: Arena<Struct>,
    pub enums: Arena<Enum>,
    pub fields: Arena<Field>,
    pub functions: Arena<Function>,
    pub bodies: Arena<Body>,
    /// Module-level scope. Both namespaces flat for v0.1 since structs + fns
    /// don't collide (struct names start uppercase by convention, but the
    /// resolver treats them in one map until the language says otherwise).
    pub items: ItemScope,
    /// Diagnostics produced during lowering. Non-empty means the input had
    /// semantic issues even if the parser was happy.
    pub diagnostics: Vec<HirDiagnostic>,
}

/// A semantic diagnostic raised during HIR lowering.
#[derive(Debug, Clone)]
pub struct HirDiagnostic {
    pub ptr: SyntaxNodePtr,
    pub msg: String,
}
