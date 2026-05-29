- [ ] remove all type errors. this should be a type checker error class
- [ ] refactor error handling to be more robust
      i am dissatisfied with what we have now - errors should be partition by class. Codegen shouldn't emit type errors for example

- [ ] so really i am kinda approaching things wrong. we should be hoisting in the hir not in codegen. that is what happening. the hir is pure and the codegen is making the decisions. this is incorrect. codegen should simply be a translation black box

- [ ] if we commit to this refactor, I would want the HIR to be pure and for there to be one true HIR for eye. that is it. we may have mutliple MIRs -> one for c codegen, one for cranelift backend. with translation machinery in between layers. That architecture makes the most sense.

- [ ] consider error handling overhaul

GEMINI DUMP ->

Here is a concise architectural briefing summarizing the findings, structural diagnoses, and the verified path forward for the **Eye Compiler**.

---

## Architectural Audit Briefing: Eye Compiler (v0.5)

### 1. Executive Summary

The Eye Compiler has successfully achieved a full vertical slice at ~10K lines of code, demonstrating elegant syntax, robust copy-on-assignment array semantics via C-struct wrapping, and a working implementation of statement/value-position `match` expressions.

However, development has hit a structural velocity bottleneck due to an inconsistent **partition of labor** between the compiler layers. To achieve an objectively correct, maintainable base that can support future features (type inference, payload enums, nested structs), the compiler must transition from ad-hoc backend manipulation to a formalized, backend-agnostic pipeline.

---

### 2. Core Diagnostic Findings

#### A. Leaky Backend Abstractions (The "Hoisting" Problem)

- **The Issue:** The C-codegen backend is currently forced to make semantic decisions, such as determining how and when to hoist value-producing expressions (like `match`) into temporary variables.
- **The Risk:** This approach fails when a value-match is nested inside conditional contexts (e.g., `if` expressions or short-circuiting operators), where unconditional hoisting violates language execution semantics. It also makes the code generation layer fragile and complex.

#### B. AST/HIR Mutation During Type Checking

- **The Issue:** High-Intermediate Representation (HIR) structural manipulation and rewriting are occurring before or alongside type validation.
- **The Risk:** This creates a chicken-and-egg dependency. The compiler attempts to restructure complex entities (like arrays inside structs or nested unions) before their types and memory layouts are fully frozen, resulting in severe design oversights.

#### C. Verification of Strategy on Type Inference

- **The Finding:** Deferring full type inference until the core kernel is structurally sound remains a **highly tactical and correct decision** for a solo developer. The current top-down (bidirectional) checking is sufficient, provided the downstream compiler layers treat type annotations as opaque data slots.

---

### 3. The Target Architecture: HIR $\rightarrow$ MIR $\rightarrow$ Codegen

To resolve the structural debt permanently, the compiler layout must explicitly separate _language semantics_ from _target translation_ by introducing a lightweight **Mid-Level Intermediate Representation (MIR)**.

```
[Raw AST] ──> [Typed HIR] ─────────────> [C-Like MIR] ─────────────────> [C Codegen]
                 │                           │                              │
         (Type Checker)             (HIR Lowering Pass)              (Emitter Black Box)
       *Strictly Read-Only* *Flattens Control Flow* *No Logic / No Decisions*
       *Stamps Type Slots* *Generates Match Temps* *Loops over MIR & Prints*
                                  *Registers Struct Layouts*

```

#### Layer Responsibilities:

1. **High-Level IR (HIR):** Pure, stable, and completely backend-agnostic. It represents the language's core syntax and features. The **Type Checker** interacts with this layer exclusively as a _read-only validator_, filling static type slots without altering the tree structure.
2. **Mid-Level IR (MIR):** The imperative bridge. A dedicated lowering pass transforms the pure HIR into a flat, linearized sequence of target-friendly primitives (e.g., variable declarations, explicit switches, flat assignments, jumps). **All variable hoisting and desugaring occur here.**
3. **Codegen (Backend):** A completely dumb translation black box. It accepts a linearized sequence of MIR instructions and mechanically translates them directly into raw C text.

---

### 4. Action Plan for the Coding Sabbatical

- **Step 1: Centralize Diagnostics (Immediate Quick-Win)**
  Extract all inline panics and ad-hoc error strings out of the core logic. Define a single, flat `TypeError` / `Diagnostic` enum. Force the type checker to populate an error accumulator rather than halting mid-pass.
- **Step 2: Enforce the "Read-Only" Type Checker Rule**
  Audit the 10K lines to ensure the type-checking phase never mutates the topology of the AST. It should only read the tree and stamp metadata onto a designated type slot per node.
- **Step 3: Sketch the Lightweight MIR Schema**
  Define a minimal, linear instruction set (e.g., basic three-address statements or explicit switch trees) that handles C-like structures. Migrate the hoisting logic out of the C emitter and into this new HIR-to-MIR lowering bridge.

By correcting these boundaries now at 10K lines, the Eye Compiler establishes a pristine foundation capable of scaling into type inference, tagged unions, and beyond without architectural regression.
