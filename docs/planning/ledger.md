# Issue log

Tracking items moved from deferred to in-progress as work proceeds.
Completed items below are summarised; the original full diagnostic is
preserved at the commit listed in each entry.

# MISC

- [ ] const perfect
- [ ] minimalised match seam, optimised for future design and generality
- [ ] sizeof, alignof
- [ ] array element checking
- [ ] evict print intrinsic -> or move to stdlib (and len intrinsic)

---

## In progress

The C-leak audit (`docs/design/CLEAK.md`, 2026-06-11) is the authoritative
ledger for the harden-before-freeze pass; the rows below summarize its open
items. Freeze and typeck are blocked behind them.

- [ ] String decay missing at struct-literal fields (CLEAK L1, the lang.eye
      compile blocker) and array-literal elements (L2). Fix: one
      `coerce(expr, expected)` point applied at every site with a known
      expected type, replacing the 4 scattered `maybe_decay` calls.

- [x] Call arity unchecked (L3): `f(1, 2, 3)` against a 2-param function
      reaches clang. **FIXED** (2026-06-11): count check added to codegen's
      `gen_call_args` (`crates/codegen/src/core/mir_emit.rs`). Skipped for
      variadic. Marked EXPERIMENTAL.

- [ ] Array-literal element types unchecked (L4): `[1, true, "x"]` into
      `[int32; 3]` - string errors in clang, bool converts silently.

- [x] Unknown struct name in struct literal (L5): `Foo { x: 1 }` with no
      `Foo` emits undeclared-identifier C. **FIXED** (2026-06-11): reject
      before codegen in `collect_items` (`crates/hir/src/core/lower/collect.rs`).
      Marked EXPERIMENTAL.

- [ ] Undeclared field type leaks clang error (L6): `off off,` in a struct.
      Attempted inline in collect (2026-06-11), but caused false positives
      for forward references (`Outer { Tag t }` before `union Tag`). Reverted.
      Needs post-collect pass with stored syntax spans.

- [x] Indexing `ptr` emits void-subscript C (L7): reject, `ptr` has no
      element type. **FIXED** (2026-06-11): reject `index_access` on `ptr`
      type in codegen (`crates/codegen/src/core/mir_emit.rs`).
      Marked EXPERIMENTAL.

- [ ] Integer-literal range vs annotation unchecked (M1): `let int32 x =
5000000000;` builds and stores 705032704. Also reaches printf varargs
      as the wrong width (M1b).

- [ ] Mixed-width arithmetic narrows (M2): binary expressions take the LHS
      type, so an int-literal LHS types a `usize` computation `int32` and
      the temp truncates. Typeck's first real customer.

- [ ] `ptr + int` emits void-pointer arithmetic, a GNU extension (P1):
      reject or document.

- [ ] String statics emitted even when unreferenced (P2): `println` inlines
      the literal into the format string; the static is dead bytes.

### 2026-06-11: Post-audit code audit (U1-U5)

New findings from the full end-to-end source audit (2026-06-11).

- [?] typegraph.rs `collect_type_nodes` misses `hir.consts`, `hir.globals`,
  and `Expr::SizeOf` (U1). Array wrapper typedefs absent from C output
  when the sole reference to `[T; N]` is in a const/global type.
  **EXPERIMENTAL FIX** (2026-06-11): walks `hir.consts` and `hir.globals`
  in `collect_type_nodes` and handles `Expr::SizeOf`. All tests pass but
  no dedicated regression test yet.

- [ ] Const-eval `apply_int` uses wrapping arithmetic unchecked against the
      declared type's range (U2). `const X: int8 = 200` evaluates to 200
      (not -56). Type inference surgery will add value-in-range checks.

- [x] Float const-eval accepts non-finite (inf, nan) from overflow literals
      (U3). **FIXED** (2026-06-11): non-finite check added in const-eval
      (`crates/hir/src/core/lower/const_eval.rs`). Returns error value.

- [ ] Const-eval `apply_cast` silently truncates int->float (to inf),
      float->int, and ignores signedness for char/bool->int (U4). Type
      inference surgery will add range checks.

- [ ] Println lowering does not validate `{}` count against argument count
      (U5). Exhausted placeholders emit `%d`, extra arguments forwarded to
      printf. Fix independently of type inference.

### 2026-06-11: Architecture audit (A1-A10)

Design and algorithmic findings from the full crate-by-crate architecture
review (2026-06-11).

- [ ] MIR lowered twice when dump flags active (A1): `--dump-mir` calls
      `mir::lower::lower_function` during dump, then `gen_mir` calls it
      again during codegen. Two traversals of every function body. Fix:
      cache MirBody arenas or gate dump to reuse them. Identified in
      performance analysis (2026-06-11).
      cache lowered MirBody in the HIR or emit on demand from a single
      lowering pass that feeds both dump and codegen.

- [x] `place_type` recurses O(depth) on every call for deeply nested
      projections (A2): codegen calls it multiple times per place (index
      access mode, pointer-likeness, specifier). A struct.array.field
      chain walks the entire chain each time. **FIXED** (2026-06-11):
      memoized via `FxHashMap<Place, Type>` cache on `MirGen`, cleared
      per-function. `PartialEq+Eq+Hash` added to `Place`, `Operand`,
      `Literal`. Marked EXPERIMENTAL.

- [ ] `mir_type_of` falls back to int32 when HIR expr_types lacks an entry
      (A3): `lower.rs:1113-1118`. If HIR misses a type annotation, MIR
      silently defaults to int32 instead of propagating an error type.
      Fix: return error type handle, let codegen emit a diagnostic instead
      of miscompiling.

- [ ] Codegen string_id panic-dodging with unwrap_or(0) (A4): if a string
      literal is absent from `string_index` (should not happen given
      `collect_strings` runs first), the fallback to 0 silently returns the
      wrong backing array. Fix: `expect()` with a descriptive message.

- [ ] LSP `BareIdentPat` in match arms classified as `ENUM_MEMBER` (A5):
      variable-capture patterns `match x { y => y }` highlight `y` as an
      enum member. The CST walk lacks name resolution. Fix: defer to HIR
      name resolution or mark as `VARIABLE` until typeck is available.

- [ ] Parser `no_struct_lit` is a single boolean not a stack (A6): RAII
      save/restore works but any grammar change that nests struct-literal
      ambiguity must handle the interaction manually. A stack of booleans
      would be more robust.

- [ ] No CFG-based MIR representation (A7): structured MIR (If/Loop/Switch)
      limits backend analysis. Value-position control flow leaves uninit
      temps (no phi nodes). A CFG lowering pass would unlock dominance,
      liveness, SSA, and dead-code elimination.

- [ ] `process::exit(1)` used instead of error propagation (A8):
      `src/main.rs` calls `process::exit(1)` at multiple error sites,
      skipping Rust cleanup (temp files, buffered output). Fix: propagate
      as `anyhow::Result<()>` from `main()`.

- [ ] LSP full-document sync with no incremental parsing (A9): every
      keystroke re-lexes, re-parses, re-highlights the entire file. No
      debounce, no cache, no partial update support. Fix: debounce
      threshold + incremental CST update when rowan supports it.

- [ ] Typegraph `collect_type_nodes` walks every body expression for every
      compilation (A10): O(V + E + bodies \* exprs) even when type
      declarations are unchanged. A dirty-tracked, cached typegraph would
      skip redundant walks for multi-file compilation.

- [ ] `if` codegen refactor - follow up on the conversation with claude
      about if statement codegen refactor (see 2.1.157).

- [ ] `print`/`println` as compiler intrinsics should be removed. We can
      compose `printf` with Eye to get what we need, then reintroduce
      `print`/`println` in the stdlib when it exists.

- [ ] `eyesrc/programs/statistics.eye` - LSP highlighting is failing
      (not urgent).

---

## Completed

### 2026-06-11 (later): coercion-point unification + companion rejects

CLEAK fix-order step 3. `LoweringCtx::coerce` (`crates/hir/src/core/lower/coerce.rs`)
is now the single coercion point - array-literal re-typing (recursive),
integer-literal typing, and `&[T; N]` decay - applied at all six sites with a
locally-known expected type: `let` init, call argument, explicit `return`,
function tail, struct-literal field, array-literal element. The four
scattered `maybe_decay` calls and the per-site array re-typing blocks are
deleted.

Closed with it:

- **L1 + L2**: string decay at struct-literal fields and array-literal
  elements (the lang.eye compile blocker). lang.eye compiled and ran.
- **M1 + M1b (T030)**: every integer literal is range-checked against the
  type it ends up with (coerced or the `int32` default) by one post-lowering
  sweep; `let int32 x = 5000000000;` is now an error instead of 705032704.
- **M4, new find (T031)**: positional struct literals (`Point { 1, 2 }`)
  silently dropped their values and zero-initialized the struct - a verified
  miscompile. Rejected; struct literals are named-only.
- **L3 (T026)**: call arity checked - exact for defined fns, minimum for
  variadic externs. Indirect calls stay unchecked (no variadic flag on fn
  types yet; ledger row above).
- **L5 (R011)**: a struct literal must name a declared struct or union.
- **L6 (R012)**: every Path name in a declared type must be a declared type -
  item signatures validated post-collect (forward refs resolve), body
  annotations and casts eagerly. `sizeof` stays lean-on-C (SIZEOF.md).
- **L7 (T027) + deref sibling (T028)**: indexing and dereferencing `ptr`
  rejected (no element/pointee type).
- **P1 decided + fixed (T029)**: arithmetic/bitwise on `ptr` rejected;
  comparisons stay; `T*` keeps C semantics.

Also: lang.eye's extern `exit(int code)` corrected to `int32` (C's `int` was
leaking through verbatim - exactly the class R012 now rejects); the salsa
`compile_file` query stops at the first errored phase like the pre-salsa
driver (MIR assumes a diagnostic-free HIR); snapshot harness terminators
moved to the stable `c source written` marker. 305 tests green, clippy clean
(one accepted `Arc<CompileResult>` Send/Sync note in eye-database), corpus
43/43 with 2 documented XFAIL, strict-C gate 41/41.

### 2026-06-11: C-leak audit + strict-C gate + mechanical fixes

Full audit of implicit type decisions across HIR lowering, MIR lowering, and
codegen, written to `docs/design/CLEAK.md` (classification: M miscompile /
L C-leak / P pedantic / T typeck-required; every M and L row has a verified
reproducer). Detection infrastructure: `scripts/check-c-strict.sh` compiles
the corpus's generated C under `-std=c11 -pedantic-errors -Wall -Wextra
-Werror`; CI gained a `corpus` job running it plus `check_all.sh`, which
gained a stale-checked XFAIL list (linkedlist intentional, lang.eye known
decay bug).

Fixed the same day (the mechanical tier):

- **Exhaustive value-match UB (M3)**: a switch HIR proved exhaustive (no
  default) emitted a tested last arm, leaving the hoist temp uninitialized
  on the rogue-value path (`-Wsometimes-uninitialized`). The last arm is now
  the chain's `else`.
- **C-keyword names (L8)**: R010 `NameIsCKeyword` rejects at collect every
  name the backend emits verbatim (item, field, parameter, enum variant,
  global, opaque type). Extern parameter names exempt (prototypes are
  types-only).
- **Unprototyped zero-param functions (L9)**: `T f()` is now `T f(void)`.
- **Empty-string zero-length array (L10)**: `data[0]` storage pads to
  `data[1]`; type-level length stays 0.
- **`%p` varargs UB (L11)**: `ptr` formats as `%p` (was `%d`); ref/ptr/fn-ptr
  `println` arguments are cast to `(void*)`.

297 tests green, clippy clean, corpus 41/41 + 2 XFAIL, strict gate 41/41.

### 2026-06-10: MIR panic on value-position `loop`

`lower_rvalue` now handles `Expr::Loop` in the same divergent-control-flow
group as `Return`/`Break`/`Continue`: it lowers the loop as a statement and
returns poison `0`. A diverging loop never reaches the poison; a breaking
loop yields `0`, consistent with `break` dropping its value today.

**Residual (Fork D):** a value-position loop cannot yield a real value
until break-with-value lands; `break v;` still drops `v`.

### 2026-06-10: Repeat array literal `[value; N]`

The array-fill primitive: `value` evaluated once, copied `N` times (value
semantics); `N` a const length. New `Expr::ArrayRepeat` / `RValue::ArrayRepeat`
(kept first-class, not desugared, so a native backend can `memset`/loop); C
emits the wrapper with `N` copies. Coerces + nests like the list literal.
`eyesrc/lang/array_fill.eye` is the showcase; `eyesrc/programs/sieve.eye`
restored and runs.

**Latent drift fixed:** the `MatchArm` guard (`guard:MatchGuard?`) was
missing from `eye.ungram`, so `cargo run -p xtask -- codegen` wiped the
hand-added `guard()` accessor; the ungram now carries it.

### 2026-06-10: Non-int32 2D arrays fix

Two root causes, both resolved:

1. **Nested coercion**: array-literal type coercion only re-typed the
   outer literal; nested inner literals kept their int32 default. Fix:
   `coerce_array_literal_type` now recurses every level, length-guarded,
   shared by all three coercion sites.
2. **False positive ICE**: a temporary codegen `panic!` ("type mismatch /
   shallow coercion") guarded the bug but also fired on valid code
   (`let int8 x = 5;`), breaking four e2e tests. Removed; replaced by
   HIR-level correctness + regression test.

### 2026-06-08: LSP pipes all diagnostics

`server/notifications.rs`'s `compute_diagnostics` now runs all three phases
(lexer, parser, HIR). Every 8-class model variant reaches the editor with
phase label, diagnostic code, notes, help, secondary labels, and severity.
`docs/features/LSP.md` updated.

### 2026-06-08: Void value in value position emits diagnostic

Added `TypeError::VoidValueInValuePosition` (T024).
`check_explicit_let_init_type` and `check_explicit_return` in `stmt.rs` now
emit this diagnostic when a call produces no value in a typed context
(`let int32 x = f();` where `f` returns void, or `return f();` in a typed
function). `docs/features/DIAGNOSTICS.md` updated.

### 2026-06-08: Missing return value in non-void function

`enforce_fn_return_type` in `stmt.rs` now emits `ReturnMissingValue` (T005)
when a non-void function has no tail expression and no explicit
`return val;`. Added `fn_block_ptr` field to `LoweringCtx` to anchor the
span on the function body block.

### 2026-06-11: Harden-before-freeze pass + performance analysis

**Code audit fixes:**

- U3 (non-finite float const-eval) - **FIXED** in `const_eval.rs`
- A4 (`string_id` unwrap_or(0) -> expect) - **FIXED** in `mir_emit.rs`
- A8 (`process::exit(1)` -> return Err) - **FIXED** in `mir_emit.rs`
- L3 (call arity check) - first sketch; superseded the same day by the full
  T026 check (variadic-aware) in `lower/expr.rs` (see the coercion-point
  entry above)
- L5 (unknown struct name in literal) - first sketch (reused R008);
  superseded by the dedicated R011 in `lower/expr.rs`
- L7 (indexing `ptr` type) - first sketch in `lower/expr.rs`; kept, joined by
  the deref reject (T028)
- U1 (walk consts/globals/SizeOf in typegraph) - **EXPERIMENTAL** in `typegraph.rs`
- A3 (mir_type_of int32 fallback) - attempted, reverted (false positives)
- L6 (undeclared field type) - attempted inline, reverted (forward-ref false
  positives); solved by the post-collect `validate_type_names` pass (R012),
  which runs after all items are collected so forward refs resolve

All 15 error sites (U1-U5, A1-A10) tagged with code comments in their source files.
All 76 tests pass (65 e2e + 7 proptest + 4 snapshot).

**Performance analysis:** `docs/performance.md`. Key findings:

- Full pipeline: ~57 µs for 58-line program (HIR 41%, parse 35%, codegen 17%)
- Rowan green tree accounts for ~60% of parse time (by-design cost)
- A1 (MIR double-lowering) is the highest-impact repeatable waste
- A2 (place_type memoization) implemented EXPERIMENTAL
- No O(N²) hot paths found - all passes are O(N) with arena allocation

### 2026-06-04/05: Early return + `main` de-magicked

**Early return:** `return expr;` and bare `return;` parse, lower (HIR
`Expr::Return` -> MIR `Return`), and emit. Three return-arity diagnostics
guard the clang-error cases (value in a void fn, missing value in a typed
fn, wrong type). Value-position return diverges correctly on both MIR
paths - wrapped in `if`/`match` (`lower_into`) and as a direct rvalue
(`let x = return 5;`, `lower_rvalue`). Restored
`eyesrc/programs/floodfill.eye`.

**`main` de-magicked:** `main` is now an ordinary function. The C entry
requirement lives only in a backend shim - the user's `main` emits as
`__eye_main`, and codegen generates `int main(void)` that adapts it. An
integer return forwards as the process exit code; every other return type
runs `main` for its effect and exits 0. The only rejection is a parameter
list (`TypeError::MainHasParams`): the shim has nothing to pass.

### MIR cutover (bundled fixes)

- **`eyesrc/programs/wierd.eye`** (acid test): a value-position `match`
  inside a value-position `if` now lowers in place. `check_unhoisted_matches`
  and the `TernaryMatch` ban were deleted. The old U001 workaround no longer
  occurs.
- **`eyesrc/programs/floodfill.eye`**: `!=` on `grid[p.y][p.x]` was a false
  "op on array": indexing a `&[[int32;8];8]` ref did not peel to the element
  type. Index typing now peels one ref/ptr to an array down to its element
  type, so `r[i]` on `&[T;N]` yields `T`.
- **`eyesrc/programs/bubblesort.eye`**: fixed by the same index-through-ref
  fix. The `>` on `xs[j as usize]` no longer false-positives as an op on
  an array.
