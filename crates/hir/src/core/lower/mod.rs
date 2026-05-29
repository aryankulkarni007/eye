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
mod ctx;
mod expr;
mod fn_body;
mod pat;
mod scopes;
mod stmt;
mod types;

use diagnostics::Sink;

use super::*;

pub use scopes::Scopes;

// ---- lowering context (defined here so child modules can access fields) ----

pub struct LoweringCtx<'a> {
    pub(super) hir: &'a HIR,
    pub(super) body: Body,
    pub(super) scopes: Scopes,
    pub(super) diagnostics: Sink<HirError>,
}

// ---- entry point ----

/// Lower a parsed file into a fresh [`HIR`]. See module docs for pass layout.
pub fn lower_source_file(file: ast::SourceFile) -> HIR {
    let mut hir = HIR::default();

    // pass 1: collect every top-level item. Forward refs resolve because
    // bodies have not been walked yet. Duplicates emit a diagnostic; the
    // later definition still overwrites the earlier binding in [`ItemScope`].
    let fn_asts = collect::collect_items(&mut hir, &file);

    // pass 2: name resolution.
    //   - type resolution: deferred. TypeRef stays as Path(name); codegen
    //     will look up the StructId itself.
    //   - value resolution: folded into pass 3 (scopes only exist inside a
    //     body, and resolution is recorded per-Expr::Path).

    // pass 3: lower each fn body.
    for (fn_id, fn_ast) in fn_asts {
        let body_id = fn_body::lower_fn_body(&mut hir, fn_id, &fn_ast);
        hir.functions[fn_id].body = Some(body_id);
    }

    hir
}
