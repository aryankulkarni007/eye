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

mod coerce;
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
use syntax::{StringTable, SyntaxNodePtr};

use super::*;

pub use scopes::Scopes;

// ---- lowering context (defined here so child modules can access fields) ----

pub struct LoweringCtx<'a> {
    pub(super) hir: &'a HIR,
    /// The working type interner for this body, owned (no `RefCell`: dynamic
    /// borrow flags cost on every intern/lookup in this hot path, and salsa
    /// query results must be `Send + Sync`). The whole-file wrapper seeds it
    /// by taking `HIR::types` and restores it from [`fn_body::FnLowerOut`]
    /// after each body; the per-fn query path seeds it with a clone of the
    /// frozen scope interner.
    pub(super) types: TypeInterner,
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
    /// EXPERIMENTAL: The lexer's string table, used to reuse canonical
    /// [`SmolStr`] allocations instead of creating fresh ones from each token's
    /// source text. Borrowed through the [`StringTable`] trait so HIR lowering
    /// does not couple directly to the lexer (QUERY.md).
    pub(super) interner: &'a dyn StringTable,
}

// ---- entry points ----

/// The item-scope half of lowering (passes 1-1.6), plus everything the
/// per-function pass needs afterwards. `hir` has every item collected,
/// validated, and its types interned, but `bodies` is empty and every
/// `Function::body` is `None`. After this returns, `hir.types` is frozen:
/// per-function lowering works on a private copy (see [`lower_fn_body`]).
pub struct CollectedFile {
    pub hir: HIR,
    /// The folded value of every top-level `const`, consumed by body lowering
    /// for body-position const-length arrays.
    pub const_values: FxHashMap<Text, ConstValue>,
    /// Every defined function with its AST node, in collection order. Holds
    /// syntax nodes, so this struct is transient: the query layer reduces it
    /// to `SyntaxNodePtr`s before caching.
    pub fn_asts: Vec<(FnId, ast::FnDef)>,
}

/// One independently lowered function body (the per-fn query result half).
pub struct LoweredBody {
    pub body: Body,
    /// The working interner this body's `TypeRef` handles resolve through: a
    /// clone of the frozen scope interner plus any types interned while
    /// lowering this body (e.g. a string literal's `&[uint8; N]`). Handles
    /// that exist in the scope interner are bit-identical here, so
    /// signature-vs-body comparisons stay valid; body-local handles are valid
    /// only through this interner.
    pub types: TypeInterner,
    pub diagnostics: Sink<HirError>,
}

/// Passes 1-1.6: collect and validate every top-level item. No bodies are
/// lowered. This is the `item_scope` query's compute function.
pub fn collect_file_scope(file: &ast::SourceFile, interner: &dyn StringTable) -> CollectedFile {
    let mut hir = HIR::default();

    // Every type lowered during collection, with its source span, for the
    // post-collect type-name validation (pass 1.6). Recorded rather than
    // checked inline because item signatures forward-reference items
    // collected later.
    let mut typed_decls: Vec<(diagnostics::Span, TypeRef)> = Vec::new();

    // pass 1a: collect `const` signatures (name, type, body) before any other
    // item, so an item's array length (`[T; N]`) can resolve `N` to a const.
    let const_asts = collect::collect_consts(&mut hir, file, interner, &mut typed_decls);

    // pass 1.5a: fold every const to its scalar value (cycle-checked). The
    // returned map drives const-length array resolution below.
    let const_values = const_eval::eval_consts(&mut hir, &const_asts);

    // pass 1b: collect top-level `let`/`mut` globals (addressable static
    // storage), then fold their initializers against the const map. Runs after
    // const folding so a global's type/initializer may reference a const.
    let global_asts =
        collect::collect_globals(&mut hir, file, &const_values, interner, &mut typed_decls);
    const_eval::eval_globals(&mut hir, &global_asts, &const_values);

    // pass 1: collect every top-level item. Forward refs resolve because
    // bodies have not been walked yet. Duplicates emit a diagnostic; the
    // later definition still overwrites the earlier binding in [`ItemScope`].
    let fn_asts =
        collect::collect_items(&mut hir, file, &const_values, interner, &mut typed_decls);

    // pass 1.6: every Path name in a collected signature must be a declared
    // type (R012) - otherwise the name is emitted verbatim into C and clang
    // reports "unknown type name" (CLEAK L6).
    collect::validate_type_names(&mut hir, &typed_decls);

    // pass 1.5: reject value-recursive struct/union types (infinite size) before
    // they reach codegen, where the type-declaration ordering would be unable to
    // place them and clang would error.
    recursion::check_value_recursion(&mut hir, file);

    // pass 2: name resolution.
    //   - type resolution: deferred. TypeRef stays as Path(name); codegen
    //     will look up the StructId itself.
    //   - value resolution: folded into pass 3 (scopes only exist inside a
    //     body, and resolution is recorded per-Expr::Path).

    CollectedFile {
        hir,
        const_values,
        fn_asts,
    }
}

/// Pass 3 for one function, against an immutable item scope. The body works
/// on a private clone of the frozen scope interner, so two bodies can lower
/// independently (and be cached independently by the query layer).
pub fn lower_fn_body(
    scope: &HIR,
    fn_id: FnId,
    fn_ast: &ast::FnDef,
    const_values: &FxHashMap<Text, ConstValue>,
    interner: &dyn StringTable,
) -> LoweredBody {
    let out = fn_body::lower_fn_with(
        scope,
        fn_id,
        fn_ast,
        const_values,
        interner,
        scope.types.clone(),
    );
    LoweredBody {
        body: out.body,
        types: out.types,
        diagnostics: out.diagnostics,
    }
}

/// Lower a parsed file into a fresh [`HIR`]. See module docs for pass layout.
/// `interner` is the lexer's string table, reused to avoid redundant
/// [`SmolStr`] allocations when converting token text to [`Text`].
///
/// Whole-file convenience wrapper over [`collect_file_scope`] +
/// [`lower_fn_body`]'s driver: bodies land in `HIR::bodies` and share the one
/// scope interner (threaded through each body sequentially, exactly like the
/// pre-query pipeline), so every `TypeRef` in the result resolves through
/// `HIR::types`. The query layer does not use this; tests, dumps, fuzzing,
/// and benches do.
pub fn lower_source_file(file: ast::SourceFile, interner: &dyn StringTable) -> HIR {
    let CollectedFile {
        mut hir,
        const_values,
        fn_asts,
    } = collect_file_scope(&file, interner);

    // pass 3: lower each fn body, threading the single shared interner.
    for (fn_id, fn_ast) in fn_asts {
        let types = std::mem::take(&mut hir.types);
        let out = fn_body::lower_fn_with(&hir, fn_id, &fn_ast, &const_values, interner, types);
        hir.types = out.types;
        hir.diagnostics.extend(out.diagnostics);
        let body_id = hir.bodies.alloc(out.body);
        hir.functions[fn_id].body = Some(body_id);
    }

    hir
}
