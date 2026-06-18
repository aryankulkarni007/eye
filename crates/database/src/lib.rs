// the house doc style keeps list-item continuation lines flush with the
// marker; clippy's `doc_lazy_continuation` (newly on by default) wants them
// indented. allowed crate-wide to keep the existing style consistent.
#![allow(clippy::doc_lazy_continuation)]

//! salsa-based query database for the eye compiler.
//!
//! this crate owns the incremental compilation infrastructure. every compiler
//! phase is a memoized [`salsa::tracked`] function over [`SourceFileInput`]
//! (or, per function, over the interned [`StableFnId`]). the concrete
//! [`Database`] is shared by the CLI binary and the LSP.
//!
//! ## query graph
//!
//! ```text
//! sourcefileinput (the only mutable state)
//! └─ lex ─ parse ─┬─ item_scope ─ lower_fn (per stablefnid)
//! │ └─ (LSP per-fn diagnostics)
//! └─ lowered_file ─ mir_map ─ c_code
//! ```
//!
//! two lowering paths coexist deliberately:
//!
//! - **per-fn** (`item_scope` + `lower_fn`): each body lowers against the
//! frozen item scope with a private interner clone, so editing one body
//! re-runs only that body's query. this is the diagnostics path.
//! - **whole-file** (`lowered_file`): all bodies share one interner, which is
//! what c generation needs - type-declaration ordering and array-wrapper
//! typedef dedup compare `TypeRef` handles across bodies, and handles from
//! independently grown per-body interners are not comparable. the c text is
//! a function of the whole file anyway, so whole-file granularity is the
//! honest cache key for `c_code`.
//!
//! `mir_map` is memoized between `c_code` and the `--dump-mir` flags: both
//! consume the same map, so no body is ever MIR-lowered twice in a revision
//! (the job the deleted `MirCache` used to do, minus the staleness risk).
//!
//! see `docs/design/SALSA.md` for the full design rationale.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ast::AstNode;
use diagnostics::Sink;
use effect::EffectMap;
use hir::core::{ConstValue, FnId, HIR, HirError, LoweredBody, Text};
use lexer::{Lexed, Lexer, SourceText};
use mir::core::MirBody;
use parser::ParseError;
use rowan::GreenNode;
use rustc_hash::FxHashMap;
use syntax::{SyntaxNode, SyntaxNodePtr};
use typeck::TypeckResults;

// ---------------------------------------------------------------------------
// salsa inputs
// ---------------------------------------------------------------------------

/// a single source file - the only mutable input in the system. setting
/// `text` bumps the revision and invalidates downstream queries.
#[salsa::input(debug)]
pub struct SourceFileInput {
    #[returns(ref)]
    pub path: String,
    #[returns(ref)]
    pub text: String,
}

/// a function with a revision-stable identity: its file plus the
/// `SyntaxNodePtr` of its `FnDef` node. an edit that leaves a function's
/// node untouched (same kind, same range) keeps its id; an edit that moves
/// or rewrites it mints a new id, which is exactly when its `lower_fn`
/// must re-run.
#[salsa::interned(debug)]
pub struct StableFnId<'db> {
    pub file: SourceFileInput,
    pub ptr: SyntaxNodePtr,
}

// ---------------------------------------------------------------------------
// the concrete database
// ---------------------------------------------------------------------------

/// the concrete salsa database. shared by the CLI driver and the LSP.
#[salsa::db]
#[derive(Default, Clone)]
pub struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

// ---------------------------------------------------------------------------
// query result types
// ---------------------------------------------------------------------------

/// the parse query result. holds the *green* tree (immutable, `Send + Sync`)
/// rather than a `SyntaxNode` (an `Rc`-based cursor that salsa cannot store);
/// callers re-root with [`ParseResult::syntax`], which is o(1).
pub struct ParseResult {
    pub green: GreenNode,
    pub diagnostics: Sink<ParseError>,
}

impl ParseResult {
    /// re-root the green tree into a traversable [`SyntaxNode`].
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// the typed AST root.
    pub fn ast(&self) -> ast::SourceFile {
        ast::SourceFile::cast(self.syntax()).expect("root node is a SourceFile")
    }
}

/// the item-scope query result: every top-level item collected and validated,
/// no bodies. `scope.types` is frozen; per-fn lowering clones it.
pub struct FileScope {
    /// item arenas + interned types + collection diagnostics. `bodies` is
    /// empty and every `Function::body` is `None` on this path.
    pub scope: HIR,
    /// folded top-level `const` values, an input to body lowering.
    pub const_values: FxHashMap<Text, ConstValue>,
    /// every defined function, in collection order, as `(arena id, stable
    /// position)`. The ptr re-roots through [`parseresult::syntax`] and
    /// interns into a [`StableFnId`] for the per-fn queries.
    pub fns: Vec<(FnId, SyntaxNodePtr)>,
    /// content digest of every item *signature* (the firewall key, S5): all
    /// item declarations with fn bodies excluded. equal across a body-only edit
    /// so `item_scope` backdates; see [`MemoEq for FileScope`](MemoEq).
    pub sig_digest: u64,
}

/// a lowered body plus the firewall digest of the inputs it was lowered from
/// (this function's source text combined with the scope signature digest).
/// `lower_fn` returns this so an unedited body's re-lowering backdates - the
/// digest is unchanged when neither the body text nor any signature moved.
pub struct LoweredFn {
    pub lowered: LoweredBody,
    pub digest: u64,
}

// ---------------------------------------------------------------------------
// memo: the query-result wrapper
// ---------------------------------------------------------------------------

/// shared ownership plus change detection for query results.
///
/// salsa stores each tracked fn's value and needs to know, on re-execution,
/// whether the new value equals the old one (*backdating*: an equal result
/// keeps the prior revision stamp, so dependents do not re-run). a re-executed
/// query always allocates a fresh `Arc`, so `Memo` cannot compare by pointer;
/// it delegates to [`MemoEq`], whose default is conservative (every
/// re-execution counts as changed - never stale, but no backdating either).
/// the signature-firewall result types ([`FileScope`], [`LoweredFn`]) override
/// `memo_eq` with a content digest so a body-only edit backdates `item_scope`
/// and the unedited bodies' queries cache-hit (segment S5, TYPECK.md).
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

/// how a query result decides equality for salsa backdating. the default is
/// conservative (`false`: a re-executed query always counts as changed),
/// reproducing the old `Arc::ptr_eq` behavior for every result that does not
/// opt in. a type overrides this with a cheap, *correct* equality - a content
/// digest, never a pointer - to let its query backdate.
pub trait MemoEq {
    fn memo_eq(&self, _other: &Self) -> bool {
        false
    }
}

impl<T: MemoEq> PartialEq for Memo<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.memo_eq(&other.0)
    }
}

// conservative (default) results: every re-execution counts as changed, exactly
// as before the firewall. only the signature-bearing results below opt in.
impl MemoEq for Lexed {}
impl MemoEq for ParseResult {}
impl MemoEq for TypeckResults {}
impl MemoEq for CheckedFile {}
impl MemoEq for String {}
impl MemoEq for FxHashMap<FnId, MirBody> {}

/// the firewall: two item scopes are equal when their *signature digest* is -
/// every item declaration unchanged (fn signatures, struct/enum/union defs,
/// const/global initializers), regardless of fn *body* edits. so a keystroke
/// inside one body produces an equal `FileScope`, `item_scope` backdates, and
/// every query that reads only the scope cache-hits.
impl MemoEq for FileScope {
    fn memo_eq(&self, other: &Self) -> bool {
        self.sig_digest == other.sig_digest
    }
}

/// the firewall, body half: a re-lowered body is equal when its source text and
/// the scope signatures it lowered against are both unchanged (`digest`
/// combines them). a sibling body edit re-runs `lower_fn` but the result
/// backdates, so the sibling `typeck_fn` (which reads only the scope and this
/// body) cache-hits. relies on lowering being deterministic and `Text` being an
/// owned `SmolStr` (no interner-id drift across edits), so equal inputs give a
/// bit-identical `LoweredBody`.
impl MemoEq for LoweredFn {
    fn memo_eq(&self, other: &Self) -> bool {
        self.digest == other.digest
    }
}

impl<T> std::ops::Deref for Memo<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// firewall digests (S5)
// ---------------------------------------------------------------------------

/// the signature digest: a hash of every top-level item with each function's
/// body block excluded. stable across a body-only edit (the firewall key),
/// changed by any signature / type-def / const-or-global-initializer edit.
/// hashes item text *content*, not byte offsets, so an edit that shifts later
/// items without changing them leaves the digest unchanged.
fn signature_digest(text: &str, ast: &ast::SourceFile) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    for item in ast.items() {
        let range = item.syntax().text_range();
        // a function contributes its signature only - everything up to the body
        // block's `{`; every other item contributes whole.
        let end = match &item {
            ast::Item::FnDef(f) => f
                .body()
                .map(|b| b.syntax().text_range().start())
                .unwrap_or_else(|| range.end()),
            _ => range.end(),
        };
        hash_text_range(text, range.start(), end, &mut h);
    }
    h.finish()
}

/// the body digest: this function's full source text combined with the scope
/// signature digest. unchanged across a sibling-body edit (this node's text and
/// every signature stay equal); changed by an edit to this body or to any
/// signature/const it lowered against.
fn body_digest(text: &str, fn_ast: &ast::FnDef, sig_digest: u64) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    sig_digest.hash(&mut h);
    let r = fn_ast.syntax().text_range();
    hash_text_range(text, r.start(), r.end(), &mut h);
    h.finish()
}

fn hash_text_range(text: &str, start: rowan::TextSize, end: rowan::TextSize, h: &mut impl Hasher) {
    if let Some(slice) = text.get(usize::from(start)..usize::from(end)) {
        slice.hash(h);
    }
    // a separator so two adjacent items' concatenated text cannot alias a
    // different item split with the same bytes.
    0xFF_u8.hash(h);
}

// ---------------------------------------------------------------------------
// tracked queries
// ---------------------------------------------------------------------------

/// tokenize the file.
#[salsa::tracked]
pub fn lex(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<Lexed> {
    let source = SourceText::new(file.text(db).to_owned());
    Memo::new(Lexer::new(&source).tokenize())
}

/// parse the token stream into the lossless green tree.
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

/// collect and validate every top-level item (HIR passes 1-1.6). no bodies.
#[salsa::tracked]
pub fn item_scope(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<FileScope> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let ast = parse.ast();
    let collected = hir::core::collect_file_scope(&ast, &lexed.interner);
    let fns = collected
        .fn_asts
        .iter()
        .map(|(fn_id, fn_ast)| (*fn_id, SyntaxNodePtr::new(fn_ast.syntax())))
        .collect();
    let sig_digest = signature_digest(file.text(db), &ast);
    Memo::new(FileScope {
        scope: collected.hir,
        const_values: collected.const_values,
        fns,
        sig_digest,
    })
}

/// lower one function body against the frozen item scope. keyed by
/// [`StableFnId`], so an edit inside one body re-runs only that body's query
/// (provided the item scope itself backdates clean).
#[salsa::tracked]
pub fn lower_fn<'db>(db: &'db dyn salsa::Database, fn_id: StableFnId<'db>) -> Memo<LoweredFn> {
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

    let lowered = hir::core::lower_fn_body(
        &scope.scope,
        arena_id,
        &fn_ast,
        &scope.const_values,
        &lexed.interner,
    );
    // the firewall digest: this body's text plus the scope signatures it lowered
    // against. an unedited sibling re-lowers to a bit-identical body with an
    // identical digest, so this query backdates and the sibling's `typeck_fn`
    // cache-hits.
    let digest = body_digest(file.text(db), &fn_ast, scope.sig_digest);
    Memo::new(LoweredFn { lowered, digest })
}

/// type-check one function body against the frozen item scope (the per-fn
/// query, S2C step d). sealed-body inference: no type fact crosses a function
/// boundary, so a body types independently of every other - on its own interner
/// clone (`lower_fn`'s), against the function's declared return type. keyed by
/// [`StableFnId`], so a body edit re-runs only this query (clean siblings stay
/// cached). the diagnostics are self-contained (display strings are baked in at
/// emit); the result's `expr_types` handles resolve through this body's
/// interner, so they are NOT comparable across bodies - the cross-body codegen
/// path keeps its own shared-interner typeck in [`lowered_file`].
#[salsa::tracked]
pub fn typeck_fn<'db>(db: &'db dyn salsa::Database, fn_id: StableFnId<'db>) -> Memo<TypeckResults> {
    let file = fn_id.file(db);
    let scope = item_scope(db, file);
    let lowered = lower_fn(db, fn_id);

    let ptr = fn_id.ptr(db);
    let arena_id = scope
        .fns
        .iter()
        .find(|(_, p)| *p == ptr)
        .map(|(id, _)| *id)
        .expect("StableFnId ptr is a collected function");
    let fn_ret = scope.scope.functions[arena_id].ret;
    let fn_ret_span = scope.scope.functions[arena_id].ret_span.clone();

    // no interner clone (S6): `lower_fn` interned this body's types into the
    // shared `item_scope` interner, and `typeck_fn` reads/interns through the
    // same one. consistency holds because `lower_fn` and `typeck_fn` re-run
    // together for an edited body (a body edit shifts the `FnDef` range -> new
    // `StableFnId` -> both queries are fresh against the current item scope);
    // a clean sibling's cached result is consumed only as diagnostics, which
    // carry baked-in strings + type names, never raw handles.
    Memo::new(typeck::check_body(
        &scope.scope,
        &lowered.lowered.body,
        fn_ret,
        fn_ret_span,
        &scope.scope.types,
    ))
}

/// every HIR diagnostic for the file: item-scope diagnostics plus each
/// function's body diagnostics, in collection order. runs the per-fn
/// `lower_fn` and `typeck_fn` queries, so a clean body costs a cache hit.
pub fn hir_diagnostics(db: &dyn salsa::Database, file: SourceFileInput) -> Sink<HirError> {
    let scope = item_scope(db, file);
    let mut out = scope.scope.diagnostics.clone();
    // per-fn lowering then type diagnostics, in collection (arena) order: a body
    // edit re-runs only that body's `lower_fn` + `typeck_fn`; clean siblings hit
    // the cache.
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(db, file, ptr);
        out.extend(lower_fn(db, fn_id).lowered.diagnostics.clone());
    }
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(db, file, ptr);
        out.extend(typeck_fn(db, fn_id).diagnostics.clone());
    }
    // effect-contract diagnostics (the `E` class) stay whole-file: the effect
    // verdict is a whole-program fixpoint (`effect::infer_file`), so it cannot
    // be per-fn. decoupling it from the codegen-oriented `lowered_file` (a
    // per-fn effect-atom query feeding a cheap fixpoint query) is future work,
    // tracked with the LSP-latency push.
    let checked = lowered_file(db, file);
    out.extend(checked.effect_diagnostics.clone());
    out
}

/// the whole-file lower + inference result: HIR with one shared interner plus
/// the per-fn type side tables and the whole-program effect map
/// (`effect::infer_file`, the fused dual-inference walk - types and effects are
/// derived in one traversal per body, run while the interner is still growable
/// so every handle in the results resolves through `hir.types`). this is the
/// c-generation path: codegen compares `TypeRef` handles across bodies
/// (type-declaration ordering, array-wrapper dedup), which requires a single
/// interner. recomputes on any file edit - the c output is a function of the
/// whole file, so that is the honest granularity.
pub struct CheckedFile {
    pub hir: HIR,
    pub typeck: FxHashMap<FnId, TypeckResults>,
    /// the whole-program effect verdict (every fn's total machine effect).
    /// computed from the same walk as `typeck`; the backend does not read it
    /// yet, but it feeds the prime gate (EFFECT.md) and is memoized here so it
    /// is not recomputed.
    pub effects: EffectMap,
    /// effect-annotation contract diagnostics (unknown effect names,
    /// declared/inferred mismatches) - the `E` class. merged into the file's
    /// diagnostics and gates c generation like the type diagnostics.
    pub effect_diagnostics: Sink<HirError>,
}

#[salsa::tracked]
pub fn lowered_file(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<CheckedFile> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let hir = hir::core::lower_source_file(parse.ast(), &lexed.interner);
    let (typeck, effects, effect_diagnostics) = effect::infer_file(&hir);
    Memo::new(CheckedFile {
        hir,
        typeck,
        effects,
        effect_diagnostics,
    })
}

/// every defined function's MIR, lowered from [`lowered_file`]. memoized so
/// `--dump-mir` and `c_code` share one lowering pass per revision.
#[salsa::tracked]
pub fn mir_map(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<FxHashMap<FnId, MirBody>> {
    let checked = lowered_file(db, file);
    Memo::new(mir::lower_all(&checked.hir, &checked.typeck))
}

/// the generated c translation unit. empty when any front-half diagnostic
/// fired: MIR lowering and the emitter assume a resolved, well-typed HIR
/// (poison expressions panic), and the driver renders diagnostics and bails
/// before reading the c anyway - same contract as the pre-salsa pipeline.
#[salsa::tracked]
pub fn c_code(db: &dyn salsa::Database, file: SourceFileInput) -> Memo<String> {
    let lexed = lex(db, file);
    let parse = parse(db, file);
    let checked = lowered_file(db, file);
    if !lexed.diags.is_empty()
        || !parse.diagnostics.is_empty()
        || !checked.hir.diagnostics.is_empty()
        || checked.typeck.values().any(|r| !r.diagnostics.is_empty())
        || !checked.effect_diagnostics.is_empty()
    {
        return Memo::new(String::new());
    }
    let mirs = mir_map(db, file);
    let seed = typeck::expr_type_seed(&checked.typeck);
    Memo::new(codegen::core::gen_mir(&checked.hir, &mirs, &seed))
}

#[cfg(test)]
mod tests;
