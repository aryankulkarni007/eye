# Salsa Architecture

Status: **BUILT (2026-06-12).** The query database is implemented in
[`crates/database`](../../crates/database): tracked functions for every phase,
wired into the CLI driver (`src/main.rs`) and the LSP. 309 workspace tests and
the 43-file corpus pass through it. The "Divergences from this plan" section
below records where the implementation deliberately departs from the original
design; the rest of this document is the original plan, kept for rationale.

## Divergences from this plan (as built)

1. **The interner does not freeze at collect time.** The plan assumed all types
   are known after item collection, but body lowering interns new types (a
   string literal's `&[uint8; N]`, body-local arrays/refs). Instead:
   `HIR.types` is a plain `TypeInterner` (no `RefCell` anywhere - the dynamic
   borrow flags also cost on every intern/lookup in the hot path);
   `LoweringCtx` *owns* a working interner, seeded by `mem::take`/restore in
   the whole-file wrapper or by cloning the frozen scope interner in the
   per-fn path. A per-fn `LoweredBody` carries its own interner: scope handles
   are bit-identical in it (the clone preserves them), body-local handles are
   valid only through it.

2. **Two lowering paths, not one.** Per-body interners mean `TypeRef` handles
   from two bodies are not mutually comparable, and codegen compares handles
   across bodies (type-declaration topo order, array-wrapper typedef dedup in
   `typegraph::collect_type_nodes`). So:
   - `item_scope` + `lower_fn(StableFnId)` is the **per-fn** path - used for
     diagnostics (`hir_diagnostics`), where cross-body identity is never
     needed. Editing one body re-runs one query.
   - `lowered_file` (the `lower_source_file` wrapper, one shared interner) is
     the **whole-file** path - feeds `mir_map` and `c_code`. The C output is a
     function of the whole file, so whole-file is the honest cache key; finer
     C-level incrementality would need stable cross-body type identity first.

3. **`mir_map` replaces `MirCache`.** A tracked `mir_map(file)` query lowers
   every defined fn once; `--dump-mir`, `--dump-mir-raw`, and `c_code` all
   consume the same memoized map, which is exactly the double-lowering the
   `MirCache` existed to prevent - without its staleness risk (salsa
   invalidates on edit; the cache never did). `codegen::gen_mir(hir, &mirs)`
   takes the pre-lowered map; `mir::lower_all(&hir)` is the convenience for
   direct callers (tests, fuzz, benches, `eye::compile_file`).

4. **Diagnostics ride in query results, not accumulators.** Each result
   (`Lexed`, `ParseResult`, `FileScope`, `LoweredBody`, `HIR`) carries its
   `Sink`; `database::hir_diagnostics` aggregates scope + per-fn sinks. Plain
   data is simpler to test than salsa accumulators and the LSP consumes it
   identically.

5. **`Memo<T>` instead of `salsa::Update` impls.** Query results wrap in
   `Memo<T>(Arc<T>)`. `PartialEq` delegates to a `MemoEq` trait whose default
   is conservative (a re-executed query counts as changed, dependents re-run -
   never stale). Per-type structural backdating is an opt-in override:
   - **landed (S5, 2026-06-16):** `FileScope` and `LoweredFn` override
     `memo_eq` with a content digest (signature digest / body digest) so a
     body-only edit backdates `item_scope` and the unedited bodies' `typeck_fn`
     cache-hit - the signature firewall (TYPECK.md). digest, not deep
     `PartialEq`: correct-by-construction (deterministic lowering + owned
     `Text`), and cheap.
   - **still conservative:** `lex`/`parse`/`lowered_file`/`mir_map`/`c_code`
     (e.g. token-stream equality letting a comment edit stop at `lex` is a
     future per-type opt-in).

6. **No `Scope` struct; `StableFnId` is not stored.** The scope result reuses
   `HIR` (bodies empty) inside `database::FileScope`, and stable ids are
   interned on demand from `(file, SyntaxNodePtr)` - storing `StableFnId<'db>`
   in a result would smuggle the `'db` lifetime into cached data (the plan's
   `'static` transmute is not needed at all). `Function::body` stays for the
   wrapper path; the per-fn path leaves it `None`.

7. **`parse` stores the `GreenNode`,** not a `SyntaxNode` (`Rc`-based cursor,
   not `Send`); `ParseResult::syntax()` re-roots in O(1). `TypedArena`'s
   phantom switched from `*const Id` to `fn() -> Id` so `HIR` is `Send + Sync`.

## Why Salsa over a custom `QueryCache`

The original QUERY.md recommended a custom lightweight memoization (Option A) with
a future migration to Salsa (Option B). After reviewing the architecture, going
directly to Salsa avoids a rewrite. The costs and benefits:

**Pros:**
- Fine-grained incremental recomputation for free (edit one fn body → only that fn's
  `lower_fn`/`typeck`/`mir` queries re-execute)
- Stable interning via `#[salsa::interned]` solves the cross-revision identity problem
  that arena indices cannot
- Salsa accumulators replace the manual `Sink<Diag>` plumbing for per-query diagnostics
- `#[salsa::tracked(returns_ref)]` gives memoization + zero-copy read for `Arc`-wrapped
  results
- No custom cache-invalidation logic to write or debug
- The same architecture rust-analyzer uses (well-trodden path)

**Cons:**
- Proc-macro dependency (`salsa` adds build-time overhead)
- Top-level item types must become Salsa structs (interned), which means their data lives
  in the database rather than in local arenas — this is the biggest restructuring
- Salsa's `#[salsa::tracked]` functions have constraints (must take a salsa struct as
  second arg, results must be `Clone`)
- The `#[salsa::db]` proc macro generates the database implementation, adding complexity
  at the integration layer

**Decision: Salsa from the start.** The restructuring cost is paid once; the custom
`QueryCache` path would pay it twice (build custom cache, then rip it out for Salsa).

## Crate layout

Flat workspace:

```
eye (bin) ────────────────────────────┐
crates/                                │
  token                                │
  diagnostics                          │
  syntax (StringTable trait + SmolStr) │
  lexer (SourceFile, Interner)         │
  parser                               │
  ast                                  │
  hir (HIR items + bodies + lowering)  │
  mir                                  │
  codegen                              │
  lsp                                  │
  xtask                                │
                                       │
  database ──── depends on everything ─┘
            ──── used by ──→ eye, lsp
```

The `database` crate is the integration layer. It knows about every compiler crate and
defines the Salsa database + all tracked functions. Both the CLI binary and the LSP
use it and never speak to compiler crates directly (except for types they need from
exports).

## Architecture

```
                    ┌──────────────────────┐
                    │   SourceFileInput     │  #[salsa::input]
                    │   (path, text)        │  (the only mutable state)
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │       lex()          │  #[salsa::tracked] → Arc<Lexed>
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │      parse()         │  #[salsa::tracked] → Arc<Parse>
                    └──────────┬───────────┘
                               │
                    ┌──────────▼───────────┐
                    │    item_scope()      │  #[salsa::tracked(returns_ref)] → Arc<Scope>
                    └──────────┬───────────┘
                               │
               ┌───────────────┼───────────────┐
               │               │               │
        ┌──────▼──────┐  ┌────▼─────┐  ┌──────▼──────┐
        │ lower_fn()  │  │lower_fn()│  │ lower_fn()  │  per StableFnId
        └──────┬──────┘  └────┬─────┘  └──────┬──────┘┐
               │              │               │       │ (depends on item_scope
        ┌──────▼──────┐  ┌────▼─────┐  ┌──────▼──────┘ for items + types)
        │  mir()      │  │ mir()    │  │  mir()       │  per StableFnId
        └──────┬──────┘  └────┬─────┘  └─────────────┘
               │              │               │
               └──────────────┼───────────────┘
                              │
                    ┌─────────▼─────────┐
                    │     c_code()      │  #[salsa::tracked] → String
                    └───────────────────┘
```

## Salsa structs

### Input (mutable)

```rust
#[salsa::input]
pub struct SourceFileInput {
    #[returns(ref)]
    pub path: String,
    #[returns(ref)]
    pub text: String,
}
```

The only mutable state in the system. Setting text through `set_text(&mut db, text)`
bumps the revision and invalidates derived queries.

### Interned (stable identity, copyable, deduplicated)

```rust
#[salsa::interned]
pub struct StableStructId<'db> {
    pub file: SourceFileInput,
    #[returns(ref)]
    pub name: String,
}

#[salsa::interned]
pub struct StableEnumId<'db> {
    pub file: SourceFileInput,
    #[returns(ref)]
    pub name: String,
}

#[salsa::interned]
pub struct StableFnId<'db> {
    pub file: SourceFileInput,
    pub node_ptr: SyntaxNodePtr,
}
```

Key design: `StableFnId` uses `SyntaxNodePtr` (rowan's stable AST position) rather
than the function name. This means:
- Editing a different function → `SyntaxNodePtr` unchanged → cache hit ✓
- Renaming a function → `SyntaxNodePtr` unchanged → cache hit (correct: the function
  identity didn't change, just its name; re-interned names in the body trigger
  recomputation at the field level)
- Adding/removing functions above this one → AST shifts → new `SyntaxNodePtr` →
  cache miss (correct: function identity changed)

### Diagnostics (accumulator)

```rust
#[salsa::accumulator]
pub struct HirDiagnostic(Diag);
```

`lower_fn` and `item_scope` push diagnostics through this accumulator. The LSP
collects them with `lower_fn::accumulated::<HirDiagnostic>(db)`.

## The Scope result

```rust
pub struct Scope {
    // Arena-indexed item storage (unchanged layout)
    pub structs: TypedArena<Struct, StructId>,
    pub enums: TypedArena<Enum, EnumId>,
    pub functions: TypedArena<Function, FnId>,
    pub fields: TypedArena<Field, FieldId>,
    pub items: ItemScope,
    pub const_values: FxHashMap<Text, ConstValue>,

    // Stable-ID → arena-index bridges
    pub struct_by_id: FxHashMap<StableStructId<'static>, StructId>,
    pub fn_by_id: FxHashMap<StableFnId<'static>, FnId>,
    pub fn_list: Vec<StableFnId<'static>>,

    // Frozen type interner (no RefCell)
    pub types: TypeInterner,

    // Diagnostics from the collection phase
    pub diagnostics: Sink<HirError>,
}
```

Not a salsa struct — plain Rust data wrapped in `Arc`. The `'static` lifetime on the
stable IDs in the maps works because salsa IDs are integers; the `'db` lifetime is
only needed for field access on the database.

## Changes to existing crates

### `hir::core`

- Split `lower_source_file` into `collect_file_scope` + `lower_fn_body`:
  - `collect_file_scope(file_ast, interner) -> (Scope, Vec<StableFnId>)` — does
    passes 1–1.5 (collect consts, eval, collect globals, eval, collect items, check
    recursion, freeze types)
  - `lower_fn_body(&Scope, fn_ast, fn_id) -> Body` — does pass 3 (single fn body)
  - `lower_source_file` stays as a convenience wrapper that calls both (for tests)
- `Function` drops `body: Option<BodyId>` — the body lives in the `lower_fn` query
- `TypeInterner` loses `RefCell` — pre-populated during collect, frozen afterward
- `HIR` struct is replaced by `Scope` for public consumption; kept internally for tests

### `mir::core`

- `lower_function` takes `&Scope` instead of `&HIR`
- Accesses frozen types through `&scope.types` (no `borrow_mut`)

### `codegen::core`

- `gen_mir` takes `&dyn Db + SourceFileInput` instead of `&HIR + &MirCache`
- Iterates `scope.fn_list`, calls `db.mir(fn_id)` for each
- Type declarations still walk `scope.structs` / `scope.enums`

### `mir::MirCache`

- Deleted entirely. Salsa's tracked `mir(db, fn_id)` replaces it.
- MIR results are cached automatically; `--dump-mir` just calls `db.mir(fn_id)`.

## Diagnostics flow

```
item_scope:          pushes item-level diags to HirDiagnostic accumulator
                     stores them in Scope.diagnostics for immediate access

lower_fn:            pushes body-level diags to HirDiagnostic accumulator

c_code:              calls lower_fn for each fn to ensure diags are accumulated

LSP:                 lower_fn::accumulated::<HirDiagnostic>(db) collects all
                     item_scope::accumulated::<HirDiagnostic>(db) collects all
                     merge + deduplicate → Vec<Diag>
```

The `Scope.diagnostics` field duplicates the accumulator for item-level errors.
This is necessary because `item_scope` returns `Arc<Scope>` and the caller needs
to check diagnostics without separately querying the accumulator. The accumulator
is the source of truth for per-function diagnostics (which are cheaper to collect
as-needed).

## Implementation sequence

| Step | What | Files | Tests pass? |
|------|------|-------|-------------|
| 1 | Create `database` crate, Cargo.toml, workspace registration | `crates/database/`, `Cargo.toml` | Yes (no code yet) |
| 2 | Define salsa structs: `SourceFileInput`, `Stable*Id` | `crates/database/src/` | Yes (no queries yet) |
| 3 | Split `lower_source_file` → `collect_file_scope` + `lower_fn_body` | `crates/hir/src/core/lower/` | **Gate** — all 76 tests must pass |
| 4 | Freeze `TypeInterner` (pre-populate during collect, remove `RefCell`) | `crates/hir/src/core/types.rs` | **Gate** |
| 5 | Define tracked functions in `database` crate | `crates/database/src/queries.rs` | Yes (not wired yet) |
| 6 | Update `mir::lower`, `codegen::core` to use `&Scope` / `&dyn Db` | `crates/mir/`, `crates/codegen/` | **Gate** |
| 7 | Remove `MirCache` | `crates/mir/src/lib.rs` | Yes (mir now tracked fn) |
| 8 | Update `main.rs` + `lib.rs` to use `Database` | `src/main.rs`, `src/lib.rs` | **Gate** |
| 9 | Wire LSP through `Database` | `crates/lsp/` | Yes (functional change) |
| 10 | Remove `lower_source_file` convenience, old `HIR` struct | `crates/hir/` | Yes (tests updated) |

Steps 3–8 are the critical path. Steps 9–10 are cleanup.

## Open questions

1. **`StableFnId` uses `SyntaxNodePtr`** — this is a rowan-internal pointer that may
   not be `Send` or `Sync`. Check whether it works as a salsa interned field (needs
   `Clone + Hash + Eq`). If not, fall back to `(file, name, index)` tuple.

2. **`TypeInterner` freeze cost** — the pre-population walk is O(total type refs in
   item signatures). For a 1000-file crate this matters. At current scale (single
   file, ~50 items) it's negligible. If it ever becomes a bottleneck, the walk can
   be parallelized because it's read-only after the initial parse.

3. **`SyntaxNodePtr` stability across file edits** — rowan's `SyntaxNodePtr` is
   based on `GreenNodePtr` which contains a `u32` offset and a `u16` kind. If the
   file is edited, offsets above the edit point shift. Salsa handles this by
   re-executing `parse()` on any edit, which produces new `SyntaxNode`s and thus
   new `SyntaxNodePtr`s. So `StableFnId` naturally changes when the file structure
   changes — correct behavior.
