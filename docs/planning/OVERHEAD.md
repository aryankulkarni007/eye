# Pipeline Overhead Investigation

Status: **Diagnostic survey only** — identifies issues, quantifies yield, assesses
risk. No fixes implemented. Each section tracks a distinct allocation or clone
pattern with estimated savings and barrier to entry.

---

## Architectural Context

The compiler has **two execution paths** with different interner ownership:

| Path | Used by | TypeInterner model | Overhead |
|------|---------|-------------------|----------|
| **Whole-file** (`lowered_file`) | `c_code`, tests, benches | `mem::take` + restore — one interner reused across all fns | No per-body clone |
| **Per-fn query** (`lower_fn` + `typeck_fn`) | LSP, incremental compilation | Each fn body clones the scope's interner, then typeck clones again | **2 deep interner clones per fn** |

Both paths are necessary: the per-fn path enables salsa's incremental recomputation
(a keystroke inside one fn doesn't re-type-check its siblings). The question is
whether each per-fn query must pay for a full interner copy.

---

## Tier 1 — Guaranteed savings, zero risk

These changes are pure removals of dead/legacy overhead. No semantic change, no
new unsafe code, no architectural refactor.

### 1.1 `check_body` signature: `&mut TypeInterner` → `&TypeInterner`

**Files:** `crates/typeck/src/lib.rs:137`, `crates/typeck/src/infer.rs:22,33`
**Dependent:** `crates/database/src/lib.rs:383`

**What:** `check_body` takes `&mut TypeInterner`, but every method called on the
interner inside `infer.rs` is `&self` (`lookup`, `intern`, `display`,
`error_type`, etc.). All 67 call sites confirmed. The `&mut` dates to the S1-S5
era before the lock-free (`papaya`) interner switch.

**Effect:** Eliminates **clone B** in `typeck_fn` (database/src/lib.rs:383),
where `lowered.lowered.types.clone()` is called solely to pass an owned copy to
`check_body`. With `&TypeInterner`, the query borrows through the `Arc` instead.

**Changes required:**
- `typeck/src/lib.rs:137` — fn signature
- `typeck/src/infer.rs:22,33` — `InferCtx.types` field type
- `database/src/lib.rs:383` — remove `.clone()`, pass `&lowered.lowered.types`

**Yield:** HIGH — the #1 hot-path allocation. Saves one deep `TypeInterner`
clone per fn body in the per-fn query path. For a file with N fns, N × ~1-5 KB
of allocation churn eliminated.

**Risk:** Zero. The `&mut` is vestigial.

---

### 1.2 `extend_from(&self, other: &Sink<K>)` on diagnostic `Sink`

**File:** `crates/diagnostics/src/lib.rs`

**What:** `Sink::extend` takes `Sink<K>` by value (move). `hir_diagnostics`
currently calls `.clone()` on each source `Sink` (behind `Arc`) to get an owned
copy, then passes to `extend`. This allocates an intermediate `Vec` that is
immediately consumed.

**Effect:** Adds `extend_from(&self, &Sink<K>)` that iterates and clones
per-element directly into the output, skipping the intermediate `Vec`
allocation. Then `hir_diagnostics` uses it.

**Changes required:**
- Add `extend_from` method to `Sink<K>` (5 lines)
- Update `hir_diagnostics` at `database/src/lib.rs:397-415`
- Clean up the blank line artifact left at `diagnostics/src/lib.rs:193-194`

**Yield:** LOW — saves one `Vec` allocation + deallocation per file. The
per-element clones are the same either way.

**Risk:** Zero (pure additive API, no callers change semantics).

---

### 1.3 Move instead of clone in enum variant duplicate check

**File:** `crates/hir/src/core/lower/collect.rs:608-609`

**What:** Two `Text` values are cloned into a `ResolveError::DuplicateVariantDecl`
diagnostic, then go unused afterward. They can be moved instead.

```rust
// before:
variant: vname.clone(),      // L608 — clone
enum_name: other_name.clone(), // L609 — clone
// after:
variant: vname,              // move
enum_name: other_name,       // move
```

**Yield:** VERY LOW — error path only (2 SmolStr clones per duplicate variant
declaration, which is rare).

**Risk:** Zero.

---

## Tier 2 — Measurable savings, low risk

These require type changes but are mechanically straightforward.

### 2.1 `Arc<TypeKind>` in TypeInterner

**File:** `crates/hir/src/core/types.rs`

**What:** The interner's `arena` (`boxcar::Vec<TypeKind>`) and `map`
(`papaya::HashMap<TypeKind, u32>`) store `TypeKind` by value. Cloning the
interner deep-copies every TypeKind — each `Path(SmolStr)` inline copy,
each `Fn{params: Vec<TypeRef>}` heap Vec, etc.

Switching the stored type to `Arc<TypeKind>` makes cloning the interner ≈ N ×
atomic refcount bumps instead of N × deep copies. The `kind.clone()` in
`intern()` becomes `Arc::new(kind)` (same allocation count — one per new
type).

**Affects:**
- Clone A in `lower/mod.rs:179` (the remaining interner clone after fix 1.1) — ~8x cheaper
- Clone of `CheckedFile.hir` (which contains `types`) — same saving
- `mem::take`/`restore` path (whole-file) — no change (take already avoids clone)

**Changes required:**
- `arena: boxcar::Vec<Arc<TypeKind>>`
- `map: papaya::HashMap<Arc<TypeKind>, u32>`
- `intern()`: `arena.push(Arc::new(kind))` instead of `arena.push(kind.clone())`
- `lookup()`: deref `&Arc<TypeKind>` to `&TypeKind` for callers
- Update pattern matches on `self.arena[i]` to deref

**Yield:** MEDIUM-HIGH — makes the largest surviving hot-path allocation ~8x
cheaper. Clone A goes from deep copy of 50-100 TypeKind values to 50-100
atomic increments.

**Risk:** LOW. `TypeKind` already implements `Hash + Eq + Send + Sync`;
`Arc<TypeKind>` inherits all three. Callers of `lookup()` get one extra
pointer chase. No semantic change.

---

### 2.2 MIR Place clone: last-iteration move in destructure loop

**File:** `crates/mir/src/lower.rs:281`

**What:** `lower_let_destructure` builds `Place::Field(Box::new(base.clone()), field)`
per field in a loop. For N fields, iterations 0..N-2 clone `base` (reused),
but iteration N-1 can move it.

```rust
for (i, (field, hid)) in fields.into_iter().enumerate() {
    let base = if i == len - 1 { base } else { base.clone() };
    let proj = Place::Field(Box::new(base), field);
    // ...
}
```

**Yield:** LOW — struct destructuring is uncommon in the corpus. Saves 1 Place
clone per destructure (≈ 24-32 bytes + any deep projection chain).

**Risk:** LOW.

---

### 2.3 MIR Place clone: split `lower_into` catch-all

**File:** `crates/mir/src/lower.rs:1049`

**What:** The catch-all arm of `lower_into` uses `target` exactly once (to
assign the lowered rvalue). But `lower_into` takes `&Place` because other arms
(`If`, `Match`, `And`/`Or`) need `target` multiple times. The catch-all clones
gratuitously.

**Fix options:**
- Split into `lower_into` + `lower_into_owned(mut self, Place)`
- Or: `Rc<Place>` in MIR IR (see tier-3 item)

**Yield:** LOW-MEDIUM — saves 1 deep Place clone per non-control-flow
expression. `target` can be an arbitrary projection chain (e.g., `a.b.c.d[i]`).

**Risk:** LOW-MEDIUM — requires API change (new method or param type).

---

## Tier 3 — Architectural changes (long-term)

These fix the root cause but require cross-crate refactors.

### 3.1 S6 global lock-free interner

**Where:** `crates/hir/src/core/types.rs` + salsa wiring

**What:** One shared `TypeInterner` for the whole file, never cloned. All body
lowering and type-checking borrow from it. `TypeRef` handles are comparable
across bodies (enabling cross-body codegen optimizations by default).

**Effect:** Eliminates clone A entirely (the remaining interner clone after
fix 1.1). The `mem::take`/`restore` pattern in the whole-file path also
disappears. Handles become global — no more per-body interner addresses.

**Yield:** VERY HIGH — eliminates the root cause of the largest allocation
pattern. Simplifies the architecture.

**Risk:** HIGH — salsa's per-fn query model currently depends on
self-contained results (`LoweredBody`, `TypeckResults` handle back to their
own interner). A global interner means handles are valid across query
boundaries, which is better but requires verifying no stale handles exist.

---

### 3.2 `Rc<Place>` in MIR IR

**Where:** `crates/mir/src/core.rs`

**What:** Change `Place` from a deep-cloned recursive enum to `Rc<Place>` (or
arena indices). `Place::clone` goes from O(depth) to O(1) refcount bump.

**Effect:** Fixes all 6 clone sites in `lower.rs` (lines 281, 384, 420, 659,
1024, 1049) plus the codegen `place_types` cache (mir_emit.rs:1315)
uniformly.

**Yield:** MEDIUM — Place cloning is not the pipeline bottleneck (interner
clone dominates), but the fix is permanent and eliminates a class of future
regressions.

**Risk:** MEDIUM — changes MIR IR public types, affects codegen.

---

### 3.3 Decouple effect diagnostics from `lowered_file`

**Where:** `crates/database/src/lib.rs`

**What:** Effect diagnostics are currently bundled in `lowered_file`
(`CheckFile.effect_diagnostics`), a whole-file query. A keystroke that only
changes an effect annotation still triggers the full `lowered_file`
recomputation.

**Effect:** Separating effect diagnostics into their own query would allow
salsa to skip the full recomputation when only effect annotations change.

**Yield:** MEDIUM — reduces unnecessary recomputation for effect-annotation
edits.

**Risk:** HIGH — salsa dependency graph restructuring. Effect inference
currently runs during `lowered_file` and its diagnostics are bundled with the
result.

---

## Rejected candidates

| Candidate | Why rejected |
|---|---|
| **`Arc` → `Rc` on `Memo<T>`** | Salsa requires `Send+Sync` on query outputs; `Rc` is neither. The rayon test (`parallel_per_fn_typeck_matches_serial`) proves concurrent access. Switching would be unsound. Atomic overhead is <0.1% of per-query work (~15-30ns vs 50-500µs). |
| **Codegen string pool double-clone** | `mir_emit.rs:82-83` clones the same `Text` twice (once for `HashMap` key, once for `Vec` element). Both are structurally necessary — both collections need owned values. Consolidating to one explicit clone changes code shape but doesn't reduce atomic ops. |
| **Most collect.rs `SmolStr` clones** | The arena-alloc pattern requires 2 owned copies: one for the arena, one for the scope-map. The clones are already deferred to error paths where possible. Only 2 genuinely avoidable clones exist (see item 1.3). |
| **Intermediate `Vec` allocations** | Only 3 `collect::<Vec<_>>()` calls exist in non-test pipeline code, all on error or formatting paths. None in the hot path. |
| **`SyntaxNodePtr::range()` clone** | Already fixed (uses Copy field instead of clone). |

---

## Verification plan

For each tier-1 fix:
1. Run `cargo test` — regression check
2. Run `cargo bench --bench compile` — compare before/after measurements
   for the affected benchmark group (`typeck`, `hir-lower`, `full-pipeline`)
3. Run `cargo xtask flamegraph --iterations 500` — compare flamegraph
   before/after for the affected stage

For tier-2 items:
- `Arc<TypeKind>`: Same bench/flamegraph comparison. Expect allocation
  reduction visible in flamegraph `alloc` events.
- MIR Place changes: Check `mir-lower` benchmark group specifically.

---

## How to read yield estimates

- **VERY HIGH** / **HIGH**: Eliminates a large allocation that occurs per fn
  body in the hot path. Visible in benchmarks and flamegraphs.
- **MEDIUM**: Reduces a moderate allocation or removes a class of future
  regressions. May or may not be benchmark-visible depending on workload.
- **LOW** / **VERY LOW**: Removes small allocations on rare code paths.
  Cosmetic or defensive — not expected to move benchmarks.
