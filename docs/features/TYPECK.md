# TYPECK: Type checking as a separate pass

Status: DESIGN, NOT BUILT. This is Track 3 of `REDESIGN.md`. It is the
precondition for the strong reading of "pure HIR." This document is the handoff
for a future session.

## Goal

Today there is no separate type-checking pass. Lowering (`lower_expr`,
`crates/hir/src/core/lower/expr.rs:48`) builds the HIR, resolves names, stamps
types into `expr_types`, and runs semantic checks, all in one walk. Track 3
lifts the type stamping and the `check_*` functions out of lowering into a
dedicated pass over the finished HIR. Lowering becomes a pure builder.

This is not full type inference. T1 (full inference) remains on hiatus. The
existing best-effort bidirectional checks stay; they only move into their own
pass and stop being interleaved with construction.

## Shape

- Lowering produces a frozen HIR (REDESIGN I1). It resolves names and
  desugars, and does no type work.
- A type-check pass reads the frozen HIR and writes a side table keyed by
  `ExprId` (the current `expr_types: ArenaMap<ExprId, TypeRef>`,
  `crates/hir/src/core/body.rs:26`, relocated out of lowering). This mirrors
  rustc `TypeckResults` and rust-analyzer `InferenceResult`.
- The pass never mutates the HIR tree. Types live only in the side table.
- Diagnostics from this pass use the typed Type / Resolve / Pattern / Const
  classes from `DIAGNOSTICS.md`.

## What moves out of lowering

The inline checks, currently fused into the lowering walk:

- `check_array_init_len` (`stmt.rs:81`)
- `check_explicit_let_init_type` (`stmt.rs:109`)
- `check_value_position_match_arms` (`stmt.rs:204`)
- return / tail checks (`stmt.rs` around 345-369)
- the inline type stamping in `lower_expr` (`expr.rs`) and `ctx.rs:52`

`check_unhoisted_matches` (`stmt.rs:275`) is not migrated: it is deleted when
MIR lands (REDESIGN I3).

## The undeterminable-type decision

Recorded in `MIR.md` and `DIAGNOSTICS.md`, owned here. When a type cannot be
determined, the type-check pass raises a Type-class diagnostic rather than
leaving a hole for a downstream guess. This lets MIR assume complete types and
deletes codegen's `int32_t` fallback (`matches.rs:104`). Confirm this contract:
it makes the type checker responsible for completeness, which suits the
project's pessimism principle but is a real scope commitment.

## Sequencing

- Independent of MIR in machinery, but coupled at one boundary: if MIR assumes
  complete types, the undeterminable-type diagnostic must exist in this pass
  before MIR can drop its fallback. Agree the boundary contract before either
  lands.
- Runs after Track 1 (diagnostics). Track 1 touches every emit site and maps the
  full check surface, which informs this split.
- Resolution stays in lowering; only type work moves. The type-check pass
  consumes resolved `Resolution` data already on the HIR.

## Open decisions for next session

- Whether Track 3 is in scope at all, or deferred. REDESIGN records this as the
  2-track vs 3-track decision. Designing it here does not commit to building it.
- Pass granularity: one type-check pass, or split into resolution-dependent
  checks vs type checks. Likely one pass to start.
- Exact name and home of the side-table result type (rename `expr_types`, or
  wrap it in a `TypeckResults` struct alongside future per-body results).
