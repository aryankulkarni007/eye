# MIR: Mid-Level IR

Status: DESIGN, NOT BUILT. This is Track 2 of `REDESIGN.md`. It is the high-risk,
high-value refactor and runs after the diagnostics track (Track 1). This
document is the handoff for a future session.

## Purpose

Today codegen walks the HIR `Body` directly and makes semantic decisions:
`hoist_matches` (`crates/codegen/src/core/matches.rs:92`) pulls value-position
matches into `_matchN` temps, and `check_unhoisted_matches`
(`crates/hir/src/core/lower/stmt.rs:275`) bans the positions codegen cannot
hoist. MIR removes both. A dedicated HIR -> MIR lowering pass flattens control
flow and generates temps; codegen becomes a dumb printer over MIR.

## The acid test (REDESIGN I3)

The refactor only earns its cost if MIR represents value-position matches in the
positions banned today, including nested in `if`, `loop`, and block branches.

Source that is rejected today:

```
let x = if cond { match e { A -> 1, B -> 2 } } else { 0 };
```

It is rejected because hoisting the match out of the `then` branch would execute
it even when `cond` is false, violating execution semantics. MIR lowers the
match in place, inside the branch, assigning a temp at the point of evaluation:

```
let t_x: i32                 // value of the if-expression
if cond {
    let t_m: i32             // match temp, declared in the branch
    switch e {
        A => t_m = 1
        B => t_m = 2
    }
    t_x = t_m
} else {
    t_x = 0
}
// uses of x become uses of t_x
```

This is why MIR succeeds where hoisting failed: hoisting tried to pull the match
*out* (wrong scope, wrong conditionality); MIR lowers it *in place* as a
statement sequence within the correct block. No ban is needed. Codegen prints
this structurally with no decisions.

## Core transformation

HIR -> MIR does two things:

1. Linearize. Every value-producing construct (match, if-as-value,
   block-as-value, nested calls) is evaluated into a temp by preceding
   statements. Operands are always trivial: a constant or a place. There are no
   value-bearing expressions nested inside other expressions. This is
   three-address form: `a + b * c` becomes `t0 = b * c; t1 = a + t0`.
2. Make control flow explicit but structured. `if`, `loop`, and `switch` are
   MIR statements holding nested blocks. Value-producing control flow has
   already been rewritten to: declare a temp, then a control-flow statement that
   assigns the temp in each branch.
3. Preserve evaluation semantics. Flattening must keep left-to-right evaluation
   order of side-effecting operands: `f() + g()` calls `f` before `g`, so it
   lowers to `t0 = f(); t1 = g(); t2 = t0 + t1`. And the short-circuit operators
   `&&` and `||` must NOT lower to `RValue::Binary` (its operands are both
   evaluated eagerly). They lower to control flow, the same in-place treatment
   as the match example: `cond && rhs` becomes a temp assigned `false` by
   default, then `if cond { <lower rhs into temp> }`. This is the operator half
   of the conditional-context problem NOTES.md flagged alongside the `if` half.

## Schema sketch

Lightweight and structured. Not SSA, not a basic-block CFG. The neutrality that
matters is in the value model (trivial operands, neutral ops and types), not in
the control-flow representation.

```rust
struct MirBody {
    locals: Vec<MirLocal>,   // source locals and generated temps, each typed
    body: MirBlock,
}
struct MirLocal { ty: Type, /* name/source-map for diagnostics */ }

struct MirBlock { stmts: Vec<MirStmt> }   // no tail; a tail became an assign

enum MirStmt {
    Let { local: LocalId, init: Option<RValue> },
    Assign { place: Place, value: RValue },
    Eval(RValue),                                  // for effect, e.g. a call
    If { cond: Operand, then: MirBlock, else_: Option<MirBlock> },
    Loop { body: MirBlock },
    Switch { scrut: Operand, arms: Vec<SwitchArm>, default: Option<MirBlock> },
    Break,
    Continue,
    Return(Option<Operand>),
}

struct SwitchArm { case: Case, body: MirBlock }    // Case = enum variant tag

enum RValue {
    Use(Operand),
    Binary(BinOp, Operand, Operand),   // arithmetic/comparison only; NOT && ||
    Unary(UnaryOp, Operand),
    Call { callee: Operand, args: Vec<Operand> },
    Ref(Place),
    Deref(Operand),
    Cast(Operand, Type),
    ArrayLit(Vec<Operand>),
    StructLit { ty: Type, fields: Vec<(Field, Operand)> },
}

enum Operand { Const(Literal), Copy(Place) }       // always trivial, never nested
enum Place {
    Local(LocalId),
    Field(Box<Place>, Field),
    Index(Box<Place>, Operand),
    Deref(Box<Place>),
}
```

Invariant of the schema: an `RValue`'s arguments are always `Operand`s, and an
`Operand` is always a constant or a place. No `RValue` nests another `RValue`.
That single rule is what makes codegen a mechanical walk.

## Codegen over MIR (the dumb printer)

Each MIR construct maps to one C form, no decisions:

- `Let` -> `T x;` or `T x = <rvalue>;`
- `Assign` -> `<place> = <rvalue>;`
- `If` -> `if (<cond>) { ... } else { ... }`
- `Loop` -> `while (1) { ... }`
- `Switch` -> `switch (<scrut>) { case <tag>: ...; break; ... }`
- `RValue::Binary` -> `a op b` (operands trivial, no recursion)
- `Return` -> `return <operand>;`

This replaces codegen's current HIR walk, `hoist_matches`, the `match_temps`
map, and the `collect_match_ids` boundary logic. All of it moves into lowering.

## The lowering pass

A builder over the HIR `Body`, mirroring the standard expression-to-operand
pattern (as in rustc THIR -> MIR). State: the current statement buffer for the
block being built, plus a temp counter.

- `lower_expr_to_operand(e) -> Operand`: emits any necessary statements into the
  current buffer and returns a trivial operand. For a value match, if, or block:
  allocate a temp, emit the control-flow statement that assigns the temp in each
  branch, return `Copy(temp)`.
- `lower_block(block) -> MirBlock`: lowers statements in order; a block tail
  becomes an assignment to the enclosing temp.
- Statement-position match (value discarded) lowers to a `Switch` with no temp.

## Types and the inference gap (open decision)

MIR locals and temps carry a `Type`, sourced from the HIR side table
`expr_types` (`crates/hir/src/core/body.rs:26`), which is partial today. Codegen
currently handles a missing type with an `int32_t` fallback
(`matches.rs:104`). The question is what MIR does when a type is absent.

Carrying that fallback into MIR is wrong for two reasons:

1. It violates I2. A type guess is a decision, and MIR is supposed to make none.
2. It is a safety regression. I3 unbans value-matches in more positions, so the
   fallback fires more often. Programs that are rejected today would become
   silently miscompiled tomorrow. Reject becoming miscompile contradicts the
   DESIGN.md pessimism principle ("if it is not explicitly proven, it is not
   valid").

Recommended (aligned with the project's pessimism): an undeterminable type is a
`Type`-class diagnostic raised at type checking, not a guess. MIR then assumes
every type is present and stays total without guessing. The fallback is deleted,
not enshrined. This is a typeck/MIR boundary decision and is also recorded in
the Track 3 type-check doc and in `DIAGNOSTICS.md` (Type class). Confirm before
building MIR, because it determines whether MIR may assume complete types.

## Invariants restated

- I2 Total back half. `lower_body` and codegen are total. Any well-typed HIR
  lowers to valid MIR; any MIR emits valid C. Neither rejects, neither emits
  diagnostics.
- I3 Acid test. The schema represents the currently-banned nested value-matches
  (see worked example). When MIR lands, `check_unhoisted_matches` and the
  `Unsupported::UnhoistedMatch` diagnostic (see `DIAGNOSTICS.md`) are deleted.
- I4 Neutrality. The value model is target-neutral: trivial operands, neutral
  ops, neutral `Type` (reuse `TypeRef`). No C-isms (no fallthrough semantics, no
  C-specific type quirks) leak into MIR.
- I5 Evaluation order. Flattening preserves left-to-right evaluation of
  side-effecting operands, and `&&` / `||` lower to control flow rather than
  `RValue::Binary`, so short-circuiting is preserved. See Core transformation
  point 3.

## Open decisions for next session

- Control-flow representation: this document recommends structured statements
  with explicit temps (lightweight, C-friendly, solves the acid test). The
  alternative is a basic-block CFG with terminators (what an optimizer and a
  future Cranelift backend prefer). Reconciliation: going from structured MIR to
  a CFG later is a mechanical, lossless pass, because the hard work (flattening
  and temp generation) is already done and shared. So structured-now does not
  trap us, provided no C-isms leak (I4). Confirm this trade before building.
- The NOTES.md idea of multiple MIRs (one per backend) is deferred. Build one
  MIR now with a clean HIR -> MIR seam. A second backend, if it comes, either
  consumes this MIR or lowers it further to a CFG; decide then.
- Reuse vs new type for `Type`. Likely reuse HIR `TypeRef` to avoid a parallel
  type system. Confirm during build.
- Where MIR lives: a new `mir` crate between `hir` and `codegen`, or a module
  inside `codegen`. A separate crate matches the layering and keeps codegen a
  pure consumer.
```
