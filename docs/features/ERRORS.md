# Error handling: errors as values, fallibility as an effect

Status: **design agreed 2026-06-19 (pair session), not built.** A first-class
error story is a modern-language baseline; Eye has none today (return codes +
manual `if`). The thesis: **errors are first-class values, and fallibility is a
tracked effect** - and Eye already having a whole-program effect system
([EFFECT.md](EFFECT.md)) is what lets it reach a design corner the rest of the
field has not: inferred + typed + payload-carrying + auto-unioned, with no
signature ceremony. This is the effect system earning its place in a systems
language.

## The decided model

### D1 - two mechanisms, split by kind (Midori, now industry consensus)

- **bugs** (out-of-bounds, div-by-zero, a contract violation) -> **panic / abort**.
  not recoverable, not values; the deferred runtime-safety / abort theme
  ([DEFER.md](../planning/DEFER.md)).
- **recoverable errors** (file missing, parse failed) -> **first-class values**,
  carried + tracked. this document.

conflating the two is the original sin (C, Java, Python); keeping them separate
is what every modern design converged on.

### D2 - inferred, typed, payload-carrying error union (the effect join = the union)

the `fail` effect carries the error value's **type**, not just a tag. a function
that can fail has `fail` in its inferred effect set; the **effect-lattice join is
the error-set union**, so a body calling things that fail with `E1` and `E2`
infers `fail<{E1, E2}>` - computed by the SCC fixpoint already run, with no manual
declaration and no `From`-style conversion plumbing.

this lands the corner the field left open (the "error typing" tension):

- **inferred**, not declared -> no `throws` clauses (dodges Java's checked-exception
  verbosity).
- signatures stay `-> T` -> no `Result<T, E>` noise threaded everywhere (dodges
  Rust's wrapping; dodges the "sum types infect the codebase" problem - you never
  hand-thread the type).
- the error is a **typed value with a payload** (beats Zig, whose errors are
  tag-only and need the out-param "diagnostic" workaround).
- the join **auto-unions** error types (beats Rust's `From`/`anyhow` conversion
  plumbing).
- only `catch` sites that *choose* to match specific variants are
  exhaustiveness-checked; breaking one on a contract change is the signal you want.

### D3 - implicit propagation by default, explicit optional (freedom both ways)

- **default: implicit.** inside a function that has (or infers) the `fail`
  effect, calling a fallible function auto-propagates its error on failure
  (early-return up the chain). no per-call marker. zero boilerplate. this is
  *freedom from* ceremony.
- **visibility is not lost** - it moves from syntax to tooling: the `fail` effect
  on the signature says "this can fail," and the LSP paints every potential-failure
  call site inline (witness-driven inlay hints / a distinct highlight). this
  answers the "hidden control flow" / no-magic concern (MEM.md) with the LSP,
  rather than ignoring it - only possible because Eye owns the pipeline + the LSP +
  has effect witnesses, the exact assets the converged languages lacked.
- **explicit is always available** - the user may mark a call to inspect/handle
  the error *there* instead of propagating (the `catch`/`else` boundary below).
  this is *freedom to* check. lean into the implicit default; build the explicit
  form as the ergonomic affordance, not the requirement.

### handling: a catch boundary, no algebraic resume

Eye has no algebraic effect handlers (no resumable continuations), so handling is
a **catch**, not a handler: a `catch` / `else` boundary runs the fallible code
and, on failure, binds the error value and runs the alternative - the failed
computation is abandoned (not resumed). this **discharges** the `fail` effect at
the boundary, turning "can fail" into a handled value. exhaustive match over the
inferred error union where the user chooses to match.

## Why the effect system makes this work (the asset map)

| Eye asset (built) | what it gives error handling |
|---|---|
| whole-program SCC fixpoint | closed-world cross-function inference of the error set (Zig's trick, for free) |
| the lattice **join** | automatic error-type **union** - no `From`/conversion plumbing |
| effect carries the **type** | error **payloads** work (Zig's wart avoided) |
| exact-match contracts | `pure` provably **cannot fail**; the prime gate excludes `fail` |
| witness trails | **why/where** a function can fail - drives the LSP visibility for the implicit default |

## Lowering (sketch)

`fail`-effecting code lowers to a hidden result-channel + **early-return-on-error**
- the shape Rust's `?` desugars to, not `setjmp`/unwinding, no magic. the effect
analysis tells codegen exactly which calls need a check + propagating return.
drops (MEM.md destructors/`defer`) **must run on the error path** as it propagates
out, same as a normal early return (Zig's `errdefer` territory).

## Prior art (the research this is built on)

| language | what it got right / wrong | lesson taken |
|---|---|---|
| C / errno | ignorable, out-of-band, conflated with valid return | errors must be values, un-ignorable |
| Java checked exceptions | checked but **declared** -> verbose, `throws Exception` escape hatch | check, but **infer**, never declare |
| C++/Python exceptions | invisible control flow, uncheckable | explicit, value-based |
| Go `if err != nil` | explicit + simple, but boilerplate + stringly-typed + ignorable | keep explicit-ness, kill the boilerplate (implicit default) |
| Rust `Result` + `?` | typed, composable; but `Result` signature noise + `From`/`anyhow` conversion | inferred union dissolves both |
| Zig error sets | closed-world **inferred** sets - the key precedent; but **tag-only, no payload** | inherit the inference, add payloads |
| Swift typed throws (SE-0413) | typed added late; "errors rarely handled exhaustively, change over time" | inferred typing > declared typing |
| Roc | open tag unions, fully inferred, compose without conversion | row-style inferred union is the ergonomic target |
| Midori (Joe Duffy) | the bugs-vs-recoverable split | D1 |

convergence reference: errors-as-values + explicit-or-tracked propagation +
bugs/recoverable split is now the cross-language consensus (matklad, "the second
great error model convergence," 2025).

## Open (not yet decided - pair these next)

- `=` **error value representation - resolved 2026-06-19.** rich payload errors
  are sum types / ADTs, now a ratified feature (the first compiler-blessed
  desugaring, [KERNEL.md](../design/KERNEL.md) "The freeze, precisely") - no longer
  gated on the far-future engine. so the error *value* side rides on the ADT
  desugaring (Phase 2), which comes after the Phase 1 hardening. an int-code-only
  interim is unnecessary - ADTs are close enough to wait for.
- `=` **error-set representation - resolved 2026-06-21 (pair session): nominal,
  closed *set* (C3, Zig-shaped), not an open structural row (Roc / C2).** each
  error is a declared nominal ADT; the `fail` effect carries a **set of those
  nominal type identities** (`set<TypeRef>`), the lattice join is **set-union**,
  computed by the existing whole-program SCC fixpoint. `catch` matches by name.
  this avoids structural / row inference entirely and stays sealed-body-clean. the
  axis: nominal (named) vs structural (shape), closed (fixed per fn) vs open
  (growable row) - Eye picks nominal + closed-per-fn. [EFFECT.md](EFFECT.md)
  S7-payload.
- `=` **keywords - decided 2026-06-21 (pair session).** the effect stays named
  `fail` (an annotation, rarely written). the **handle construct is `try`/`catch`**:
  a `try { ... } catch e { ... }` block, with the postfix `expr catch e { ... }` /
  `expr catch { NotFound => .., ParseErr p => .. }` also available (catch over a
  block expression, reusing the match machinery - bare-ident, no pipes). `try` is
  the familiar attempted-block opener; under implicit propagation it is convenience
  over necessity, chosen for familiarity. the **raise word is `raise`** (`raise e`
  produces an error and bails the path). `throw` rejected (imports the unwinding
  mental model Eye does not use - lowering is early-return); `fail e` rejected
  (ugly); `else` / `handle` / `save` rejected in favor of `try` / `catch`.
- `=` **catch exhaustiveness - resolved 2026-06-21.** `catch e { }` is a catch-all
  that discharges the `fail` effect fully. `catch { variants }` must be exhaustive
  over the nominal closed error set (C3) or carry a catch-all arm - the same rule as
  `match`; breaking on a newly-added variant is the intended signal.
- `=` **drops on the error path / errdefer - resolved 2026-06-21.** the error
  early-return runs the same scope drops as a normal return, so **auto-drop + move
  subsumes errdefer's main case**: an owned value being built is freed on the error
  path (still owned) and kept on success (it moves out, drop suppressed) - the C
  goto-cleanup / reverse-mental-map pain auto-drop exists to kill. errdefer is
  therefore mostly unnecessary in Eye (Zig needs it only for lacking destructors);
  its residual case is rolling back **non-value side effects** on the error path
  only, which is rare and **deferred (YAGNI)** until a concrete need.
- `=` **dependency on EFFECT.md S7 - resolved 2026-06-21: depends only on the
  cheap S7-*payload* upgrade** (the `fail` set carrying `set<TypeRef>`), **not** on
  row-polymorphic effects. the payload is closed-world + sealed-body-clean;
  row-poly is demoted to a far-future escape hatch (monomorphization subsumes it,
  [EFFECT.md](EFFECT.md) S7). so error handling is *not* gated on the comptime /
  row-poly class - only on ADTs (Phase 2) + the payload upgrade.
- `=` **implicit propagation - kept (D3), reaffirmed 2026-06-21.** the default is
  robust auto-propagation that handles every fallible call by one uniform rule;
  `catch` is the opt-in for when you want to *modify* that default. it is invisible
  in plain text but **predictable** (one uniform rule, the `fail` effect on the
  signature, LSP-painted call sites), which is the accepted bar - magic is fine if
  predictable ([MEM.md](../design/MEM.md) commitment #3, 2026-06-21 reframe). there
  is no redundant explicit-propagate marker (`?` / `try`); `catch` is the explicit
  form.

tracked in [ledger.md](../planning/ledger.md) "error handling".
