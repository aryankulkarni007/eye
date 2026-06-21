# Current capabilities

What Eye compiles and runs today, and the mechanism behind each feature. This is
the present-tense companion to [FARFUTURE.md](planning/FARFUTURE.md) (long-term vision)
and [VISION.md](design/VISION.md) (kernel/stdlib thesis): everything below is built and
tested in the tree, not aspirational. For the version-by-version ledger see
[FUTURE.md](planning/FUTURE.md); for what is deliberately absent see [KERNEL.md](design/KERNEL.md)
and [DEFER.md](planning/DEFER.md).

Status anchor: verified against the working tree on 2026-06-10. Not all of this
is committed or tagged. NOTE (2026-06-21): this is a dated snapshot and now lags
the tree - it predates the typeck split S2-S6, let-from-init inference, Rust-style
FFI (variadic / opaque types), and LSP hover. For authoritative current status see
[TYPECK.md](../features/TYPECK.md), [FFI.md](../features/FFI.md), and the
[ledger](../planning/ledger.md); this doc needs a full refresh.

## What Eye is

A small, statically-typed systems language. Source transpiles to C and links
through `clang` to a native binary. The runtime is raw and machine-level (no GC,
no borrow checker, no hidden allocation); the correctness work happens at compile
time in the diagnostics layer.

## Pipeline

```
.eye source -> lexer -> rowan CST -> typed AST -> HIR -> MIR -> C -> clang -> native binary
```

The MIR stage is live: codegen lowers HIR to a resolved mid-level IR
([MIR.md](features/MIR.md)) and then mechanically prints MIR to C. The older HIR-walk
emitter is deleted, so the C backend no longer walks HIR directly.

Each stage is a focused crate, modelled on the rust-analyzer architecture:

| Crate | Role |
|-------|------|
| `token` | Static token kinds, `T![...]` macro |
| `lexer` | Logos lexer, interner, source-text helpers |
| `syntax` | `SyntaxKind` + rowan-typed nodes/tokens (lossless CST) |
| `parser` | Pratt parser with error recovery |
| `ast` | Typed AST generated from `crates/ast/eye.ungram` |
| `hir` | Name resolution + arena HIR + all semantic diagnostics |
| `mir` | Resolved mid-level IR (the codegen input) |
| `codegen` | HIR -> MIR lowering, then MIR -> C |
| `diagnostics` | Shared diagnostic taxonomy (the 8 classes) |
| `lsp` | `eye-lsp` server (semantic tokens + parser diagnostics) |

Diagnostics are source-mapped at the lexer, parser, and HIR stages. The driver
hard-stops before codegen if any stage reported an error, so codegen only ever
sees a resolved, well-typed program.

## Language surface

### Items

- **Functions** in call-form: `name(params) -> Ret { ... }`. A void function
  omits the `->` arrow entirely. The last expression in a block is its value (no
  `return` needed), and `return expr;` / `return;` are also available.
- **Structs**: `structure Name { ty field, ... };`. Struct literals
  `Name { field: val, ... }`, including nested literals. Fields accessed with
  `.`, with one level of auto-deref through a reference.
- **Enums**: `enum Name = A | B | C;`. C-level tagless enums, flat variant index.
  Variants reached as `Name.Variant` or bare `Variant` when unambiguous.
- **Unions**: `union Name { ty field, ... };`. Overlapping storage, one field per
  literal.
- **FFI**: `extern { name(params) -> Ret; ... }`. The linker binds the symbols;
  `ptr` lowers to `void*`.
- **`const`**: `const Type NAME = expr;` at the top level. A const is a *value*,
  not storage: it inlines at every use and has no address (`&NAME` is illegal).
  The initializer is a bounded const-expr fold (literals, the full operator set,
  const-of-const with cycle detection, numeric / `as` casts). A `usize` const is
  usable as an array length (`[T; SIZE]`, the A6 case). Scalar-only, top-level
  only ([CONST.md](features/CONST.md)).
- **Globals**: top-level `let` / `mut` bindings are addressable static storage -
  the storage half of the value/storage split, distinct from `const`'s value
  half. The initializer must be const-evaluable; `let` is read-only, `mut` is
  writable (immutable-by-default enforced), `&G` is legal, and they emit as
  file-scope C statics.

### Types

- **Machine integers**: `int8`..`int64`, `uint8`..`uint64`, `usize` / `isize`
  (platform width). Integer literals are decimal, or carry a base prefix:
  `0x`/`0X` hex, `0b`/`0B` binary, `0o`/`0O` octal. The value is parsed in HIR
  and emitted in decimal, so C never sees the prefix.
- **Floats**: `float32` -> C `float`, `float64` -> C `double`. Float literals and
  arithmetic; `%f` printing.
- **`bool`** with `true` / `false`.
- **Pointers / references**: `T*` and `&T`; address-of `&` and deref `*`.
- **Fixed arrays**: `[T; N]` where `N` is an integer literal. Array literals
  `[a, b, c]`, index `base[i]` as rvalue and lvalue, multi-dimensional. Arrays
  are **value types**: they copy on init and assign, and pass and return by value
  (struct-wrap representation in C). `&[T; N]` is the no-copy reference and keeps
  its length. `len(x)` is a compile-time `usize` constant on `[T; N]` and
  `&[T; N]`.
- **Function pointers**: the type is `(A, B) -> R` (void target `(A)` with no
  arrow). A bare function name decays to a value of its signature (no `&`
  needed). Both direct calls `f(x)` and indirect calls through a binding work.
  Function pointers are usable as `let`/`mut` bindings, parameters, return
  values, struct fields, and array elements ([FNPTR.md](features/FNPTR.md)).
- **String literals**: a string literal is `&[uint8; N]` - a reference to a
  NUL-terminated byte static, reusing the array machine (`len`, indexing, OOB).
  `println` renders it `%s`; escape sequences decode to bytes, so `N` is the
  decoded length; `char` is `uint8`. A `&[T; N]` decays to `&T` (a pointer cast)
  at `let`-init, argument, and return position, so strings pass to functions and
  FFI (`extern strlen(string s)` works). Length-erased slices `&[T]` stay stdlib
  ([STRING.md](features/STRING.md)).

### Bindings

- `let` (immutable) and `mut` (mutable), with an explicit type:
  `let int32 x = 0;`. Initialization is mandatory (valid-by-construction; there
  is no `null` literal and no uninitialized binding).
- **Immutable by default**: assigning to a `let` binding is rejected
  (`TypeError::AssignToImmutable`), including through a field or index projection
  rooted in it (`p.f = v` on a `let`-bound `p`). A write through a pointer
  (`*p = v`) is the raw-pointer escape and is not tracked. Parameters are
  mutable for now (no `mut`-parameter syntax yet). Full rules in [MUT.md](features/MUT.md).
- Type inference for untyped `let` is **on hiatus** by design until the kernel
  surface stabilizes. An untyped `let` / `mut` is **rejected**
  (`TypeError::MissingTypeAnnotation`, T025) rather than emitting a placeholder -
  a placeholder reached codegen as an `Error` type (`void* /* ERROR TY */`) and
  only `clang` caught it. The annotation is mandatory until inference lands.
- **Struct destructuring** in a `let`: `let Point { x, y } = p;` binds every field
  (exhaustive / irrefutable), with field rename (`x: px`), nested struct values,
  and call-result initializers (spilled to a temp, then projected). Struct
  destructuring in match arms is not yet built. This is the chosen kernel
  destructure primitive, distinct from refutable sum-type matching
  ([MATCH.md](features/MATCH.md)).

### Control flow

- `if` / `else`.
- `loop` / `break` / `continue` (the only loop primitive; `while` / `for` are
  deliberately stdlib-derivable, not kernel).
- `match` over enums, ints, chars, and bools: literal patterns work (`1`, `'a'`,
  `true`), exhaustiveness checking over bool/enum (int/char require `_`),
  duplicate-arm and unreachable-after-wildcard diagnostics, and guards
  (`pat if expr -> body`). Kernel match is a minimal tag-dispatch skeleton - no
  payloads, or-patterns, range arms, or struct-patterns-in-match-arm yet (the B2
  seam reserves these for the future macro engine; see
  [KERNEL.md](design/KERNEL.md)). Irrefutable struct destructuring is built in
  `let` position only (see Bindings).
- `return expr;` / `return;` (early return).

### Expressions

- **Value-position `if` and `match`**: a conditional or match can be the value of
  a `let`, a function argument, an operand, or a return tail. A value-position
  `match` has its arm types checked against one result type. A value-position
  `if` must yield a value on every path: an else-less (or nested else-less) `if`
  in a `let` init / `return` / tail is rejected (`VoidValueInValuePosition`).
  Cross-branch result-type consistency for `if` (e.g. `if c { 5 } else { true }`)
  is not yet enforced - that is part of the typeck pass. Arbitrarily nested
  value-position expressions lower correctly (the MIR cutover declares a temp and
  assigns per branch, so the old one-level hoist limit is gone; the acid test is
  `eyesrc/programs/wierd.eye`).
- **Operators**: arithmetic (`+ - * / %`, `%` integer-only), bitwise
  (`& | ^ << >> ~`), comparison (`== != < > <= >=`), logical (`&& || !`),
  assignment (`=`) and every compound form (`+= -= *= /= %= &= |= ^= <<= >>=`),
  each desugaring to `a = a <op> b`.
- **Casts**: `expr as Type` (C cast semantics).
- **Grouping**: `( expr )` as a precedence escape hatch; lowers transparently.

### Intrinsics

- **`println("fmt", args...)`**: lowers to one `printf`. Each `{}` placeholder is
  replaced by a conversion specifier chosen from the corresponding argument's HIR
  type; the arguments forward in order; a trailing newline is always appended.
  Literal `%` is escaped to `%%`; UTF-8 in the format string is preserved.
  Compound args (whole array / struct / union) are rejected. Recognition is by
  name, so a user-defined `println` shadows it. (`println` is the sole print
  intrinsic today - there is no bare `print`; the vision moves it to the stdlib
  once variadic FFI lands. [PRINT.md](features/PRINT.md))
- **`sizeof(T)`**: compile-time `usize` byte size of a named type (builtin,
  struct, union, or enum). Lowers to C `sizeof(ctype)`; no Eye layout model.
  Compound-type args (`sizeof(&T)`, `sizeof([T; N])`) are deferred
  ([SIZEOF.md](features/SIZEOF.md)).
- **`len(x)`**: compile-time array length, above.

## Correctness guarantees

Eye's identity is a raw runtime with a strict compiler. The compiler refuses
ill-formed programs rather than emitting C that only `clang` would later reject.

### The 8-class diagnostic model

Every diagnostic carries a `Class` (`crates/diagnostics`): `Lex`, `Syntax`,
`Grammar`, `Resolve`, `Type`, `Pattern`, `Const`, `Unsupported`. HIR populates
the four semantic classes:

- **Resolve** (`R`): duplicate items, unknown/mismatched enum variants, enum or
  struct type name used in value position, and use of an undeclared name. The
  last one is load-bearing: because MIR is a *resolved* IR, an unknown identifier
  is a hard `UnresolvedName` error rather than text passed verbatim to C, which
  is what the old HIR-walk backend did (an undeclared `printf` used to reach the
  linker).
- **Type** (`T`): let/match-arm/return type mismatches, return-arity errors
  (value in a void function, missing value in a typed one), `%` on a float,
  binary op on an array, calling a non-function value, a parameterized `main`,
  struct-literal missing/unknown fields, union literal field count, recursive
  value types, and assignment to an immutable binding.
- **Pattern** (`P`): non-exhaustive match, duplicate arm, unreachable arm, guard on irrefutable arm.
- **Const** (`C`): array length not a literal / integer / zero / too large,
  literal out-of-bounds index, negative index, and the const-expr fold failures -
  non-const initializer, const-of-const cycle, unknown name in a const, const
  division by zero, taking `&` of a const, and assigning to a const.

### No-footgun rules

Where C is silently dangerous, Eye picks the least-surprising rule even when it
diverges from C ([FUTURE.md](planning/FUTURE.md) v0.6):

- Precedence is Rust-style, not C-style: every bitwise op binds tighter than
  comparison, so `a & b == c` is `(a & b) == c`.
- Comparison is non-associative: `a < b < c` is a parse error.
- Assignment in an `if` condition (`if x = 5`) is an error; use `==`.
- Struct literals must name every field; missing or unknown fields are errors.
- Bindings are immutable by default; mutation needs `mut` ([MUT.md](features/MUT.md)).

### Value-recursion check and type topology

C requires a type to be declared before it is embedded by value. Codegen runs an
**object-topology pass** ([TOPOLOGY.md](features/TOPOLOGY.md)): a shared dependency graph
(`crates/hir/src/core/typegraph.rs`) classifies each type edge as a hard edge
(embedded by value) or a soft edge (behind a pointer/reference). Types are
emitted as forward-declared named tags, then defined in topological order of the
hard edges. The same graph drives an HIR check that rejects genuinely
infinite-size types (`structure A { A a }`, mutual, or through an array) as
`RecursiveValueType`, while pointer cycles (linked lists, trees, self-reference,
`&[Self; N]`) compile. The C wrapper structs, named tags, and ordering are
backend artifacts, not language concepts.

## What is deliberately not here

These are conscious exclusions, not gaps. Kept out of the unoverwriteable kernel
because the vision derives them in the stdlib via supermacros, or because their
design surface is not yet open:

- `while` / `for`, payload/sum-type enums, generics, OOP/vtables, owned strings,
  slices `&[T]` (length-erased fat pointers), `Vec`/`Option`/`Result`/iterators.
- Type inference for untyped `let` (on hiatus).
- Runtime bounds traps and lifetime/escape analysis (both blocked on Eye having
  no abort/panic mechanism and no runtime length; one later safety theme).
- Variadic `extern ...`, opaque named FFI pointer types (`FILE*`), dropping the
  auto-`#include`, and evicting `println` to the stdlib. These are the remaining
  kernel-completion items ([KERNEL.md](design/KERNEL.md)); `const`, globals,
  `sizeof`, and first-class string literals all landed 2026-06-06 and are
  documented above. `[value; N]` repeat-array literals are built in the working
  tree (`eyesrc/lang/array_fill.eye`). `alignof` is not built.
- Multi-file modules, a separate typecheck pass (checks live in lowering),
  optimizations, incremental compilation, non-C backends.

## Where to look

| Want | File |
|------|------|
| Run a program | `cargo run -- eyesrc/<dir>/<file>.eye` (README has dump flags) |
| Sample programs | `eyesrc/lang/` (per-feature), `eyesrc/programs/` (`physics.eye`, `mandlebrot.eye`, `wierd.eye`, ...), `eyesrc/ffi/`. `./eyesrc/check_all.sh` compiles all of them. |
| MIR design | [MIR.md](features/MIR.md) |
| Type emission order | [TOPOLOGY.md](features/TOPOLOGY.md) |
| Function pointers | [FNPTR.md](features/FNPTR.md) |
| Arrays | [ARRAY.md](features/ARRAY.md) |
| Diagnostics design | [DIAGNOSTICS.md](features/DIAGNOSTICS.md) |
| Run all tests | `cargo test --workspace` (e2e + snapshot + proptest) |
| Property-based tests | `cargo test --test proptest` — 7 invariants over random input |
| Fuzz testing | `cargo fuzz run --fuzz-dir fuzz <target>` — 3 targets: `fuzz_lexer`, `fuzz_parser`, `fuzz_full` |
| CI pipeline | `.github/workflows/ci.yml` — cross-platform, lint, msrv, bench, fuzz, docs |
| Install from GitHub Releases | `scripts/install.sh` — `curl -fsSL https://raw.githubusercontent.com/anomalyco/eye/main/scripts/install.sh | sh` |
| What is left in the kernel | [KERNEL.md](design/KERNEL.md) |
| Deferral reasons | [DEFER.md](planning/DEFER.md) |
