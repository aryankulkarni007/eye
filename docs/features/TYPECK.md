# TYPECK: sealed-body type inference

Status: BUILD IN PROGRESS. Ratified 2026-06-12. S0-S1 built, S2 in progress
(step B/C cutover), S3-S6 designed but not built. This document is the
engineering design and the ratified inference strategy; status sigils track
what exists in the working tree. [EFFECT.md](EFFECT.md) designs the second
lattice on the same machine. [PARALLEL.md](../design/PARALLEL.md) records the
parallelism substrate this strategy is built for.

## Goal

Today lowering (`crates/hir/src/core/lower/`) does four jobs in one walk:
builds the HIR, resolves names, stamps types into `Body::expr_types`, and runs
the type judgments - and at coercion sites (`coerce.rs`) it *mutates* the tree
(injects decay casts, retypes literals and array elements). The split:

+ `crates/typeck` crate exists and checks frozen HIR
+ `TypeckResults` side table defined: `expr_types`, `diagnostics`, `visited`
- `adjustments` and `local_types` maps not yet populated (S2 step C)
- lowering's stamping still active; shadow harness validates parity (S1)
- `coerce.rs` not yet deleted (S2 step C)
+ `InferObserver` trait defined and wired (seam for effects, S4)

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

+ built: `infer_expr` walks every `Expr` variant in `typeck/src/infer.rs`
- not yet: expectations (`Expectation` enum) threaded through `infer_expr`
  (currently walker is type-only, no downward-propagation of expected types)

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
fn expect(&mut self, found: TypeRef, expected: Expectation, id: ExprId) -> TypeRef
// 1. equal (or Error poison either side) -> found
// 2. decay applies -> record Adjustment::Decay, return expected type
// 3. mismatch -> T-class diagnostic carrying the cause chain, return Error
```

`expect` is coerce's successor with the missing third rule: today a
non-coercing mismatch leaks to per-site checks or to clang; after, nothing
leaks. No solver, no variables: the frozen kernel resolves entirely in one
walk (explicit signatures, no generics; an unannotated `let` takes its
initializer's type).

Integer literals: a literal adopts the expected integer type when an
expectation exists; the `int32` default applies only when none does
(`let x = 5` stays `int32`). The M1 range sweep moves into the pass and runs
against the adopted type.

### Tier 2 - provenance-carrying expectations (built day one)

+ `Cause` and `Expectation` enums defined in `crates/typeck/src/lib.rs`
- not yet threaded through `infer_expr` — the walker does not receive or
  propagate expectations (S3 work)

An expectation carries *why* it exists:

```rust
pub enum Expectation { None, HasType(TypeRef, Cause) }

pub enum Cause {
    LetDecl(SyntaxNodePtr),
    Param { callee: Text, idx: u32, decl: SyntaxNodePtr },
    ReturnDecl(SyntaxNodePtr),
    FieldDecl { strukt: Text, field: Text, decl: SyntaxNodePtr },
    ArmConsistency { first_arm: SyntaxNodePtr },
    ElemType { decl: SyntaxNodePtr },
    // Horizon 2 extension point, not built now:
    // Expansion { origin: OriginId, inner: Box<Cause> }
}
```

Near payoff: every type mismatch is natively a two-span diagnostic -
"mismatch here, expected `int32` because of the return type declared there."
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

+ built in `crates/typeck/src/lib.rs`:
  + `expr_types: ArenaMap<Idx<Expr>, TypeRef>` — populated during walk
  + `diagnostics: Sink<HirError>` — judgment errors collected
  + `visited: FxHashSet<ExprId>` — reachable expressions tracked
- not yet built (S2 step C):
  - `adjustments: ArenaMap<Idx<Expr>, Adjustment>` — defined but never stored
  - `local_types: ArenaMap<Idx<Local>, TypeRef>` — not even defined in code

```rust
pub struct TypeckResults {
    /// COMPLETE (post-S2 invariant): every expression has a real type or
    /// `Error` (with a diagnostic already emitted).
    pub expr_types: ArenaMap<Idx<Expr>, TypeRef>,
    /// Context-directed adjustments MIR applies when reading an expression.
    /// Replaces coerce's HIR mutation; initially one kind (decay).
    /// NOT YET POPULATED (S2 step C).
    pub adjustments: ArenaMap<Idx<Expr>, Adjustment>,
    /// Every local's resolved type. NOT YET DEFINED (S2 step C).
    pub local_types: ArenaMap<Idx<Local>, TypeRef>,
    pub diagnostics: Sink<HirError>,
}
```

+ `Adjustment` enum defined (single variant `Decay(TypeRef)`) but unpopulated

Precedent: rustc `TypeckResults`, rust-analyzer `InferenceResult` (which
stores `expr_adjustments` the same way). The pass never mutates the HIR; the
adjustment table is what lets decay stay a typeck concern while typeck stays
pure. Side table over typed-HIR because one HIR serves many result sets and
`Body` stays shareable.

Entry point:

```rust
// S1-S5 (current single-threaded interner model):
pub fn check_body(scope: &HIR, body: &Body, types: &mut TypeInterner) -> TypeckResults
// S6 (lock-free interner): `types: &TypeInterner`
```

+ current reality (S1-S5): single-threaded `TypeInterner`, `check_body` takes
  `&mut TypeInterner`. `Database` owns one interner; `check_file` take-restore
  dance (`mem::take(&mut hir.types)`, restore after) keeps handles comparable.
  No `boxcar`, `papaya`, or `dashmap` in dependencies.
- design (S6): one global `TypeInterner` in the `Database`, lock-free
  internals — `boxcar::Vec` arena (lock-free append, stable addresses, atomic
  indexed reads) plus `papaya::HashMap` for dedup (lock-free reads). `intern`
  takes `&self`; a lost insert race leaves a dead arena slot nothing references
  (tolerated, no reservation protocol). This kills the per-fn interner clone,
  the whole-file take-and-restore dance, and the incomparable-handles split
  between the two query paths. Fallback if `papaya` disappoints: `dashmap`
  (sharded locks, drop-in, no longer lock-free).

## Judgments

+ migrated to typeck (S2 step B, in `crates/typeck/src/infer.rs`):
  + `check_int_literal_ranges` (M1)
  + `binary_judgments` — array ops, ptr arithmetic, enum arithmetic, float modulo
  + `index_judgments` — ptr index, OOB, negative index
  + `site_coerce` — coercion mirror for array literals and int literal adoption
+ still in lowering (not yet migrated):
  + return/tail checks
  + match-arm consistency
  + let-init type checks
  + enum opacity (T035)
  + ref-of-non-place (T036)
  + struct-literal field exhaustiveness
  + assign-in-condition (F2)

Everything lowering checks today moves over (explicit-init type, array-init
length, value-position match-arm consistency, return/tail checks, enum
opacity T035, ref-of-non-place T036, literal ranges), plus the deferred
judgments (S3). The deferred list, with the rulings that close their design
questions (ratified 2026-06-12):

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

- MIR's `mir_type_of` `int32` fallback (ledger A3, the silent amplifier)
  becomes an ICE: after typeck, a missing type is a compiler bug.
+ NOT YET live: A3 fallback still `int32` in `mir_type_of` (S2 step C).
  Shadow harness prevents regressions until cutover.
+ Codegen keeps its existing contract (never sees a diagnosed program).

## Boundary rulings (design-space audit, 2026-06-12)

- **Top-level initializers.** Global and const initializer expressions
  belong to no fn body, so `check_body` never sees them. A
  `check_item_initializers` entry point in the same crate runs once per
  file; the const declared-type judgment lives there. Global initializers
  are const-expr today, so they have no effect surface (assumption
  recorded; revisit if initializers ever call fns).
+ **The void rule.** A call to a fn with no return type produces no value;
  using it where a value is expected is a T-class error carrying the call's
  cause. `VoidValueInValuePosition` (T024) exists and fires.
~ **Pattern judgments split.** Type-dependent pattern checks (literal
  pattern domain vs scrutinee type, variant-belongs-to-enum) move to
  typeck; purely structural ones (duplicate binding, exhaustive destructure
  shape) stay in lowering. Not yet migrated — judgments still in lowering.
+ **Mutability checking stays in lowering.** Immutable-by-default
  enforcement is a place/storage judgment over resolution, not a type
  judgment; it works pre-typeck and gains nothing from moving. Already
  in lowering, not duplicated in typeck.
- **LSP consumers.** `typeck_fn` results feed type-on-hover and inlay
  hints. NOT YET built: no per-fn query exists (S2 step D), LSP hover
  still reads from lowering's `expr_types`.

## Salsa wiring and the signature firewall

+ `database::lowered_file` runs `typeck::check_file(&mut hir)` and stores
  `CheckedFile { hir, typeck: FxHashMap<FnId, TypeckResults> }`
+ `mir_map` reads `typeck` from `CheckedFile` for every `lower_function` call
+ `hir_diagnostics` merges typeck diagnostics into the HIR sink
+ `c_code` gates on typeck diagnostics (same as lowering diagnostics)
- per-fn `typeck_fn(StableFnId) -> Memo<TypeckResults>` query NOT BUILT
  (S2 step D). Currently runs whole-file path for all bodies.

```text
SourceFileInput
  └─ lex ─ parse ─┬─ item_scope ─ lower_fn ─ typeck_fn   (per StableFnId; NOT BUILT)
                  └─ lowered_file ─ [typeck::check_file] ─ mir_map ─ c_code
                                  └─ effect_map (whole-file fixpoint, NOT BUILT)
```

`typeck_fn(StableFnId) -> Memo<TypeckResults>` depends on `lower_fn`; editing
one body re-runs one body's check. The whole-file path runs `check_body` per
body with the shared interner (same reasoning as `lowered_file`). `c_code`
and the driver gate on typeck diagnostics exactly as they gate on lowering
diagnostics today. `hir_diagnostics` grows the typeck sink.

**The firewall** (ratified in-build, 2026-06-12): today `Memo`'s
`Arc::ptr_eq` means no query ever backdates, so any edit re-runs everything
downstream of `item_scope`. For keystroke-flat latency the signature data
gains structural equality:

1. First step (segment S5): structural `PartialEq` on the signature portion
   of `FileScope`, so a body edit that leaves signatures unchanged backdates
   `item_scope` and every other body's `typeck_fn` is a cache hit. Keystroke
   cost becomes: reparse one file + recheck one body + the effect fixpoint -
   flat in project size.
2. End state (with the multifile milestone): per-item signature queries
   (`fn_signature(StableFnId) -> Signature`, derived `Eq`), so salsa's
   read-tracking gives caller-precise invalidation automatically.

Effect queries compose with this naturally: per-fn atom results and the
whole-file effect map derive `Eq`, so they backdate exactly (EFFECT.md).

## The parallel wave (segment S6)

**STATUS: NOT BUILT.** No `rayon`, `boxcar`, `papaya`, or `dashmap`
dependency exists in the workspace. Type interner is single-threaded
(`&mut self` on `intern`). All query work is single-threaded.

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

Validation spike first: salsa's parallel-snapshot API is version-sensitive,
so S6 opens by proving snapshot + cancellation + rayon against the pinned
version; the fallback is running the pure check fns over pre-collected
inputs inside one query (D3 makes this trivial).

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
  + no-op impl built (seam for S4). `Cause`/`Expectation` enums defined but
  not yet threaded through `infer_expr`.
~ **S2 - cutover (IN PROGRESS).** step A (MIR reads TypeckResults) BUILT:
  `lower_function` takes `&TypeckResults`, `mir_type_of` reads from it,
  `database::lowered_file` runs `typeck::check_file`. step B (diagnostics
  infrastructure) PARTIAL: int-literal ranges, binary/array/enum/ptr
  judgments migrated to `typeck/src/infer.rs` with 286-line test suite in
  `typeck/tests/judgments.rs`; remaining judgments (return/tail/match/let)
  still in lowering. step C (delete coerce + stamping + shadow harness) NOT
  YET: `TypeckResults::adjustments` and `local_types` not populated; A3
  `int32` fallback still live; shadow harness still required. step D (per-fn
  `typeck_fn` query) NOT YET - described in design but not implemented.
- **S3 - new judgments (NOT BUILT).** The deferred ledger list: M2 operand
  unification, assignment non-value, cast lattice, struct-field value types,
  call argument types, const declared-type value check, tail-expression
  enforcement in value-position blocks, L4 cross-element judgment,
  `types_compatible` integer-leniency deletion.
- **S4 - effects (NOT BUILT).** [EFFECT.md](EFFECT.md): no `crates/effect/`
  exists. `InferObserver` seam is ready (S1). `EffectSet`, contextual
  annotation surface, fixpoint (Tarjan on call graph), witness-edge
  diagnostics, E-class - all design only.
- **S5 - the firewall (NOT BUILT).** Structural signature backdating.
  `Memo<T>` still `Arc::ptr_eq`; no structural equality on `FileScope` or
  any query result.
- **S6 - the parallel wave (NOT BUILT).** No `boxcar`, `papaya`, `dashmap`,
  or `rayon` dependencies in the workspace. Type interner is plain
  `Vec<TypeKind>` + `FxHashMap` with `&mut self` for intern. Single-threaded
  only.
- **S7 - row-polymorphic effects (NOT BUILT).** Effect variables on fn types
  for precise higher-order effect tracking. Requires S6 lock-free
  infrastructure. See EFFECT.md "Path forward".

## Open at build time

~ Pointer-operand arithmetic judgment (noted under M2) — design settled,
  implementation pending S3. Currently all ptr arithmetic is rejected
  (`ArithmeticOnPtr`, T029).
- Shard count and shard-key choice for the global interner (S6).
- Whether `TypeckResults` itself gets structural backdating (cheap to add
  once diagnostics derive `Eq`; decide on measurement).
