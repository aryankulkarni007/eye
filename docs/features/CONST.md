# `const` in the Eye kernel

**Status: built 2026-06-06, verified end to end (`eyesrc/lang/const.eye`, e2e and HIR
unit tests).** This is Horizon 0, Component 1 ([HORIZON0.md](design/HORIZON0.md)). The
deferred pieces of the ratified design are listed at the end and in
[DEFER.md](planning/DEFER.md).

## The thesis

A `const` is a compile-time **value**, not storage. It has no guaranteed address
(`&const` is illegal), so a reference to it is **inlined** to its folded scalar
rather than read from a C symbol. This is the value/storage split the design
draws: `const` is a value; a top-level `let`/`mut` (addressable static data,
Component 3, not yet built) is storage with an address.

```
const int32   MAX  = 100;          -- a compile-time value
const int32   DBL  = MAX * 2;      -- a const-expr may reference other consts
const float64 TAU  = 3.14 * 2.0;   -- the operator set folds
const usize   SIZE = 4;            -- usable as an array length (A6)
```

`const` is **not** compile-time execution. The initializer is a bounded
const-expr fold, not CTFE: it does not run functions at compile time. That
(generics, the prime VM, the macro engine) is the far-future prime layer
([PRIME.md](PRIME.md)).

## Surface

`const <type> <name> = <expr>;` at the top level, or as a statement inside a
block (added 2026-06-11). The type is always explicit (no inference at the
floor). Grammar: a `const_def` item arm and a `block()` statement arm
(`crates/parser/src/grammar.rs`), the `const` keyword token, and one AST
`ConstDef` node for both positions (`crates/ast/eye.ungram`).

### Block-scope `const`

A `const` statement has the same semantics as the top-level form - a folded
value, inlined at every reference, `&` and assignment rejected, usable as an
array length - scoped to its declaring block, with inner-block shadowing like a
`let`. Differences from the top-level pass:

- The initializer folds *during body lowering*, at the declaration site,
  against the consts visible there: top-level consts plus enclosing local
  consts. Only strictly-earlier declarations are visible, so a local const
  cycle is impossible (a self-reference resolves to an outer binding or is
  `ConstUnknownName`).
- The folded value lives in `Body::local_consts` (the module-level `Const`
  arena sits behind `&HIR` and cannot grow during body lowering); the name
  resolves through the lexical scope stack to `Resolution::LocalConst`.
- A runtime local in the initializer is rejected (`ConstUnknownName` - it is
  not a const), and a runtime local *shadowing* a const hides it from
  const-exprs the same way it hides it from name resolution.
- The visible-const lookup is the `ConstEnv` trait
  (`crates/hir/src/core/lower/const_eval.rs`): the top-level passes use the
  finished name -> value map, body lowering layers the scopes over it
  (`ScopedConsts`). `lower_type_ref` / `array_len` take the same trait, so
  `let [int32; N] xs` works with a local `N`.
- The declaration itself emits nothing in MIR (`Stmt::Const` is skipped);
  references inline the value through the same path as top-level consts.

Exercised by `eyesrc/lang/const.eye` and the `local_const_*` e2e tests
(scoping, shadowing, array length, negative spill, rejections).

## The bounded const-expr fold

`crates/hir/src/core/lower/const_eval.rs` folds a deliberately narrow expression
subset to a scalar `ConstValue` (`Int(i128)`, `Float(f64)`, `Bool`, `Char`):

- integer / float / bool / char **literals** (scalar only - no aggregates);
- the **operator set** (arithmetic, bitwise, comparison, logical) over operands
  of matching kind - no implicit numeric promotion, matching the explicit-cast
  rule;
- **references to other consts**, resolved by memoized, cycle-checked recursion;
- a numeric **`as` cast** between scalar kinds.

It rejects, as a `ConstError` (class `C`): a definition cycle (`ConstCycle`), a
name that is not a const (`ConstUnknownName`), a non-const operation such as a
function call (`NotAConstExpr`), and integer division by zero (`ConstDivByZero`).
A failed fold leaves the const's `value == None` (poison) after a diagnostic, so
later lowering never folds an un-diagnosed bad const.

## Pipeline

`const` threads through the existing pipeline rather than adding a stage:

| Stage | What `const` adds |
|-------|-------------------|
| token / syntax / grammar / AST | `const` keyword, `ConstDef` node + `Item` arm |
| HIR items | `Const { name, ty, value }` arena, `ItemScope.consts`, `Resolution::Const` |
| lowering order | pass **1a** `collect_consts` then pass **1.5a** `eval_consts`, both *before* `collect_items`, so an item's array length can read a folded const |
| HIR expr | a const reference is a value (`expr_type` = the declared type); `&const` and `const = ..` are rejected |
| MIR | `Resolution::Const` inlines the folded value: a non-negative scalar is a trivial constant operand; a negative integer spills its unary-negation rvalue to a temp (literals are unsigned) |
| codegen | none - the emitter only ever sees `Operand::Const`; no `static const` symbol is emitted |

### Why the value inlines instead of emitting a C symbol

Both were on the table ([HORIZON0.md](design/HORIZON0.md): "folds to its value, or emits
its C symbol"). Inlining is chosen because it *is* the ratified semantics: a
const is a value with no address, and a value with no address is substituted, not
stored. A named `static const` symbol would give the const an address - that is
the *globals* representation (Component 3). It also matches Rust (`const` inlines;
`static` is addressable). The evaluator must produce values regardless (array
lengths need a `u64`, const-of-const needs folding), so inlining is also the
smaller build.

## A6: const-length arrays

`const` unblocks `[T; N]` with a const `N` (the last outstanding v0.7 array
deliverable, [ARRAY.md](ARRAY.md), [DEFER.md](planning/DEFER.md)). `array_len`
(`crates/hir/src/core/lower/types.rs`) folds the length slot against the
evaluated const map: a bare integer literal takes the fast path; anything else is
folded as a const-expr, so `[int32; SIZE]` and `[int32; SIZE * 2]` both resolve.
A runtime local as a length is rejected (`ConstUnknownName` - it is not a const).

## Deferred from the floor

Ratified but not in this build (see [DEFER.md](planning/DEFER.md)):

- ~~**Local (block-scope) `const`**~~ - built 2026-06-11, see "Block-scope
  `const`" above.
- **Aggregate const values** (`const [int32; 3] xs = [1,2,3]`) - scalar-only
  floor; an addressable aggregate is the globals primitive (Component 3).
- **`const` type/value checking** (`const bool B = 5`) - the fold is lenient like
  the rest of the pre-inference front end; revisit with the typeck split.
- **`sizeof`-tainted const-expr** (`const usize N = sizeof(T)`) - needs Component
  2 (`sizeof`); a sizeof-tainted expr cannot fold to an Eye value and must emit a
  C constant expression unfolded.
