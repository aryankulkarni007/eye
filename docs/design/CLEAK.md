# C-leak audit: implicit type decisions in the pipeline

Status: audit performed 2026-06-11 against the uncommitted tree; fix-order
step 3 (coercion-point unification + companions) BUILT later the same day
(305 tests green). Every row marked VERIFIED was reproduced with a minimal
program that day; rows marked INSPECTION were found by reading the code and
have no reproducer yet. This document is the ground-truth ledger for the
harden-before-freeze pass: the kernel freeze and the typeck split are blocked
on the rows below being fixed or explicitly accepted.

Scope: every site where the pipeline chooses a C type, emits a name, or
converts a value without a type judgment. Sources read end-to-end:
`crates/hir/src/core/lower/{expr,stmt,types,collect}.rs`,
`crates/mir/src/lower.rs`, `crates/codegen/src/core/{mir_emit,types}.rs`.

Detection infrastructure added with this audit:

- `scripts/check-c-strict.sh`: compiles every corpus `.eye`, then
  syntax-checks the generated C under
  `-std=c11 -pedantic-errors -Wall -Wextra -Werror` (unused-variable family
  suppressed, each suppression documented in the script). CI job `corpus`
  runs it plus `check_all.sh`.
- `eyesrc/check_all.sh` gained an XFAIL list (linkedlist = intentional,
  lang = the decay bug) so it is CI-runnable; a stale XFAIL fails the run.

## Classification

- **M (miscompile)**: Eye accepts, clang accepts, the binary computes wrong
  values. Worst class.
- **L (C-leak)**: Eye accepts, clang errors. The no-footgun contract says Eye
  must reject these itself.
- **P (pedantic)**: Eye accepts, clang accepts by default, rejected under the
  strict gate or formally undefined.
- **T (typeck-required)**: the correct fix is a type judgment that belongs in
  the Horizon 1 typeck pass; patching it into lowering would be the wrong
  layer.

## M: miscompiles

| id | finding | status |
|----|---------|--------|
| M1 | Integer literal out of the annotated type's range emits the raw decimal: `let int32 x = 5000000000;` builds and stores 705032704 (clang warns, `-Wconstant-conversion`, not an error). VERIFIED. Root: literals are typed `int32` unconditionally (`types.rs:literal_type`) and never range-checked against the declared type. | FIXED 2026-06-11 (T030): a literal at any coercion site adopts the expected integer type; a single post-lowering sweep (`check_int_literal_ranges`) checks every integer literal's value against the type it ended up with, including the bare `int32` default. Negated literals check the negative bound. usize/isize ranges assume the LP64 targets the backend supports |
| M1b | Same literal through `println`: `println("{}", 5000000000)` emits `printf("%d", 5000000000)`; the argument is C `long`, the spec is `%d` - varargs UB. VERIFIED. | FIXED 2026-06-11: falls out of the M1 sweep - a bare literal stays `int32` and is checked against that, so the `%d`/`long` pair can no longer be emitted |
| M2 | Mixed-width arithmetic narrows: a binary expression takes the LHS type (`expr.rs` "simplification until full inference"), so `(7 - (current_addr & mask))` with `usize` operands types `int32` from the literal `7`, and the MIR temp truncates the C `size_t` result to 32 bits (lang.eye `align_alloc`). VERIFIED 2026-06-11 (lang.eye audit). Also asymmetric: `x + 7` types as `x`, `7 + x` types as `int32`. | OPEN, T: needs an operand-unification rule (widest / annotated target), this is typeck's first real customer |
| M3 | Exhaustive value-position match emitted an `if`/`else if` chain with no `else`: the hoist temp stayed uninitialized on the rogue-value path (enum from a bad FFI cast), and clang flagged `-Wsometimes-uninitialized`. VERIFIED via strict gate (wierd.c, calculator.c, match_prim.c). | FIXED 2026-06-11: a switch with no default (HIR proved it exhaustive) emits its last arm as the chain's `else` |
| M4 | Positional struct literal silently dropped its values: `Point { 1, 2 }` lowered to a literal with NO fields, emitted `(Point){ }`, and printed `0 0` - built clean, ran wrong. VERIFIED 2026-06-11 (found while building the coercion point: lowering carries fields by name only, and the nameless values were skipped). | FIXED 2026-06-11 (T031): positional fields are rejected; struct literals are named-only until positional initialization is designed |

## L: C-leaks (Eye accepts, clang errors)

| id | finding | status |
|----|---------|--------|
| L1 | String decay missing at struct-literal field init: `Syllable { str: "cvc" }` puts the wrapper-pointer cast into a `const char*` field. VERIFIED (lang.eye compile blocker). Decay exists at 4 sites (let-init / call arg / return / tail); struct-lit fields are a missing 5th. | FIXED 2026-06-11: `LoweringCtx::coerce` (crates/hir/src/core/lower/coerce.rs) is the single coercion point - array-literal re-typing, integer-literal typing, decay - applied at all six sites: let init, call arg, explicit return, fn tail, struct-lit field, array-lit element |
| L2 | Same decay gap at array-literal elements: `let [char*; 3] xs = ["a","b","c"]`. VERIFIED (lang.eye audit). The `[ptr; N]` workaround compiles only because C converts any `T*` to `void*`. | FIXED 2026-06-11: array-literal elements recurse through the full `coerce`, so the decay rewraps each element and is written back into the literal |
| L3 | Call arity unchecked: `add(1, 2, 3)` and `add(1)` against a 2-param function both reach clang ("too many arguments"). VERIFIED. The arg-coercion loop runs over `min(args, params)` and nothing checks the count. | FIXED 2026-06-11 (T026): exact count for a defined fn, minimum (named params) for a variadic extern. Indirect calls through a fn-pointer value stay unchecked: `TypeKind::Fn` carries no variadic flag, so an exact check would falsely reject a pointer to a variadic extern - typeck's fn-type redesign picks this up. Arg *types* stay T |
| L4 | Array-literal element types unchecked: `let [int32; 3] xs = [1, true, "x"]` - the string element is a clang error, and the `true` converts silently (a footgun in itself). VERIFIED. The literal is typed from its first element only. | PARTIAL 2026-06-11: elements now go through `coerce` (literal typing, decay), so homogeneous-but-defaulted literals are sound; the cross-element *type check* (`true`/`"x"` against `int32`) is a type judgment and stays T - the string still reaches clang, the bool still converts |
| L5 | Unknown struct name in a struct literal: `Foo { x: 1 }` with no `Foo` declared emits `(Foo){ .x = 1 }` - "use of undeclared identifier". VERIFIED. The literal's type is interned as `Path("Foo")` with no existence check. | FIXED 2026-06-11 (R011): the literal's name must be a declared struct or union |
| L6 | Undeclared field type leaks: `structure Arena { off off, }` emits `off off;` - "unknown type name". VERIFIED (lang.eye audit). No type-name resolution pass exists. | FIXED 2026-06-11 (R012): post-collect pass validates every Path name in item signatures (fields, params, returns, globals, consts) against primitives + declared items, span-anchored on the type node; body sites (`let` annotations, casts, local consts) check eagerly. `sizeof` args stay exempt (lean-on-C layout authority, SIZEOF.md). Forward references still resolve - validation runs after all items are collected |
| L7 | Indexing a `ptr` (C `void*`): `p[0]` emits `p_0[0]` - "operand of type 'void' where arithmetic or pointer type is required". VERIFIED. | FIXED 2026-06-11 (T027). Sibling found and fixed the same day: *dereferencing* `ptr` leaked a void indirection (`-Wvoid-ptr-dereference`) - rejected too (T028) |
| L8 | C-keyword names emitted verbatim: field `.struct = ...`, parameter `switch`, function `typedef`, etc. VERIFIED. | FIXED 2026-06-11: R010 `NameIsCKeyword` rejects at collect for every name the backend emits verbatim (item, field, parameter, enum variant, global, opaque type). Extern parameter names exempt (prototypes are types-only) |
| L9 | Zero-parameter functions emitted as `T f()` (unprototyped, deprecated) instead of `T f(void)`. VERIFIED via strict gate (every corpus file). | FIXED 2026-06-11: `comma_params` emits `void` |
| L10 | Empty string emitted `uint8_t data[0]` - a zero-length array is a GCC/clang extension, rejected under `-pedantic-errors`. VERIFIED. | FIXED 2026-06-11: storage pads to `data[1]`; type-level length stays 0; only `""` can produce it (`[T; 0]` is rejected upstream) |
| L11 | `%p` formatting: a `&Struct` argument was passed to `%p` without a `void*` cast (formally UB, `-Wformat-pedantic`), and a `ptr` value fell to the `%d` default spec (varargs UB). VERIFIED via strict gate (print.c). | FIXED 2026-06-11: `spec_for_type` maps `ptr` to `%p`; `gen_println_value` casts ref/ptr/fn-ptr arguments to `(void*)` |

## P: pedantic / strict-gate-only

| id | finding | status |
|----|---------|--------|
| P1 | `ptr + int` emits `void*` arithmetic - a GNU extension, compiles by default, rejected under `-pedantic-errors`. VERIFIED. The strict gate cannot see it today because the corpus does not exercise it after Eye compilation succeeds (the repro used a cast that warns instead). | DECIDED + FIXED 2026-06-11 (T029): arithmetic/bitwise on `ptr` is rejected (no element size to scale by; the no-footgun rule beats C compatibility). Comparisons stay allowed. Typed pointers (`T*`) keep C arithmetic semantics |
| P2 | A string static is emitted per unique literal even when `println` inlines the literal into the format string, leaving the static unreferenced (`-Wunused-const-variable`, suppressed in the gate). Dead bytes in every binary with a `println` literal. INSPECTION + gate evidence. | OPEN, emit statics only for literals referenced as values |

## T: typeck-required (recorded for Horizon 1 scoping)

No fixes here until the typeck pass exists; patching these into lowering is
the wrong layer. Each is a concrete requirement for the pass design:

- Struct-literal field **value** types unchecked (`P { x: "hello" }` with
  `int32 x` reaches clang). lang.eye audit, VERIFIED.
- Call argument **types** unchecked (swapped args accepted). lang.eye audit.
- `as` casts unrestricted any-to-any; the cast lattice is a design item.
- `const` declared type vs folded value unchecked (DEFER row).
- Binary-expression typing: LHS-type rule (M2) replaced by unification.
- Integer-literal typing: `int32` default (M1) replaced by expected-type
  propagation with range check.
- `mir_type_of` fallback: a missing `expr_types` entry silently types a temp
  `int32` (`mir/lower.rs`). Measured never to fire on the corpus, but it is
  the silent amplifier under every typing gap above; typeck flips it to a
  hard error.
- `types_compatible` integer-family leniency (any int matches any int) masks
  real mismatches in match arms and returns; needed today because of the
  `int32` literal default, removable with it.
- Assignment expressions type as their RHS, not the target (INSPECTION;
  assignments in value position are rare).
- Duality of `ptr` (`Path("ptr")`, opaque `void*`) vs `Ptr(inner)` (`T*`)
  appears at every type dispatch; typeck should give `ptr` a real
  representation instead of a magic path name.

## Latent / edge findings (INSPECTION, unverified, low priority)

- Local-name mangling edge: parameters keep their bare source name while
  locals get `name_id` suffixes; a parameter literally named `x_3` can
  collide with a local `x` whose MIR id is 3.
- A user-defined (non-extern) Eye function named `printf` suppresses the
  emitter's own prototype while `println` still calls `printf` - the call
  would hit the Eye function with C-string arguments.
- Non-ASCII char literals emit multibyte C char constants
  (implementation-defined); known print-UTF-8 gap.
- `&` of a non-place expression spills to a temp and takes the temp's
  address silently (`&(a + b)` is accepted); ratify or reject.
- `println` format parsing: `{` without `}` passes through; there is no
  escape for a literal `{}`.
- A guarded switch whose unguarded arms already cover the domain (guarded
  duplicates first) still has an uninitialized-temp corner: the flag chain
  has no default and C cannot prove the flag gets set. Needs S5 usefulness
  analysis or default synthesis.
- Enum values accept arithmetic (`A + B` compiles as C int arithmetic);
  decide whether enums are ordinal or opaque.

## Fix order (agreed 2026-06-11, harden-before-freeze)

1. DONE: detection infrastructure (strict gate, CI corpus job, XFAIL list).
2. DONE: mechanical fixes M3, L8, L9, L10, L11.
3. DONE 2026-06-11: coercion-point unification - `LoweringCtx::coerce`
   (lower/coerce.rs) applied at all six expected-type sites. Closed L1, L2,
   L4's literal half; companions L3, L5, L6, L7, M1/M1b, P1, plus the M4
   positional-struct-lit miscompile and the deref-of-`ptr` leak found while
   building it. lang.eye's original blocker is gone (it compiled and ran);
   it has since grown a `const [char*; 24]` and is XFAIL again on the
   scalar-only const floor (DEFER), not on any C-leak.
4. NEXT: typeck split (Horizon 1), scoped by the T section above; lang.eye
   plus this ledger's reproducers become the regression corpus.
5. THEN: match S4/S5 on the typed pipeline; freeze last; lang.eye compiling
   and running clean is the freeze acceptance test.
