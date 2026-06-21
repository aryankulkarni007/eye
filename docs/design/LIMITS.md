# Compiler architecture limitations

The pipeline works. It produces correct binaries from correct input. Its
limitations show up not in correctness but in velocity, latency, and what the
architecture permits a future contributor to build. This doc is the ledger.

Status: **PARTLY SUPERSEDED** - the salsa migration (2026-06-12,
[SALSA.md](SALSA.md)) and the S2-S6 typeck / parallel builds resolved several
constraints below (the missing query boundary, `RefCell<TypeInterner>`, the fused
passes). resolved rows are marked; the rest are live.

## Batch pipeline, all-or-nothing

The compiler is a single pass chain (`main.rs`):

```
lex -> parse -> ast -> lower_source_file -> gen_mir -> clang
```

Every phase runs to completion on the whole input before the next phase starts.
There is no caching, no incrementality, no demand-driven computation. A
one-character edit re-lexes, re-parses, re-lowers, and re-codegens the entire
file.

This is the correct design for a batch compiler processing one file at a time.
It is the wrong design for:

- **The LSP.** Every keystroke runs the full pipeline from scratch. The current
  LSP sidesteps this by *never calling the HIR at all* - it does CST-only
  highlighting and parser diagnostics. Semantic errors (name resolution, type
  mismatches) are invisible in the editor.
- **Multi-file modules.** Cross-file name resolution would re-lower every file
  on any edit.
- **Iterative development.** Changing a type definition re-lowers every function
  body, even ones that never reference that type.

## Fused passes inside lowering

`lower_source_file` (`crates/hir/src/core/lower/mod.rs:70`) does everything in
one `&mut HIR` monolith:

1. Collect const signatures
2. Fold consts to values
3. Collect globals
4. Fold globals
5. Collect all items (structs, enums, fns, extern)
6. Check value recursion
7. Lower every function body

There is no way to:
- Ask "what types are in scope?" without lowering everything.
- Cache a function body while re-parsing another.
- Re-run only the type checker after editing a single expression.

The decomposition is correct (multi-pass), but the passes are fused into one
function with no intermediate persistence.

## Codegen embeds MIR lowering

`codegen::core::mir_emit::gen_function` (line 292) calls
`lower_function(self.hir, body, ...)` inline during C emission. MIR is never
materialized as an independently inspectable IR. The `--dump-mir-raw` flag
re-lowers inside the dump path. There is no `mir(FnId) -> MirBody` query to
compose against.

## `RefCell<TypeInterner>` breaks query purity (RESOLVED 2026-06-12)

RESOLVED by the salsa migration: the interner is now lock-free and `&self`-interning
(`boxcar`/`papaya`, no `RefCell`, no `&mut`; `crates/hir/src/core.rs` notes "plain
(no `RefCell`)"). The description below is the original pre-salsa limitation, kept
for rationale.

Was: `HIR.types: RefCell<TypeInterner>` was the sole interior-mutability point in
the entire HIR, and it propagated everywhere:

- **Lowering** interps types during body construction.
- **MIR lowering** calls `self.hir.types.borrow_mut()` for error-type fallback
  (`crates/mir/src/lower.rs` throughout).
- **Codegen** calls `borrow_mut()` at init to pre-cache `error_ty`
  (`crates/codegen/src/core/mir_emit.rs:141`).
- **Field-type lookups** call `borrow_mut()` for the fallback path
  (`crates/hir/src/core/lower/ctx.rs`).

In a query system, two invocations of the same query with the same inputs must
produce the same output. A mutable interner that acquires new entries on every
error-type lookup breaks this contract - the Nth invocation may or may not have
interned the error type already, changing `TypeRef` handles.

This is manageable in a single-threaded batch compiler (the `RefCell` borrow
check panics if misused). It is a showstopper for incremental recomputation.

## Arena IDs are session-dependent

Every `Idx<T>` (which is just `la_arena`'s `u32` index) is valid only against
the specific `Arena<T>` that produced it. Across two compilations of the same
file - or across two edit keystrokes in the LSP - the same struct gets a
different `StructId`. A query cache keyed on these IDs would silently return
stale results after a recompile.

`la_arena` provides no removal and no stable identity. This is ideal for the
batch case (compaction, no fragmentation) and incompatible with persistence.

## No dependency tracking

If a struct field type changes, the current pipeline re-lowers every function
body. There is no way to know that `fn foo` references `struct Point` (so it
must be re-lowered) while `fn bar` does not (so it can stay cached).

The information to compute this exists in the HIR (every `Expr::Field` and
`TypeKind::Path` carries the name). But nothing tracks it.

## LSP bypasses HIR entirely

The LSP crate (`crates/lsp`) depends on `hir` in its `Cargo.toml` but never
calls `lower_source_file` or accesses any semantic analysis. Every
`textDocument/didChange` re-lexes and re-parses from scratch, classifies tokens
against the CST, and sends semantic tokens. HIR-level diagnostics (name
resolution, type errors, const errors) are invisible in the editor.

This is not a missing feature - it is a structural consequence of the batch
pipeline. Building HIR on every keystroke is too expensive; the LSP chose the
layer it could afford.

## Consequences

| Problem | Symptom | Root cause |
|---------|---------|------------|
| LSP has no semantic diagnostics | Editor shows parse errors only | HIR too expensive to build on every keystroke |
| No incremental compilation | Full rebuild on every change | No query granularity, no caching |
| Typeck is fused into lowering | Cannot add type inference without rewriting lowering | Lowering does too many things in one walk |
| MIR is not a standalone IR | Cannot cache per-function MIR, cannot inspect without re-lowering | Codegen calls `lower_function` inline |
| IDs are not stable | Cannot cache anything across edits | `Idx<T>` is arena-relative |

## What is NOT a limitation (preemptive)

These are sometimes cited as limitations but are not, given the project's scope:

- **Single-file compilation.** Multi-file modules are a feature, not an
  architectural gap. The pipeline generalizes to multiple files without
  restructuring.
- **Single-threaded pipeline.** The pipeline is sequential, not serialized. A
  query architecture enables parallelism, but the current throughput (single
  .eye file, sub-second compile) does not demand it.
- **C backend.** The backend is a C text emitter. MIR is target-neutral (I4 in
  REDESIGN.md). A future non-C backend swaps the emitter, not the pipeline.
