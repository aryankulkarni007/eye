# Deferral ledger

Things consciously deferred - not bugs, not oversights, decisions to do later.
Each row records what was deferred, why, and the condition that would bring it
back. Add a row whenever a "not now / out of scope / later" call is made.

A deferral here is a ratified choice. If a feature is missing because it was
never considered, that belongs in a limitations table, not here.

## Open deferrals

| Item | Deferred from | Why | Revisit when |
|------|---------------|-----|--------------|
| Runtime bounds traps (abort on dynamic out-of-bounds index) | v0.7 | Introduces Eye's first runtime-error/abort concept - its own design surface, larger than arrays. Language not mature enough. | A runtime-error/panic mechanism is designed deliberately, post-v0.7. |
| Escape / lifetime analysis (returning `&local`) | v0.7 | Returning a reference to a stack local (`&a` of a local array or struct) makes a dangling pointer. clang warns, but Eye does not catch it - there is no borrow/lifetime analysis, which is a large separate design surface. | A lifetime/borrow analysis is designed, likely alongside the runtime-safety theme. |
| Slices `&[T]` (length-erased dynamic view) | v0.7 | A slice is a fat pointer `{T* ptr; usize len}` carrying a runtime length - a container, which the vision puts in stdlib, not the kernel. `&[T; N]` (static length) covers the in-kernel reference need. | Supermacro/stdlib container work begins. |
| ~~Arrays as struct/union fields~~ RESOLVED 2026-06-05 | v0.7 | Was: the array wrapper typedef emitted after the nominal types, so a struct field of array type referenced an undeclared type. Now the codegen prelude is the object-topology pass ([TOPOLOGY.md](features/TOPOLOGY.md)): one dependency-ordered emission with named-tag forward declarations. Arrays (and unions, nested structs, self-referential pointers, `&[Self; N]`) as struct fields all work; value-recursive types are rejected with `RecursiveValueType`. | Done. |
| `let` type inference (T1) | v0.5 | Deferred until the kernel surface stops moving; untyped `let` still emits a placeholder by design. Also gates the residual `int32` match-temp fallback. | The language is stable. |
| Array literal element-type checking (`[1, true]`) | v0.7 | A heterogeneous literal is silently accepted (the type comes from the first element). A correct check needs coercion rules - e.g. `[x, 1]` with `x: usize` and an int-literal `1` must coerce - so it depends on the deferred typecheck pass, not a standalone element-uniformity test. Tied to [T1]. | The separate typecheck pass / inference (T1) lands. |
| ~~Two corpus programs unsupported: `bubblesort`/`file` (undeclared libc)~~ RESOLVED 2026-06-11 | Track 2 / Segment 5 | Was: the MIR cutover closed the unresolved-name accident (`ResolveError::UnresolvedName`), leaving programs that called undeclared libc rejected - they needed variadic `printf` and `FILE*`-typed `fopen`/`fgets`, which Eye could not declare. Now Rust-style FFI is built ([FFI.md](../features/FFI.md)): variadic externs (`...`, extern-only, last position), opaque FFI types (`extern { type FILE; }` -> forward typedef), and the auto-`#include <stdio.h>` is dropped - the extern block is the sole prototype; `println` (still an intrinsic, eviction is post-typeck) auto-supplies a `printf` prototype when the program declares none. Both programs compile and run; `bubblesort_runs` + the C-seam e2e tests cover them. (`floodfill` was restored 2026-06-04 by early-return; `raytracer` already declared its externs.) | Done. |

## In-scope but not yet built

Not deferrals - still targeted for the version named, just not done.

| Item | Version | Why last | Needs |
|------|---------|----------|-------|
| ~~Const / named-length arrays `[T; N_const]` (A6)~~ RESOLVED 2026-06-06 | v0.7 | Was: length had to be an integer literal. Now `const` (Horizon 0, Component 1) is built, so a fixed-array length may be a `const` reference or a const-expr over consts (`[int32; SIZE]`, `[int32; SIZE * 2]`). `array_len` folds the length against the evaluated const map ([CONST.md](features/CONST.md)). | Done. |

## const sub-deferrals (Horizon 0, Component 1)

`const` shipped 2026-06-06 as a top-level, scalar-only floor ([CONST.md](features/CONST.md),
[HORIZON0.md](design/HORIZON0.md)). These pieces of its ratified design are deferred:

| Item | Deferred from | Why | Revisit when |
|------|---------------|-----|--------------|
| ~~Local (block-scope) `const`~~ RESOLVED 2026-06-11 | H0 C1 | Was: the const-value map was built top-level, before bodies. Now `const` is also a statement (a `block()` grammar arm, the same `ConstDef` node): the initializer folds during body lowering against the consts visible at the declaration (top-level plus enclosing local consts, a `ConstEnv` lookup layered over the lexical scopes), the folded value lives in `Body::local_consts`, and a reference inlines it exactly like a top-level const - `&`/assignment rejected, drives array lengths, lexically scoped with shadowing ([CONST.md](features/CONST.md)). | Done. |
| Aggregate const values (`const [int32; 3] xs = [1,2,3]`) | H0 C1 | The floor is scalar-only ([HORIZON0.md](design/HORIZON0.md)): `ConstValue` holds a scalar. An addressable aggregate is the *globals* primitive (Component 3, a top-level `let`), not a const value. | Component 3 (addressable static data) lands. |
| `const` type/value checking (`const bool B = 5`) | H0 C1 | The fold is lenient like the rest of the pre-inference front end: the declared type is recorded but the folded value is not checked against it. | The typeck split (Horizon 1) lands. |
| `sizeof`-tainted const-expr (`const usize N = sizeof(T)`) | H0 C1 | `sizeof` (Component 2) is now built, but const still *inlines* its folded value and emits no C symbol. A sizeof-tainted const-expr cannot fold to an Eye value, so it must emit a named C constant expression (`static const size_t N = sizeof(ctype);`) - the const-symbol path. Today `sizeof` in a const-expr is rejected as `NotAConstExpr`. | const grows a sizeof-tainted symbol path (small, on top of Components 1 + 2). |

## sizeof sub-deferrals (Horizon 0, Component 2)

`sizeof` shipped 2026-06-06 as a named-type-only `usize` intrinsic leaning on the
C backend ([SIZEOF.md](features/SIZEOF.md), [HORIZON0.md](design/HORIZON0.md)). Deferred:

| Item | Deferred from | Why | Revisit when |
|------|---------------|-----|--------------|
| Compound-type arguments (`sizeof(&T)`, `sizeof([T; N])`, `sizeof(T*)`) | H0 C2 | The intrinsic reads its argument from the AST as a bare name; a compound type needs type-in-argument parsing. Rejected with `SizeofNotAType`. No floor container math requires them. | A container needs the size of a pointer/array type directly. |
| `alignof` | H0 C2 | Same mold (emit C `_Alignof`); optional ([HORIZON0.md](design/HORIZON0.md)). | A container needs alignment. |

## Match (Horizon 0, Component 4)

S0 (seam refactor), S1 (literal patterns + int/char/bool domains), and S2 (`let`
struct destructuring + whole-value bare-ident binding) shipped 2026-06-06
([HORIZON0.md](design/HORIZON0.md), [MATCH.md](features/MATCH.md)). Deferred:

| Item | Deferred from | Why | Revisit when |
|------|---------------|-----|--------------|
| Ignore / throw-away destructure fields (`Point { x, .. }`) | H0 C4 / S2 | Destructuring is exhaustive at the floor (binds every field) by decision; partial binding needs a `..`/`_` field-ignore surface. | When partial destructure is wanted. |
| Struct patterns *in match arms* (`match p { Point { x, y } -> .. }`) | H0 C4 / S2 | Needs the scrutinee as a *place* for field projection + guard nesting; bundled with guards. `let` destructure (a place already) shipped; match arms wait. | S3 (guards). |
| Nested destructure patterns (`Point { a: Inner { .. } }`) | H0 C4 / S2 | The floor binds each field to a name (shorthand or rename); a nested sub-pattern is recursive structure not needed for "bind every field over several lines". | When nested destructure is wanted. |

The S1 literal deferrals remain:

| Item | Deferred from | Why | Revisit when |
|------|---------------|-----|--------------|
| Duplicate / unreachable **literal** arms (`match n { 1 -> .., 1 -> .. }`) | H0 C4 / S1 | Enum duplicate arms error today (`DuplicateArm`); literal dups do not yet. Redundancy/usefulness analysis over literals (and later ranges/or-patterns) is the S5 pass, not S1. | S5 exhaustiveness/usefulness pass. |
| Out-of-range literal arm (`match n8 { 256 -> .. }`) | H0 C4 / S1 | Range-fit of a literal against the scrutinee's integer width needs the same usefulness machinery. Silently accepted today (C compares the widened value). | S5, with range-coverage. |
| Negative integer literal patterns (`match n { -1 -> .. }`) | H0 C4 / S1 | Int literals are unsigned (`u128`) in HIR; a leading `-` in pattern position needs grammar + signed handling. Not needed for the S1 domains. | When signed-literal patterns are wanted. |

## Notes

- Nested value-position `match`/`if` (one as another's branch tail value) was
  **resolved**, not deferred onward: the Track 2 MIR cutover lowers it in place
  (`mir::lower::lower_into` declares a temp and assigns it per branch), so the
  old one-level hoist limit and its `/* UNHOISTED ... */` marker are gone. The
  acid test (`eyesrc/programs/wierd.eye`) compiles and runs by default. See
  [MIR.md](features/MIR.md).
- The array-wrapper mangle injectivity hole (`&int` once collided with a user
  type named `ref_int`) was **fixed**, not deferred: the mangle now
  length-prefixes type names. See `crates/codegen/src/core/arrays.rs` and its
  unit tests.
- Runtime safety and slices are linked: both are blocked on Eye having no
  runtime-length and no abort mechanism. They are likely a single later theme.
- Const-length arrays are the smallest of the three and the most likely to be
  pulled forward into a near-future version.
