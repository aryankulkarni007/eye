# Kernel-completeness gap analysis

When can we call the Eye kernel "complete"? The kernel is **unoverwriteable**
([VISION.md](VISION.md)): anything in it is permanent, no deprecation path. So
"complete" does not mean "has many features" - it means **every primitive a
supermacro provably cannot synthesize is present, and nothing else is**.

The discriminating test (from VISION.md): *a feature belongs in the kernel iff a
supermacro provably cannot synthesize it.* This doc audits the current surface
against that test and lists what stands between today and a freezable kernel.

Status legend: BUILT / MISSING / PARTIAL. The base audit was taken against the
tree on 2026-06-04; rows landed since then carry their own date (const, sizeof,
globals, and string literals all built 2026-06-06). Verified, not copied from
prose.

## What the kernel already has (BUILT)

functions, calls, `structure`, fields, raw data pointers (`&T`, `T*`, `&`, `*`),
`if`/`else`, `loop`/`break`/`continue`, `match` (enum, no payloads), machine
ints (`int8..int64`, `uint8..uint64`, `usize`/`isize`), `as` casts, `union`,
arrays (`[T; N]`, `&[T; N]`, `len`), the operator set (arithmetic, bitwise,
comparison, logical, `+=`/`-=`), and FFI `extern`. Floats are also built - see
below; the prose docs that called them out-of-scope were stale.

## Genuinely-missing kernel substrate

A macro cannot fake these. They are the gap.

| Item | Status | Why it is kernel | Notes |
|------|--------|------------------|-------|
| **Function pointers** | BUILT (2026-06-05) | A code address is the code-side analog of a raw data pointer; macros can't manufacture it. This is the substrate vtables / iterators / callbacks bottom out on - the OOP-stdlib vision needs it. | `(A, B) -> R` function type; a function name decays to a value of its signature (`FnAsValue` removed); direct + indirect (`op(x)`) calls; function pointers as `let`/param/return/struct-field/array-element. Built on the object-topology pass. Non-callable calls rejected (`CallNonFunction`). [FNPTR.md](features/FNPTR.md), [TOPOLOGY.md](features/TOPOLOGY.md). |
| **`sizeof` / layout intrinsic** | BUILT 2026-06-06 | `malloc(n * sizeof(T))` cannot be written without it, and a macro can't compute a type's size/alignment portably. This is to containers what function pointers are to vtables. | `sizeof(T)` is a compile-time `usize` in the `len` mold (callee-name-sniffed, user-shadowable). Leans on the C backend: lowers to `sizeof(ctype)`, no Eye layout model. Floor = bare named type (builtin/struct/union/enum); compound types (`sizeof(&T)`, `sizeof([T;N])`) and the sizeof-tainted const-expr path are deferred ([DEFER.md](planning/DEFER.md)). `alignof` not built (optional). [SIZEOF.md](features/SIZEOF.md), [HORIZON0.md](HORIZON0.md) C2. |
| **Variadic `extern` (`...`)** | BUILT 2026-06-11 | FFI is the kernel's machine seam; a C ABI seam that can't express variadics is incomplete. Unblocks `printf` and the `bubblesort`/`file` corpus programs. | `...` as the last entry of an extern signature (extern-only, needs one named param first - both parser-rejected otherwise); `Function::variadic`; the prototype gains `, ...` and calls pass extra trailing operands unchanged. [FFI.md](features/FFI.md). |
| **Opaque / named FFI pointer types** (`FILE*`) | BUILT 2026-06-11 | Same seam: `fopen`/`fgets` need a `FILE*`-typed value. | `extern { type FILE; }` declares an opaque type: a forward typedef (`typedef struct FILE FILE;`), no definition, legal behind `*`/`&` only (value-position use is a C incomplete-type error until typeck). The auto-`#include <stdio.h>` is DROPPED: the extern block is the sole prototype; `println` (still an intrinsic) auto-supplies `int printf(const char *, ...);` when no user `printf` is declared. Restored `bubblesort`/`file`. [FFI.md](features/FFI.md). |

## Chosen ergonomic primitive (not strictly irreducible, but the natural core)

| Item | Status | Honest framing |
|------|--------|----------------|
| **Early return** (`return expr;` / `return;`) | BUILT (2026-06-04) | Strictly, `return x` is subsumable by labeled-break-with-value out of the function-body block - so it is not "provably unsynthesizable" the way function pointers are. But Eye has neither labeled break nor break-with-value, and `return` is the more natural control-flow primitive, a peer of `if` / `loop` / `break`. Adopted as a *chosen* kernel primitive, not an irreducible one. Now parses, lowers (HIR `Expr::Return` -> MIR `Return`), and emits; three return-arity diagnostics guard the clang-error cases (value in a void fn, missing value in a typed fn, wrong type); value-position return diverges correctly. Restored `floodfill`. |

## Substrate the vision leans on but has not built

| Item | Status | Why it matters |
|------|--------|----------------|
| **Compile-time const** | BUILT 2026-06-06 | `const <type> <name> = <expr>;` at the top level, with a bounded const-expr fold (literals, the operator set, const-of-const cycle-checked, numeric casts). A const is a *value* (inlined, no address); A6 const-length arrays now resolve. Scalar-only, top-level-only floor ([CONST.md](features/CONST.md), [HORIZON0.md](HORIZON0.md) Component 1). |
| **Top-level / global storage** | BUILT 2026-06-06 | Addressable static data - the *storage* half of the value/storage split, distinct from `const` (the value half). Top-level `let`/`mut` globals are built ([HORIZON0.md](HORIZON0.md) C3, Part A): const-evaluable initializer, `let` read-only / `mut` writable (immutable-by-default enforced), `&G` legal, emitted as file-scope C statics (`eyesrc/lang/global.eye`). **String literals (Part B) remain DESIGNED** ([STRING.md](features/STRING.md)): `&[uint8;N]`, with the length-polymorphism/decay resolution that keeps slices in stdlib. |
| **String literals as first-class values** | BUILT 2026-06-06 | A string literal is `&[uint8; N]` (`eyesrc/lang/string.eye`, `eyesrc/ffi/caesar.eye`, [STRING.md](features/STRING.md)): a reference to a NUL-terminated byte static, reusing the array machine (`len`, indexing, OOB). `print` renders it `%s` (closing the old `%d` bug); escapes decode to bytes so `N` is the decoded count; char = uint8. **Decay built**: a `&[T; N]` decays to `&T`/`string` at let-init / arg / return (a pointer cast), so strings pass to functions and FFI (`extern strlen(string s)` works). DEFERRED: empty-string storage (`&[uint8; 0]` hits the zero-length-array rule); embedded-`\0` truncates `strlen`/`%s`. |

## Subtractive: what must leave the kernel

| Item | Why | Blocked on |
|------|-----|------------|
| **`println` intrinsic** | The vision puts printing in the stdlib (compose `printf` via `eeye`), not the kernel ([ledger.md](planning/ledger.md)). Kernel-complete means `println` is *not* in the kernel. The old `print` intrinsic is already gone; `println` remains. | RECLASSIFIED 2026-06-11. The prerequisites (string literals, variadic FFI) are built - a user program can call `printf` directly today ([FFI.md](features/FFI.md)), so the intrinsic is now sugar over a reachable primitive, not load-bearing. That satisfies the subtractive criterion (deletable without losing expressive power) even though it is not deleted. Deleting it *now* is rejected: `{}` formatting is type-directed at codegen (`spec_for_type` picks the C conversion from the argument's type), and hand-written `%` specifiers are exactly the wrong-specifier/wrong-width UB that selection prevents. No Eye-level replacement can exist yet - an Eye function has no variadics, no generics, no comptime - so a prelude alone cannot host `{}` formatting; the real receiver is the prime layer ([PRIME.md](features/PRIME.md), [HORIZON0.md](HORIZON0.md) Component 5 update). Does not block the freeze. |

## The decision that gates freezing the kernel - RESOLVED: B2 (extensible)

**Match extensibility (VISION.md hinge 1).** Resolved 2026-06-04 in favour of
**B2, extensible match**. Rationale: B1 (rich kernel match) would bake sum types
into the unoverwriteable kernel forever - the conventional "be Rust" choice, but
the one that abandons Eye's composable-core thesis. "Make Eye its own language"
means *that* thesis (stdlib supermacros), not built-in ADTs, so B2 is the
identity-aligned call.

What the decision **binds today is thin and negative**: kernel `match` keeps a
closed pattern set (enum variant, int/char/bool literal, wildcard `_`, binding,
guard, and irrefutable struct destructure in `let`) - a clean lowering seam that
future stdlib pattern registration can hook with new forms. No payload/sum-type
patterns or or-patterns are baked in. The positive half (stdlib *registers* pattern lowerings)
is inert until the registration engine exists, so in the near term B2 and B3 are
behaviourally identical; the only live difference is the standing commitment not
to grow kernel match. The concrete deliverable of "decided" is therefore a
design sketch of (a) the minimal kernel-match skeleton and (b) the seam, not new
runtime behaviour. The macro-engine *timing* (when registration becomes real) is
a separate fork - see the bootstrap hinge below.

### The seam: Option A (chosen) - AST-level desugaring

The question is *where* the extensibility seam lives. Two candidates:

| Option | Seam location | Tradeoff |
|--------|---------------|----------|
| **A (CHOSEN)** | AST → HIR lowering boundary | Macro engine rewrites extension patterns (`Some(val)`, struct patterns) into kernel `Pat::Variant` / `Pat::Literal` / `Pat::Wildcard` / `Pat::Bind` before HIR lowering sees them. HIR/MIR/codegen remain total: they see only the closed set of pattern forms and make no decisions. |
| **B (rejected)** | MIR `dyn ArmTest` trait objects | Extensions register `impl ArmTest` that codegen calls to decide emission. Extension code gains privilege to decide what C codegen emits - violates I2 (codegen must be a mechanical walk). |

**Option A preserves I2** - the MIR `SwitchArm` / `Case` / `ArmTest` stays a closed
enum (see [`MIR.md`](features/MIR.md)), and codegen stays an exhaustive `match` over it with
no trait-object dispatch. Extension patterns hit the kernel only as already-known
kernel forms; the kernel never opens a dynamic seam. This is the same architectural
boundary as the `if`/`loop` lowering: the macro engine can rewrite `while` to
`loop`+`if`+`break`, but `loop` itself stays a closed MIR statement.

## Explicitly NOT kernel - resist adding

All stdlib-derivable via supermacros; adding them to the kernel would violate the
subtractive thesis: `while` / `for` (over `loop`+`if`+`break`), payload enums /
sum types, generics, OOP / vtables, `Vec` / `Option` / `Result` / iterators,
owned strings, slices `&[T]` (length-erased fat pointer). Convenience control
flow - break-with-value, labeled break - is low-priority and derivable; defer
([FUTURE.md](planning/FUTURE.md) Fork D).

## Basic surface gaps (grammar audit 2026-06-05)

A grammar/token audit found small primitives that were thin or unenforced -
distinct from the strategic kernel items above. Three shipped in v0.8
([FUTURE.md](planning/FUTURE.md)); the rest are deferred, none strategic.

**Shipped:**

- **Integer literal base prefixes** - `0x`/`0b`/`0o`. The lexer was decimal-only;
  a systems language with the full bitwise operator set needs hex literals.
- **Compound assignment completeness** - only `+=`/`-=` existed; the other eight
  forms (`*= /= %= &= |= ^= <<= >>=`) now desugar to `a = a <op> b`.
- **Immutable-by-default enforcement** - `let`/`mut` keywords and the `mutable`
  flag already existed but nothing enforced them; assigning a `let` binding is now
  a `T` diagnostic ([MUT.md](features/MUT.md)).

**Deferred (small, none strategic):**

- Digit separators in integer literals (`1_000_000`).
- Float exponent form (`1e9`, `1.5e-3`) and a regex fix: `[0-9]+(\.[0-9]+)+`
  lets `1.2.3` lex as one float token; it should be one optional fractional part.
- `mut` parameters - parameters are mutable for now; immutable-by-default
  parameters need a `mut` marker in the grammar ([MUT.md](features/MUT.md)).
- Range patterns in `match` (`match n { 1..5 -> .. }`) - a design call
  tied to the B2 kernel-match skeleton seam (above). Int/char/bool literal
  patterns already built (S1, 2026-06-06).
- Explicit enum discriminant values (`A = 5`) - needed to match a C enum's ABI
  across the FFI seam; couples to the C-seam plumbing work.

## Separate axis: runtime safety (deferred, not expressiveness)

Bounds traps (abort on dynamic OOB) and escape / lifetime analysis (dangling
`&local`) are part of a "complete" language in a *safety* sense but are
orthogonal to kernel expressiveness. Both are blocked on Eye having no abort /
panic mechanism and no runtime length; likely one later theme ([DEFER.md](planning/DEFER.md)).

## Rough edges found while auditing (not missing features)

- **Float `%`** - FIXED 2026-06-04. `5.5 % 2.0` used to reach the C backend and
  fail as `double % double`. Now caught in HIR as `TypeError::ModuloOnFloat`
  (T001), in the F1/F2/F3 no-footgun mold: `%` is integer-only, rejected on a
  float operand with a clear diagnostic (hint: `fmod`). Mirrors binary-op-on-array.
- **Float doc drift.** Floats (`float32`->`float`, `float64`->`double`, float
  literals, arithmetic, `%f` printing) are built and run, but FUTURE.md's v0.4
  type row and v0.6 modulo line called them out-of-scope. Corrected.

## Sequencing - identity first, C-seam lazy

Re-sorted 2026-06-04 by *whose language each item serves*. An earlier draft put
the FFI/C-seam bundle ahead of the identity substrate; that was building a clean
C transpiler, not Eye. The FFI items are still kernel (the kernel bottoms out at
the machine via FFI) - they are just **low-identity**, so they are sequenced
lazily, not demoted.

**Eye-identity substrate (the real kernel-completion work):**

1. **Early return** - BUILT 2026-06-04. Pure Eye control flow, orthogonal to
   everything else; shipped independently of the strategy. Restored `floodfill`.
2. **Function pointers** - Eye's first-class code values. The substrate Eye's own
   vtables / iterators / OOP stdlib are built on. Hand-written for now; no macro
   engine required to make them useful.
3. **Const / comptime** - the foundation the eventual supermacro engine stands on
   (generics = comptime + AST instantiation). Also unblocks A6 const arrays.
4. **Minimal `match` skeleton + lowering seam** - the concrete deliverable of the
   B2 resolution above. Keep kernel match minimal; design the seam.

**C-seam plumbing (necessary kernel, low identity - do lazily, minimally):**

5. Variadic `extern ...`, opaque FFI pointer types, drop the auto-`#include`,
   evict the `print` intrinsic to (proto-)stdlib. Unblocks `bubblesort`/`file`.
   Real, but it is C-bridge polish - it does not make Eye more itself.

Floats are done bar the now-fixed `%` guard. `while`/`for`/generics/containers
stay out (stdlib via supermacros).

## The other gate: bootstrap hinge (VISION.md hinge 2) - RESOLVED: far-future

Resolved 2026-06-04: the supermacro / pattern-registration engine stays
**far-future (~v10, the vision default)**. Near-term identity work ships as
**kernel primitives**, and any vtable / iterator / sum-type usage is **hand-written**
until the engine arrives. No premature mega-project; the engine automates the
hand-written patterns later.

Consequence for this roadmap: the identity substrate (early return, function
pointers, const/comptime, minimal match) is built directly into the kernel now -
none of it waits on a macro engine. The B2 match seam is *designed* now but its
registration half stays inert (as above) until the far-future engine. A small,
conscious cost is accepted: a few things that will eventually be stdlib live as
kernel primitives in the interim - acceptable because they are genuine machine
substrate (fn pointers, const) rather than derivable containers.
