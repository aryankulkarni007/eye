# Query-driven compiler architecture

Status: **DESIGNED, SUPERSEDED.** This document defines the *why* and *what* of
the query-driven architecture. The *how* (concrete implementation) is now
[`SALSA.md`](SALSA.md), which uses Salsa 3.x from the start instead of the
custom memoization layer described here.

## Why now

LIMITS.md identifies three structural problems that all resolve to the same
root cause: **the pipeline has no query boundary.** Passes are fused, results
are not cached, and the LSP cannot afford the HIR.

Redesigning before the kernel freezes means the query decomposition shapes how
new passes (typeck, effects) are born. Redesigning after the freeze means
migrating frozen passes out of a fused monolith - the same trap REDESIGN.md
called out at 10K lines. The codebase is larger now; the cost is higher.

## Target architecture

```
                 ┌─────────────────┐
                 │   SourceFile    │  (salsa input / mutable state)
                 │   (path, text)  │
                 └────────┬────────┘
                          │
                   ┌──────▼──────┐
                   │    lex()    │  query ──► Lexed
                   └──────┬──────┘
                          │
                   ┌──────▼──────┐
                   │   parse()   │  query ──► Parse (CST)
                   └──────┬──────┘
                          │
                   ┌──────▼───────┐
                   │ collect()    │  query ──► ItemScope (name→id maps)
                   └──────┬───────┘
                          │
              ┌───────────┼───────────┐
              │           │           │
       ┌──────▼────┐ ┌───▼────┐ ┌───▼──────┐
       │lower_fn() │ │lower() │ │lower()   │  query per FnId ──► Body
       └──────┬────┘ └───┬────┘ └───┬──────┘
              │           │           │
       ┌──────▼────┐ ┌───▼────┐ ┌───▼──────┐
       │typeck()   │ │typeck()│ │typeck()  │  query per FnId ──► TypeckResult
       └──────┬────┘ └───┬────┘ └───┬──────┘
              │           │           │
       ┌──────▼────┐ ┌───▼────┐ ┌───▼──────┐
       │mir()      │ │mir()   │ │mir()     │  query per FnId ──► MirBody
       └──────┬────┘ └───┬────┘ └───┬──────┘
              │           │           │
              └───────────┼───────────┘
                          │
                   ┌──────▼──────┐
                   │  c_code()   │  query ──► String
                   └─────────────┘
```

Each box is a **query**: a function `fn query(db: &dyn Db) -> Result<T>` whose
return value is memoized until its inputs change. Queries form a DAG: `c_code`
depends on `mir` for each function, which depends on `typeck`, which depends on
`lower_fn`, which depends on `collect`, which depends on `parse`, which depends
on `lex`, which depends on `source_text`.

## The Db trait

The database is the central state holder. Both the CLI driver and the LSP share
one:

```rust
/// The compiler database. Every query is a method on this trait.
pub trait Db {
    // ── Mutable inputs (only these change across revisions) ──

    /// Set the source text for a file. Bumps the revision counter and
    /// invalidates all cached queries.
    fn set_source_text(&mut self, file: FileId, text: String) -> &mut Self;

    // ── Ingredient queries (memoized, depend on inputs) ──

    fn lex(&self, file: FileId) -> &Lexed;
    fn parse(&self, file: FileId) -> &Parse;
    fn item_scope(&self, file: FileId) -> &ItemScope;
    fn ast_root(&self, file: FileId) -> &ast::SourceFile;

    // ── Per-function queries ──

    fn lower_fn(&self, fn_id: FnId) -> &Body;
    fn typeck(&self, fn_id: FnId) -> &TypeckResult;
    fn mir(&self, fn_id: FnId) -> &MirBody;

    // ── Output queries ──

    fn c_code(&self, file: FileId) -> &str;
}
```

## What changes

### 1. Stable IDs across revisions

`la_arena::Idx<T>` is arena-relative. A `StructId` from one compile session is
invalid in the next. Query caching requires stable identity.

**Decision: adopt salsa's model** - tracked structs and interned values get
`u32` IDs that are assigned by the database and remain stable as long as the
input content is unchanged. The `la_arena` arenas become implementation details
of the collection query, not the public handle type.

Migration path:
- Replace `pub structs: Arena<Struct>` with `structs: Vec<StructId>` +
  `FxHashMap<Text, StructId>`.
- Make `Struct`, `Enum`, `Function`, `Body` tracked (or interned) structs whose
  identity is content-hash or position-based, not arena-slot-based.
- Keep `Idx<Expr>`, `Idx<Stmt>`, `Idx<Pat>` as arena-relative within a single
  `Body` - they are never referenced across function boundaries, so session
  stability is not required there.

### 2. Replace `RefCell<TypeInterner>`

The type interner is the only interior-mutability point and the hardest
constraint on query purity. Two options, in preference order:

**A) Pre-populate and freeze.** During `collect`, walk every item signature and
intern every type the program can mention. After collection, the interner is
immutable. All later queries (body lowering, typeck, MIR, codegen) read from it
without mutation. The `error_ty` fallback is not needed if the type checker
guarantees completeness (which is the TYPECK contract).

This is the principled option. It moves type interning to the collection phase,
where it belongs - all program-level types are known after items are parsed.

**B) Interner as query output.** Have `typeck` return a `(TypeckResult, Arc<TypeInterner>)`.
Every query that adds types returns a *new* interner alongside its primary
result. Salsa sees the new interner as a different output, so callers that
depend on it get re-executed. Heavier, but preserves the current pattern of
on-demand interning.

**Decision: (A) pre-populate and freeze.** The collection query already walks
every item signature. Interning all referenced types is a natural extension.
The cost (one extra walk of small data) is negligible.

### 3. Split the lowering monolith

`lower_source_file` becomes three query groups:

```
collect(db, file) -> ItemScope + populated arenas
   │
   ├── const_eval(db, file) -> FxHashMap<Text, ConstValue>
   │
   └── lower_fn(db, fn_id) -> Body        (one query per function)
         │
         └── typeck(db, fn_id) -> TypeckResult
               │
               └── mir(db, fn_id) -> MirBody
```

Each function body is lowered independently. `lower_fn(FnId)` depends on
`collect(FileId)` for name resolution, not on other function bodies. Editing
one function's body invalidates only its own `lower_fn` + `typeck` + `mir`
queries.

### 4. Separate MIR lowering from codegen

`mir(FnId) -> MirBody` becomes a standalone query. Codegen calls it as a
dependency:

```rust
fn c_code(db: &dyn Db, file: FileId) -> String {
    let scope = db.item_scope(file);
    for fn_id in scope.functions.values() {
        let mir = db.mir(*fn_id);        // ← query, not inline call
        emit_fn(mir);
    }
}
```

This gives `--dump-mir-raw` for free (call the `mir` query and print it),
caches per-function MIR across codegen runs, and makes the MIR a real IR that
a future non-C backend can consume without re-lowering.

### 5. Wire LSP through the database

```rust
fn semantic_tokens(db: &dyn Db, file: FileId) -> SemanticTokens {
    let parse = db.parse(file);          // cached, incremental
    let scope = db.item_scope(file);     // cached
    // CST + name resolution for highlighting
}

fn diagnostics(db: &dyn Db, file: FileId) -> Vec<Diag> {
    let mut out = vec![];
    out.extend(db.parse(file).diagnostics.clone());
    if let Some(scope) = db.item_scope(file) {
        for fn_id in scope.functions.values() {
            out.extend(db.typeck(*fn_id).diagnostics.clone());
        }
    }
    out
}
```

Now the LSP calls `diagnostics(FileId)` on every keystroke. Salsa
automatically re-executes only the affected queries. A comment edit re-runs
nothing (the CST is unchanged). A function body edit re-runs only that
function's lowering, typeck, and MIR. A struct field type edit invalidates
every function that references that struct.

## Memoization strategy

### Option A: Custom lightweight (recommended now)

A simple memoization wrapper:

```rust
struct QueryCache<T> {
    /// The revision at which this value was computed.
    verified_at: Cell<u64>,
    value: RefCell<Option<T>>,
}

impl<T: Clone> QueryCache<T> {
    fn get_or_compute(&self, db: &dyn Db, compute: impl FnOnce() -> T) -> T {
        if self.verified_at.get() == db.revision() {
            return self.value.borrow().clone().unwrap();
        }
        let value = compute();
        self.verified_at.set(db.revision());
        *self.value.borrow_mut() = Some(value.clone());
        value
    }
}
```

No dependency tracking - any edit busts everything. This is coarse but trivial
to implement and gives immediate wins:
- LSP can call HIR queries without re-lowering on every keystroke (full
  pipeline runs once, subsequent keystrokes return cached results).
- Same query called N times in one pass runs once.
- The query shape is established for a future salsa swap.

**Cost:** edit a comment → still re-lex. Acceptable at current scale.

### Option B: Salsa (when needed)

Replace `QueryCache` with `#[salsa::tracked]` structs and `#[salsa::query]`
annotations. The `Db` trait becomes a `salsa::Database`. Fine-grained
invalidation happens automatically.

The migration from (A) to (B) is mechanical: the query signatures and
decomposition do not change, only the memoization backing.

## Migration plan

### Phase 1: Decompose (this session)

- Split `lower_source_file` into `collect`, `lower_fn`, `typeck`, `mir` queries.
- Decouple MIR lowering from codegen: `lower_function` becomes `mir(FnId)` query.
- Introduce `Db` trait with revision counter.
- Wrap each query in `QueryCache`.

**Deliverable:** Pipeline still runs top-to-bottom, but each phase has a query
boundary. LSP can call any query and get a cached result within a revision.

### Phase 2: Stabilize IDs (next session)

- Replace `la_arena::Idx<T>` for top-level items with database-assigned IDs.
- Make `TypeInterner` pre-populated and frozen after collection.
- Remove `RefCell<TypeInterner>`.

**Deliverable:** No interior mutability. IDs survive across revisions (same
struct → same `StructId` as long as the source is unchanged).

### Phase 3: Wire LSP (gated on Phase 1)

- Add `set_source_text` to the LSP's `didChange` handler.
- Replace CST-only highlighting with `semantic_tokens(db, file)` query.
- Add `diagnostics(db, file)` query that returns HIR diagnostics.

**Deliverable:** Semantic errors in the editor. Incremental by construction.

### Phase 4: Salsa (optional, deferred)

- Replace `QueryCache` with `#[salsa::tracked]`.
- Drop the manual revision counter.

**Deliverable:** Fine-grained incremental recomputation. No change to query
shape.

## Open decisions for next session

- **FileId scheme.** Single-file today. For multi-file: path-hash, sequential
  index, or content-addressed? Path-hash is simplest and stable across
  revisions.
- **Query error handling.** Return `Result<T, QueryError>` from every query,
  or store diagnostics in the database? The TYPECK pattern (diagnostics as
  side-channel via accumulator) is clean but requires a salsa-style
  accumulator. Interim: each query returns its diagnostics inline.
- **Body ID stability.** `BodyId = Idx<Body>` is arena-relative and two
  compilations of the same function produce different IDs. For MIR caching this
  does not matter (the query key is `FnId`, not `BodyId`), but the inconsistency
  is worth noting.
- **Parallelism.** Salsa supports parallel query evaluation. The custom
  `QueryCache` does not (it requires `&mut Db` for writes). Parallelism is not a
  current need but the Db trait should not preclude it.
