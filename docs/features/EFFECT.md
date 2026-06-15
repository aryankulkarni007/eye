# EFFECT: kernel machine-effect inference

Status: RATIFIED 2026-06-12. **SEGMENT S4 COMPLETE 2026-06-14** (`crates/effect`):
the `EffectSet` lattice + per-body `EffectJudge` on the `typeck::InferObserver`
seam (atoms `io`/`ffi`/`state`), the whole-program Tarjan-SCC fixpoint
(`infer_effects`/`infer_file` -> `EffectMap`; recursion absorbed by the SCC
union, fn-pointer calls = full live set), the fused database wiring
(`CheckedFile.effects`, one walk for types + effects), contextual effect
**annotations** (`io render(...)`, an `EffectList` CST node, stored on
`Function`), the **`EffectError` (E) diagnostic class** with the exact-match
contract check + **witness trails**, and salsa wiring (the E class gates C
generation). 29 effect/database tests. The walk runs on the shadow-mode walk - sound because the atoms
are resolution-derived and the one type-dependent case (`*p`) reads the
operand type, which `corpus_generates_no_error_type` (S2C C3) proves the
walker always computes. The strategic frame is PRIME.md D6 (row/set-typed
inferred effects with concrete machine atoms; kernel tracks, engine handles);
this document is the engineering design.

## Crate boundary (ratified 2026-06-12)

Effect inference is its own crate, `crates/effect`, fused into the type walk
through an observer seam owned by `crates/typeck`:

```rust
// typeck exposes (as built 2026-06-14):
pub struct ObserverCx<'a> {          // scope + body + interner + types-so-far
    pub scope: &'a HIR, pub body: &'a Body,
    pub types: &'a TypeInterner, pub expr_types: &'a ArenaMap<Idx<Expr>, TypeRef>,
}
pub trait InferObserver {
    // `ty` is Option<TypeRef> under S1's partial contract; it tightens to a
    // plain TypeRef once the S2C cutover gives the completeness contract.
    fn visit(&mut self, id: ExprId, expr: &Expr, ty: Option<TypeRef>, cx: &ObserverCx<'_>);
}
// effect implements it (EffectJudge: atoms + callees; witnesses TODO).
// The driver composes: check_body_with(scope, body, ret, types, &mut judge).
```

Dependency chain `hir <- typeck <- effect <- database`. The walker knows
nothing about effects; generic dispatch monomorphizes the hook (zero cost);
`impl InferObserver for ()` gives type-only checking. The row upgrade - and
even deleting effects outright - never touches `typeck`. The `EffectError`
*data* enum lives in `hir/core/errors.rs` with the other classes (the
`HirError` aggregate is there and `effect` depends on `hir`, so this keeps
the graph acyclic); the effect crate is its only producer.

## The lattice

`EffectSet` is a bitset. `pure` is the empty set (the lattice bottom); union
is the join. Atoms:

| atom | live now | produced by |
|------|----------|-------------|
| `io` | yes | `print` / `println` (the printf seam) |
| `ffi` | yes | calling an `extern` fn; dereferencing a raw pointer (`*T` / `ptr`) |
| `state` | yes | reading or writing a `mut` global |
| `alloc` | reserved | a real heap allocator (today `malloc` is an extern, so `ffi`) |
| `panic` | reserved | bounds traps (DEFER: runtime-safety theme) |
| `diverge` | reserved | non-termination analysis (gates prime totality) |

Reserved atoms have bits assigned now and no producer; they start firing when
their primitive lands. The set is open (`conc` / `nondet` slot in later).

Row-readiness (PRIME D6 upgrade axis 1): the effect type is shaped as
`atoms: u8` plus a dormant tail slot for a future effect variable
(effect-polymorphic `map` over an effectful fn). The kernel ships
monomorphic sets; the tail is the Tier 3 seam of the second lattice and
obeys the sealed-body invariant - effect variables, when they exist, are
body-local and resolved at the seal.

`&T` dereference is not an effect: shared references come from `&place` on
checked Eye values. Raw-pointer dereference is `ffi` because the pointer's
provenance is outside Eye's model. There are no `unsafe` blocks; `ffi` *is*
the unsafe boundary, explicit and propagating (PRIME D6).

## Inference: fused per-body walk + whole-program fixpoint

1. **Atom collection (fused into the type walk, ratified 2026-06-12).**
   Types and effects are inferred *simultaneously*: the bidirectional type
   walk calls the effect judge at each visit, on the same traversal - one
   walk per body, two lattices. This works because the only type-dependent
   atom classification (`*p` is `ffi` only when `p` is a raw pointer, not
   `&T`) needs exactly the operand type the walk just computed on its way
   up; every other atom is resolution-only (`io` from the println
   intrinsic, `ffi`-call from `Resolution::Fn` + `is_extern`, `state` from
   `Resolution::Global` + mutability, call edges from resolution). The two
   lattices stay modular as two judge structs sharing the walker, so the
   row upgrade touches only the effect judge. Output per fn: `(EffectSet,
   Vec<FnId> callees, witnesses)`. Tier 3 caveat, recorded: an unsolved
   body-local type variable at a deref defers that atom to the body seal
   (where every variable is resolved by contract).
2. **Fixpoint (whole program, microscopic).** Build the call graph from the
   per-fn callee lists, Tarjan SCC (the machinery pattern already exists in
   `typegraph.rs`), walk the condensation in topological order, union
   upward. An SCC's effect is the union over its members - recursion needs
   no iteration, the condensation *is* the fixpoint. O(V + E) on byte-sized
   sets. This is the one inherent wait in the system: a whole-program
   verdict cannot exist before every body is walked - but it waits on the
   walks, not on type checking as a phase.

Calls through fn-pointer values conservatively assume the full live set
(`io | ffi | state`) until effect rows land on fn types - sound, honest, and
tightening later is the relaxing direction.

## Witness edges (provenance, second lattice)

Same philosophy as Tier 2 type causes: an effect verdict carries *why*. The
fixpoint records, per fn and per atom, one witness - either the body
expression that produced the atom directly, or the callee that introduced
it. A violation diagnostic then walks witnesses to a concrete primitive:

```
error[E001]: `report` declares `pure` but has effect `io`
  -> report calls `render`, which calls `println` here
```

Cost: one `FnId`-or-`ExprId` per atom per fn. It is the difference between
"has effect io" and an explanation, and it extends through macro expansion
frames exactly as type causes do.

## Inference is total; annotations are never required

There is no program effect inference cannot judge: atoms are
resolution-derivable (plus the one type-assisted deref case, fed by the
fused walk), recursion is absorbed by the SCC condensation, and the one
unknowable case (calls through fn-pointer values) has a defined
conservative answer rather than a failure mode. No annotation is ever
load-bearing for inference. Annotations exist for:

- **Regression pinning** (the near-term value): `pure` on a fn turns "a
  `println` appeared three calls deep" into a compile error.
- **Module boundaries** (later): separate compilation cannot see other
  modules' bodies, so exported fns will need declared effects then. The
  surface is designed now; requiring it is deferred to modules.
- Not the prime gate - that checks inferred sets directly.

Unannotated code stays transparent: the LSP surfaces inferred effects via
hover/inlay hints.

## Annotations and the checking contract

Surface (PRIME D6, decided): bare space-prefixed effect keywords before the
fn name. `pure` asserts the empty set.

```
pure square(int32 n) -> int32 { n * n }
io   report(int32 n) { ... }
io alloc render(Scene s) -> Image { ... }
```

Refinement (ratified 2026-06-12): effect names are **contextual keywords**,
not reserved words - recognized only in effect position (identifiers
preceding the fn name in an item definition; the keyword-less fn grammar
makes `IDENT+ IDENT (` unambiguous at item level). `let state = ...` and
`mut io = ...` stay legal: globally reserving common identifiers like
`state` is exactly the footgun class the no-footguns rule exists to kill.
The parser accepts any identifier sequence in effect position; collection
validates against the atom set, and an unknown name is an E-class "unknown
effect" diagnostic listing the valid set (better recovery than a parse
error).

Effects are inferred by default; an annotation is an optional published
contract. The contract is **exact match** (ratified 2026-06-12): the declared
set must equal the inferred set - declaring `io` on a fn inference finds
pure is an error, as is the reverse. Truthful annotations, checkable today
because inference is whole-program; relaxing to an upper-bound contract
later (when modules need stable API headroom) is the permitted direction
under freeze asymmetry. Unannotated fns get the inferred set with no
ceremony (dodging the Java checked-exceptions failure mode).

## Diagnostics: the E class

Effect errors are a new diagnostic class, `EffectError`, code prefix `E`
(`E001`...), under the same never-renumber policy as the other classes. The
taxonomy grows from eight classes to nine; classes partition by concern and
effects are a new concern with its own pass phase. Payload stays structured
(declared set, inferred set, witness chain) until render time, per the
typeck law.

Anticipated members: declared/inferred mismatch; prime-gate violation
(below, dormant until the VM).

## The prime gate (dormant)

PRIME D6, restated against this design: a `prime fn` must have an effect set
excluding `io` / `ffi` / `state` and must be `not diverge`; it may `alloc`
and may `panic` (a comptime panic is a compile error). The gate is one set
check on this machinery; it activates with the prime VM (PRIME D5), nothing
to build now beyond keeping the atoms it needs distinct.

## Salsa wiring

WIRED 2026-06-14: the database's `lowered_file` query calls
`effect::infer_file(&mut hir) -> (typeck_map, EffectMap)` - the fused
dual-inference driver: one walk per body produces both the type side tables and
the per-body effect results, and the fixpoint runs over them. `CheckedFile`
grew an `effects: EffectMap` field beside `typeck`, so types and effects are
computed in one traversal and memoized together (no second walk). The standalone
`typeck::check_file` stays for the non-salsa, type-only paths (the `src/lib.rs`
compile helper, benches, judgment tests); `effect::infer_effects` is the
effect-only standalone driver. No backend consumer reads `effects` yet - it
feeds the annotation contract check and the prime gate.

- Per-fn atom results ride inside the per-body walk; once the per-fn
  `typeck_fn` salsa query lands (S2C step D), the per-fn `EffectResult` derives
  `Eq` and backdates exactly (a body edit that does not change a fn's atoms or
  callees stops the ripple). The current whole-file `lowered_file` recomputes on
  any edit - the same conservative granularity the type side uses pre-D.
- `EffectMap` derives `Eq`, so unchanged verdicts do not invalidate downstream
  consumers (future: backend optimizations driven by effect proofs - DCE, CSE,
  auto-parallelization per PRIME D6).
- In the LSP, the whole-program fixpoint runs off the latency path
  (background wave); per-body type diagnostics never wait on it.

## Build pieces (segment S4)

Status 2026-06-14: segment S4 is COMPLETE. All six pieces are BUILT in
`crates/effect` (29 tests across the crate + database). S7 (row-polymorphic
effects, see "Path forward" below) is the designed upgrade.

1. [x] **Annotations.** The parser nests contextual effect identifiers before
   the fn name in an `EffectList` CST node (`io render(...)`; the name is the
   ident immediately before `(`, so `FnDef::name()` is unchanged). Hand-written
   `SyntaxKind` only - no ungram/xtask churn, and corpus files stay
   annotation-free so the tree-sitter parity gate is untouched. Collection
   stores `Function.declared_effects: Vec<(Text, Span)>` (raw names + spans);
   the effect crate validates them (unknown name = E001).
2. [x] `EffectSet` bitset + the row-ready shape.
3. [x] Atom collection fold in `crates/effect` (fused via the observer seam;
   reads resolution + the operand type for `*p`).
4. [x] **Call graph + condensation fixpoint + witnesses.** `infer_effects` /
   `infer_file` -> `EffectMap`: Tarjan-SCC condensation over the
   `EffectResult.callees` edges, union upward, fn-pointer calls = full live set
   via `EffectResult.indirect`. Witnesses: `EffectResult.local_witness` records,
   per live atom, the primitive that produced it in this body
   (`Println`/`Extern`/`RawDeref`/`MutGlobal`/`Indirect`); a mismatch
   diagnostic DFS-walks the call graph (`witness_trail`, only on the error
   path) to the leaf primitive, naming the via-chain.
5. [x] **`EffectError` class (E codes), exact-match check, wiring.** `Class::Effect`
   (`E`) added to the taxonomy; `EffectError { UnknownEffect, EffectMismatch }`
   in `hir/errors.rs` (data only; effect crate is sole producer). `check_contracts`
   compares each annotated fn's declared set to its inferred set (exact match);
   `infer_file` returns the `Sink<HirError>`, stored in `CheckedFile.effect_diagnostics`,
   merged into `hir_diagnostics` and gating `c_code`.
6. [x] **Tests.** Per-atom units; recursion (SCC union), transitive propagation,
   fn-pointer conservatism; exact-match rejection both directions; unknown
   effect; unannotated = no contract; witness trail; the `CheckedFile.effects`
   wiring + the E-class gating C generation (database tests).

## Path forward (next build order)

S4 effects is complete. The remaining inference work is on the type side:
the S2C cutover (C2/C4/C5 + D) and S6 parallelism (ledger).

### S7 - Row-polymorphic effects (DESIGNED, NOT BUILT)

The kernel ships monomorphic `EffectSet` (a `u8` bitset), which means every
call through a fn-pointer value conservatively assumes the full live set
(`io | ffi | state`). This is sound but imprecise: a higher-order `map` that
happens to receive an `io` lambda is itself classified as `io | ffi | state`
rather than just `io`.

S7 upgrades the effect representation to row-polymorphic form:

```rust
pub struct EffectSet {
    atoms: u8,            // live atoms, same as today
    tail: Option<EffectVar>, // dormant in S4, active in S7
}
```

An `EffectVar` is a body-local unification variable, born and solved inside
one body's inference task (obeying the sealed-body invariant - TYPECK.md
Tier 3). Effect rows attach to `TypeKind::Fn` as part of the fn-pointer's
signature, so `map : ([T] -> [T] with e) -> [T] -> [T] with e` tracks
precisely.

The fixpoint remains Tarjan SCC condensation over the call graph, but the
join widens from bitset-union to row unification (with the same SCC-as-
fixpoint property for monomorphic effect atoms). Fn-pointer calls no longer
force the conservative live set - they instantiate the effect variable from
the callee's row.

**Precondition:** S6 lock-free interner, because effect variables introduce
a new kind of interned handle on `EffectVar` that must be `Send + Sync`.

**Dependency:** S7 is independent of S5 and the S2 cutover but requires S6's
lock-free infrastructure. It can land any time after S6.

Relaxations deferred under freeze asymmetry (EFFECT.md decisions): the
exact-match contract -> upper-bound (when modules need API headroom); reserved
atoms (`alloc`/`panic`/`diverge`) become valid annotation names when their
producers land; the prime gate activates with the prime VM.

Note: this slice runs on the shadow-mode walk; once the S2C cutover lands
(ledger), the seam's `ty` tightens to non-`Option` and the deref atom needs no
restatement (the operand type is already the source of truth).
