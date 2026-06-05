# REDESIGN: Compiler Pipeline

Status: PARTLY BUILT. Track 1 (diagnostics) and Track 2 (MIR) are shipped; Track
3 (typeck/lowering split) is designed but not built. This file records the target
architecture and the invariants the refactor holds itself to; the per-track
status is below. `DESIGN.md` holds the long-term language vision; this file is the
concrete pipeline plan grounded in the current code.

- **Track 1 - Diagnostics: BUILT** (2026-05-31). Typed per-crate error enums + a
  shared `diagnostics` crate. See [`DIAGNOSTICS.md`](features/DIAGNOSTICS.md).
- **Track 2 - MIR: BUILT** (cutover complete). HIR -> MIR lowering moved
  control-flow flattening and temp/hoist generation out of codegen; codegen is a
  dumb MIR -> C printer. The `check_unhoisted_matches` ban (I3) is deleted, so
  nested value-position match compiles. See [`MIR.md`](features/MIR.md).
- **Track 3 - Typeck split: DESIGNED, NOT BUILT.** Lifting the inline `check_*`
  and type stamping out of lowering into a pass over frozen HIR. See
  [`TYPECK.md`](features/TYPECK.md).

The verified-current-state and target-pipeline sections below were written
pre-build; read them as the plan of record, with the per-track status above as
the source of truth for what exists.

## Why now

At roughly 10K lines the layer boundaries are still cheap to move. Adding more
features (payload enums, nested structs, type inference) before fixing the
boundaries makes the same refactor progressively harder, because each feature
must then be migrated out of two layers instead of being born in the right one.

## Verified current state

Checked against source, not inferred:

- Type information lives in a side table, `expr_types: ArenaMap<ExprId, TypeRef>`
  (`crates/hir/src/core/body.rs:26`). The HIR tree is not mutated to carry
  types. This already matches the "pure HIR + side-table types" model.
- There is no separate type-checking pass. Lowering (`lower_expr`,
  `crates/hir/src/core/lower/expr.rs:48`) performs name resolution, type
  stamping, and semantic checks in one walk. The `check_*` functions in
  `crates/hir/src/core/lower/stmt.rs` run inline during that walk.
- `match` hoisting happens in codegen. `hoist_matches`
  (`crates/codegen/src/core/matches.rs:92`) generates `_matchN` temps;
  `collect_match_ids_rec` (`matches.rs:117`) stops at `If`, `Loop`, and `Block`
  boundaries because those positions cannot be hoisted unconditionally.
- The HIR bans what codegen cannot emit. `check_unhoisted_matches`
  (`crates/hir/src/core/lower/stmt.rs:275`) rejects value-position matches in
  the positions `collect_match_ids_rec` skips. The backend's limitation is
  therefore constraining the language surface.
- Codegen does not emit type errors. It guards on empty diagnostics and
  degrades to comments such as `/* INVALID PATTERN */` (`matches.rs:37`) rather
  than rejecting.
- Diagnostics are a single untyped struct, `HirDiagnostic { ptr, msg: String }`
  (`crates/hir/src/core/body.rs` region around the `core.rs:64` definition).
  There is no partition by error class.
- Type information is partial. Many call sites read `expr_types.get(...)` and
  bail when absent (for example `crates/hir/src/core/lower/expr.rs`). The
  `int32_t /* match temp type unknown */` fallback (`matches.rs:104`) is the
  visible symptom of incomplete type coverage reaching codegen.

## Target pipeline

```
[Lexer + AST]  ->  [HIR]  ->  [Inference]  ->  [MIR]  ->  [Codegen]
 black box         pure        side table      flat IR    dumb emitter
 (settled)         resolved,   keyed by        target     MIR -> text
                   desugared   ExprId          neutral
```

## Layer responsibilities

1. Lexer + AST: settled. Out of scope for this refactor.
2. HIR: resolved, lightly desugared semantic tree. Name resolution
   (`Resolution`) and light desugaring (struct-field shorthand,
   `body.rs:193`) stay here. Frozen after lowering. The tree is never mutated
   by later passes. This is not a raw AST mirror.
3. Inference: a pass over the finished HIR that writes a separate result keyed
   by `ExprId`. Does not mutate HIR. Equivalent to rustc `TypeckResults` and
   rust-analyzer `InferenceResult`. The current inline `expr_types` stamping
   moves here.
4. MIR: target-neutral flat IR. Heavy desugaring lands here: control-flow
   flattening and temp generation (the hoisting currently in codegen). This is
   the layer that absorbs the work the HIR and codegen do at the same time
   today.
5. Codegen: dumb emitter. Walks MIR and prints C text. No decisions.

## Invariants

These are the rules that make the layering real rather than cosmetic.

- I1 Frozen HIR. After lowering, no pass mutates HIR nodes. Inference and any
  analysis write side tables keyed by `ExprId`.
- I2 Total back half. MIR-lowering and codegen are total functions. Any
  well-typed HIR lowers to valid MIR, and any MIR emits valid C. Neither
  rejects a program and neither produces diagnostics. All program rejection
  happens at or before inference. This is the test of whether the mess was
  extracted or merely relocated.
- I3 Acid test for MIR. The MIR design must represent the value-position
  matches that `check_unhoisted_matches` rejects today. If the refactor does
  not unban nested value-matches, the MIR layer did not earn its cost.
  Unbanning requires lowering to record the match type for those positions, a
  modest extension of the existing first-arm-type logic, not full inference.
- I4 Target-neutral MIR. MIR is flat three-address / basic-block-shaped IR, not
  C-shaped. A C-shaped MIR forces a future second backend to redo control-flow
  flattening. "C-like" is acceptable as a v1 simplification only if the shape
  stays neutral.

## Design corrections caught during review

- No optimization on HIR. HIR is nested and structured; optimizers want a flat
  IR. Optimization belongs on MIR, or is skipped entirely for now and left to
  the C compiler (`cc -O2`) and a future Cranelift backend. The "optimize HIR"
  step is removed from the plan.
- Inference completeness bounds MIR. MIR-lowering generates typed temps, so its
  output quality is limited by how much inference fills in. MIR does not repair
  type gaps; it inherits them.

## Tracks and sequencing

Three independent tracks. Each can land green on its own.

- Track 1: Diagnostics. Replace the untyped `HirDiagnostic.msg` string with a
  typed error enum and an accumulator. Low risk, independent. Touching every
  emit site also maps the full type-check surface, which informs Track 3.
  Agreed to do this first.
- Track 2: MIR. Introduce HIR -> MIR lowering. Move control-flow flattening and
  temp generation out of codegen. Reduce codegen to a MIR emitter. Subject to
  I2, I3, I4.
- Track 3: Type-check / lowering split. Lift the inline `check_*` functions and
  type stamping out of lowering into a dedicated pass over the finished HIR.
  Lowering becomes a pure builder.

Per-track design docs (full handoff detail): Track 1 `docs/DIAGNOSTICS.md`,
Track 2 `docs/MIR.md`, Track 3 `docs/TYPECK.md`. All three are designed and
pinned. None is built yet. Build order: Track 1 first (errors, no MIR
prerequisite), then Track 2, then Track 3, with the undeterminable-type boundary
contract (MIR.md / TYPECK.md) agreed before MIR drops its type fallback.

Note on Track 3 scope: the candidate of running the type checker on the AST was
considered and set aside. The AST has no name resolution and no desugaring, so
an AST-based checker would duplicate the resolution and desugaring that lowering
already produces, and would fight the rust-analyzer-style layout the codebase is
built on. The goal (separating type checking from lowering) is met by checking
the finished, pure HIR, not by moving the checker up to the AST.

## Scope decision: 3-track (LOCKED)

Scope of "pure HIR" is locked to **3-track** as of 2026-05-31. Pure means both
backend-agnostic and type-check separated from lowering. Track 1 + Track 2 +
Track 3. The full north star: type checking runs as a dedicated pass over the
finished HIR, and lowering becomes a pure builder.

Considered and rejected: 2-track (backend-agnostic only, lowering stays fused
with type checking). Smaller, but leaves the resolution+stamping+checks fused in
one walk that this refactor exists to separate.

Track 1 is zero-regret and required by both paths; it proceeds first regardless.
Build order is unchanged: Track 1 (errors), then Track 2 (MIR), then Track 3
(typeck/lowering split).
