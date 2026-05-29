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
| Arrays as struct/union fields | v0.7 | The array wrapper typedef is emitted after the nominal types, so a struct field of array type would reference an undeclared type. Rejected with a clear diagnostic for now. | Codegen gains a type-dependency topological sort over structs/unions/array wrappers. |
| `let` type inference (T1) | v0.5 | Deferred until the kernel surface stops moving; untyped `let` still emits a placeholder by design. Also gates the residual `int32` match-temp fallback. | The language is stable. |
| Array literal element-type checking (`[1, true]`) | v0.7 | A heterogeneous literal is silently accepted (the type comes from the first element). A correct check needs coercion rules - e.g. `[x, 1]` with `x: usize` and an int-literal `1` must coerce - so it depends on the deferred typecheck pass, not a standalone element-uniformity test. Tied to [T1]. | The separate typecheck pass / inference (T1) lands. |
| Nested value-position `match`/`if` (one as another's branch tail value) | v0.7 | `hoist_values` pre-declares a temp for each value `match`/`if` reachable in a flat expression and recurses into an `if`'s condition, but not into branch interiors. A value `match`/`if` used as the *tail value of a branch* is emitted as `/* UNHOISTED ... */`. Hoisting it needs the temp declared and filled inside that branch's scope before the assignment - a recursive per-branch hoist. This is the one-level limit `match` always had, now symmetric for `if`. | Codegen grows a per-branch hoist, or this lowering moves to MIR where value vs statement position is explicit (see [MIR.md](MIR.md)). |

## In-scope but not yet built

Not deferrals - still targeted for the version named, just not done.

| Item | Version | Why last | Needs |
|------|---------|----------|-------|
| Const / named-length arrays `[T; N_const]` (A6) | v0.7 | Lowest-priority array deliverable; should not block A1-A5. | A compile-time-constant concept (length is an integer literal today): literal -> named const -> minimal const-expr. |

## Notes

- The array-wrapper mangle injectivity hole (`&int` once collided with a user
  type named `ref_int`) was **fixed**, not deferred: the mangle now
  length-prefixes type names. See `crates/codegen/src/core/arrays.rs` and its
  unit tests.
- Runtime safety and slices are linked: both are blocked on Eye having no
  runtime-length and no abort mechanism. They are likely a single later theme.
- Const-length arrays are the smallest of the three and the most likely to be
  pulled forward into a near-future version.
