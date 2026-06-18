# lsp roadmap

this is the build plan from the current 3-feature server to a language server
that exceeds rust-analyzer's real-world capability. it is forward-looking: each
phase has a scope, a duration, and named deliverables. status is bolded at each
phase header. see [LSP.md](../features/LSP.md) for the current capability audit
and [TYPECK.md](../features/TYPECK.md) for the typeck split the intelligence
phase depends on.

for grounding, read alongside:

- [LSP.md](../features/LSP.md) — current capability audit (48 features missing)
- [MASTERPLAN.md](MASTERPLAN.md) — horizon-level strategic map
- [TYPECK.md](../features/TYPECK.md) — sealed-body inference (phase 3 prerequisite)
- [FUTURE.md](FUTURE.md) — language and compiler status
- [VFS.md](../features/VFS.md) — the build-tool virtual filesystem (archiver substrate)

---

## why we can beat rust-analyzer

rust-analyzer is the gold standard but carries structural constraints that eye,
by construction, does not:

| constraint | rust-analyzer | eye lsp opportunity |
|---|---|---|
| language complexity | traits, generics, lifetimes, proc macros → many answers are best-effort or deferred | no traits, generics, lifetimes → **100% correct type and reference answers**, always |
| incremental engine | custom, battle-hardened but opaque | salsa from day one — same correctness model, **leaner query graph** (no trait resolution, no chalk) |
| pipeline coupling | name resolution, type inference, and macro expansion deeply interleaved | **typeck split (horizon 1)** makes queries pure — diagnostics, types, and references decoupled, each independently cached |
| const evaluation | limited (const generics are hard, const fn hit a complexity wall) | eye's const system is scalar-only, small, finite → **inline preview of every folded value**, every time |
| generated code | no output to show | eye transpiles to c → **inline c preview** as a native ide feature |
| workspace model | every crate is a compilation unit; analysis needs full build graph | single-file + archiver-backed vfs (see [VFS.md](../features/VFS.md)) — **no build graph, no metadata, instant open** |

---

## where the tree stands

audited 2026-06-13 against `crates/lsp`. reference: rust-analyzer.

### shipped (3 server-side features)

| feature | what it does | r-a equivalent |
|---|---|---|
| text document sync (FULL) | notifies server on open/change/close; replaces entire buffer on each edit | same (r-a has INCREMENTAL too) |
| semantic tokens (full) | syntax highlighting via HIR-enriched CST; 15 token types, 1 modifier | same (r-a has more token types, delta, and range) |
| diagnostics | phase-gated errors from lexer, parser, and HIR; published on open/change, cleared on close; error codes match CLI output | same (r-a has faster inlay diagnostics via incremental re-check) |

### not yet implemented (48 features missing)

| feature | priority | prerequisites |
|---|---|---|
| hover (type info + docs) | high | wire HIR type query → markdown |
| go to definition | high | `NameRef` → HIR `ItemScope` target resolution |
| go to type definition | medium | same + type lookup from HIR |
| find references | medium | needs cross-file reference index |
| rename | medium | find references + HIR writeback |
| code completion | medium | type-aware completion engine; snippets |
| document symbols | medium | outline from HIR `ItemScope` |
| workspace symbols | low | multi-file symbol index |
| inlay hints | low | HIR type annotations on let-bindings |
| code actions | low | fix suggestions per diagnostic code |
| formatting | low | no formatter exists; CST round-trip could enable it |
| call hierarchy | low | call graph from MIR `MirBody` |
| semantic tokens range | low | trivial once full tokens work |
| semantic tokens delta | low | needs result-ids and diff computation |
| on-type formatting | low | auto-indent, bracket closing |
| completion resolution | low | additional text edits after completion |
| signature help | low | function param info on `(` |
| code lenses | low | no test framework yet |
| type hierarchy | low | not in HIR yet |

### comparison: what rust-analyzer has that we explicitly do not chase

- proc-macro expansion: eye has no proc macros
- trait resolution / chalk: eye has no traits
- generics / lifetime inference: eye has no generics or lifetimes
- built-in documentation viewer: eye has no doc comments yet
- extensive `cfg` / conditional compilation: eye has no conditional compilation
- lint integration (clippy): eye has no external lint system

### server shape

- binary: `eye-lsp` (`crates/lsp/src/main.rs`)
- transport: stdio via `lsp-server` v0.7
- compilation: backed by salsa `Database` (`crates/database`). every `didOpen`/`didChange` mutates a `SourceFileInput` handle; per-function results cached independently.
- phase-gated diagnostics: lexer → parser → HIR, short-circuiting exactly as the CLI driver does.

### typeck split status (prerequisite for phase 3)

from [TYPECK.md](../features/TYPECK.md), verified against working tree:

+ **S0 — representation prep (BUILT).** `TypeKind::RawPtr` replaces
  `Path("ptr")` magic at every judgment site. `TypeKind::Fn` carries
  variadic flag. 21 dispatch sites verified.
+ **S1 — shadow pass (BUILT).** `crates/typeck/` walker (`infer.rs`,
  816 lines) handles every `Expr` variant. Shadow harness (`shadow.rs`)
  validates parity against lowering stamps. 335 workspace tests + corpus
  regression all green. `InferObserver` trait + no-op impl built.
~ **S2 — cutover (IN PROGRESS).** step A (MIR reads `TypeckResults`) BUILT.
  step B (diagnostics migration) PARTIAL: int ranges, binary/array/enum/ptr
  judgments migrated to typeck with 286-line test suite (`judgments.rs`);
  return/tail/match/let judgments still in lowering. step C (delete
  coerce + stamping + A3 ICE + shadow) NOT YET — `adjustments`/`local_types`
  maps unpopulated, A3 `int32` fallback still live. step D (per-fn
  `typeck_fn` query) NOT YET.
- **S3 — new judgments (NOT BUILT).** M2 operand unification, assignment
  non-value, cast lattice, struct-field value types, call argument types,
  const declared-type check.
- **S4 — effects (NOT BUILT).** No `crates/effect/` exists. `EffectSet`,
  fixpoint, E-class diagnostics — design only.
- **S5 — firewall (NOT BUILT).** Structural signature backdating. `Memo<T>`
  still `Arc::ptr_eq`.
- **S6 — parallel wave (NOT BUILT).** No `rayon`/`boxcar`/`papaya` deps.

intelligence-phase LSP features (completion, inlay hints, signature help)
gate on S2 completion (for type-aware completion) and S3 (for deferred
judgment coverage).

---

## The VFS / archiver substrate

the build tool will not use a `target/` directory. instead, it produces a
compressed, cryptographically-sealed **virtual filesystem** — a single `.ivlt`
archive that contains every intermediate artifact and every dependency.

the vision (prototyped in C at `vlt/`, will be rewritten in zig then ported to
rust):

- **no `target/`** — all build state lives in a content-addressed archive. the
  tool reads it, produces a new version, writes it back. zero directory tree to
  manage, zero stale artifacts, zero hash-invalidation bugs.
- **compressed + cryptographically sealed** — every entry is checksummed. the
  archive is self-verifying on read. tamper detection, bit-rot detection, and
  incremental-rebuild safety come from the same design.
- **streaming library imports** — `import "std/io.eye"` does not fetch the
  entire stdlib. the header is resolved, the required symbols are identified,
  and only the needed contents stream from the archive. a function that calls
  `print` pulls exactly the `print` lowering — not the entire io module.
- **embedded stdlib** — the archive for the standard library can be linked into
  the `eye` binary itself. zero disk I/O to resolve `import "std/"` paths. the
  compiler fires up with the full stdlib in memory, decompressed once at init.
- **self-healing** — on read, each block is verified against its hash. a corrupt
  entry is either reconstructed (if the build tool tracks provenance) or
  re-fetched. the compiler never silently serves corrupt data.

the rust rewrite (designed here, built when the C prototype matures and the zig
port validates the wire format) will be a `crates/vfs` crate with:

```
crates/vfs/
├── src/
│   ├── lib.rs          # public API: Archive, Entry, Reader, Writer
│   ├── format.rs       # wire format: header, block, hash chain
│   ├── compress.rs     # compression codec (zstd / brotli)
│   ├── crypto.rs       # sealing: blake3 hash chain, optional ed25519 sign
│   ├── stream.rs       # streaming entry reader (pull bytes for one path)
│   ├── embed.rs        # #[link_section] archive embedding + loader
│   ├── repair.rs       # self-healing: reconstruct from hash chain
│   └── fs.rs           # vfs filesystem trait (lookup, read, entries)
```

key design decisions (ratified for the rust version):

- **content-addressed, not name-addressed** — entries are keyed by `blake3(path +
  content)`. the name → hash index is a separate tree. this is what makes
  deduplication, incremental rebuild, and corruption detection converge.
- **block-level streaming** — entries are split into blocks (default 64 KiB).
  each block has its own hash. the reader requests byte ranges; the archive
  serves only the blocks that cover those bytes. `import "std/io.eye"` that
  resolves to a single function scans the block index and streams exactly the
  blocks containing that function.
- **two-tier compression** — the hash index is uncompressed (fast random access).
  entry data is zstd-compressed per-block. the compressor can be trained on the
  eye stdlib for a custom dictionary (smaller output, faster decompression).
- **embedding** — the archive for the stdlib is compiled into a `&[u8]` static
  via `#[link_section]` or `include_bytes!`. the `EmbeddedArchive` reader
  serves it without any system calls. fallback to disk-based `.ivlt` for user
  project archives.
- **read-only at rest** — the archive format is append-only. the build tool
  produces a new archive with the changed entries; it never modifies in place.
  this is what makes the sealing chain work: every version is a strict superset
  (or replacement) rooted at a new manifest hash.
- **caching layer** — `VfsCache` wraps the archive reader with a
  recently-read-entry cache (LFU, ~256 entries). the LSP's hot loop
  (per-keystroke re-analysis) hits this cache for every file it has open.

this crate is not in the critical path for phases 0-2. it becomes important at
phase 3 (workspace symbols, multi-file reference index) and essential at phase 5
(streaming library imports, embedded stdlib preview). the prototype in C at
`vlt/` + the coming zig port will validate the wire format; the rust crate will
be built when the format is stable.

---

## phase 0 — foundation

**status: not started. estimated 4 weeks. prerequisite for all feature work.**

| item | effort | description |
|---|---|---|
| 0.1 — integration test harness | 3-5 days | snapshot-based harness: send JSON-RPC messages over an in-memory transport, assert response shape and content. cover all phases: initialize, didOpen, semanticTokens, diagnostics, hover, goto-def. the single highest-leverage item — every subsequent feature is testable without clicking an editor. |
| 0.2 — salsa structural backdating | 2-3 days | implement structural equality on `Memo<T>` so a comment-only edit stops at the lex query and never re-runs HIR. already planned in the ledger (salsa.md divergence 5). cuts diagnostic latency for trivial edits by 40%+. |
| 0.3 — LSP performance benchmarks | 2-3 days | criterion benchmarks for: didOpen → diagnostics latency, semanticTokens response, hover/def query time. baseline before any optimization. target sub-1ms for common operations. |
| 0.4 — request cancellation + timeouts | 2 days | support `$/cancelRequest`. wrap long-running requests with a timeout. essential UX during large-file edits. |
| 0.5 — VFS / source manager | 3-5 days | scaffold the `crates/vfs` crate with a basic `SourceManager` that reads files from disk, tracks open files, and maps URI ↔ file path. does not need the archive format yet — plain filesystem reads are sufficient for phases 0-2. |
| 0.6 — VS Code extension | 3-5 days | minimal TypeScript extension: `package.json` + `extension.ts` that spawns `eye-lsp` via stdio. enables real-editor testing for every feature. neovim config as secondary target. |

### sequencing within phase 0

```
week 1: 0.1 (harness) + 0.3 (benchmarks) — test infra + baseline
week 2: 0.4 (timeouts) + 0.6 (vscode) — reliability + dogfooding
week 3: 0.5 (vfs/scaffold) — multi-file foundation
week 4: 0.2 (backdating) — performance foundation
```

---

## phase 1 — navigation and information

**status: not started. estimated 3 weeks. highest-ROI feature set.**

these are the features users notice first: hover to see what something is,
ctrl-click to jump to its definition, outline to navigate the file.

| feature | effort | description |
|---|---|---|
| 1.1 — hover | 3-5 days | for `NameRef`: resolve via HIR `Resolution` → look up declared type from `ItemScope`/`Body::locals`/`Body::expr_types` → format as markdown. for `FnDef`/`StructDef` etc: show signature. for consts: show folded value. `BodySourceMap` gives exact CST position → LSP `Range`. |
| 1.2 — go to definition | 3-5 days | for `NameRef`: `Resolution` gives `FnId`/`StructId`/`LocalId` → `SyntaxNodePtr` from `ItemScope` or `BodySourceMap` → convert to LSP `Location`. zero salsa queries beyond what hover already does. |
| 1.3 — go to type definition | 2-3 days | `Body::expr_types[id]` → `TypeRef` → if `TypeKind::Path(name)`, look up `ItemScope` for the struct/enum → return its `SyntaxNodePtr`. shares 80% infrastructure with 1.2. |
| 1.4 — document symbols | 2-3 days | walk `ItemScope` (functions, structs, enums, consts, globals) + `Body::locals` → produce `SymbolInformation[]` with LSP `SymbolKind`. tree structure enables VS Code outline / breadcrumbs. |
| 1.5 — peek definition / prepare call hierarchy | 1-2 days | `textDocument/prepareCallHierarchy` — same infrastructure as go-to-def, different LSP envelope. enables in-editor peek. |

### key implementation detail

every feature in this phase reads from `lowered_file` (salsa-cached) or
`item_scope` (salsa-cached). the slowest path is a cache miss, which still
completes in <200 µs for a 100-line file. the common case (cache hit) is a
hash lookup + `SyntaxNodePtr` resolution: sub-10 µs.

---

## phase 2 — references and refactoring

**status: not started. estimated 3 weeks. gated on VFS for cross-file features.**

| feature | effort | description |
|---|---|---|
| 2.1 — find references | 5-7 days | walk `Body::exprs` in the open document → collect every `Expr::Path(Resolution)` → group by target ID → return `Location[]`. cache the per-file reference index on `DocumentStore`, invalidate on text change. for cross-file, iterate every open document's cached index. |
| 2.2 — rename | 3-5 days | same infrastructure as find-references. `textDocument/rename` → verify new name is valid (no collision in `ItemScope`, not a reserved word) → respond with `WorkspaceEdit` containing `TextEdit[]` for every reference. |
| 2.3 — workspace symbols | 2-3 days | query `ItemScope` for every file in the workspace → aggregate into a flat searchable symbol map. `textDocument/workspaceSymbol` → fuzzy-match against symbol names. |

### the reference index

the core data structure for this phase:

```rust
// stored on DocumentStore, per URI
struct ReferenceIndex {
    // for every resolved name, all its usage sites
    by_target: FxHashMap<ResolvedTarget, Vec<Location>>,
    // for every usage site, what it resolves to
    by_source: FxHashMap<SyntaxNodePtr, ResolvedTarget>,
}
```

`ResolvedTarget` mirrors the HIR `Resolution` enum but with arena IDs
converted to stable identifiers (file + `SyntaxNodePtr` of the definition).
this is what makes the index persist across salsa revision bumps — as long as
the definition node hasn't moved, the reference index stays valid.

---

## phase 3 — intelligence

**status: not started. estimated 4 weeks. gated on typeck split (horizon 1, S2-S6).**

this is where the editor stops being a fancy text editor and starts being an IDE.
every feature here reads from `TypeckResults`, which does not exist until the
typeck split is complete.

| feature | effort | description |
|---|---|---|
| 3.1 — inlay hints | 3-5 days | for untyped `let` bindings: show `: Type` from `Body::locals[id].ty`. for function call args: show parameter names. use `BodySourceMap` to find CST positions for the hint. |
| 3.2 — signature help | 2-3 days | on `(` after a function name: show `Function::params` with types and `ret` type. active parameter highlighting based on cursor position in the argument list. |
| 3.3 — code completion | 5-7 days | lexicon-based: keywords + local bindings (from `Body::locals`) + `ItemScope` names. type-aware: filter completions by expected type at cursor (infer from `LetStmt::ty`, call arg position, return-type position). snippets for common patterns (`fn`, `struct`, `if`, `match`, `extern`). |
| 3.4 — code actions | 3-5 days | map each diagnostic code to a fix: "add missing `;`" (parser), "add type annotation" (HIR), "rename to match declaration" (HIR). use `TextEdit` with computed CST position. |
| 3.5 — completion resolution | 1-2 days | additional text edits after completion selection: add `()` for functions, add `{}` for structs, add `:` after parameter name. |

### completion engine sketch

```rust
struct CompletionContext<'db> {
    db: &'db Database,
    file: SourceFileInput,
    position: LspPosition,
    // computed from parsed tree
    expected_type: Option<TypeRef>,
    is_call_arg: bool,
    is_let_init: bool,
    is_return_pos: bool,
    // available names
    locals: Vec<(Text, TypeRef)>,
    item_scope: &'db FileScope,
    keywords: &'static [(Text, CompletionItemKind)],
}
```

the engine filters candidates by `expected_type` when available (type-aware
mode), and falls back to lexical filtering otherwise. the non-type-aware path
can ship before the typeck split; the type-aware path gates on it.

---

## phase 4 — polish

**status: not started. estimated 4 weeks. independent of typeck split.**

this phase is about making the LSP feel premium — the difference between "it
works" and "it's a joy to use."

| feature | effort | description |
|---|---|---|
| 4.1 — incremental text sync | 2-3 days | switch from FULL to INCREMENTAL sync. apply `TextDocumentContentChangeEvent` ranges to the buffer instead of replacing the entire text. avoids full-parse churn on single-character edits in large files. |
| 4.2 — semantic tokens range + delta | 2-3 days | range: compute tokens for a sub-range (prerequisite: incremental computation of token diffs). delta: compare new token set against previous result ID, send only what changed. cuts semantic token message size by 90%+ on repeated edits. |
| 4.3 — formatting | 5-7 days | build a formatter using the lossless CST (rowan). the green tree preserves every whitespace token; the formatter rewrites trivia nodes (indentation, spacing around operators, line breaks). even a minimalist formatter (consistent indentation, spacing after keywords) is a major UX win. |
| 4.4 — on-type formatting | 2-3 days | auto-indent after newline (match previous line's indentation), auto-close brackets/braces, insert `;` where the parser expects it. |
| 4.5 — error span polish | 2 days | ledger architecture row: reduce lexing-time calculations, trim spans at emit time, scan for smart spans only on the error path. |
| 4.6 — LSP response time optimization | 3-5 days | profile common requests. precompute the reference index per file. optimize hot paths in semantic token computation (token_kind.rs is a match over SyntaxKind — already fast, but the CST walk in cst.rs can be memoized). target: <1ms for hover/def, <5ms for semantic tokens on 1000-line file. |

---

## phase 5 — better than rust-analyzer

**status: not started. estimated 4 weeks (parallel with phases 3-4).**

these are features that eye can offer that r-a cannot, precisely because the
language is simpler. they are eye's moat — the things a rust user would see and
say "i wish my IDE did that."

| feature | effort | description |
|---|---|---|
| 5.1 — inline const preview | 2-3 days | hover over a `const` or const-evaluated expression → show the folded value inline. `const BUFFER_SIZE = 256 * 4;` → hover shows `= 1024`. for existing const declarations, show the value in a ghost-text hint. |
| 5.2 — generated C preview | 3-5 days | custom LSP request `eye/codePreview` → return the generated C for a function or file. VS Code integration: side panel or peek view. useful for debugging codegen, learning what eye lowers to, and verifying no UB. |
| 5.3 — MIR dump in editor | 2-3 days | request `eye/mirDump` → return MIR for the current function. builds on `--dump-mir` infrastructure. a teaching tool: "what does `match` lower to?" |
| 5.4 — type flow visualization | 3-5 days | hover over a variable → highlight all expressions in the current scope that share its type (computed from `expr_types`). helps trace how a value propagates through a function. |
| 5.5 — compile-to-save validation | 2 days | on `textDocument/didSave`, run full pipeline (including MIR + codegen) and report errors. catches issues diagnostics miss (e.g. C codegen failures). |
| 5.6 — zero-latency type errors | 2-3 days | once typeck split is complete: emit type diagnostics on keystroke with no perceptible delay. salsa per-function caching makes this realistic now. the gap is not latency but correctness (typeck split). |
| 5.7 — interactive rename preview | 3-5 days | on rename request, show a diff of every reference before committing. VS Code inline diff UI. the user sees what changes before they confirm. |
| 5.8 — "explain this error" | 3-5 days | custom request `eye/explainError` → for a given diagnostic, return a pedagogical explanation: what the error means, the rule it violates, a correct example, a link to the relevant docs. the compiler already has unique error codes (T001-T036, R001-R015, P001-P009, C001-C012); each one maps to an explanation. |

### the streaming library preview (stretch goal)

combines phase 5.2 (C preview) with the VFS archiver: `import "std/io.eye"`
shows the resolved source content in a hover or peek view — decompressed from the
archive on demand, never written to disk. the user hovers over an import path and
sees the exact source the compiler will read. this is the kind of thing that is
only possible with a VFS-native build tool.

---

## the critical path

```
phase 0 ───────────────────────────────────────────────────┐
  ├─ 0.1 (test harness) ── blocks feature correctness       │
  ├─ 0.2 (backdating) ──── phase 4 perf targets             │
  ├─ 0.5 (VFS scaffold) ── blocks phase 2 cross-file refs   │
  └─ 0.6 (VS Code) ─────── blocks e2e UX validation         │
                                                             ▼
phase 1 ──────→ phase 2 ──────→ phase 3 ──────→ phase 4 ───→ phase 5
(nav, 3wk)      (refs, 3wk)     (intel, 4wk)    (polish, 4wk) (beyond, 4wk)
                                  ↑                    ↑
                              typeck split          backdating
                              (S2-S6, TYPECK.md)   (phase 0.2)
```

### the three real blockers

1. **typeck split S2-S6** — type-aware completion, hover types, inlay hints all
   depend on `TypeckResults` as a first-class query output. without it, phase 3
   ships at half power (lexicon completion only, no type filtering).

2. **salsa structural backdating (phase 0.2)** — without it, every keystroke
   re-runs the full pipeline. the current `Arc::ptr_eq` memo comparison means a
   changed file always invalidates everything. this is the single biggest
   performance lever and the difference between "fast enough" and "instant."

3. **VFS / source manager (phase 0.5)** — without a source manager that reads
   files from disk and tracks open files, workspace features (find references
   across files, workspace symbols) are limited to buffers. the archiver
   prototype in `vlt/` + zig port will validate the format; the rust `crates/vfs`
   crate is the production version.

---

## success metrics

| metric | current | phase 1 target | phase 4 target | "better than r-a" target |
|---|---|---|---|---|
| hover latency (100-line file) | n/a | <5 ms | <1 ms | <0.5 ms |
| go-to-def latency (100-line file) | n/a | <5 ms | <1 ms | <0.5 ms |
| semantic tokens (500-line file) | ~3 ms | ~3 ms | <1 ms | <0.5 ms |
| completion latency | n/a | <10 ms | <3 ms | <1 ms |
| diagnostics on keystroke | ~1 ms | ~1 ms | <0.5 ms | <0.2 ms (with backdating) |
| feature coverage (vs r-a comparable set) | 3/48 | 9/48 | 25/48 | 40/48 (remaining 8 don't apply) |
| client support | none | VS Code | VS Code + Neovim | VS Code + Neovim + Helix |
| test coverage | 11 inline | 50+ integration | 200+ integration | 500+ integration |

---

## sequencing rules

every date below is relative to phase 0 start. the LSP plan is independent of
the kernel freeze (declared) and the typeck split (in progress), but phase 3
gates on S2 completion.

| window | phase | dependencies cleared | deliverable |
|---|---|---|---|
| weeks 1-4 | 0 — foundation | nothing | test harness, benchmarks, vscode client, vfs scaffold, backdating |
| weeks 5-7 | 1 — navigation | 0.1, 0.4, 0.6 | hover, go-to-def, document symbols |
| weeks 8-10 | 2 — references | 0.5, 1.1-1.2 | find references, rename, workspace symbols |
| weeks 11-14 | 3 — intelligence | S2-S6 of typeck | inlay hints, completion, signature help, code actions |
| weeks 15-18 | 4 — polish | 0.2 | incremental sync, formatting, token delta, perf optimization |
| weeks 11-18 | 5 — beyond (parallel) | typeck + vfs | const preview, C/MIR preview, explain error, rename preview |

**total: ~4.5 months** to exceed rust-analyzer's core feature set for the eye
language. phase 5 features ship incrementally from month 3 onward.

---

## appendices

### a — the reference index schema

```rust
/// a name in the program that can be jumped to.
enum Definition {
    Function(FileId, SyntaxNodePtr),
    Struct(FileId, SyntaxNodePtr),
    Enum(FileId, SyntaxNodePtr),
    EnumVariant(FileId, SyntaxNodePtr),
    Const(FileId, SyntaxNodePtr),
    Global(FileId, SyntaxNodePtr),
    Local(BodyId, LocalId),       // body-local index
    Param(BodyId, usize),        // body-local parameter index
}

/// all known references in a file, indexed by definition.
struct PerFileIndex {
    by_definition: FxHashMap<Definition, Vec<Location>>,
    // every NameRef in the file, resolved
    name_refs: Vec<(SyntaxNodePtr, Definition)>,
}
```

### b — response time budget

the LSP spec allows servers to take as long as they need, but user perception
follows the RAIL model:

| operation | budget | eye target | margin |
|---|---|---|---|
| key → diagnostic | 50 ms (frame) | <500 µs | 100x |
| hover | 100 ms (gesture) | <1 ms | 100x |
| completion | 200 ms (typing pause) | <3 ms | 66x |
| go-to-def | 200 ms (click anticipation) | <1 ms | 200x |
| semantic tokens (full) | 16 ms (animation frame) | <1 ms | 16x |

these margins are realistic because eye's compilation pipeline is ~57 µs for a
58-line file. the LSP adds only serialization and position-mapping overhead on
top.
