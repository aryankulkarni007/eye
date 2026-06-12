# Eye LSP Capability Audit

Audited locally on 2026-06-12 against `crates/lsp`.
Reference: rust-analyzer (the baseline for "what a language server can do").

## Server Shape

- Binary: `eye-lsp` (`crates/lsp/src/main.rs`)
- Transport: stdio via `lsp-server`
- Entry point: `eye_lsp::run`, which sends `initialize` capabilities and then enters the server message loop
- Logging: `EYE_LSP_LOG=1` prints a one-line startup message to stderr
- Compilation: backed by the salsa `Database` (`crates/database`). Every `didOpen`/`didChange` mutates a `SourceFileInput` handle; the server calls granular Salsa-tracked queries (`database::lex`, `database::parse`, `database::hir_diagnostics`, `database::lowered_file`) so per-function results are cached independently.

## Current Feature Set

### Implemented (3 server-side features)

| Feature | What it does | Rust-analyzer equivalent |
|---|---|---|
| Text document sync (FULL) | Notifies server on open/change/close; replaces entire buffer on each edit | Same (but r-a supports INCREMENTAL too) |
| Semantic tokens (full) | Syntax highlighting via HIR-enriched CST; 15 token types, 1 modifier | Same (r-a has more token types, delta requests, and range requests) |
| Diagnostics | Phase-gated errors from lexer, parser, and HIR; published on open/change, cleared on close; error codes match CLI output | Same (r-a has faster inlay diagnostics via incremental re-check) |

### Not yet implemented (48 features missing)

| Feature | Priority | Prerequisites |
|---|---|---|
| **Hover** (type info + docs) | High | Wire HIR type query → markdown; deferred per #39 |
| **Go to definition** | High | `NameRef` → HIR `ItemScope` target resolution |
| **Go to type definition** | Medium | Same + type lookup from HIR |
| **Find references** | Medium | Needs cross-file reference index (not just per-file names) |
| **Rename** | Medium | Find references + HIR writeback (rename in AST → re-lex) |
| **Code completion** | Medium | Type-aware completion engine; snippets for keywords/patterns |
| **Document symbols** | Medium | Outline from HIR `ItemScope` (fns, structs, consts, globals) |
| **Workspace symbols** | Low | Multi-file symbol index |
| **Inlay hints** (type hints) | Low | HIR type annotations on let-bindings and return positions |
| **Code actions** | Low | Fix suggestions per diagnostic code (e.g. "add missing variant") |
| **Formatting** | Low | No formatter exists; CST round-trip could enable trivial reformatting |
| **Call hierarchy** | Low | Call graph from MIR `MirBody` call instructions |
| **Semantic tokens range** | Low | Trivial once full tokens work |
| **Semantic tokens delta** | Low | Needs result-ids and diff computation |
| **Import management** | Low | No module system yet |
| **Multi-file / project** | Low | Multiple `SourceFileInput` handles + inter-file resolution in HIR |
| **On-type formatting** | Low | Auto-indent, bracket closing |
| **Completion resolution** | Low | Additional text edits after completion |
| **Signature help** | Low | Function param info on `(` |
| **Workspace folders / config** | Low | Multi-root workspace support |
| **Code lenses** (run/test) | Low | No test framework |
| **Type hierarchy** | Low | Not in HIR yet |

### Comparison: what rust-analyzer has that we are explicitly not chasing

- **Proc-macro expansion**: Eye has no proc macros
- **Trait resolution / Chalk**: Eye has no traits
- **Generics / lifetime inference**: Eye has no generics or lifetimes
- **Built-in documentation viewer**: Eye has no doc comments yet
- **Extensive `cfg` / conditional compilation**: Eye has no conditional compilation
- **Lint integration (clippy)**: Eye has no external lint system

### How the LSP dispatches requests

Implemented in `crates/lsp/src/server/requests.rs`.

- `shutdown`: handled through `Connection::handle_shutdown`.
- `textDocument/semanticTokens/full`: retrieves the `SourceFileInput` from `DocumentStore`, calls `database::lex`, `database::parse`, and `database::lowered_file` queries, then computes semantic tokens from the HIR-enriched CST.
- `textDocument/hover`: *deferred*. Not advertised and no handler registered. Planned for a future pass that exposes HIR type information as markdown content.
- `textDocument/definition`: *deferred*. Not advertised and no handler registered. Planned for a future pass that resolves `NameRef` targets from the HIR `ItemScope`.
- Unknown requests: answered with JSON-RPC `-32601` (`Method not found`).

If semantic tokens are requested for a URI that is not present in the document store, the server returns an empty token list.

### How the LSP handles notifications

Implemented in `crates/lsp/src/server/notifications.rs`.

- `textDocument/didOpen`: creates a `SourceFileInput` via `Database::new`, stores it in `DocumentStore`, and publishes diagnostics from all three compiler phases.
- `textDocument/didChange`: calls `SourceFileInput::set_text` on the existing handle (bumps the salsa revision, invalidating cached query results), then publishes diagnostics.
- `textDocument/didClose`: removes the document and publishes an empty diagnostic list.
- Unknown notifications: ignored.

## Diagnostic Pipeline

Implemented in `crates/lsp/src/diagnostics.rs`; orchestrated in `server/notifications.rs`.

**All three phases are published.** The server calls `database::lex`, `database::parse`, and `database::hir_diagnostics` separately. Short-circuit semantics match the CLI driver: lexer errors hide parse diagnostics, parse errors hide HIR diagnostics. The granular query approach means per-function body diagnostics (`database::hir_diagnostics` → `database::lower_fn`) are cached independently — an edit that re-parses but leaves a body node unchanged re-runs only the changed body's `lower_fn`.

- Severity: `Severity::Error` -> `DiagnosticSeverity::ERROR`; `Severity::Warning` -> `DiagnosticSeverity::WARNING`.
- Published on open/change.
- Cleared on close.
- Versions are not attached to `PublishDiagnosticsParams`.
- Notes and help hints are folded into the message body (the text the editor shows on hover).
- Secondary labels become `related_information` for multi-span diagnostics (e.g. a conflicting earlier definition).
- Each diagnostic carries its code (e.g. `T001`, `R003`, `P004`) in the `code` field as a string, matching the CLI renderer.

## Semantic Tokens

Implemented in `crates/lsp/src/highlight`.

The server combines lexer token classes with CST-guided identifier classification, enriched by HIR name resolution for pattern-variable disambiguation (A5 fix). The advertised legend is:

1. `type`
2. `enum`
3. `struct`
4. `parameter`
5. `variable`
6. `property`
7. `enumMember`
8. `function`
9. `method`
10. `keyword`
11. `comment`
12. `string`
13. `number`
14. `operator`
15. `fallback`

The only advertised modifier is `readonly`, but the token builder always emits modifier bitset `0`.

Current classification coverage:

- Lexer-only: keywords, booleans, wildcard, numbers, strings/chars, comments, and operators.
- CST-guided: struct names, enum names, union names as `struct`, enum variants, function names, extern function names, parameters, local bindings, fields/properties, type references, struct literal type names, field expressions, casts, array/index expressions, match patterns, and nested block expressions.
- HIR-enriched: `BareIdentPat` in match arms is classified as `VARIABLE` when the name is absent from `ItemScope::variants`, and `ENUM_MEMBER` when it matches a known variant (fixes the A5 pattern-variable bug in `highlight/cst.rs:307`).

## Document Store

Implemented in `crates/lsp/src/documents.rs`.

- In-memory map keyed by URI string, mapping to salsa `SourceFileInput` handles (not raw strings).
- Tracks only open documents.
- Does not read files from disk.
- `didOpen` creates a new `SourceFileInput`; `didChange` mutates the existing handle via `set_text` (bumps salsa revision).

## Verification

Ran:

```sh
cargo test -p eye-lsp
```

Result: passed. The local test suite reported 8 passing library tests, 0 binary tests, and 0 doctests.
