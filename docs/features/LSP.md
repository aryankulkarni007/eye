# Eye LSP Capability Audit

Audited locally on 2026-05-29 against `crates/lsp`.

## Server Shape

- Binary: `eye-lsp` (`crates/lsp/src/main.rs`)
- Transport: stdio via `lsp-server`
- Entry point: `eye_lsp::run`, which sends `initialize` capabilities and then enters the server message loop
- Logging: `EYE_LSP_LOG=1` prints a one-line startup message to stderr

## Advertised Capabilities

The server capabilities are built in `crates/lsp/src/legend.rs`.

| Capability | Status | Notes |
| --- | --- | --- |
| Text document sync | Supported | Advertises `TextDocumentSyncKind::FULL`; change handling replaces the whole buffer |
| Semantic tokens | Supported | Advertises full-document semantic tokens only |
| Semantic token range requests | Not supported | `range: None` |
| Semantic token delta/refresh | Not supported | No result ids or delta request handling |
| Completion | Not supported | Not advertised and no request handler |
| Hover | Not supported | Not advertised and no request handler |
| Go to definition/declaration/type definition/implementation | Not supported | Not advertised and no request handler |
| References | Not supported | Not advertised and no request handler |
| Rename | Not supported | Not advertised and no request handler |
| Formatting/range formatting/on-type formatting | Not supported | Not advertised and no request handler |
| Document symbols/workspace symbols | Not supported | Not advertised and no request handler |
| Code actions/code lenses/inlay hints | Not supported | Not advertised and no request handler |
| Workspace folders/configuration/file watching | Not supported | Not advertised and no request handler |

## Request Handling

Implemented in `crates/lsp/src/server/requests.rs`.

- `shutdown`: handled through `Connection::handle_shutdown`.
- `textDocument/semanticTokens/full`: computes semantic tokens for the currently open document text.
- Unknown requests: answered with JSON-RPC `-32601` (`Method not found`).

If semantic tokens are requested for a URI that is not present in the document store, the server currently computes tokens for an empty string.

## Notification Handling

Implemented in `crates/lsp/src/server/notifications.rs`.

- `textDocument/didOpen`: stores the full text and publishes parser diagnostics.
- `textDocument/didChange`: applies the last content change as a full-buffer replacement and publishes parser diagnostics.
- `textDocument/didClose`: removes the document and publishes an empty diagnostic list.
- Unknown notifications: ignored.

## Diagnostics

Implemented in `crates/lsp/src/diagnostics.rs`; orchestrated in `server/notifications.rs`.

**Only parser diagnostics are published.** The LSP runs the lexer and parser on
open/change and publishes their diagnostics. HIR diagnostics (name-resolution,
type, pattern, const-eval) are not published because the LSP currently lacks
the compilation pathway to produce them (the HIR is reached only through
`lower_source_file`, which the LSP does not call). Adding HIR diagnostics is
gated on the query architecture ([QUERY.md](../design/QUERY.md)).

- Severity: `Severity::Error` → `DiagnosticSeverity::ERROR`;
  `Severity::Warning` → `DiagnosticSeverity::WARNING`.
- Published on open/change.
- Cleared on close.
- Versions are not attached to `PublishDiagnosticsParams`.
- Notes and help hints are folded into the message body (the text the editor shows
  on hover).
- Secondary labels become `related_information` for multi-span diagnostics
  (e.g. a conflicting earlier definition).
- Each diagnostic carries its code (e.g. `T001`, `R003`, `P004`) in the `code`
  field as a string, matching the CLI renderer.

## Semantic Tokens

Implemented in `crates/lsp/src/highlight`.

The server combines lexer token classes with CST-guided identifier classification. The advertised legend is:

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

The only advertised modifier is `readonly`, but the current token builder always emits modifier bitset `0`.

Current classification coverage:

- Lexer-only: keywords, booleans, wildcard, numbers, strings/chars, comments, and operators.
- CST-guided: struct names, enum names, union names as `struct`, enum variants, function names, extern function names, parameters, local bindings, fields/properties, type references, struct literal type names, field expressions, casts, array/index expressions, match patterns, and nested block expressions.

Reserved or effectively unused today:

- `method`
- `fallback`
- `readonly` modifier

## Document Store

Implemented in `crates/lsp/src/documents.rs`.

- In-memory map keyed by URI string.
- Tracks only open documents.
- Does not read files from disk.
- Does not support incremental edit application despite accepting `didChange`.

## Verification

Ran:

```sh
cargo test -p eye-lsp
```

Result: passed. The local test suite reported 7 passing library tests, 0 binary tests, and 0 doctests.
