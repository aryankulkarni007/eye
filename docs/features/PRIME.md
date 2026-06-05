# Prime: the compile-time execution architecture

`prime` is Eye's keyword for compile-time execution (the role Zig calls
`comptime`): code the compiler runs *first*, before runtime, to prime the
program. This doc captures the architecture for that layer and the compiler
refactor it implies. Everything here is DESIGN / DECISION, not built.

Reads alongside [VISION.md](design/VISION.md) (the thesis), [HORIZON0.md](design/HORIZON0.md)
(the near-term kernel freeze), [MASTERPLAN.md](planning/MASTERPLAN.md) (sequencing), and
[FARFUTURE.md](planning/FARFUTURE.md).

## The distinction that drives everything: const is not prime

These were conflated in earlier docs. They are different layers:

- **const** = compile-time constant *values*. `const int32 MAX = 100;` plus a
  bounded const-expr fold (literals, the operator set, const references,
  `sizeof`). Finite, modest, near-term. This is [HORIZON0.md](design/HORIZON0.md)
  Component 1.
- **prime** = compile-time *execution*: running code at compile time. This is what
  `generics = prime + AST instantiation` actually needs, and const gives **none**
  of it.

const does not unlock generics. The generics-unlocking part is prime *execution*
operating on AST, which is the macro engine. So the old "const/comptime is the
bottleneck that unlocks generics" framing was wrong: const is a finite kernel-floor
item; the bottleneck for the *vision* is prime execution, and that is far-future.

## The power gradient (floor to engine)

"prime" is not one feature; it is a power gradient.

| # | Layer | What it is | Side of the line |
|---|-------|-----------|------------------|
| 1 | const-expr fold | evaluate `2 * 3` at compile time | kernel floor (Horizon 0) |
| 2 | const refs + `sizeof` | `const M = N + sizeof(T)`, with a symbol table | kernel floor (Horizon 0) |
| 3 | CTFE | run a *pure Eye fn* at compile time (`const N = fact(5)`) | the hinge |
| 4 | types as prime values | a fn takes a `type`, inspects fields / size, branches on it | engine |
| 5 | AST as prime value | prime code builds and emits AST (quote / splice, hygiene) | engine |
| 6 | compiler hooks | register vtable / pattern lowerings (the B2 seam), new syntax | engine (the identity) |

The macro engine is layers 4-6 = CTFE (3) running code whose values include types
and AST. So **CTFE is the real load-bearing build**, sitting on the kernel/engine
boundary; const (1-2) is the trivial part below it.

## Near-term is layered, not a fixpoint

prime, bidirectional type inference, and effects are *mutually recursive in the
limit*. But the **cyclic** fixpoint (inference calls prime calls inference, and it
must converge rather than error) only goes live with the macro engine (layers
4-6). Near-term interactions are one-directional and layered:

- `const N = sizeof(T)` - type to value.
- `[int32; N]` - value to type.
- `const N = sizeof([int32; N])` - a cycle, which is an **error**, caught by the
  cycle detection already in `crates/hir/src/core/typegraph.rs`.

So near-term the requirement is only "type resolution may call const-eval, with
cycle detection." That is layered recursion, not a fixpoint, and it does **not**
require a query engine. The query engine is forced only by the far-future cyclic
fixpoint and by incrementality.

## Decisions

### D1 - macro engine: far-future, "when the infrastructure is in place"

The supermacro / pattern-registration engine (layers 4-6) stays far-future
(~v10). Build the substrate first (const, then the typeck surgery, then effects,
then the prime VM); the engine automates the hand-written patterns later. This
confirms the bootstrap-hinge resolution in [KERNEL.md](design/KERNEL.md).

### D2 - the real foundation is the HIR / inference decoupling (typeck surgery)

The current pipeline's actual structural defect is that type-stamping is fused
into HIR lowering (AUDIT's #1 finding). That is what makes the pipeline "not built
for the new design" - not the absence of a query engine. The fix is a structural,
frozen HIR plus a **separate bidirectional inference pass** (infer inside-out,
check outside-in) over it. This is the next large build after const, and it is the
foundation effects and prime both stand on.

### D3 - the invariant first move

Write inference as **pure functions** (explicit inputs, no HIR mutation). This is
the first step whether or not a query engine is ever adopted:

- if no query engine: a clean bidirectional checker over frozen HIR;
- if a query engine: it *wraps* those pure functions - inference logic reused
  untouched, only orchestration (query granularity, memoization) added.

So typeck-first is not "building it twice." The only rework is the orchestration
layer, which is small next to a big-bang rewrite.

### D4 - query engine (salsa-style): deferred, decided later on merits

A demand-driven query engine is the right *eventual* spine (it resolves the
cyclic fixpoint and gives incrementality), but it is forced only by far-future
work. Defer the decision until the query shape is known from real experience.
Decide it on its own merits (API churn, lock-in), **not** by analogy to the WASM
decision - they are different maturity bets.

### D5 - the prime VM: WASM (decided)

The compile-time execution machine is a WASM VM (embed an industrial runtime such
as wasmtime). A pure Eye `prime fn` compiles to a WASM module, runs sandboxed, and
returns a prime value. Rationale: the sandbox *is* the host-compiler-safety
guarantee (a macro provably cannot corrupt the compiler's memory), enforced by the
runtime, not only by the type system; plus portability and speed, leaning on
decades of engineering rather than reinventing it. Tree-walking (trivial, slow,
unsafe) and a custom bytecode (middle) were the alternatives; WASM wins because
its sandboxing aligns exactly with the effect-safety goal.

Open fork (backend phase, not now): the Eye-to-WASM path is itself a backend.
Its relationship to a future Cranelift native backend (Horizon 3) is undecided -
one lowering target from MIR, or two?

### D6 - effects: kernel machine-effects now, algebraic effects + handlers later

Effects are a kernel feature: a macro cannot synthesize a *sound* effect
discipline over the kernel's own primitives. They ride the **same** bidirectional
inference machine as types - inferred bottom-up, checked top-down, just a
different lattice. Effects thread every stage: annotation (parse) -> inference (the
typeck layer) -> the prime gate (D5) -> backend optimization (effect proofs drive
DCE, CSE, auto-parallelization).

**Where this sits in the prior art.** The design space has four families:
algebraic effects + handlers (Koka, Eff, Effekt, OCaml 5 - the frontier;
OCaml shipped handlers *untyped*, a signal that typed effects are hard);
row-typed effect inference (Koka, Unison, old PureScript); monadic (Haskell);
and ad-hoc "function colors" (Rust `unsafe`/`async`/`const fn`, Swift
`throws`/`async`/`rethrows`, Java checked exceptions - the cautionary tale). Eye's
design is **row/set-typed inferred effects (the Koka family), with concrete
machine atoms** - the principled end, not the ad-hoc end. Inference-default with
optional annotations dodges Java's ceremony problem.

**The kernel / engine split (same thesis as match).**

- **Kernel** = the irreducible **machine effects**, derived by the discriminating
  test - *one atom per kind of side-effect a kernel primitive produces*, which a
  macro cannot synthesize:
  - `io` (the `print`/`printf` seam), `ffi` (extern call + raw-pointer deref),
    `state` (mut-global read/write, D7 globals), `panic` (bounds-traps, DEFER).
  - **Anticipated slots** (lattice has room, enforced when their primitive lands):
    `alloc` (a real heap allocator; today malloc is an extern, so `ffi`),
    `diverge` (non-termination - gates prime totality). The set is **open**;
    `conc` / `nondet` slot in later.
  - `pure` = the empty effect set (the lattice bottom). A fn's effect = the union
    of its atoms and its callees'.
- **Engine / stdlib** (far-future) = **user-definable abstract effects + handlers**
  (exceptions, async, generators, coroutines, custom effects) as *libraries*, not
  kernel features. Handlers are to control-flow what supermacros are to syntax.
  Do **not** bake exceptions / async into the kernel - they arrive via handlers
  through the engine, exactly as sum types arrive via the match seam (Component 4).

**Two upgrade axes - design for them, build the simple form now** (mirrors D3):

1. **set -> row.** The flat effect *set* is a Koka *row* without its polymorphic
   tail. Effect-polymorphism (`map` over an effectful fn) needs that tail. Shape
   the effect type so the tail variable is an *addition*, not a rewrite. Build
   monomorphic effects first.
2. **tracking -> handlers.** The kernel ships effect *tracking / checking* (the
   sound, tractable part). Algebraic effect *handlers* (the control mechanism) are
   deferred to the engine, where they are earned - heeding the OCaml-untyped
   warning.

**The prime gate, precise.** A `prime fn` must have effect set excluding `io` /
`ffi` / `state` and must be `¬diverge` (a non-terminating comptime fn hangs the
compiler). It *may* `alloc` (comptime owns a heap) and *may* `panic` (which
becomes a compile error). This is how D5's safety guarantee is expressed in the
type system as well as the runtime.

**No `unsafe` blocks.** The `ffi` effect *is* the unsafe boundary: a `pure` fn
that derefs a raw pointer or calls an extern is a type error unless it declares
`ffi`. The boundary is explicit and propagating, not a block that hides things.
One fewer kernel concept.

**Annotation surface.** Effects are mostly inferred; an annotation is a rare,
optional published contract (usually just `pure`). Surface = bare space-prefixed
keywords on the keyword-less fn (effects are reserved words, the name sits right
before `(`, so it is unambiguous):

```
pure square(int32 n) -> int32 { n * n }
io   report(int32 n) { ... }
io alloc render(Scene s) -> Image { ... }
```

This reads as type-qualifier adjectives and keeps the common single-effect case
clean (a bracketed-set form `[io, alloc] f(...)` was the runner-up; rejected for
taxing the common case). Public-API effect contracts become required when modules
arrive; whole-program inference covers the unannotated until then.

### D7 - two orthogonal axes

Kernel-freeze (language semantics) and compiler-refactor (implementation
architecture) are independent. The language kernel can freeze
([HORIZON0.md](design/HORIZON0.md)) while the compiler is rebuilt underneath. `const-eval`
and `sizeof` are the one seam between them: build them as the first const-eval
consumers, not as bolt-ons to the old pipeline.

### D8 - types and AST as first-class prime values

The prime VM's value domain is scalars plus `type` values plus `ast` / `code`
values. With this, a macro is just a prime function `fn(ast) -> ast` (or
`fn(type) -> type`). Reflection falls out: `sizeof` / `alignof` / field lists are
accessors on a type-value, so Horizon 0's `sizeof` is the first seed of this data
model.

### D9 - async registration at project init

The macro engine runs the prime VM at project init, asynchronously, to execute
supermacro definitions and register new syntax (parser extensions) and new
lowerings (the B2 seam, vtable lowerings) as query inputs. After init the compiler
reasons about the extended language as if native, because extensions are
first-class inputs, not post-hoc text rewriting. This is the top of the stack and
is gated on everything above.

## The stack

```
            async macro engine        D1, D9 - registers syntax + lowerings at init
                    |                          (extensions become query INPUTS)
            prime VM (WASM)            D5, D8 - values = scalars + types + AST
                    |
        bidirectional inference        D2, D6 - types AND effects, one machine, two lattices
                    |                          (surgically removed from HIR)
        [ query engine, if/when ]      D3, D4 - wraps the pure inference fns; deferred
                    |
            frozen kernel HIR          <- Horizon 0 (const, sizeof, static-data, match-seam)
```

Read bottom-up for build order, top-down for dependency order.

## The sequence

1. **const + const-expr fold** (+ `sizeof`, static-data, match-seam) -
   [HORIZON0.md](design/HORIZON0.md). const-eval is the first, VM-less prime consumer.
2. **typeck surgery** (D2, D3) - decouple inference from HIR, bidirectional, pure
   functions. The real foundation.
3. **effects** (D6) - the same inference layer.
4. **prime VM** (D5, D8) - real compile-time execution; types and AST as values.
5. **macro engine** (D1, D9) - async registration; far-future.

A query engine (D4) slots under steps 2-5 if and when incrementality and the
cyclic fixpoint demand it.

## The standing risk

Rebuilding the foundation of a green 203-test compiler before any new capability
lands is the highest-risk move available. The discipline is **incremental
migration that keeps the suite green at every step**. That is not a cheap hack
versus a big-bang rewrite; it is the complete design built in correct dependency
order and validated continuously.
