# Eye

A small, statically-typed language that transpiles to C and links through
`clang`. The compiler is written in Rust as a workspace of focused crates,
modelled after the rust-analyzer architecture: lossless CST, typed AST,
arena-backed HIR, and a stateless code generator.

```
.eye source  -->  lexer  -->  rowan CST  -->  typed AST  -->  HIR  -->  C  -->  clang  -->  native binary
```

## Documentation

| File                                             | Purpose                                                                                                     |
| ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| [`FUTURE.md`](docs/FUTURE.md)                    | **Status ledger** — what ships per version (v0.1–v0.4), limitations, oversights, v0.5 roadmap, future forks |
| [`VISION.md`](docs/VISION.md)                    | Long-term language vision (kernel vs stdlib, supermacros) — not current implementation                      |
| [`adding-features.md`](docs/adding-features.md)  | How to extend the pipeline (lexer → HIR → codegen)                                                          |
| [`editor-setup.md`](docs/editor-setup.md)        | Configure `eye-lsp` in VS Code / Cursor                                                                     |
| [`MATCH.md`](docs/ATCH.md)                       | Kernel-scope design note for `match` as discrete discriminant dispatch                                      |
| [`LSP.md`](docs/LSP.md)                          | Capability audit for the current `eye-lsp` server                                                           |
| [`M5.md`](docs/M5.md)                            | Historical design brief for v0.3 match codegen hoist                                                        |
| [`crates/ast/eye.ungram`](crates/ast/eye.ungram) | Grammar source; run `cargo run -p xtask -- codegen` after edits                                             |

## Layout

| Path             | Purpose                                                    |
| ---------------- | ---------------------------------------------------------- |
| `src/main.rs`    | `eye` binary - driver wiring the pipeline together         |
| `crates/token`   | Static token kinds and `T![...]` macro                     |
| `crates/lexer`   | Logos-based lexer, interner, source-text helpers           |
| `crates/syntax`  | `SyntaxKind` + rowan-typed `SyntaxNode`/`Token`            |
| `crates/parser`  | Pratt parser, error recovery, snapshot tests               |
| `crates/ast`     | Generated typed AST over the CST                           |
| `crates/hir`     | Name resolution + arena-allocated HIR                      |
| `crates/codegen` | HIR -> C transpile                                         |
| `crates/lsp`     | `eye-lsp` language server (semantic tokens + parser diags) |
| `crates/xtask`   | Codegen helpers (regenerating AST from ungrammar)          |
| `eyesrc/`        | End-to-end sample programs                                 |
| `tests/`         | Workspace-level integration tests                          |

## Prerequisites

- Rust toolchain (stable, edition 2024).
- `clang` on `$PATH` - used as the C backend.
- `clang-format` is optional; the driver formats generated C when present
  and falls back to raw layout otherwise.

## Build

```sh
# debug build of the eye driver + all crates
cargo build

# release build (used for sample-program benchmarks)
cargo build --release

# just type-check; faster than build
cargo check --workspace
```

## Compile an Eye program

```sh
# default: quiet compile, writes <file>.c and the linked binary alongside
cargo run -- eyesrc/main.eye

# show internal IRs for debugging
cargo run -- eyesrc/main.eye --dump-cst
cargo run -- eyesrc/main.eye --dump-ast
cargo run -- eyesrc/main.eye --dump-symbols
cargo run -- eyesrc/main.eye --dump-hir

# combine flags freely
cargo run -- eyesrc/physics.eye --dump-ast --dump-hir

# print driver help
cargo run -- --help
```

The driver writes `eyesrc/main.c` and an executable `eyesrc/main` next to
the source file. Run it directly:

```sh
./eyesrc/main
```

## Tests

```sh
# every crate in the workspace
cargo test --workspace

# one crate at a time
cargo test -p eye-parser
cargo test -p eye-hir
cargo test -p eye-codegen
cargo test -p eye-lsp

# a single test by substring
cargo test -p eye-codegen print_format_specifiers
```

Snapshot tests use `insta`. Review and accept changes with:

```sh
cargo insta review
```

## Regenerating the AST

The typed AST is generated from `crates/ast/eye.ungram`. After editing the
grammar:

```sh
cargo run -p xtask -- codegen
```

This rewrites `crates/ast/src/generated.rs`.

## Lints & formatting

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

## Cleaning up

```sh
# blow away ./target (does not touch generated C/binaries in eyesrc/)
cargo clean
```
