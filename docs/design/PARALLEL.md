<!-- this document contains plans for parallel, asynchronous and multi-threaded workflows-->
<!-- in the compiler pipeline-->

- parallelised bidirectional type and effect inference
  RATIFIED 2026-06-12 as **sealed-body inference** - full design in
  docs/features/TYPECK.md (types) and docs/features/EFFECT.md (effects).
  Summary: no inference fact ever crosses a fn boundary (signatures are the
  only inter-fn channel), so per-body checking is embarrassingly parallel;
  wave 1 = rayon over bodies (types + effect atoms in one task), wave 2 = the
  effect fixpoint (Tarjan condensation, bitset unions - the only serial
  part, O(V+E) on byte-sized sets).

  BUILD STATUS as of 2026-06-15:
  + S1 (shadow-mode walker): BUILT
  + S2 (cutover, steps A-D): IN PROGRESS (A-C1-C3 complete, C4 complete,
    C2+C5+D remaining)
  + S3 (new judgments): DESIGNED, NOT BUILT
  + S4 (effect inference + fixpoint + annotations + contract): BUILT (2026-06-14)
  + S5 (signature firewall): DESIGNED, NOT BUILT
  + S6 (parallel wave: rayon per-body + lock-free interner): DESIGNED, NOT BUILT
  + S7 (row-polymorphic effects): DESIGNED, NOT BUILT (see EFFECT.md)

  An earlier draft of this section named Hindley-Milner for types. Replaced:
  global HM is what forfeits the per-body parallelism above (its purpose is
  inferring unannotated signatures, which the Explicit Contract bans).
  Unification, if ever needed, arrives as TYPECK.md Tier 3: body-local
  variables solved at the seal, never escaping a signature. Row-polymorphic
  effects upgrades from the monomorphic kernel to effect variables on fn
  types (S7), enabling precise tracking through higher-order fns.

  Threaded through the LSP via salsa parallel queries + cancellation; the
  whole-program effect fixpoint runs off the latency path. Cycle handling:
  the SCC condensation *is* the fixpoint (no iteration); constraint-graph
  partitioning beyond that is unnecessary at the chosen lattice cost.

---

The segmentation S0-S7 tracks the build plan. After S6 (parallel wave) and
S7 (row-polymorphic effects), the dual inference engine reaches its designed
end state: per-body checking is embarrassingly parallel, effect tracking is
precise through higher-order fn calls, and the lock-free interner eliminates
the last shared-memory bottleneck.

- parallelised lexing and parsing
  trivial with rayon work-stealing pool

- parallel HIR lowering
  will require preliminary pass -> resolution pass too

- dependency barrier (global resolution)
  RESOLVED for the type layer 2026-06-12: the lock-free architecture exists
  and needs no RwLock. The one shared-write structure (the type interner)
  goes lock-free via `boxcar::Vec` (append-only arena, atomic reads) +
  `papaya::HashMap` (lock-free-read dedup); everything else is task-owned
  (per-body results) or read-shared (salsa snapshots). See TYPECK.md "The
  parallel wave". Global *symbol* interning at the multifile milestone uses
  the same pattern (`lasso::ThreadedRodeo`); worker-collect-then-merge
  remains the fallback if a lock-free table ever disappoints.

- Cranelift backend (independent of the inference engine):
  The Cranelift native codegen is on the way, independent of all inference
  work. The MIR boundary already exists, so this is a backend swap. Payoffs:
  eliminate C and clang from the toolchain, provide in-memory compilation
  (JIT and LSP uses), and enable target-specific optimizations without C
  semantics. See MASTERPLAN.md Horizon 3.

- determinism gate for S6 (parallel wave):
  The parallel wave introduces nondeterministic `TypeRef` ordering because
  parallel interleaving changes `intern` call order. A CI gate must diff the
  C output of the full corpus under parallel vs serial execution, proving
  byte-identical output despite nondeterministic handle values. The design
  rule: no observable output may depend on `TypeRef` numeric order; tie-
  breaks in codegen ordering must use names/structure, not handles. The gate
  enforces this rule mechanically.

- memory pressure:
  for very large code repositories, storing all ASTs in memory while we operate on them is
  not feasible: we have a few options

  serialised ast caching
  serialise the cst to a tmp file or mmap it and drop the rowan objects. pull them back
  when the LSP needs to re-parse the file

  arena-based per-file allocation
  using an arena, we can minimise memory synchronise

  lazy resolution
  if the file is in the dependency graph of the current target, then we can lazily load it's
  symbols

---

## The Strategic Brief: Project "No-Leak" Extensibility

**The Irony:** To build an infinitely extensible language that "injects" paradigms, you must first build an incredibly rigid, "boring" kernel that knows absolutely nothing about those paradigms.

**The Core Principle:** If the Inference Engine has to know about your extensions, you have failed. The extensions must die (desugar) before they ever reach the semantic brain.

### 1. The "Phase-Lock" Pipeline (The Immunity Shield)

To prevent the Type and Effect systems from becoming codependent and un-parallelizable, the compiler must act as a **Staged Transformer**:

- **Phase 0: Registration (The Bootstrap):** Load library definitions and register procedural parsing hooks into the Grammar Registry.
- **Phase 1: Procedural Parsing:** Convert arbitrary/injected syntax into an "Opaque Token Tree." The compiler doesn't understand the _meaning_ yet, only the _structure_.
- **Phase 2: Syntax-Directed Desugaring:** A direct structural rewrite that turns Opaque Trees into **Kernel HIR**. This is pure code-to-code substitution. No types are consulted.
- **Phase 3: The Parallel Brain:** Now that the HIR is purely Kernel-native, spawn $N$ threads to perform Bidirectional Type and Effect Inference in parallel.
- **Phase 4: Verification/Culling:** If the generated code is semantically invalid, the Inference Engine errors out, mapping the failure back to the original source via **Origin Tracking**.

### 2. The Golden Rules

- **The Explicit Contract:** All functions must have explicit signatures (parameters/returns). This breaks the inference deadlock and allows the Effect system to run _without_ waiting for the Type system.
- **Syntax-Directed Only:** If your desugaring requires type information, move it to a later pass. Desugaring must be structurally blind to ensure it can run before the semantic engines.
- **Constraint Partitioning:** Do not solve the graph globally. Partition the dependency graph into independent components. Solve trivial partitions in parallel; serialize only the necessary cycles (recursive functions/interdependent effects).

### 3. Why this won't sink

- **Separation of Concerns:** The "Macro/Generics" logic never touches the "Inference/Type" logic. If the Type Checker breaks, it's a bug in the Type Checker, not because a macro injected an unexpected generic type.
- **Parallelism:** Because the engine only sees "Concrete Kernel HIR," you can utilize worker pools to process function bodies independently. You aren't managing a "God Object" HIR; you are managing a queue of independent function-inference tasks.
- **Memoization:** By memoizing at the pass boundary, you get the performance of "incremental compilation" without the nightmare of building a complex, stateful graph-invalidation system.

---

> **The Executive Summary:**
> It is "very hard" because you are building a compiler that compiles itself into existence through a chain of rigid, pure transformations. You are replacing global, monolithic inference with local, parallel verification. The complexity isn't in the logic; it's in the **discipline** of enforcing the phase boundaries.
