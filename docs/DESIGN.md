# The Project: A Forensic Systems Language

I’m building a language that’s pure wizardry for the user, but for the library author? It’s a murder trial. The onus of proof is entirely on you. If you want to ship a feature, you’re constructing a defense case, and I’m making sure the compiler is the judge that doesn’t take bribes, doesn’t guess, and doesn’t allow "magic."

#### The Architectural Pivot: The Kernel as a Meta-Platform

The compiler isn’t a monolithic block. It’s a kernel—a lean, effect-tracking engine that is effectively **modern, ergonomic C plus everything needed to make language extension programming possible.**

We aren't hard-coding features. We’re building vertical slices. If I want OOP, I’m not changing the compiler; I’m writing a library that defines the tokens, the grammar, the lowering logic, and the diagnostic handlers. That library gets shipped as a first-class citizen of the `devlib`.

#### The "Forensic" Core Principles

- **Maximal Pessimism:** SFINAE is dead. The compiler assumes every ambiguity is a failure. If it’s not explicitly proven, it’s not valid.
- **Effect-Tracking Inference:** The kernel tracks effects (Read, Write, Suspend, Mutate) alongside types. It doesn't just know _what_ the data is; it knows _what it does_.
- **Interaction Contracts (Bridges):** If two libraries manipulate the same resource (like a thread context), the effect tracker flags a collision. The build halts. We don't guess—we write a `bridge` block. That block is the contract that reconciles the collision. It’s a formal proof obligation that the developer must satisfy.
- **The Diagnostic Bus:** Errors aren't cryptic strings; they are messages sent to `CompilerErrorHandler` actors. Because we’re shipping these with the `devlib`, we can translate a low-level AST failure into a high-level, human-readable forensic report.

#### The "Meta.dev" Abstraction Layer

I'm not forcing authors to write raw compiler internals. That's a suicide mission. I'm building `meta.dev`—a high-level DSL that lets authors define tokens, parse nodes, and generate diagnostics without touching the compiler’s guts.

- **Safe Mode Extensibility:** If you stick to `meta.dev`, you get standard diagnostics and guaranteed IR compliance. If you go rogue and touch the Kernel, the `Provenance IR` tags your code, and you are 100% on the hook for any instability.

#### The Workflow: The "Defendant's" Responsibility

1. **Declare Effects:** Every library defines its footprint using `meta.dev`.
2. **Enumerate Conflicts:** The LSP tracks these effects and lists the collision points.
3. **Construct the Bridge:** The author writes an `interplay` block. This is the "defense case"—the formal logic proving that the composition is safe.
4. **Verification:** The kernel runs the proof via a CTFE engine. If it holds, the code compiles. If there’s a hole in the argument, the compiler points to the exact line where the contract was violated.

#### The Implementation Plan

We’re transpiling to C now to get the kernel stable, then moving to **Cranelift**. From there, the entire standard library gets rebuilt on that IR. It’s a monumental task, but this is the only way to get true composability without the nightmare of hidden interactions or undefined behavior.

If anyone is capable of creating this, it’s me. I’m not interested in "moving fast and breaking things." I’m interested in building correctly and ensuring it _never_ breaks.
