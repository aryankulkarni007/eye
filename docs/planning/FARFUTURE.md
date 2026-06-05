Here is the executive brief of the architectural blueprint for **Eye** based on our session. You are moving away from the crowded "memory-safety fortress" design space to claim a wide-open territory: **a systems language focused entirely on radical ergonomics and user-driven extensibility.**

---

## 1. The Core Philosophy

* **The Target:** A modern, ergonomic C. It rejects the cognitive tax of a restrictive borrow checker, choosing instead to grant the programmer total machine-level freedom at runtime.
* **The Paradox:** While the runtime is raw and unhindered, the compiler itself is an ultra-intelligent, semantic reasoning engine.
* **The "Microkernel" Boundary:** The base language consists only of lean, orthogonal, explicit primitives (structs, nominal arrays, function pointers, mandatory initialization). It is completely valid-by-construction (no `null` literals).

---

## 2. The Extensibility Engine (Language Injections)

Instead of hardcoding high-level features (like OOP classes or traits) into the compiler core, Eye allows developers to teach the compiler new paradigms via macro extensions. The extension runtime desugars high-level paradigms straight down into the unshakeable base substrate.

To make these injected features feel completely native, the compiler relies on:

* **Token Trees & Syntax Hygiene:** Tracking a `Syntax Context ID` on every token so generated code never collides with local user code.
* **Multi-Span Diagnostics:** Origin-tracking engine that maps lower-level type-checker errors back to the exact high-level macro syntax the user wrote, preserving the language illusion.
* **Query-Based Architecture:** Moving away from linear compiler passes to a demand-driven query pipeline, allowing extensions to resolve and inject symbols lazily on demand.

---

## 3. The Semantic Brain (Types & Effects)

* **Bidirectional Type Checking:** Splitting type analysis into inference (inside-out) and checking (outside-in) modes. This keeps compile times blazingly fast while easily resolving anonymous closures and expressions.
* **Front-Loaded Effect System:** Tracking behaviors (`pure`, `alloc`, `unsafe`) directly in front of the function name.
* *For Extensions:* Keeps compile-time macro execution safe from silently corrupting the host compiler’s memory space.
* *For Optimizations:* Feeds the backend definitive proof to easily trigger aggressive Dead Code Elimination (DCE), Common Subexpression Elimination (CSE), and fearless automatic parallelization.



---

## 4. Native Execution (The Cranelift Jump)

While C generation serves as a perfect bootstrap vehicle today, the ultimate destination is a machine-code generator like Cranelift.

* **Semantic Freedom:** Complete liberation from C's undefined behavior models and host alignment quirks.
* **Zero-Cost Safety:** Lowering explicit conditional traps directly into lean, hardware-level conditional jumps for bounds checking.
* **Tooling Speed:** Eliminating disk I/O overhead by compiling Cranelift IR straight to binary completely in-memory.

> **In Short:** You are building a platform where developers can safely invent their own perfectly-tailored language paradigms without ever sacrificing the speed of raw machine code or the clarity of native compiler errors.
