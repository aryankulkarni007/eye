# Master plan: the road ahead

This is the strategic map from the current tree to the long-term vision. It
orders the remaining work by what is gated on what, and names the critical path.
It is forward-looking: status here is "where each item sits in the plan", not a
claim that anything past Horizon 0 is built.

For grounding, read alongside:

- [KERNEL.md](design/KERNEL.md) - the concrete kernel-completeness gap ledger (the
  source for Horizon 0).
- [FARFUTURE.md](FARFUTURE.md) - the far-future execution brief (the source for
  Horizons 2-3).
- [VISION.md](design/VISION.md) - the kernel/stdlib thesis the whole plan serves.
- [CAPABILITIES.md](dev/CAPABILITIES.md) - what compiles and runs today.

## Where the tree stands

The kernel is mostly built. Recently landed: function pointers (2026-06-05),
early return (2026-06-04). Floats were already built. Structs, unions, enums,
fixed arrays (`[T; N]`, `&[T; N]`, `len`), raw pointers, `match` skeleton,
machine ints, the operator set, and FFI `extern` are all in. The MIR cutover is
complete: the backend is a MIR to C dumb printer, so the backend boundary needed
for a later native codegen already exists.

What stands between now and a freezable kernel is small and finite. Everything
past that is the vision: large, and gated on the kernel freezing first.

## Horizon 0 - Freeze the kernel (finite, near-term)

The kernel is **unoverwriteable** ([VISION.md](design/VISION.md)): once frozen, nothing
in it can be removed or changed. "Complete" means every primitive a supermacro
provably cannot synthesize is present, and nothing else is. These are the
remaining MISSING rows from [KERNEL.md](design/KERNEL.md):

| Gap | Why it blocks freeze | Identity weight |
|-----|----------------------|-----------------|
| **const + global storage** | Generics are defined as comptime plus AST instantiation; without const there is no comptime, so no supermacro engine is ever possible. Also gates A6 const-length arrays. | High - core Eye work |
| **`match` skeleton + lowering seam** | B2 (extensible match) is resolved. The deliverable is to design the seam and keep kernel match minimal; the registration half stays inert until the macro engine. | High |
| `sizeof` / `alignof` | `malloc(n * sizeof(T))` cannot be written without it; a macro cannot compute a type's size portably. | Medium |
| variadic `extern ...` | The C ABI seam is incomplete without it. Unblocks `printf` and the bubblesort / file corpus. | Low (C bridge) |
| opaque FFI pointer types (`FILE*`) | `fopen` / `fgets` need a typed pointer. Pairs with dropping the auto-`#include`. | Low (C bridge) |
| string literals as first-class values | Today the literal is untyped and `print` renders it `%d`. Needed to pass externs a real pointer. | Medium |
| **evict `print` intrinsic** | Subtractive. The vision puts `print` in the stdlib (compose `printf`), not the kernel. Blocked on first-class strings plus variadic FFI. | Identity |

Sequencing (from KERNEL.md, sorted by identity, not by ease): const/comptime
first, then the match seam, then the C-seam plumbing last and lazily. When this
table clears, the kernel can freeze.

## Horizon 1 - The semantic brain (gated on freeze)

From [FARFUTURE.md](FARFUTURE.md) section 3 and the top gap in [AUDIT.md](design/AUDIT.md).

- **Type checking as a separate pass** ([TYPECK.md](features/TYPECK.md), designed, not
  built). Today type stamping is fused into HIR lowering, which AUDIT flags as
  the single biggest structural gap. Splitting it out yields a bidirectional
  checker (inference inside-out, checking outside-in) over frozen HIR. It is a
  prerequisite for effects and for macro errors that map back to user syntax.
- **Effect system** - `pure` / `alloc` / `unsafe` in front of the function name.
  Two payoffs: compile-time macro execution cannot corrupt the host compiler's
  memory, and the backend gets definitive proof to drive dead-code elimination,
  common-subexpression elimination, and automatic parallelization.

Effects depend on the typeck split. The typeck split depends only on the freeze.
So typeck is the next large structural build after const.

**Status:** The typeck split (S1 shadow mode) and effect system (S4 fixpoint +
annotations + contracts) are BUILT as of 2026-06-14. The remaining build work
on the dual inference engine is the S2 cutover (C2/C4/C5/D), S3 new judgments,
S5 signature firewall, S6 parallel wave, and S7 row-polymorphic effects.

## Horizon 2 - The extensibility engine (far-future, ~v10)

From [FARFUTURE.md](FARFUTURE.md) section 2. This is the identity of the language:
users inject paradigms (OOP, iterators, sum types) as stdlib supermacros that
desugar down to the frozen substrate. [KERNEL.md](design/KERNEL.md) resolved the timing:
this stays far-future, and any vtable / iterator / sum-type use is hand-written
until the engine arrives. It needs three subsystems first:

1. **Query-based architecture** - move from linear passes to a demand-driven
   query pipeline so extensions can resolve and inject symbols lazily.
2. **Token trees + syntax hygiene** - a syntax-context id on every token so
   generated code never collides with user locals.
3. **Multi-span origin-tracking diagnostics** - map low-level type-checker errors
   back to the high-level macro syntax the user wrote, preserving the illusion
   that the injected feature is native.

The `match` B2 seam goes live in this horizon. Do not start it until const and
the typeck split have landed.

## Horizon 3 - Native codegen via Cranelift (in progress, independent)

From [FARFUTURE.md](FARFUTURE.md) section 4. Cranelift native codegen is **on
the way** as an independent work stream. The MIR boundary already exists, so
this is a backend swap. Payoffs: eliminate C and clang from the toolchain,
remove C's undefined-behavior model from the compilation path, enable
in-memory compilation (JIT compilation and LSP evaluation), lower bounds traps
to lean conditional jumps (zero-cost safety), and remove disk I/O from the
compile loop. This adds no language power (it is a transparent backend
replacement), so it runs on its own timeline, independent of the inference
engine work.

## Orthogonal axis - runtime safety (deferred)

Bounds traps (abort on dynamic out-of-bounds) and escape / lifetime analysis
(dangling `&local`). Both are blocked on Eye having no panic / abort mechanism
and no runtime length. One later theme, off the critical path. See
[DEFER.md](DEFER.md).

## The critical path

```
const (finite) ---> (kernel freeze) ----------------------------------------+
       |                                                                     |
       +--> typeck surgery (S1 built, S2 cutover, S3 pending) ----+         |
                  |                                                |         |
                  +--> effects (S4 built, S7 row-polymorphic = pending)      |
                  |                                                          |
                  +--> prime VM (WASM) ---> macro engine ---> generics, containers, identity payoff
                                              (far-future, "when ready")

Cranelift --- independent, on the way (MIR boundary ready)
Inference infrastructure: S5 (firewall), S6 (parallel wave) --- parallelizable, post-cutover
```

Correction (see [PRIME.md](features/PRIME.md)): an earlier draft called the bottleneck
"const / comptime" and claimed it unlocks generics. That conflated two layers.
**const** (compile-time constant *values*) is a finite kernel-floor item.
**prime** (compile-time *execution*) is what generics actually need, and it is
far-future. So const does not unlock generics; the macro engine does, later.

The real near-term foundation is the **typeck surgery** - decoupling type
inference from HIR lowering (AUDIT's #1 structural finding). That is what makes the
pipeline ready for effects and prime. The query-engine refactor is deferred and
decided later on its own merits, not pulled forward now ([PRIME.md](features/PRIME.md) D4).

## Near-term fork - RESOLVED

The build order is settled (2026-06-05, see [PRIME.md](features/PRIME.md)):

1. **const** + const-expr fold (+ `sizeof`, static-data, match-seam) - finish the
   kernel floor and freeze ([HORIZON0.md](design/HORIZON0.md)). **The immediate target.**
2. **typeck surgery** - the real foundation; inference as pure functions over
   frozen HIR (PRIME.md D2/D3).
3. **effects** - the same bidirectional inference layer (PRIME.md D6).
4. **prime VM (WASM)** and **macro engine** - far-future (PRIME.md D1/D5).

Cranelift stays parallel and independent. The macro engine is built "when the
infrastructure is in place," not pulled forward.
