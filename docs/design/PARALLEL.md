<!-- this document contains plans for parallel, asynchronous and multi-threaded workflows-->
<!-- in the compiler pipeline-->

- parallelised bidirectional type and effect inference
  i.e. each system will have a preliminary pass on all source files (multi-threaded)
  since we will already have a dependency graph, we can use this for the second
  resolution pass.

  I imagine we will implement
  Hindley-Miller approach to type inference
  Row-Polymorphism approach to effect inference

  This should also be threaded through the LSP/

  We will need to consider cycle detection and main thread bottlenecking
  in that case we may need to look into constraint graph partitioning, where we
  will parallelise the resolution pass by splitting into sub-graphs

- parallelised lexing and parsing
  trivial with rayon work-stealing pool

- parallel HIR lowering
  will require preliminary pass -> resolution pass too

- dependency barrier (global resolution)
  perhaps we should implement a global symbol table protected by RwLock -> maybe even design
  an architecture that doesn't need locks
  then we can parallelise symbol collection whereby each worker thread reports the symbols it
  finds and then we synchronise

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
- **Phase 2: Syntax-Directed Desugaring:** A "dumb" structural rewrite that turns Opaque Trees into **Kernel HIR**. This is pure code-to-code substitution. No types are consulted.
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
