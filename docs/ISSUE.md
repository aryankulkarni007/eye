here is where we log outstanding issue that have been moved from
deferred to in progress, but not yet completed

- [ ] we need to implement early return functionality using return expr;

- [ ] we need to follow up on the if codegen refactor

- [ ] note: we actually don't need print as a compiler intrinsic. we can compose printf with eeye to get what we want. then we can reintroduce print and println as required in the stlib

- [ ] LSP only has parser diags -> pipe all diags into it

- [ ] make sure to follow up on the conversation with claude about if statements codegen refactor -> 2.1.157

- [ ] look at eyesrc/statistics.eye -> lsp highlighting failing (not urgent fix)

- [x] non-int32 2D arrays don't work -> look at eyesrc/arr_test.eye. FIXED.
  Two root causes, both now resolved:
  1. Array-literal type coercion (let/return/call-arg) only re-typed the outer
     literal onto the declared array type; nested inner literals kept their
     int32 default, so a `[[usize;2];2]` outer wrapper held `int32` inner
     wrappers -> C type error. Fix: `coerce_array_literal_type` in
     crates/hir/src/core/lower/stmt.rs now recurses every level, length-guarded,
     shared by all three coercion sites.
  2. A temporary codegen `panic!` ("ICE: ... type mismatch / shallow coercion")
     in crates/codegen/src/core/stmt.rs guarded this bug but also fired on valid
     code (`let int8 x = 5;`, since integer literals default to int32), breaking
     four e2e tests. Removed; replaced by HIR-level correctness + regression
     tests (nested_array_literal_coerces_inner_element_type).
  Note: this is NOT the deferred type-inference work. General inference (track 2)
  still subsumes the per-site coercion; this fix is local and correct now.

- [x] take a look for eyesrc/floodfill.eye - FIXED. The `!=` on `grid[p.y][p.x]`
  was a false "op on array": indexing a `&[[int32;8];8]` ref did not peel to the
  element type. Index typing in crates/hir/src/core/lower/expr.rs now peels one
  ref/ptr to an array down to its element type, so `r[i]` on `&[T;N]` yields `T`.
  Builds and runs correctly. Covered by index_through_ref_to_array_yields_element_type.

- [ ] take a look for eyesrc/wierd.eye - DEFERRED (L1). This is the
  match-in-ternary restriction, not a regression: a value-position `match` in a
  branch of a value-position `if` cannot be hoisted today because codegen would
  render the `if` as `cond ? a : b` and a C `switch` is a statement, not an
  expression. It already fails gracefully (clear U001 + documented `let`
  workaround), so it does not erode trust the way the panics did. The real fix
  is the `gen_if_statement` change to hoist the branch-tail match / recurse with
  the temp - the same "if codegen refactor" tracked above (and 2.1.157). Kept
  separate on purpose.

```rust
➜ eye git:(main) cargo r eyesrc/wierd.eye
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
Running `target/debug/eye eyesrc/wierd.eye`
compiling...
lowering AST to HIR...
[U001] Error: match in a conditional (ternary) expression is not supported yet; bind it to a `let` first
   ╭─[ <unknown>:18:13 ]
   │
18 │ ╭─▶             match shape {
   ┆ ┆
22 │ ├─▶             }
   │ │
   │ ╰─────────────────── match in a conditional (ternary) expression is not supported yet; bind it to a `let` first
───╯
➜ eye git:(main)
```

- [x] take a look for eyesrc/bubblesort.eye - FIXED by the same index-through-ref
  fix as floodfill. The `>` on `xs[j as usize]` no longer false-positives as an
  op on an array. Builds, sorts correctly. (The in-file `let`-workaround comment
  is now stale - the direct comparison form works too.)

```rust
➜ eye git:(main) cargo r eyesrc/bubblesort.eye
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
     Running `target/debug/eye eyesrc/bubblesort.eye`
compiling...
lowering AST to HIR...
[T001] Error: cannot apply `>` to an array
    ╭─[ <unknown>:18:16 ]
    │
 18 │             if xs[j as usize] > xs[(j + 1) as usize] {
    │                ──────────────────┬──────────────────
    │                                  ╰──────────────────── cannot apply `>` to an array
────╯
➜ eye git:(main)
```
