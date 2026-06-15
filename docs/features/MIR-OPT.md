# MIR-OPT: optimization passes over structured MIR

**status: design. not built.** the compiler currently defers all optimization to
the C compiler (`cc -O2`). this document designs the first MIR-level
optimization pipeline — passes that run between HIR→MIR lowering and MIR→C
codegen — for the triple payoff of smaller generated C, faster compilation
through the C compiler, and genuine understanding of optimization headroom
before the Cranelift backend.

for grounding, read alongside:

- [MIR.md](MIR.md) — the MIR schema and lowering design
- [TYPECK.md](TYPECK.md) — typeck split (MIR reads `TypeckResults` for types)
- [performance.md](../performance.md) — current pipeline timing (~57 µs for 58-line program, MIR <1%)
- [MASTERPLAN.md](../planning/MASTERPLAN.md) — strategic horizon map
- [VFS.md](VFS.md) — the archiver build tool (MIR opts fit in the incremental build)

---

## why optimize MIR at all?

the compiler's performance budget today:

| stage | share (complex) | absolute (raytracer, 58 lines) |
|-------|----------------:|-------------------------------:|
| lex | 6.7% | 3.84 µs |
| parse | 35.1% | 19.96 µs |
| HIR lower | 40.6% | 23.12 µs |
| MIR lower | 0.8% | 0.45 µs |
| codegen + overhead | 16.8% | ~9.5 µs |
| **full pipeline** | **100%** | **56.9 µs** |

MIR lowering is noise (0.8%). adding passes after it will not slow the
frontend noticeably. the payoff is in the backend:

- **C compiler speed.** `cc -O2` on a 100-line generated C file takes ~50-200 ms.
  a MIR pass that runs in 1-10 µs and reduces C complexity by 20% directly
  reduces user wait time. the MIR opt replaces some of the C compiler's work
  with work the compiler controls — and MIR has type information the C
  compiler does not.
- **generated C quality.** dead stores, redundant temps, and never-taken
  branches in generated C are confusing to debug. MIR opts remove them before
  emission.
- **Cranelift readiness.** a Cranelift backend wants a CFG, and many of these
  passes are standard CFG passes that are easier to implement and verify over a
  flat representation. building them over structured MIR now (via the CFG
  conversion pass) means they drop into the Cranelift backend unchanged.
- **incremental build.** with the VFS archiver ([VFS.md](VFS.md)), cached MIR
  bodies are cheap to re-opt when nothing changed. the opt pass output can be
  cached alongside the MIR body.

### counterargument (dealt with)

> the C compiler (`cc -O2`) already optimizes the generated C. why duplicate
> that work?

because MIR has information the C compiler does not:

| information | in MIR | in generated C | C compiler sees |
|---|---|---|---|
| type of every expression | explicit `Type` | implicit (from C type system) | yes, after lowering |
| array length | `N` in `ArrayLit` | struct wrapper | yes, if not opaque |
| enum domain (discriminant range) | `Variant(ArmTest)` | `int` comparison | mostly |
| **signedness of operations** | from HIR type | C `int` default | no, if types match |
| **bounds of match scrutinee** | enum tag range | opaque int | no |
| **alias analysis** | places tracked | opaque C pointers | limited |
| **const-evaluated values** | `Operand::Const(Literal)` | inlined constant | yes, but C99 const |

the MIR optimizer can, for example, eliminate a match-arm dead branch because
it knows the scrutinee's enum domain; the C compiler sees an `if (x == 3 || x
== 4)` chain and has no proof that `x` cannot be `5`.

---

## architecture

### pass pipeline

```
HIR → MIR lowering
       │
       ▼
   ┌──────────┐
   │ canonical │  — normalize MIR (sort struct fields, rename temps,
   └──────────┘     simplify trivial constructs)
        │
        ▼
   ┌──────────┐
   │ simplify  │  — constant folding, copy propagation, dead code
   └──────────┘     elimination (can iterate)
        │
        ▼
   ┌──────────┐
   │ local opt │  — local value numbering, if simplification,
   └──────────┘     redundant let elimination
        │
        ▼
   ┌──────────┐
   │ CFG lift  │  — convert structured MIR to basic-block CFG
   └──────────┘     (optional: only for passes that need it)
        │
        ▼
   ┌──────────┐
   │ global    │  — loop invariant code motion, global value numbering
   └──────────┘     (over CFG, deferred to S2)
        │
        ▼
   ┌──────────┐
   │ CFG lower │  — convert CFG back to structured MIR (if CFG lift used)
   └──────────┘
        │
        ▼
   MIR → C codegen
```

the pipeline is a `Vec<Box<dyn MirPass>>`:

```rust
pub trait MirPass {
    /// descriptive name (for `--dump-mir-after=passname`)
    fn name(&self) -> &'static str;
    /// run the pass on one MIR body.
    /// the pass may mutate `body` in place.
    fn run(&self, body: &mut MirBody, types: &TypeInterner);
}
```

passes are composed by the driver:

```rust
pub fn optimize(
    bodies: &mut FxHashMap<FnId, MirBody>,
    types: &TypeInterner,
    passes: &[&dyn MirPass],
) {
    for pass in passes {
        for (_, body) in bodies.iter_mut() {
            pass.run(body, types);
        }
    }
}
```

each pass is independently testable: given a `MirBody`, run the pass, assert
the output is semantically equivalent and smaller/simpler.

---

## pass catalog

### S1 — early passes (structured MIR, no CFG conversion)

these passes operate on the existing structured MIR directly. they are cheap
and safe: they reduce code size without changing control flow structure.

#### P1: constant folding

```
input:
  let t0 = const 2;
  let t1 = const 3;
  let t2 = t0 + t1;
output:
  let t2 = const 5;
  // t0, t1 are dead; DCE removes their lets
```

fold `RValue::Binary(op, Const(a), Const(b))` → `Operand::Const(result)`.
fold `RValue::Unary(op, Const(a))` → `Operand::Const(result)`.
identity folds: `x + 0 → x`, `x * 1 → x`, `x - 0 → x`, `x / 1 → x`,
`true && x → x`, `false || x → x`.

**cost:** `O(exprs)` per body. trivial.

**status:** `+` lowering already folds primary consts at HIR time; this pass
catches what HIR folding missed (propagation through MIR temps).

#### P2: copy propagation

```
input:
  let t0 = x;
  use(t0);
  let t1 = t0;
  use(t1);
output:
  use(x);
  let t1 = x;   // after first propagate
  use(t1);      // after second propagate → use(x) (t1 dead)
```

replace `Operand::Copy(Place::Local(a))` with the RHS of `a`'s defining
`let` or `assign`, provided `a` is never assigned again between the
definition and this use. uses reaching-definition analysis (a local dataflow:
scan forward from each definition, track the last write to each local).

**cost:** `O(stmts * locals)` naive, `O(stmts)` with a linear scan.

**safety:** sound only for `let { local, init }` where `local` is never
`Assign`ed. MIR has no aliasing of `Place::Local` (each local is SSA-like
in practice), so this is safe without a full alias analysis.

#### P3: dead store elimination

```
input:
  let x = 1;
  let x = 2;
  use(x);
output:
  let x = 2;   // (first store dead)
  use(x);
```

remove `Assign { place, value }` when `place` is never read again (live
analysis: scan backward from each return/break, track which locals are
live). also remove `let x;` (no init) when `x` is never assigned or read.

**cost:** `O(stmts)` per body with a backward liveness scan.

#### P4: dead code elimination (block-level)

```
input (inside a block):
  return(Some(a));
  let x = f();
  use(x);
output:
  return(Some(a));
  // let x = f(); and use(x); removed
```

remove all statements after a terminator (`Return`, `Break`, `Continue`)
within the same block. this is already done in `lower.rs::terminated()` for
straight-line code but not after DCE/copy-prop creates new dead blocks.

**cost:** `O(stmts)` scan.

#### P5: redundant let elimination

```
input:
  let x = a + b;
  use(x);
output:
  let x = a + b;
  use(x);   // x used once → inline: use(a + b)
```

when a `let` has exactly one use (direct `Copy`), replace the use site
with the init expression and remove the `let`. careful: the init may have
side effects or be expensive; only inline trivial `RValue`s (`Use(op)`,
`Unary`, `Binary`) and only when the `let` dominates the use site.

**cost:** `O(stmts)` with a use-count pass.

#### P6: if simplification

```
input:
  if (const true) { then_block } else { else_block }
output:
  then_block   // else_block removed

input:
  if (const false) { then_block } else { else_block }
output:
  else_block   // then_block removed
```

fold `If { cond: Operand::Const(Literal::Bool(true)), ... }` and `false`.
also fold `if (const true)` nested in loops, etc.

**cost:** trivial.

#### P7: switch arm dead elimination

```
// scrutinee is enum E { A, B, C }
// arms: A → ..., B → ...
// C arm missing (dead code)
output: same (but after reachability analysis)
```

remove `MatchArm` entries for enum variants that do not exist (impossible —
HIR exhaustiveness already ensures this). more useful: remove arms after an
`Always` arm (catch-all):

```
switch x {
  A → ...
  _ → ...   // Always arm
  B → ...   // DEAD: after Always
}
```

remove arms past `Always`. the HIR already rejects `UnreachableAfterWildcard`,
so this is a defense-in-depth canonicalization.

**cost:** trivial.

### S2 — intermediate passes (require CFG conversion)

these passes need a flat basic-block representation. the CFG conversion pass
(P8) comes first.

#### P8: CFG conversion (structured → flat)

transform structured MIR to a basic-block CFG:

```
// structured:
if (cond) {
  block A
} else {
  block B
}
// after CFG:
block entry:
  br(cond, block_a, block_b)
block_a:
  block A
  br(block_merge)
block_b:
  block B
  br(block_merge)
block_merge:
  // continuation
```

`MirBlock` gains a new representation:

```rust
enum Terminator {
    Goto(BasicBlockId),
    Branch(Operand, BasicBlockId, BasicBlockId),
    Switch(Operand, Vec<(ArmTest, BasicBlockId)>, Option<BasicBlockId>),
    Return(Option<Operand>),
    Unreachable,
}

struct BasicBlock {
    stmts: Vec<MirStmt>,
    terminator: Terminator,
}

struct CfgBody {
    blocks: Vec<BasicBlock>,
    entry: BasicBlockId,
}
```

the conversion is mechanical: walk the structured block tree, create new
blocks for each `if`/`loop`/`switch` branch, emit terminators for each
`break`/`continue`/`return`. `loop` body → back-edge to loop header block.

**cost:** `O(blocks)` to construct. trivial.

**note:** P8 also implements the reverse (CFG → structured `MirBlock` tree)
by reconstructing structured control flow from the CFG. this is needed so
that passes that produce CFG can be lowered back to structured MIR for
codegen (which emits structured C `if`/`while`/`if-else-if`).

#### P9: unreachable block elimination (CFG)

```
// block A has no predecessors; it is unreachable
// remove it
```

scan the CFG from `entry`, mark reachable blocks, remove unreachable ones.
this catches dead code after P4 (which only removes statements within a
block, not entire blocks).

**cost:** `O(blocks + edges)` DFS.

#### P10: loop invariant code motion (CFG)

```
// loop body:
//   let t0 = x + y;    // x, y invariant
//   let t1 = t0 * a[i]; // t0 invariant
//   a[i] = t1;
// → hoist outside loop:
//   let t0 = x + y;
//   loop {
//     let t1 = t0 * a[i];
//     a[i] = t1;
//   }
```

identify natural loops from the CFG (back-edge detection). for each loop,
find statements whose operands are all invariant (defined outside the loop
and never modified inside). hoist them to the loop pre-header.

**cost:** `O(blocks * depth)` with standard loop analysis.

#### P11: global value numbering (CFG)

detect redundant computations across basic blocks. if `a + b` is computed in
two blocks and `a`, `b` have the same reaching definitions, replace the
second with a copy of the first.

**cost:** `O(stmts * dom_tree_depth)` — deferred, S2 or later.

#### P12: switch arm sorting (structured MIR)

```
// original: B, C, A (declaration order)
// sorted: A, B, C (enum declaration order)
// → better branch prediction, more C compiler optimization
```

reorder `Switch` arms to match the enum's declaration order. the C compiler
generates better code for `if-else-if` chains that follow the enum's natural
order (hint: likely variant first, etc.).

**cost:** trivial sort.

---

## codegen integration

### optimizing codegen

once MIR opts exist, the codegen emitter (`mir_emit.rs`) can be simplified:

- no need to generate dead stores (they are removed by P3)
- no need to handle `RValue::Use(Operand::Const(x))` in complex positions
  (P1 folds them)
- `switch` as `if-else-if` chain is already optimal after P12 sorting
- `Place::Index` bounds checks can be hoisted (deferred, P10)

### caching optimized MIR

the optimization pipeline is deterministic and pure (no mutable state, same
input → same output). optimized `MirBody` can be cached alongside the
lowered MIR in the salsa database:

```
lowered_file → MirBody
  → opt_passes → OptimizedMirBody (cached, keyed by MirBody hash)
  → c_code (reads optimized body)
```

when a body's HIR changes, the lowered MIR hash changes → opt re-runs. when
nothing changes, salsa cache hits. the VFS archiver ([VFS.md](VFS.md)) stores
the optimized MIR for incremental rebuild.

---

## verification

### semantic preservation

every pass must be semantics-preserving. verification strategy:

1. **unit tests:** for each pass, a set of `(input MirBody, expected output
   MirBody)` pairs. the test runner runs the pass, asserts structural equality
   of the output.

2. **e2e corpus:** run the full pipeline with and without optimization on every
   corpus program. assert that:
   - optimized and unoptimized binaries produce identical output for every input
   - optimized compilation is not slower than unoptimized (wall clock, `cc -O0`)
   - optimized C source is not larger than unoptimized

3. **determinism lock:** run the optimizer twice on the same input, assert
   bit-exact output. this catches any nondeterminism (hash iteration order,
   arena allocation order).

### safety invariants

- no pass may introduce new `LocalId`s or change the parameter order.
- no pass may remove a `Return` or `Break`/`Continue` that terminates a block.
- no pass may change the type of any `Operand`, `Place`, or `Local`.
- every pass must leave MIR well-formed (verified by a
  `assert_valid(&self)` method on `MirBody` that runs in debug builds).

---

## performance budget

| pass | estimated cost (58-line raytracer) | cumulative opt time |
|------|----------------------------------:|--------------------:|
| P1: constant folding | ~0.2 µs | 0.2 µs |
| P2: copy propagation | ~0.5 µs | 0.7 µs |
| P3: dead store elimination | ~0.3 µs | 1.0 µs |
| P4: dead code elimination | ~0.1 µs | 1.1 µs |
| P5: redundant let elimination | ~0.3 µs | 1.4 µs |
| P6: if simplification | ~0.1 µs | 1.5 µs |
| P7: switch arm dead elim | ~0.1 µs | 1.6 µs |
| P8: CFG conversion | ~1.0 µs | 2.6 µs |
| P9: unreachable block elim | ~0.5 µs | 3.1 µs |
| P10: loop invariant code motion | ~3.0 µs | 6.1 µs |
| CFG → structured lowering | ~1.0 µs | 7.1 µs |

total optimization budget: **<10 µs** for a 58-line program. this is 12% of
the current full pipeline (57 µs) and replaces 50-200 ms of `cc -O2` work.
the payoff increases with file size: for a 1000-line file, MIR opts grow
sub-linearly (~50-100 µs) while `cc -O2` grows linearly (500-2000 ms).

---

## build plan

### S1 — early passes (structured MIR only, no CFG)

| item | effort | delivers |
|---|---|---|
| 1.1 pass infrastructure | 2 days | `MirPass` trait, `optimize` driver, `--dump-mir-after=passname` CLI flag |
| 1.2 P1 + P6 (const folding + if simp) | 2 days | measurable C size reduction, trivial safety |
| 1.3 P2 + P5 (copy prop + redundant let) | 3 days | temp count reduction |
| 1.4 P3 + P4 (DSE + DCE) | 2 days | dead code removal |
| 1.5 P7 (switch arm cleanup) | 1 day | canonical switch form |
| 1.6 corpus verification | 2 days | e2e output diff, performance measurement |

**S1 total: ~2 weeks.** every pass runs on structured MIR, no CFG, low risk.

### S2 — CFG passes

| item | effort | delivers |
|---|---|---|
| 2.1 P8 CFG conversion + reverse | 5 days | `CfgBody` representation, structured↔CFG, `assert_valid` |
| 2.2 P9 unreachable block elimination | 1 day | block-level dead code (over CFG) |
| 2.3 P10 loop invariant code motion | 5 days | loop opt, only for programs with loops |
| 2.4 P12 switch arm sorting | 1 day | better codegen for matches |
| 2.5 corpus verification + perf | 2 days | e2e output diff, compare `cc -O2` time vs unoptimized |

**S2 total: ~3 weeks.** higher risk (CFG conversion must be bug-for-bug
equivalent to structured MIR). reverse lowering (CFG → structured) must be
verified against all corpus programs.

### S3 — caching + VFS integration

| item | effort | delivers |
|---|---|---|
| 3.1 salsa cache for optimized MIR | 2 days | no re-opt on cache hit |
| 3.2 VFS archive of optimized bodies | 3 days | incremental rebuild: read optimized MIR from archive |
| 3.3 determinism lock (`--opt-determinism-check`) | 1 day | CI gate for optimizer correctness |

**S3 total: ~1 week.**

---

## deferred (not yet designed)

| feature | why deferred |
|---------|--------------|
| LLVM/Cranelift backend | the passes above produce structured MIR; if a native backend uses CFG, P8 already serves it. design the backend's IR separately. |
| Inlining | no multi-file module system yet. cross-file inlining is blocked on VFS + module resolution. |
| Auto-vectorization | needs a target model. deferred to the native backend. |
| Profile-guided optimization | no profiling infrastructure. deferred until Eye has a runtime. |
| ThinLTO-style cross-module opt | blocked on multi-file. |

---

## open questions

**Q1: should unsound optimizations ever fire?** the project's pessimism
principle ("if it is not explicitly proven, it is not valid") says no. every
pass must be provably correct. when in doubt, don't transform. the C
compiler can catch what MIR misses.

**Q2: `int32` fallback (A3) blocks some opts.** constant folding a `Binary`
whose type is unknown (the `int32` fallback) is safe but misleading. S2
cutover (typeck complete) eliminates the fallback and makes every opt pass
sound by construction. until then, P1 skips operands with the fallback type.

**Q3: should optimization be opt-in (`-O1`, `-O2`, `-O3`)?** yes, but the
minimum is `-O1` (S1 passes, always safe). `-O2` adds S2 passes. `-O3`
adds S3 passes. the default for `eye build` is `-O2`; the default for
`eye-lsp` is `-O1` (latency-sensitive).

**Q4: does the `pure` effect annotation enable any optimizations?** yes.
a function annotated `pure` (or inferred pure) can be CSE'd, reordered,
rematerialized, and speculated. the optimizer reads `EffectSet` from the
effect pass (S4 of TYPECK) and uses it to justify transformations. without
the effect pass, every function is conservatively assumed effectful (no
optimization).
