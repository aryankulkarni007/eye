# Modifier blocks: lexical regions that select an alternate lowering

Status: **pattern ratified 2026-06-21 (pair session). `wrapping` designed-not-built;
the rest banked.** legend [STYLE.md](../STYLE.md).

a modifier block is a lexical scope that selects a **predefined alternate lowering**
of the kernel operations written inside it, leaving the default behavior everywhere
else. the canonical member is `wrapping { }` (arithmetic edge semantics,
[KERNEL.md](KERNEL.md)):

```
sum = 0
wrapping {
    for x in data { sum = sum + x }   // +,-,* wrap here; outside they trap
}
```

## why it exists (the recurrence)

the pattern was not invented for arithmetic - it is the spelling several
already-deferred escapes were missing. each is a safe default with an ergonomic
regional opt-out (the nudge, [PHILOSOPHY.md](PHILOSOPHY.md)):

| block | default it opts out of | status | home |
|---|---|---|---|
| `wrapping { }` | arithmetic traps on overflow | `-` designed | [KERNEL.md](KERNEL.md) arithmetic |
| `arena(a) { }` | allocations are individually auto-dropped | `?` banked | [MEM.md](MEM.md) (the parked manual-dealloc opt-out) |
| `unchecked { }` | bounds checks on index | `?` banked | bounds / runtime-safety theme ([DEFER.md](../planning/DEFER.md)) |
| `comptime { }` | code runs at runtime | `?` banked | [PRIME.md](../features/PRIME.md) |

same shape every time: default-safe, opt out regionally, opt out ergonomically.

## the four rails (the bounds that keep it a feature, not a footgun)

1. **lexical only.** a modifier affects the operations written textually inside the
   block, never callees. a called function keeps its own default. dynamic scope - a
   caller silently changing a callee's semantics - is the worst footgun class and is
   forbidden.
2. **kernel operations, not arbitrary behavior.** a block selects among predefined
   alternate lowerings of operators and builtin operations. it does not rewrite
   arbitrary function behavior (which overload? recompile the callee?) - that is
   ill-defined and reintroduces rail 1's footgun.
3. **compiler-blessed and finite.** the modifier set is hand-written in the compiler
   (a lowering mode, no new kernel primitive - freeze-safe per the [KERNEL.md](KERNEL.md)
   "features as desugarings" rule). user-defined modifier blocks are the macro /
   extensibility engine ([PRIME.md](../features/PRIME.md)), far-future and gated. this
   line is load-bearing: blessed-finite is a feature; user-definable is the macro
   engine, and keeping them on opposite sides of the freeze is what makes the pattern
   pure upside.
4. **yagni.** `wrapping` is built first. the other modifiers are banked and added only
   when their own feature lands; no general modifier framework is built speculatively
   (the same trap rejected for row-polymorphic effects - machinery for maybe-useful).

## semantics

- a modifier block lowers to a **lowering-mode flag** carried on the scope through the
  AST -> HIR -> MIR seam. the kernel operations inside resolve to their alternate
  lowering (e.g. `wrapping` lowers `+` to a plain `a + b` under `-fwrapv` rather than
  the checked-and-trap form). no kernel primitive, MIR node, or codegen path is added -
  it is a selection among existing lowerings, which is why it obeys the freeze.
- effects compose normally: `wrapping` removes the `panic` atom from the arithmetic
  inside it (no trap = no panic), which is the precise reason it restores
  auto-vectorization.

## open sub-questions (deferred)

- `?` granularity: block (`wrapping { }`) vs whole-function (`wrapping fn`) vs single
  expression (`wrapping(expr)`). the block is primary; the others are conveniences.
- `?` nesting / override: an inner block re-establishing the outer default (e.g.
  `checked { }` inside `wrapping { }`).
- `?` the exact keyword surface, and whether `arena(a)` (parameterized) and the bare
  forms share one grammar.

## cross-links

- arithmetic: [KERNEL.md](KERNEL.md) "Defined arithmetic edge semantics",
  [ledger](../planning/ledger.md) class C.
- the parked memory-model opt-out that `arena(a) { }` would spell: [MEM.md](MEM.md).
- the freeze rule it obeys: [KERNEL.md](KERNEL.md) "The freeze, precisely".
