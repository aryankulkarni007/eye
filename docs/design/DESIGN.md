# Design vision: the forensic meta-platform

Status: ASPIRATIONAL, NOT BUILT. None of the mechanisms below exist in the
compiler today. This document records one strand of the long-term vision - the
compiler as an effect-tracking extension platform. The canonical kernel/stdlib
thesis is [VISION.md](VISION.md); the far-future execution brief is
[FARFUTURE.md](planning/FARFUTURE.md); for what the compiler actually does today see
[CAPABILITIES.md](dev/CAPABILITIES.md) and [AUDIT.md](AUDIT.md). The audit notes
explicitly that the forensic / effect-tracking / bridge model here is not
implemented; the present compiler is a conventional statically-typed,
C-transpiled language with a strong front end and an emerging MIR.

## The kernel as a meta-platform

The compiler is not a monolith but a kernel: a lean, effect-tracking engine that
is modern ergonomic C plus the substrate needed to make language extension
possible. Features are not hard-coded. A feature is a vertical slice - a library
that defines its own tokens, grammar, lowering, and diagnostic handlers - shipped
as a first-class citizen of the developer library (`devlib`). OOP, for example,
would be a library that defines a vtable through the compiler internals, not a
struct-plus-function-pointer emulation.

## Forensic core principles

The onus of proof is on the extension author, not the compiler. The compiler does
not guess.

- **Maximal pessimism.** Every ambiguity is treated as a failure. Nothing is
  valid unless explicitly proven. (Contrast C++ SFINAE-style overload selection,
  which the model rejects.)
- **Effect-tracking inference.** The kernel tracks effects (Read, Write, Suspend,
  Mutate) alongside types. It knows not only what data is but what an operation
  does.
- **Interaction contracts (bridges).** When two libraries manipulate the same
  resource (a thread context, say), the effect tracker flags the collision and
  halts the build. The author resolves it by writing a `bridge` block: a formal
  proof obligation that reconciles the two effects. The build proceeds only once
  the contract is satisfied.
- **The diagnostic bus.** Errors are structured messages routed to error-handler
  actors rather than cryptic strings. Because the handlers ship with the
  `devlib`, a low-level AST or type failure can be translated into a high-level,
  domain-specific report.

## The meta.dev abstraction layer

Extension authors are not expected to write raw compiler internals. `meta.dev` is
a high-level DSL for defining tokens, parse nodes, and diagnostics without
touching the compiler's guts.

- **Safe mode.** Staying within `meta.dev` yields standard diagnostics and
  guaranteed IR compliance.
- **Privileged mode.** Reaching past `meta.dev` into the kernel is permitted, but
  a Provenance IR tags the code so the author owns any resulting instability.
  This is the privilege-ring model from [VISION.md](VISION.md), seen from the
  tooling side.

## The author workflow

1. **Declare effects.** Every library declares its footprint via `meta.dev`.
2. **Enumerate conflicts.** The LSP tracks effects and lists collision points.
3. **Construct the bridge.** The author writes the `bridge` block - the proof
   that the composition is safe.
4. **Verify.** A compile-time evaluation (CTFE) engine runs the proof. If it
   holds, the code compiles; if there is a gap, the compiler points to the exact
   line where the contract was violated.

## Implementation path

C transpilation is the bootstrap vehicle that stabilizes the kernel. The
long-term target is a native code generator (Cranelift), after which the standard
library is rebuilt on that IR. This is the same Cranelift jump described in
[FARFUTURE.md](planning/FARFUTURE.md). The goal is composability without hidden
interactions or undefined behaviour; the effect tracker and bridge contracts are
the mechanism by which composition stays sound.

## Relationship to the rest of the doc set

This document is the most speculative of the vision set and the furthest from the
current code. It should be read as a target, not a description. The load-bearing,
already-ratified decisions (unoverwriteable kernel, sum-types-as-stdlib,
supermacro horizon, match extensibility) live in [VISION.md](VISION.md) and
[KERNEL.md](KERNEL.md); the effect system and bridge model here are not yet
designed at that level of commitment.
