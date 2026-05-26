# Editor setup for Eye

The `eye-lsp` language server provides semantic syntax highlighting and parser
diagnostics for `.eye` files.

## Build the server

```sh
cargo build -p eye-lsp
```

The binary is `target/debug/eye-lsp` (or `target/release/eye-lsp` with `--release`).

## VS Code / Cursor

Add to your workspace or user `settings.json`:

```json
{
  "eye.languageServerPath": "${workspaceFolder}/target/debug/eye-lsp"
}
```

If your editor uses the generic `languageServerExample` pattern, configure a
custom language server for the `eye` file extension:

```json
{
  "languageServerExample.languageServers": {
    "eye": {
      "command": "${workspaceFolder}/target/debug/eye-lsp",
      "filetypes": ["eye"]
    }
  }
}
```

Exact keys depend on your LSP client extension. The server speaks LSP over stdio.

## Capabilities

- **Semantic tokens** (full document): keywords, types, structs, enums, functions,
  parameters, locals, fields, literals, comments, operators.
- **Diagnostics**: parser errors pushed on open/change; cleared on close.
- **Document sync**: full buffer replacement (not incremental).

## Debugging

Set `EYE_LSP_LOG=1` when starting the server to print a one-line startup message
on stderr (safe for LSP — not stdout).

## Limitations

See [`FUTURE.md`](FUTURE.md) — *Editor support (eye-lsp)*.
