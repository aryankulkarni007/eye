//! Salsa-based query database for the Eye compiler.
//!
//! This crate owns the incremental compilation infrastructure. Every compiler
//! phase is a memoized [`salsa::tracked`] function over [`SourceFileInput`]
//! (or, per function, over the interned [`StableFnId`]). The concrete
//! [`Database`] is shared by the CLI binary and the LSP.
//!
//! ## Query graph
//!
//! ```text
//! SourceFileInput (the only mutable state)
//!   └─ lex ─ parse ─┬─ item_scope ─ lower_fn (per StableFnId)
//!                   │                 └─ (LSP per-fn diagnostics)
//!                   └─ lowered_file ─ mir_map ─ c_code
//! ```
//!
//! Two lowering paths coexist deliberately:
//!
//! - **Per-fn** (`item_scope` + `lower_fn`): each body lowers against the
//!   frozen item scope with a private interner clone, so editing one body
//!   re-runs only that body's query. This is the diagnostics path.
//! - **Whole-file** (`lowered_file`): all bodies share one interner, which is
//!   what C generation needs - type-declaration ordering and array-wrapper
//!   typedef dedup compare `TypeRef` handles across bodies, and handles from
//!   independently grown per-body interners are not comparable. The C text is
//!   a function of the whole file anyway, so whole-file granularity is the
//!   honest cache key for `c_code`.
//!
//! `mir_map` is memoized between `c_code` and the `--dump-mir` flags: both
//! consume the same map, so no body is ever MIR-lowered twice in a revision
//! (the job the deleted `MirCache` used to do, minus the staleness risk).
//!
//! See `docs/design/SALSA.md` for the full design rationale.

use std::sync::Arc;

use ast::AstNode;
use diagnostics::Sink;
use hir::core::{ConstValue, FnId, HIR, HirError, LoweredBody, Text};
use lexer::{Lexed, Lexer, SourceText};
use mir::core::MirBody;
use parser::ParseError;
use rowan::GreenNode;
use rustc_hash::FxHashMap;
use syntax::{SyntaxNode, SyntaxNodePtr};

// ---------------------------------------------------------------------------
// Salsa inputs
// ---------------------------------------------------------------------------

/// A single source file - the only mutable input in the system. Setting
/// `text` bumps the revision and invalidates downstream queries.
#[salsa::input(debug)]
pub struct SourceFileInput {
    #[returns(ref)]
    pub path: String,
    #[returns(ref)]
    pub text: String,
}

/// A function with a revision-stable identity: its file plus the
/// `SyntaxNodePtr` of its `FnDef` node. An edit that leaves a function's
/// node untouched (same kind, same range) keeps its id; an edit that moves
/// or rewrites it mints a new id, which is exactly when its `lower_fn`
/// must re-run.
#[salsa::interned(debug)]
pub struct StableFnId<'db> {
    pub file: SourceFileInput,
    pub ptr: SyntaxNodePtr,
}

// ---------------------------------------------------------------------------
// The concrete database
// ---------------------------------------------------------------------------

/// The concrete Salsa database. Shared by the CLI driver and the LSP.
#[salsa::db]
#[derive(Default, Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

// ---------------------------------------------------------------------------
// Query result types
// ---------------------------------------------------------------------------

/// The parse query result. Holds the *green* tree (immutable, `Send + Sync`)
/// rather than a `SyntaxNode` (an `Rc`-based cursor that salsa cannot store);
/// callers re-root with [`ParseResult::syntax`], which is O(1).
pub struct ParseResult {
    pub green: GreenNode,
    pub diagnostics: Sink<ParseError>,
}

impl ParseResult {
    /// Re-root the green tree into a traversable [`SyntaxNode`].
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// The typed AST root.
    pub fn ast(&self) -> ast::SourceFile {
        ast::SourceFile::cast(self.syntax()).expect("root node is a SourceFile")
    }
}

/// The item-scope query result: every top-level item collected and validated,
/// no bodies. `scope.types` is frozen; per-fn lowering clones it.
pub struct FileScope {
    /// Item arenas + interned types + collection diagnostics. `bodies` is
    /// empty and every `Function::body` is `None` on this path.
    pub scope: HIR,
    /// Folded top-level `const` values, an input to body lowering.
    pub const_values: FxHashMap<Text, ConstValue>,
    /// Every defined function, in collection order, as `(arena id, stable
    /// position)`. The ptr re-roots through [`ParseResult::syntax`] and
    /// interns into a [`StableFnId`] for the per-fn queries.
    pub fns: Vec<(FnId, SyntaxNodePtr)>,
}

// ---------------------------------------------------------------------------
// Memo: the query-result wrapper
// ---------------------------------------------------------------------------

/// Shared ownership plus conservative change detection for query results.
///
/// Salsa stores each tracked fn's value and needs to know, on re-execution,
/// whether the new value equals the old one (backdating). Our result types
/// (token streams, HIR, MIR) have no meaningful `PartialEq`, so `Memo`
/// compares by `Arc::ptr_eq`: a re-executed query always allocates a fresh
/// `Arc`, counts as changed, and dependents re-run. Conservative - never
/// stale. Structural backdating can be added per result type later.
#[derive(Debug)]
pub struct Memo<T>(pub Arc<T>);

impl<T> Memo<T> {
    fn new(value: T) -> Self {
        Memo(Arc::new(value))
    }
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Memo(self.0.clone())
    }
}

impl<T> PartialEq for Memo<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> std::ops::Deref for Memo<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Tracked queries
// ---------------------------------------------------------------------------

/// Tokenize the file.
#[salsa::tracked]
pub fn lex(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<Lexed> {
    let source = SourceText::new(file.text(db).to_owned());
    Memo::new(Lexer::new(&source).tokenize())
}

/// Parse the token stream into the lossless green tree.
#[salsa::tracked]
pub fn parse(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<ParseResult> {
    let source = SourceText::new(file.text(db).to_owned());
    let lexed = lex(db, file);
    let parse = parser::parse(&lexed.tokens, &source);
    Memo::new(ParseResult {
        green: parse.green.green().into(),
        diagnostics: parse.diagnostics,
    })
}

/// Collect and validate every top-level item (HIR passes 1-1.6). No bodies.
#[salsa::tracked]
pub fn item_scope(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<FileScope> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let collected = hir::core::collect_file_scope(&parse.ast(), &lexed.interner);
    let fns = collected
        .fn_asts
        .iter()
        .map(|(fn_id, fn_ast)| (*fn_id, SyntaxNodePtr::new(fn_ast.syntax())))
        .collect();
    Memo::new(FileScope {
        scope: collected.hir,
        const_values: collected.const_values,
        fns,
    })
}

/// Lower one function body against the frozen item scope. Keyed by
/// [`StableFnId`], so an edit inside one body re-runs only that body's query
/// (provided the item scope itself backdates clean).
#[salsa::tracked]
pub fn lower_fn<'db>(db: &'db dyn salsa::Database, fn_id: StableFnId<'db>) -> Memo<LoweredBody> {
    let file = fn_id.file(db);
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let scope = item_scope(db, file);

    let ptr = fn_id.ptr(db);
    let root = parse.syntax();
    let fn_ast = ast::FnDef::cast(ptr.to_node(&root)).expect("StableFnId ptr is a FnDef");
    let arena_id = scope
        .fns
        .iter()
        .find(|(_, p)| *p == ptr)
        .map(|(id, _)| *id)
        .expect("StableFnId ptr is a collected function");

    Memo::new(hir::core::lower_fn_body(
        &scope.scope,
        arena_id,
        &fn_ast,
        &scope.const_values,
        &lexed.interner,
    ))
}

/// Every HIR diagnostic for the file: item-scope diagnostics plus each
/// function's body diagnostics, in collection order. Runs the per-fn
/// `lower_fn` queries, so a clean body costs a cache hit.
pub fn hir_diagnostics(db: &dyn salsa::Database, file: SourceFileInput) -> Sink<HirError> {
    let scope = item_scope(db, file);
    let mut out = scope.scope.diagnostics.clone();
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(db, file, ptr);
        out.extend(lower_fn(db, fn_id).diagnostics.clone());
    }
    out
}

/// The whole file lowered with one shared interner (`lower_source_file`).
/// This is the C-generation path: codegen compares `TypeRef` handles across
/// bodies (type-declaration ordering, array-wrapper dedup), which requires a
/// single interner. Recomputes on any file edit - the C output is a function
/// of the whole file, so that is the honest granularity.
#[salsa::tracked]
pub fn lowered_file(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<HIR> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    Memo::new(hir::core::lower_source_file(parse.ast(), &lexed.interner))
}

/// Every defined function's MIR, lowered from [`lowered_file`]. Memoized so
/// `--dump-mir` and `c_code` share one lowering pass per revision.
#[salsa::tracked]
pub fn mir_map(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<FxHashMap<FnId, MirBody>> {
    let hir = lowered_file(db, file);
    Memo::new(mir::lower_all(&hir))
}

/// The generated C translation unit. Empty when any front-half diagnostic
/// fired: MIR lowering and the emitter assume a resolved, well-typed HIR
/// (poison expressions panic), and the driver renders diagnostics and bails
/// before reading the C anyway - same contract as the pre-salsa pipeline.
#[salsa::tracked]
pub fn c_code(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<String> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let hir = lowered_file(db, file);
    if !lexed.diags.is_empty() || !parse.diagnostics.is_empty() || !hir.diagnostics.is_empty() {
        return Memo::new(String::new());
    }
    let mirs = mir_map(db, file);
    Memo::new(codegen::core::gen_mir(&hir, &mirs))
}

#[cfg(test)]
mod tests;
