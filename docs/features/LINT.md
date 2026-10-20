# LINT - warning-severity diagnostics

status: + BUILT 2026-07-12 (P0 T1: the first four lints + the severity gate).

## the model

a lint is an ordinary diagnostic whose severity is `Warning`. there is no
tenth diagnostic class: class stays the *concern* axis (a type-shaped lint is
`T`-class), severity is the *failure* axis. ruled with the T1 build
(severity-only over a `Class::Lint`).

- `diagnostics::Severity { Error, Warning }` - existed since the 8-class
  model, unused until T1.
- `Diagnostic::severity()` defaults `Error`; a warning variant overrides
  (`impl Diagnostic for HirError`, `crates/hir/src/core/errors.rs`).
- **a warning never fails a build.** the driver renders every diagnostic but
  halts before codegen only when an `Error`-severity entry exists
  (`src/main.rs`). `--check` behaves the same: warnings render, exit 0.
- both renderers map severity: CLI ariadne `ReportKind::Warning` (yellow),
  LSP `DiagnosticSeverity::WARNING` (editor squiggle).
- tests: warning-severity entries are excluded from the judgments suite's
  `diags()` (accepted = no *errors*); lint assertions use `warnings()`.

## the lints (P0 T1)

all live in `crates/typeck/src/infer/judgments.rs` (`check_lints`, driven
from `InferCtx::run` after the error judgments, so every sweep sees the
fully-stamped `expr_types`).

| code | name | fires on | stays silent on |
| ---- | ---- | -------- | --------------- |
| T48 | dead expression statement | a statement whose expression is pure (literal, path, field, index, binary, unary, ref, deref, cast, sizeof, len, array/struct literal over pure parts) | calls, assignments, control flow, poison; block tails (value positions) |
| T49 | unused binding | a local or parameter never referenced by any `Path` expression | `_`-prefixed names; assignment targets count as uses (write-only detection is future work) |
| T50 | unsigned compare tautology | `u < 0` (always false), `u >= 0` (always true), `u <= 0` (only `== 0`; help suggests writing that), mirrored literal-first forms | `==`, `!=`, `u > 0` (meaningful); signed operands |
| T51 | ref-to-local returned | `return &local` / implicit-return tail `&local`, walking field/index projections to a local root (params included - by-value copies) | a `Deref` in the chain (pointee storage); refs that arrive as values (escape-analysis scope, DEFER) |

T47 is reserved for `NonIntegerIndex` (the class-A frontend-owns-clang row).

clang parity: T48 = `-Wunused-value`, T49 = `-Wunused-variable`/
`-Wunused-parameter`, T50 = `-Wtautological-compare`, T51 =
`-Wreturn-stack-address`. eye fires them at the frontend with clean spans,
ahead of the generated C ever reaching clang (silent-safety quadrant,
PHILOSOPHY.md).

## wrapper DCE (same T1 batch, codegen not a lint)

the type-declaration topology over-collects array wrappers: an array type
reachable only through a decayed reference (an inlined-println string
literal) never materializes its wrapper. `gen_all`
(`crates/codegen/src/core/mir_emit/mod.rs`) now captures each type def and
the tail sections into side buffers, then keeps an array wrapper only when
other kept text names it - a fixpoint, since a nested wrapper is named by
its outer wrapper's definition. struct/union/opaque declarations are never
dropped (the user declared them; only wrappers are synthesized).

## future

- write-only binding lint (`-Wunused-but-set-variable` analogue).
- match-arm dead values (a pure arm expression in a discarded match) -
  skipped for noise until block-bodied arms land.
- `-Wall` pass-through on the prod clang invocation is P0 T2 (a separate
  net under this one).
- lint-level control (allow/deny per code) - no surface yet; not designed.
