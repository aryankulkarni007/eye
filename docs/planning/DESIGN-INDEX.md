# Design index

A topic-level map of the active design threads and where each canonically lives.
The [ledger](ledger.md) tracks granular open items; this is the navigation layer.
Status legend ([STYLE.md](../STYLE.md)): `+` built, `~` partial, `-` designed/not
built, `?` open question, `=` definition/principle.

Most of the design below was worked out in the 2026-06-18/19 pair sessions.

## Principles (the lens every decision passes through)

- `=` **silent safety** - catch everything, stay silent/low-friction; pay for
  safety with inference + defaults, not annotations; ceremony only for the
  dangerous/significant direction. the Batman frame. **the nudge** (2026-06-19):
  safe path easy, unsafe path uphill-but-ergonomic; Eye = safe-by-default-with-
  escape-hatches, not safe-by-proof. [PHILOSOPHY.md](../design/PHILOSOPHY.md).
- `=` **the safety quadrant** - priority function: silent-unsafe (fix first) >
  loud-unsafe / loud-safe (converge to silent-safe) > missing-capability.
  [PHILOSOPHY.md](../design/PHILOSOPHY.md).
- `=` **no footguns / kernel minimalism** - least-surprising rule; everything that
  can be stdlib is; the kernel is the frozen primitive floor. [KERNEL.md](../design/KERNEL.md).
- `=` **design as a pair** - propose + recommend, I decide, then capture.
  (working agreement, not a doc.)

## Built this session

- `+` **LSP hover** - function name -> signature + inferred effect; `let` binding
  -> type; expr -> type. `crates/lsp/src/server/requests.rs`, [LSP.md](../features/LSP.md).

## Agreed design, not built

- `~` **value / resource model** - **ratified 2026-06-19: auto-drop is the
  default, manual dealloc the opt-in** (the nudge). ownership is a **type property**
  (owning type auto-drops, raw `*T` never does), **not** inferred tag-inheritance
  (set aside as confusing). copy vs move (owning = move-only + use-after-move
  reject); **borrow model** = `&T` shared (write-through rejected) / `&mut T`
  mutable; the **mutability axis is enforced now**, but **aliasing-xor-mutation is
  deferred** (revised 2026-06-21: a holey intraprocedural guardrail is
  unpredictable; the shared-XOR-mutable uniqueness rule lands whole-program with
  escape analysis, not before). not a borrow checker; aliasing holes accepted until
  then. kernel-robust,
  stdlib optional. open: opt-out granularity/spelling (arena vs `disown`), what
  seeds `drop`, leak = warn or silent. [MEM.md](../design/MEM.md), [MUT.md](../features/MUT.md).
- `-` **mutability completion** - const-by-default parameters + `mut` marker;
  `&mut T` safe mutable borrow; FFI const-correctness (`nonnull` on refs/strings,
  fixes the `memcpy` builtin-redecl warning). designed 2026-06-21 (`&mut` = option
  B): const-params + FFI-const are build-only; `&mut` ships as an honest typed,
  non-`ffi`, non-null mutable borrow enforcing only the **mutability axis** -
  aliasing-xor-mutation deferred to escape analysis (predictable-magic: no holey
  partial check). sequencing: const-params before `&mut`. [MUT.md](../features/MUT.md).
- `-` **error handling** - errors = first-class values; fallibility = a `fail`
  effect. D1 bugs/recoverable split, D2 inferred typed payload-carrying error
  union (effect join = error-set union), D3 implicit propagation + explicit
  optional + LSP visibility; `catch` boundary (no algebraic resume).
  **error-set repr ratified 2026-06-21: nominal closed *set* (C3, Zig-shaped),
  not an open structural row; depends only on the cheap S7-*payload* (`set<TypeRef>`),
  not row-poly.** surface 2026-06-21: raise = `raise e`; handle =
  `try { } catch e { }` (+ postfix `expr catch e { }` / `catch { variants }`,
  reuses match); implicit propagation kept (default robust, `catch` to modify);
  errdefer subsumed by auto-drop + move. [ERRORS.md](../features/ERRORS.md).
- `-` **main entry contract** - omitted return type -> `int32` (main only) +
  implicit `return 0`; argc/argv (`main(int32 argc, string* argv)`);
  recursive-main ban. exit codes via `-> int32` already work. [MAIN.md](../features/MAIN.md).
- `-` **blocks as expressions** - a block is a first-class expression everywhere
  (Rust rule: tail expr = value); subsumes block-bodied match arms (verified
  broken today). [ledger](ledger.md).
- `~` **modifier blocks** - lexical regions that select a predefined alternate
  lowering of kernel ops (the nudge generalized). `wrapping { }` is the first
  (arithmetic, designed); `arena(a)` / `unchecked` / `comptime` banked as the
  spelling for parked escapes. four rails: lexical-only, kernel-ops-not-arbitrary,
  compiler-blessed-finite (**user-defined = far-future macro engine**), yagni
  (build `wrapping` only). [MODBLOCK.md](../design/MODBLOCK.md).

## Audited / gaps identified

- `~` **frontend-vs-clang safety audit** - two safety nets (bare prod build vs
  corpus strict-C gate), three classes (A clang-errors, B clang-warns-builds-anyway
  = silent today, C runtime-UB). decided: prod gains `-Wall` (not `-pedantic`, not
  `-Werror`), opt-in `--strict`. [ledger](ledger.md).
- `~` **kernel arithmetic edge semantics - decided 2026-06-21 (option Y):
  trap-by-default.** signed/unsigned overflow, over-width shift, div/mod-by-zero,
  neg `INT_MIN`, `INT_MIN/-1` all trap (the reserved `panic` atom, a bug per
  ERRORS.md D1) - no silent wrap. wrapping is opt-in via a lexical **`wrapping { }`
  modifier block** (sigils rejected); it doubles as the auto-vectorization opt-out.
  saturate/checked = stdlib. rejected wrap-default + trap-debug/wrap-release.
  sequencing: `-fwrapv` + shift-define + div-zero `abort()` now, flip-to-trap + the
  region with the abort path. [KERNEL.md](../design/KERNEL.md), [ledger](ledger.md).
- `!` recurring meta-finding: the audits checked *which primitives exist*, not
  whether each is *fully defined* at its edges (caught: arithmetic UB, non-integer
  index T47, one-operand-checked judgments). a per-judgment two-operand sweep is owed.

## Explained, build deferred

- `=` **generics fight sealed-body** - generic inference needs inter-procedural
  type flow (unification / monomorphization-coupling); sealed-body is defined by
  forbidding it. resolution: generics = comptime monomorphization (each instance an
  ordinary sealed body, no HM ever), so they wait on the prime/comptime VM.
  [TYPECK.md](../features/TYPECK.md). **(2026-06-21)** monomorphization is *also*
  the answer to row-polymorphic effects: each monomorphic instance gets a concrete
  effect set for free, so per-instance effect precision needs no effect variables.
  the only residue (fn-pointers in data / dynamic dispatch) is crossed by an
  *inferred concrete effect on the fn-pointer type* (joined at unification), not by
  row variables - so row-poly is demoted to a far-future escape hatch, not a
  pre-commitment. [EFFECT.md](../features/EFFECT.md) S7. why mono over an erasure /
  dictionary (Swift-witness) scheme that avoids code duplication: erasure needs
  uniform/boxed representation (kills Eye's unboxed value semantics) or a runtime
  witness-table ABI (indirect, non-inlinable - the wrong trade for a C-level lang),
  and Eye is *already* whole-program so mono's separate-compilation cost is sunk;
  open re-examination noted in the dependency chain below.

## Experimental / parked / far-future

- `?` **clang-import** - translate C headers into Eye externs (`@cImport`/bindgen);
  serves silent-safety in FFI but needs a C frontend + conflicts with the
  drop-clang Cranelift goal. parked. [FARFUTURE.md](FARFUTURE.md).
- `-` **concurrency** (effect-driven auto-parallelization), **extensibility
  engine** (macros / token-tree hygiene / comptime) - far-future. [FARFUTURE.md](FARFUTURE.md).
- `-` **organisation** - multi-file modules / imports / visibility, the `.ivlt`
  VFS archiver. [FUTURE.md](FUTURE.md) Fork C.

## The dependency chain (what gates what)

```
abort / panic theme  ->  bugs-side of errors + runtime-safety (class C, bounds, div-zero)
sum types (Fork B2)  ->  rich error VALUES (payload errors), ADTs
  recursive ADTs (Box)  ->  comptime (owning Box = generic); raw-*T manual interim
S7-payload effects (set<TypeRef>, cheap, closed-world)  ->  the `fail` effect carrying error types
  [row-poly effect VARIABLES demoted: mono subsumes; escape hatch only]
comptime / prime VM  ->  generics (monomorphization)  [+ subsumes row-poly effects]
escape / lifetime analysis  ->  full memory safety (closes the borrow-guardrail holes, dangling &local)
```

Open re-examination (2026-06-21): *why monomorphization at all?* mono is not forced
by sealed-body (an erasure / dictionary-passing scheme is also HM-free) - it is
chosen by Eye's unboxed value semantics + C-level zero-cost target + already-being
-whole-program. the no-code-duplication alternative (Swift-style value-witness
tables) exists but taxes runtime (indirect, non-inlinable) and adds a runtime
subsystem. **decided 2026-06-21: mono is the default; `dyn` / erased form is a
future language extension** (the nudge - fast default, opt into indirection for
bloat-sensitive spots). mono bloat is mitigated for *free* by the C backend -
clang `-O2` + lld ICF fold byte-identical instances (`Vec<uint32>` / `Vec<int32>`
often emit the same code), DCE drops unused instantiations, inlining absorbs small
ones - the same lean-on-the-backend pattern as `sizeof`. the real residual cost is
compile time, not binary size.

RESOLVED 2026-06-19 (the cross-cutting question): **ADTs / sum types with payloads
are now an official feature** - built as the first *compiler-blessed desugaring*
to the frozen kernel (no new primitive, no engine needed), [KERNEL.md](../design/KERNEL.md)
"The freeze, precisely". The freeze is reframed: features may be added as
desugarings to the frozen kernel, never as new kernel primitives. ADTs are the
value side of error handling and make plain enums far more useful.

**Sequencing (ratified 2026-06-19, phases P0-P5 expanded 2026-06-21).** harden the
current pipeline first, then add features in dependency order. canonical plan:
[ledger](ledger.md) "Phased implementation plan".
- `-` **P0 - current pipeline correctness + hardening** (no new surface): pointer /
  `usize->ptr` gaps, arithmetic UB-kill + the abort/panic keystone, non-integer index
  (T47) + the one-operand->two-operand sweep, frontend-vs-clang class A/B + `-Wall` +
  lints, C-attribute stamping (`nonnull` / `format` / FFI-const), codegen hygiene,
  block-bodied-match bug, main-entry contract.
- `-` **P1** trap-default arithmetic + `wrapping { }` + const-default params.
- `-` **P2 (the reorder)** move/drop/use-after-move keystone + `&mut` - pulled
  *before* ADTs/errors, since owning-payload drop + error-path drops need it (the
  2026-06-19 order wrongly tailed it under "mutability completion").
- `-` **P3** ADTs (3a non-owning, 3b owning drop glue) -> **P4** error handling
  (S7-payload + try/catch) -> **P5+** comptime/generics + escape analysis.
