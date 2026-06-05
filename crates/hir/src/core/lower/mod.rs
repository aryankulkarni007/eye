//! The lowering logic: AST -> arena HIR with name resolution and the v0.3
//! match exhaustiveness check. Entry point is [`lower_source_file`].
//!
//! Pipeline runs in three passes:
//! 1. [`collect_items`] registers every top-level [`Struct`], [`Enum`], and
//!    [`Function`] in [`HIR::items`]. Forward refs work because bodies have
//!    not been walked yet. Duplicate declarations emit a [`HirDiagnostic`];
//!    the later definition still overwrites the earlier one in [`ItemScope`],
//!    and both items keep their arena slots so existing IDs do not invalidate.
//! 2. Name resolution. Type resolution is deferred to codegen: a [`TypeRef`]
//!    stays as a `Path(name)` string with no `StructId` attached. Value
//!    resolution (locals + items) is folded into pass 3 since lexical scopes
//!    only exist inside a body.
//! 3. [`lower_fn_body`] walks each fn's `Block` with a fresh [`LoweringCtx`].
//!    Each [`Expr::Path`] carries its [`Resolution`] so later passes never
//!    redo the lookup.
//!
//! Split by concern (same layout as `codegen::core`):
//! - [`scopes`]: lexical scope stack for locals.
//! - [`ctx`]: [`LoweringCtx`] allocation, resolution, and field-type lookup.
//! - [`types`]: AST type and literal lowering helpers.
//! - [`collect`]: pass 1 item collection.
//! - [`fn_body`]: pass 3 function-body driver.
//! - [`stmt`]: blocks and statements.
//! - [`pat`]: match-arm pattern lowering.
//! - [`expr`]: expression lowering.

mod collect;
mod const_eval;
mod ctx;
mod expr;
mod fn_body;
mod pat;
mod recursion;
mod scopes;
mod stmt;
mod types;

use diagnostics::Sink;
use rustc_hash::FxHashMap;
use syntax::SyntaxNodePtr;

use super::*;

pub use scopes::Scopes;

// ---- lowering context (defined here so child modules can access fields) ----

pub struct LoweringCtx<'a> {
    pub(super) hir: &'a HIR,
    pub(super) body: Body,
    pub(super) scopes: Scopes,
    pub(super) diagnostics: Sink<HirError>,
    /// The enclosing function's declared return type, used to check explicit
    /// `return` statements as they are lowered. `None` for a void function.
    /// `main` is ordinary here (its C `int` entry point is a backend shim, not
    /// a language rule), so a void `main()` carries `None` like any void function.
    pub(super) fn_ret: Option<TypeRef>,
    /// The folded value of every top-level `const`, so a body-position array
    /// length (`let [int32; SIZE] xs`) can resolve a const count.
    pub(super) const_values: &'a FxHashMap<Text, ConstValue>,
    /// The function body's block pointer, used to anchor diagnostics that apply
    /// to the whole function body (e.g. missing return value).
    pub(super) fn_block_ptr: Option<SyntaxNodePtr>,
}

// ---- entry point ----

/// Lower a parsed file into a fresh [`HIR`]. See module docs for pass layout.
pub fn lower_source_file(file: ast::SourceFile) -> HIR {
    let mut hir = HIR::default();

    // pass 1a: collect `const` signatures (name, type, body) before any other
    // item, so an item's array length (`[T; N]`) can resolve `N` to a const.
    let const_asts = collect::collect_consts(&mut hir, &file);

    // pass 1.5a: fold every const to its scalar value (cycle-checked). The
    // returned map drives const-length array resolution below.
    let const_values = const_eval::eval_consts(&mut hir, &const_asts);

    // pass 1b: collect top-level `let`/`mut` globals (addressable static
    // storage), then fold their initializers against the const map. Runs after
    // const folding so a global's type/initializer may reference a const.
    let global_asts = collect::collect_globals(&mut hir, &file, &const_values);
    const_eval::eval_globals(&mut hir, &global_asts, &const_values);

    // pass 1: collect every top-level item. Forward refs resolve because
    // bodies have not been walked yet. Duplicates emit a diagnostic; the
    // later definition still overwrites the earlier binding in [`ItemScope`].
    let fn_asts = collect::collect_items(&mut hir, &file, &const_values);

    // pass 1.5: reject value-recursive struct/union types (infinite size) before
    // they reach codegen, where the type-declaration ordering would be unable to
    // place them and clang would error.
    recursion::check_value_recursion(&mut hir, &file);

    // pass 2: name resolution.
    //   - type resolution: deferred. TypeRef stays as Path(name); codegen
    //     will look up the StructId itself.
    //   - value resolution: folded into pass 3 (scopes only exist inside a
    //     body, and resolution is recorded per-Expr::Path).

    // pass 3: lower each fn body.
    for (fn_id, fn_ast) in fn_asts {
        let body_id = fn_body::lower_fn_body(&mut hir, fn_id, &fn_ast, &const_values);
        hir.functions[fn_id].body = Some(body_id);
    }

    hir
}
