# TYPECK: sealed-body type inference

Status: BUILD IN PROGRESS. Ratified 2026-06-12. S0-S2 built (cutover C1-C5
complete - lowering no longer types any expression, typeck is the sole type
source), S3 complete 2026-06-16 (judgments, including M2b strict-width), S4
effects built, S5 firewall built 2026-06-16, S6 parallel wave built 2026-06-16,
Unit/Never types built 2026-06-17 (the void rule, below), the Tier-2 expectation
spine built 2026-06-17 (downward propagation + the unified `expect` funnel,
below), let-from-init inference built 2026-06-18, two-span `Cause` diagnostics
built 2026-06-18 (the secondary "declared here" label renders). Remaining: S7
row-poly effects (designed, not built). LSP hover built 2026-06-18. This document is the engineering design
and the ratified inference strategy; status sigils track what exists in the
working tree. The cast lattice ruling lives in [CAST.md](CAST.md). [EFFECT.md](EFFECT.md) designs the second
lattice on the same machine. [PARALLEL.md](../design/PARALLEL.md) records the
parallelism substrate this strategy is built for.

## Goal

Today lowering (`crates/hir/src/core/lower/`) does four jobs in one walk:
builds the HIR, resolves names, stamps types into `Body::expr_types`, and runs
the type judgments - and at coercion sites (`coerce.rs`) it *mutates* the tree
(injects decay casts, retypes literals and array elements). The split:

+ `crates/typeck` is the sole type source for the frozen HIR (cutover done)
+ `TypeckResults` side table populated: `expr_types`, `adjustments`,
  `local_types`, `diagnostics`, `visited`
+ lowering no longer types any expression - `Body::expr_types` deleted at C5,
  `coerce.rs` deleted, the shadow harness deleted
+ `InferObserver` trait wired; `crates/effect` implements it (S4 dual inference)

## The strategy: sealed-body inference

Ratified 2026-06-12. One invariant, three tiers.

### The governing invariant

**No inference fact ever crosses a fn boundary.** Signatures are the only
inter-fn type channel; concrete effect sets in signatures are the only
inter-fn effect channel (EFFECT.md). This is the Explicit Contract rule
(PARALLEL.md) elevated to architectural law. Consequences:

- Per-body checking is embarrassingly parallel, permanently - including after
  the macro engine, including after any in-body inference power upgrade.
  Power upgrades happen inside the seal, never through it.
- Salsa dependency edges stay coarse and stable (a body's check reads only
  the signatures it calls), which is what lets the signature firewall
  (below) backdate precisely.
- The only whole-program computation is the effect fixpoint, which the
  lattice choice keeps at bitset-union cost.

This is the structural property that makes Sorbet and types-first Flow fast
(explicit signatures, per-declaration parallel checking) and that global
Hindley-Milner inference forfeits. Eye gets it by kernel ruling, not
retrofit.

### Tier 1 - the bidirectional spine (always on)

+ built: `infer_expr(id, expected)` walks every `Expr` variant in
  `typeck/src/infer/`; bottom-up synthesis is complete and is the sole type
  source for MIR.
+ built (2026-06-17): expectations flow *down* through every transparent node.
  `infer_expr` and `infer_block` take an `Expectation`; a block forwards it to
  its tail, an `if`/`match` to each branch/arm (re-tagged `IfBranch`/`MatchArm`
  by `rebind`), a `return` to its value; an imposing site (let-init, call
  argument, struct field) starts a fresh one; an operand position passes `None`.
  the bottom-up type is funneled through the single `expect` (below). this
  replaced the external `site_coerce` (one-level forwarding) and the scattered
  per-site mismatch checks.
+ built (2026-06-18): **let-from-init inference** - the spine makes an untyped
  `let x = <init>` bind `x` to the init's bottom-up synthesized type (no
  inference variables; the type already exists). lowering no longer rejects an
  untyped let; the `infer_stmt` Let arm records `local_types[x]` from the init's
  type when it is concrete (`is_inferrable`: not `Error`/`()`/`!`), and MIR reads
  that fallback. T025 `MissingTypeAnnotation` now fires only for a value-less
  init (`()`/`!` - nothing to bind). this is annotation-omission, distinct from
  the Tier-2 CHECKING spine.
+ built (2026-06-18): two-span `Cause` diagnostics. the imposing-site causes
  (`Arg`/`Field`/`Return`) carry the declaration's span (`decl`); the mismatch
  `TypeError` variants carry it too and override `Diagnostic::secondary_labels`,
  so a mismatch renders the primary span plus a secondary "parameter/field/return
  type declared here" label. the decl spans live on the HIR signature (`Param`/
  `Field` gained `span`, `Function` gained `ret_span`); the signature digest is
  text-based, so the firewall is unaffected.

```rust
fn infer_expr(&mut self, id: ExprId, expected: Expectation) -> TypeRef
```

Types flow up from leaves; expectations flow down from declared types. The
expectation sources are exactly today's coercion sites: `let` declared types,
call arguments (against the callee signature), explicit `return` and the fn
tail (against the declared return), struct-literal fields (against the field
type), array-literal elements (against the element type), match arms in value
position (against the result type). Every found/expected meeting funnels
through one judgment point:

```rust
fn expect(&mut self, id: ExprId, found: Option<TypeRef>, expected: Expectation) -> Option<TypeRef>
// 1. no expectation / Error poison -> found unchanged.
// 2. coerce_to(exp, id): adopt a literal/divergent value to the expected width,
//    re-type a value-position if/match onto it, coerce an array literal, file
//    Adjustment::Decay. the value keeps its own type on a decay.
// 3. mismatch at an atomic Arg/Field/Return site -> the cause's TypeError. a
//    transparent container (if/match/block) delegates to the branch/arm
//    consistency checks; the adopt-only causes (LetDecl/IfBranch/MatchArm) own
//    their mismatch in a separate judgment.
```

`expect` is coerce's successor with the missing rule, the funnel every
found/expected meeting passes through. A non-coercing mismatch no longer leaks
to clang. No solver, no variables: the frozen kernel resolves entirely in one
walk (explicit signatures, no generics; an unannotated `let` takes its
initializer's type). The funnel keeps the found type on a reported mismatch
(rather than poisoning to `Error`) so a rejected program's remaining checks
still see concrete types; a diagnosed program never reaches codegen.

Integer literals: a literal adopts the expected integer type when an
expectation exists; the `int32` default applies only when none does
(`let x = 5` stays `int32`). The M1 range sweep moves into the pass and runs
against the adopted type.

### Tier 2 - provenance-carrying expectations

+ `Cause` and `Expectation` defined in `crates/typeck/src/lib.rs`
+ threaded through `infer_expr` (2026-06-17): every `HasType` carries a `Cause`,
  which the funnel uses to (a) select the mismatch `TypeError` variant and (b)
  pick that site's assignability policy
+ built (2026-06-18): the *secondary span* - `Arg`/`Field`/`Return` carry the
  declaration's `decl: Option<Span>`, the funnel forwards it into the mismatch
  variant, and `secondary_labels` renders it as the "declared here" label

An expectation carries *why* it exists. As built, the cause names the site (and
the data its `TypeError` needs); the doc's far design adds the declaration's
`SyntaxNodePtr` per variant for the second span:

```rust
pub enum Expectation { None, HasType(TypeRef, Cause) }

pub enum Cause {
    LetDecl,                  // adopt-only (let-init check owns the mismatch)
    Arg { index: usize },     // -> ArgTypeMismatch
    Return,                   // -> ReturnTypeMismatch
    Field { name: Text },     // -> StructFieldTypeMismatch
    IfBranch,                 // adopt-only (if-branch consistency owns it)
    MatchArm,                 // adopt-only (match-arm consistency owns it)
    // two-span extension: a SyntaxNodePtr per variant for the imposing decl.
    // Horizon 2: Expansion { origin: OriginId, inner: Box<Cause> }
}
```

Near payoff (once the second span lands): every type mismatch is natively a
two-span diagnostic - "mismatch here, expected `int32` because of the return
type declared there."
Far payoff (the native-errors-for-injected-features aspiration, MASTERPLAN
Horizon 2): when the macro engine desugars injected syntax to kernel HIR, an
origin table wraps causes in `Expansion` frames and a type error inside
generated code walks the chain back to the user's syntax. Multi-span
origin-tracking diagnostics become a new `Cause` variant on machinery that
existed from day one, not a bolted-on subsystem.

Supporting rule, kept as law: diagnostics stay structured facts (found type,
expected type, cause chain, ExprId) until render time - never pre-rendered
strings. The typed diagnostic model already works this way; it is what lets
a future macro register an error lens that re-vocabularizes its own
desugarings.

### Tier 3 - dormant body-local variables (designed seam, not built)

`Expectation` and `expect` are written against a seam where an inference
variable can appear; the kernel never creates one. When future power needs
variables (macro-generated code with elided types, generic instantiation
checking), they activate under two rules inherited from the invariant:

- **Body-local only.** Born and solved inside one body's task; never escapes
  into a signature, never creates a cross-fn constraint. The solver is a
  per-body union-find, not a global graph.
- **Solved at the seal.** When a body's check ends, every variable is
  resolved or it is a T-class diagnostic with its cause chain.
  `TypeckResults` stays complete; MIR's no-holes contract never weakens.

This replaces the Hindley-Milner line in the original PARALLEL.md draft:
unification arrives (if ever) as a sealed, dormant tier, not a global
discipline.

## TypeckResults (the side table)

+ built and populated in `crates/typeck/src/lib.rs`:
  + `expr_types: ArenaMap<Idx<Expr>, TypeRef>` - populated during walk
  + `adjustments: ArenaMap<Idx<Expr>, Adjustment>` - decay recorded (C4)
  + `local_types: ArenaMap<Idx<Local>, TypeRef>` - unannotated let + match
    bindings bound from scrutinee/init (C2)
  + `diagnostics: Sink<HirError>` - judgment errors collected
  + `visited: FxHashSet<ExprId>` - reachable expressions tracked

```rust
pub struct TypeckResults {
    /// COMPLETE (post-S2 invariant): every expression has a real type or
    /// `Error` (with a diagnostic already emitted).
    pub expr_types: ArenaMap<Idx<Expr>, TypeRef>,
    /// Context-directed adjustments MIR applies when reading an expression.
    /// Replaces coerce's HIR mutation; one kind today (decay). Populated (C4).
    pub adjustments: ArenaMap<Idx<Expr>, Adjustment>,
    /// Every local's resolved type. Populated (C2): unannotated `let` and
    /// match bindings, bound from initializer / scrutinee.
    pub local_types: ArenaMap<Idx<Local>, TypeRef>,
    pub diagnostics: Sink<HirError>,
}
```

+ `Adjustment` enum (single variant `Decay(TypeRef)`); recorded at decay sites,
  read by MIR's adjustment-aware `lower_operand`/`lower_rvalue` wrappers

Precedent: rustc `TypeckResults`, rust-analyzer `InferenceResult` (which
stores `expr_adjustments` the same way). The pass never mutates the HIR; the
adjustment table is what lets decay stay a typeck concern while typeck stays
pure. Side table over typed-HIR because one HIR serves many result sets and
`Body` stays shareable.

Entry point:

```rust
// S6 (lock-free interner): `intern(&self)`, so `check_body` borrows shared.
pub fn check_body(scope: &HIR, body: &Body, types: &TypeInterner) -> TypeckResults
```

+ BUILT (S6): the `TypeInterner` is lock-free - a `boxcar::Vec` arena (lock-free
  append, stable addresses, so `lookup` hands out `&TypeKind` with no guard) plus
  a `papaya::HashMap` for dedup. `intern` takes `&self`; a lost insert race
  leaves a dead arena slot nothing references (tolerated - `get_or_insert` elects
  one canonical index, so structural equality stays a handle compare). This
  killed the per-fn interner clone, the whole-file take-and-restore dance, and
  the `&mut TypeInterner` signatures across hir-lowering / typeck / effect (all
  now `&TypeInterner`). Fallback if `papaya` disappoints: `dashmap`.
~ divergence from the original plan: the interner stays homed in the `HIR`
  (`item_scope`'s `HIR::types`, shared per file), not lifted into the `Database`.
  The HIR home is enough for sharing - both query paths read the one `item_scope`
  interner, the per-fn path consumes its results only as (handle-independent)
  diagnostics - and avoids threading a custom `EyeDatabase` trait through every
  query. A `Database`-homed interner (process-monotonic handles across revisions)
  is a later refinement if LSP hover ever needs cross-revision handle stability.

## Judgments

+ all type-directed judgments now live in typeck (`crates/typeck/src/infer/`
  + `check_matches`); the cutover (C5) deleted lowering's type computation:
  + `check_int_literal_ranges` (M1)
  + `binary_judgments` - array ops, ptr arithmetic, enum arithmetic, float modulo
  + `index_judgments` - ptr index, OOB, negative index
  + `expect`/`coerce_to` - coercion at array literals, int/float literal
    adoption, decay (the spine funnel; was `site_coerce` before 2026-06-17)
  + return/tail checks, value-position match-arm and `if`-branch consistency
  + let-init type + array-init length
  + enum opacity (T035), `LenNotArray`/`LenFieldOnArray`/`PrintCannotFormat`
  + struct-literal field value types (T38), call argument types (T37)
  + the S3 judgments (M2, cast lattice, F1/F2/F3, L4, assignment-non-value)
+ stays in lowering (structural, place/storage - not type judgments):
  + mutability / immutable-by-default, ref-of-non-place (T036), ref-of-const
  + arity, struct-literal field-set exhaustiveness, destructure shape
  + `println` arity (structural)
  + U2/U4 const range + cast checks (const-eval, layered below inference)

The deferred S3 list below is now fully built; it is kept for the rulings
that closed each design question (ratified 2026-06-12):

- **M2 operand rule: strict same-type.** Binary arithmetic, bitwise, and
  comparison operands must have equal types after literal adoption; a
  mismatch (`usize + int32`) is a type error telling the user to cast.
  No promotion, no narrowing - the M2 miscompile class dies by rejection.
  Result type = the operand type (comparisons: `bool`). Pointer-typed
  operands keep the kernel's existing pointer rules; their exact judgment is
  specified at build time against the corpus.
- **Assignment is non-value.** `x = y` (and compound assigns) in value
  position is a type error (Rust's rule, minus the `()` value). Kills
  `if x = y` and `a = b = c`. Statement-position assignment is unaffected.
  Today it silently types as the RHS; that was a ledger-accepted typing gap,
  so tightening it in typeck is the sanctioned fix, not a freeze violation.
- **Cast lattice for `as`** (today: unrestricted any-to-any). Allowed:
  integer<->integer (any width or signedness), integer<->float,
  char->integer, bool->integer, enum->integer, ptr<->ptr (any pointee),
  integer<->ptr (the NULL idiom `0 as ptr`). Rejected: anything->bool (write
  `!= 0`), anything->char, integer->enum (no validity check exists),
  struct/array/fn-type casts. Every rejection is relaxable later.
- **Struct-literal field value types** checked against declared field types.
- **Call argument types** checked against the signature (arity already is).
- **`const` declared type vs folded value**, plus const-eval U2 (value range)
  and U4 (cast truncation) checks. Direction stays layered per PRIME: type
  resolution may call const-eval, const-eval never calls back into
  inference; cycles are typegraph's job.
- **Tail-expression enforcement** in value-position blocks (the `malloc`
  tail in an `int32*` `if` arm).
- **L4 cross-element judgment**: every array-literal element against the
  element type.
- **`types_compatible` integer leniency deleted**: with literal adoption in
  the pass, compatibility is exact `TypeRef` equality (plus Error poison).

Two representation repairs land as prep (segment S0, below), because both the
old and new code paths need them:

- **`ptr` gets a real `TypeKind`** (e.g. `TypeKind::RawPtr`), killing the
  magic `Path("ptr")` dispatched at every type judgment.
- **`TypeKind::Fn` gains a variadic flag**, closing the indirect-call arity
  hole (L3 residue).

## Poison discipline

+ built: `Error` absorbs — any judgment with an `Error` operand emits nothing
  and returns `Error`. `Resolution::Unresolved` types as `Error` silently.
+ enforced at each operand-combining judgment in `infer.rs`

## The completeness contract

When the pass cannot determine an expression's type it emits a T-class
diagnostic and records `Error` - it never leaves a hole. Consequences:

+ MIR's `mir_type_of` `int32` fallback (ledger A3, the silent amplifier) is now
  an ICE: after typeck, a missing type is a compiler bug. Proven never-fires by
  `corpus_generates_no_error_type` (codegen only runs on a diagnostic-free
  program where the walker is total).
+ Codegen keeps its existing contract (never sees a diagnosed program).

## Boundary rulings (design-space audit, 2026-06-12)

- **Top-level initializers.** Global and const initializer expressions
  belong to no fn body, so `check_body` never sees them. A
  `check_item_initializers` entry point in the same crate runs once per
  file; the const declared-type judgment lives there. Global initializers
  are const-expr today, so they have no effect surface (assumption
  recorded; revisit if initializers ever call fns).
+ **The void rule + Unit/Never (BUILT 2026-06-17).** Backed by a real
  Rust-style type pair, so the walker is total over all control flow (closes
  the cascading MIR-ICE on a value-less `if`/`loop`):
  - `TypeKind::Unit` (`()`, the value-less completing type) - a tail-less block,
    an else-less `if`, a bare assignment, a void fn body. Pre-interned
    (`unit_ty()`); spellable in source (`f() -> () { }`, parser `UnitType` node,
    normalized to void in `InferCtx::new`). A value-position expr that types
    `()` is rejected: `VoidValueInValuePosition` (T024), via the post-walk
    `check_value_position_voids` sweep. A void-fn call used as a value lands here.
  - `TypeKind::Never` (`!`, the bottom/divergent type) - `return`/`break`/
    `continue`/bare `loop {}`. Pre-interned (`never_ty()`); inference-internal
    only (no source syntax). Coerces to any expected type: the Never-absorbing
    `join`/`join_opt` makes branch unification uniform (`if c { 5 } else { return }`
    is `int32`; `loop {}` satisfies any return type), and `types_compatible`
    short-circuits on it.
  Both render to C `void`; `mangle` uses `unit`/`never`.
+ **Pattern judgments split.** Built (C2, rust-analyzer name-based model):
  typeck's `check_matches` owns domain/coverage/exhaustiveness/dup/unreachable
  + variant-belongs (`PatternEnumMismatch`) and binds `local_types`; structural
  pattern classification (bare ident = variant iff name in `ItemScope::variants`
  else binding) stays in lowering, type-free.
+ **Mutability checking stays in lowering.** Immutable-by-default
  enforcement is a place/storage judgment over resolution, not a type
  judgment; it works pre-typeck and gains nothing from moving. Already
  in lowering, not duplicated in typeck.
+ **LSP consumers.** The per-fn `typeck_fn(StableFnId)` query is BUILT (S2
  step D) and feeds `hir_diagnostics` per fn. The hover handler is BUILT
  2026-06-18 (`crates/lsp/src/server/requests.rs`, `textDocument/hover`): a
  cursor position resolves to a byte offset (`SourceText::offset_utf16`, the
  inverse of `line_col_utf16`), `hover_type` finds the innermost
  `body.source_map.expr` range covering it, and reads that expr's type from the
  fn's `TypeckResults.expr_types` (rendered via the interner). inlay hints are a
  later surface; no inlay yet.

## Salsa wiring and the signature firewall

+ `database::lowered_file` runs the fused `effect::infer_file(&mut hir)` (one
  walk = types + effects) and stores `CheckedFile { hir, typeck, effects, ... }`
+ `mir_map` reads `typeck` from `CheckedFile` for every `lower_function` call
+ per-fn `typeck_fn(StableFnId) -> Memo<TypeckResults>` BUILT (S2 step D):
  sealed-body check over one `lower_fn` body on its own interner clone, keyed by
  `StableFnId` so a body edit re-runs only that query
+ `hir_diagnostics` sources type diags per-fn from `typeck_fn`, plus the
  whole-file effect diags (E-class), merged into the HIR sink
+ `c_code` gates on typeck + effect diagnostics (same as lowering diagnostics)

```text
SourceFileInput
  └─ lex ─ parse ─┬─ item_scope ─ lower_fn ─ typeck_fn   (per StableFnId; BUILT)
                  └─ lowered_file ─ [effect::infer_file] ─ mir_map ─ c_code
                                  └─ effect_map (whole-file fixpoint; BUILT)
```

`typeck_fn(StableFnId)` depends on `lower_fn`; editing one body re-runs one
body's check (and, with the S5 firewall below, only that one). The whole-file
`lowered_file` path runs the fused walk per body with the shared interner
(codegen needs comparable handles across bodies). `c_code` and the driver gate
on typeck + effect diagnostics exactly as they gate on lowering diagnostics.

**The firewall**: before it, `Memo`'s `Arc::ptr_eq` meant no query ever
backdated, so any edit re-ran everything downstream of `item_scope`. For
keystroke-flat latency the signature data gains structural equality:

1. + First step (segment S5) BUILT 2026-06-16. `Memo`'s blanket `Arc::ptr_eq`
   `PartialEq` became a `MemoEq` trait (default conservative-false, the old
   behavior), overridden for the two firewall results by a **content digest**
   (not a deep `PartialEq` - correct-by-construction since lowering is
   deterministic and `Text` is an owned `SmolStr`, no interner-id drift):
   - `FileScope.sig_digest` - a hash of every item with fn *bodies* excluded; a
     body-only edit leaves it equal, so `item_scope` backdates.
   - `LoweredFn.digest` - this body's text combined with `sig_digest`; a sibling
     body edit re-runs `lower_fn` but it backdates, so the sibling `typeck_fn`
     cache-hits. Keystroke cost becomes: reparse one file + recheck one body +
     the effect fixpoint - flat in project size. Verified both directions
     (`body_edit_backdates_the_sibling_typeck`, `signature_edit_reruns_every_body`).
2. - End state (with the multifile milestone): per-item signature queries
   (`fn_signature(StableFnId) -> Signature`, derived `Eq`), so salsa's
   read-tracking gives caller-precise invalidation automatically.

Effect queries compose with this naturally: per-fn atom results and the
whole-file effect map derive `Eq`, so they backdate exactly (EFFECT.md).

## The parallel wave (segment S6)

**STATUS: BUILT 2026-06-16.** The lock-free interner, the whole-file fused
per-body fan-out, and the determinism gate are in the working tree. The type
interner is `boxcar::Vec` + `papaya::HashMap` with `&self` interning; the
whole-file driver (`effect::infer_file` -> `collect_results`) runs the fused
type+effect walk one task per body across `rayon`, interning into the one shared
interner with no clone. Built but deliberately scoped out (below): the per-fn
`hir_diagnostics` fan-out (the S5 firewall already makes that path
one-body-incremental, so parallelizing mostly-cache-hits is low value) and
Wave 0 parallel *lowering* (`lower_source_file` stays serial - body allocation
into the HIR arena needs `&mut`, and lowering is cheap next to the walk).

Threads, not async/await: inference is CPU-bound, so concurrency is
structured parallelism over salsa snapshots (cheap read-shared handles onto
the memoized storage, one per worker). Async exists only at the LSP I/O
boundary. Three lanes:

```text
LSP main loop (owns the DB; an edit = a revision bump)
  ├─ foreground lane: per-request snapshot -> open-file diagnostics, hover
  ├─ background lane: one snapshot -> whole-program work (effect fixpoint,
  │                   full-project check), surfaces results when ready
  └─ rayon pool: shared compute workers both lanes fan out onto
```

- Wave 0, per file / per fn: lex, parse, and `lower_fn` fan out (body
  lowering reads only the item scope, so it is seal-isolated too).
- Wave 1, embarrassingly parallel: one task per body runs the *fused* walk -
  type inference and effect-atom collection on the same traversal. The
  fusion crosses a crate boundary through an observer seam: `typeck` owns
  `trait InferObserver` (called per visit with the just-computed type) and
  `crates/effect` implements it (EFFECT.md "Crate boundary"); `()` is the
  type-only no-op impl. Tasks own their results outright - no shared
  writes.
- Wave 2, the serial core: join, then the effect fixpoint (Tarjan
  condensation, bitset unions) on the joining thread - O(V + E) on
  byte-sized sets, no synchronization.
- Cancellation is the load-bearing latency mechanism: a revision bump
  unwinds in-flight queries at their next query boundary (salsa's
  `Cancelled` protocol), so a keystroke abandons stale work instead of
  waiting behind it. Code rule it imposes: no side effects inside queries
  an unwind could half-complete (D3's pure functions already guarantee
  this).
- The data plane is lock-free end to end: rayon's work-stealing deques, the
  boxcar/papaya interner, task-owned results. Salsa's control plane (memo
  tables) has internal synchronization we neither see nor fight.
- Crate bill: `rayon`, `boxcar`, `papaya` (this segment);
  `lasso::ThreadedRodeo` at the multifile milestone for global symbol
  interning.

**Determinism laws** (parallel runs must be byte-identical to serial runs):

1. Diagnostics are collected per body and rendered in collection order,
   never completion order.
2. No observable output may depend on `TypeRef` numeric order: handles are
   stable within a process (append-only interner, so backdating is
   unaffected), but parallel interleaving changes their values across runs.
   Tie-breaks in codegen ordering must use names/structure, not handles.
   Gate: run the corpus twice under the wave, diff the C byte-for-byte.

+ **Validation spike DONE (2026-06-16).** salsa's parallel-snapshot API is
version-sensitive, so S6 opened by proving it against the pinned 0.27. Result:
`Database: Send` (not `Sync`), so the model is owned db *clones* (cheap
`Storage` handle bumps onto the shared, internally synchronized memo tables)
moved into workers via `into_par_iter`; interned ids and inputs are valid
across clones of the same storage. The per-fn `typeck_fn` query is already
seal-isolated, so it parallelizes with zero interner change and is trivially
deterministic (no shared whole-file interner = no handle-order dependence;
law #2 vacuous, law #1 held by the order-preserving collect). Proven by
`database::tests::parallel_per_fn_typeck_matches_serial` (parallel per-fn
diagnostics == serial, in collection order). The fallback (pure check fns over
pre-collected inputs inside one query) was not needed.

Multifile readiness (later milestone, designed for now): one
`SourceFileInput` per file, an import-graph query, per-file item scopes, and
global resolution through per-file export tables with structural equality -
editing file A re-resolves file B only when A's exports changed. Cold start:
daemon-first (the LSP is the daemon; a watch-mode CLI reuses the same
database); on-disk persistence stays deferred.

## Migration plan (suite green at every step)

+ **S0 - representation prep (BUILT).** `RawPtr` kind + Fn variadic flag in
  the current code. Mechanical, touches every `Path("ptr")` dispatch site.
  Verified: every judgment site dispatches on `TypeKind::RawPtr`, not
  `Path("ptr")`.
+ **S1 - the pass, in shadow mode (BUILT).** `crates/typeck` exists with
  Tier 1 spine + Tier 2 causes + the Tier 3 seam. The pass re-derives types
  over lowered HIR while lowering still stamps. Shadow harness asserts parity
  (335 workspace tests + corpus regression, all green). `InferObserver` trait
  + no-op impl built (seam for S4). `Cause`/`Expectation` enums defined here
  (threaded through `infer_expr` later, by the 2026-06-17 spine build).
+ **S2 - cutover (BUILT, C1-C5 complete).** step A: MIR reads `TypeckResults`
  (`lower_function` takes it, `mir_type_of` reads it). step B: all judgments
  migrated to `typeck/src/infer/` + `check_matches` (tests in
  `typeck/tests/judgments.rs`). step C (irreversible): `coerce.rs` deleted,
  lowering's `Body::expr_types` deleted, `adjustments`/`local_types` populated,
  A3 fallback is an ICE, shadow harness deleted. step D: per-fn
  `typeck_fn(StableFnId)` query built; `hir_diagnostics` sources type diags
  per-fn.
+ **S3 - new judgments (BUILT).** M2 literal-width operand adoption + M2b
  strict-width reject (T44, two distinct concrete widths), assignment non-value
  (T39), cast lattice (T43, [CAST.md](CAST.md)), struct-field value types (T38),
  call argument types (T37), const declared-type value+kind check (U2/C13,
  C14) + cast truncation (U4), F1 if-branch consistency (T41), F2
  negation-on-unsigned (T40), F3 float-literal adoption, L4 cross-element (T42),
  `types_compatible` integer-family leniency fully removed (exact-width at
  arguments/fields/returns, the boundary analogue of M2b; the spine forwards
  the expected width into value-position `if`/`match` branches so branch literals
  adopt). Open: tail-expression enforcement in value blocks (the raw-ptr->
  typed-ptr leniency, e.g. `malloc()` into `int32*` - a `ptr`-ergonomics ruling).
+ **S4 - effects (BUILT).** [EFFECT.md](EFFECT.md): `crates/effect` implements
  the `InferObserver` seam. `EffectSet` bitset (io/ffi/state), contextual
  annotation surface, Tarjan-SCC condensation fixpoint, witness-trail
  diagnostics, E-class (9th). Fused with the type walk (one traversal).
+ **S5 - the firewall (BUILT 2026-06-16).** Structural signature backdating via
  a content digest. `Memo<T>`'s `PartialEq` delegates to a `MemoEq` trait
  (default conservative-false); `FileScope.sig_digest` (bodies excluded) and
  `LoweredFn.digest` (body text ^ sig_digest) override it, so a body-only edit
  backdates `item_scope` and unedited bodies' `typeck_fn` cache-hit. See "The
  firewall" above. Open: lex/parse/whole-file backdating + per-item
  `fn_signature` queries (multifile).
+ **S6 - the parallel wave (BUILT 2026-06-16).** Lock-free interner
  (`boxcar`+`papaya`, `&self` intern); the whole-file fused per-body walk fans
  out across `rayon` (`collect_results`), each body interning into the one
  shared interner with no clone; the determinism gate passes (corpus C
  byte-identical across 8 separate-process runs/file, plus the
  `parallel_inference_is_deterministic` regression test over fresh databases).
  Scoped out (documented above): the per-fn `hir_diagnostics` fan-out (firewall
  makes it low value) and Wave 0 parallel lowering.
- **S7 - row-polymorphic effects (NOT BUILT).** Effect variables on fn types
  for precise higher-order effect tracking. Requires S6 lock-free
  infrastructure. See EFFECT.md "Path forward".

  Note: the **Tier-2 expectation spine** (downward `Expectation` propagation,
  the unified `expect` funnel) was built 2026-06-17 (not numbered as a segment -
  orthogonal to S6/S7). `infer_expr(id, expected)` threads an `Expectation`
  through every transparent node; the funnel adopts/coerces and reports the
  cause-specific mismatch. The one remaining in-kernel inference piece is the
  two-span render (the secondary span the `Cause` will carry).

## Open at build time

~ Pointer-operand arithmetic judgment (noted under M2) - design settled, still
  deferred (no corpus program needs it). All ptr arithmetic stays rejected
  (`ArithmeticOnPtr`, T029); relax when a real program needs pointer math.
- Shard count and shard-key choice for the global interner (S6).
- Whether `TypeckResults` itself gets structural backdating (cheap to add
  once diagnostics derive `Eq`; decide on measurement).
