# Eye

[![CI](https://github.com/anomalyco/eye/actions/workflows/ci.yml/badge.svg)](https://github.com/anomalyco/eye/actions/workflows/ci.yml)

> **Quick install:** `curl -fsSL https://raw.githubusercontent.com/anomalyco/eye/main/scripts/install.sh | sh`

A small, statically-typed language that transpiles to C and links through
`clang`. The compiler is written in Rust as a workspace of focused crates,
modelled after the rust-analyzer architecture: lossless CST, typed AST,
arena-backed HIR, and a stateless code generator.

```
.eye source  -->  lexer  -->  rowan CST  -->  typed AST  -->  HIR  -->  MIR  -->  C  -->  clang  -->  native binary
```

## Documentation

| File                                             | Purpose                                                                                                     |
| ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| [`docs/dev/README.md`](docs/dev/README.md)       | **Doc index** - the full map of the `docs/` set by category and status                                      |
| [`docs/dev/CAPABILITIES.md`](docs/dev/CAPABILITIES.md) | **Current capabilities** - what compiles and runs today, and the mechanism behind each              |
| [`docs/planning/FUTURE.md`](docs/planning/FUTURE.md) | **Status ledger** - what ships per version (v0.1-v0.7), limitations, oversights, roadmap, future forks     |
| [`docs/design/VISION.md`](docs/design/VISION.md) | Long-term language vision (kernel vs stdlib, supermacros) - not current implementation                      |
| [`docs/dev/adding-features.md`](docs/dev/adding-features.md) | How to extend the pipeline (lexer → HIR → MIR → codegen)                                         |
| [`docs/dev/editor-setup.md`](docs/dev/editor-setup.md) | Configure `eye-lsp` in VS Code / Cursor                                                                     |
| [`docs/features/MATCH.md`](docs/features/MATCH.md) | Kernel-scope design note for `match` as discrete discriminant dispatch                                    |
| [`docs/features/LSP.md`](docs/features/LSP.md)   | Capability audit for the current `eye-lsp` server                                                           |
| [`docs/planning/M5.md`](docs/planning/M5.md)     | Historical design brief for v0.3 match codegen hoist                                                        |
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
| `crates/diagnostics` | Shared diagnostic taxonomy (8 classes), `Span`, `Sink`     |
| `crates/hir`     | Name resolution + arena-allocated HIR + semantic diagnostics |
| `crates/mir`     | Mid-level IR + HIR -> MIR lowering                         |
| `crates/codegen` | MIR -> C emitter (dumb printer)                            |
| `crates/lsp`     | `eye-lsp` language server (semantic tokens + parser diags) |
| `crates/xtask`   | Codegen helpers (regenerating AST from ungrammar)          |
| `eyesrc/lang/`   | Feature-demonstration sample programs                      |
| `eyesrc/programs/` | Full algorithm sample programs                          |
| `eyesrc/ffi/`    | Foreign-function interface sample programs                 |
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
cargo run -- eyesrc/programs/main.eye

# show internal IRs for debugging
cargo run -- eyesrc/programs/main.eye --dump-cst
cargo run -- eyesrc/programs/main.eye --dump-ast
cargo run -- eyesrc/programs/main.eye --dump-symbols
cargo run -- eyesrc/programs/main.eye --dump-hir

# combine flags freely
cargo run -- eyesrc/programs/physics.eye --dump-ast --dump-hir

# print driver help
cargo run -- --help
```

The driver writes `<file>.c` and an executable binary alongside the source
file. Run it directly:

```sh
./eyesrc/programs/main
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

### Property-based tests

[`proptest`](https://docs.rs/proptest) verifies the lexer and parser never panic
on any UTF-8 input, that token streams always tile the source without gaps,
that diagnostics have non-empty spans, and that structurally valid small
programs survive the full compilation pipeline:

```sh
cargo test -p eye proptest  # runs the proptest suite
# or run all tests including proptest (may take a few minutes):
cargo test --workspace
```

### Fuzz testing

The compiler is fuzzed with [`cargo-fuzz`](https://rust-fuzz.github.io/book/).
Three fuzz targets live in `fuzz/`:

| Target         | Description                                                  |
| -------------- | ------------------------------------------------------------ |
| `fuzz_lexer`   | Random UTF-8 → lexer (catch panics in logos callbacks)       |
| `fuzz_parser`  | Random UTF-8 → lexer + parser (catch panics in event stream) |
| `fuzz_full`    | Clean-parsed programs → HIR → MIR → codegen (catch semantic panics) |

```sh
# install cargo-fuzz (nightly required)
cargo install cargo-fuzz --locked

# build-check all targets
cargo fuzz build --fuzz-dir fuzz

# run each target for 30 seconds
cargo fuzz run fuzz_lexer --fuzz-dir fuzz -- -max_total_time=30
cargo fuzz run fuzz_parser --fuzz-dir fuzz -- -max_total_time=30
cargo fuzz run fuzz_full --fuzz-dir fuzz -- -max_total_time=30
```

CI runs a 2-second smoke test of each target on every PR to catch
regressions. Full fuzzing campaigns are run locally or scheduled.

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

## Installation (pre-built binary)

```sh
# auto-detect platform, download latest release, install to /usr/local
curl -fsSL https://raw.githubusercontent.com/anomalyco/eye/main/scripts/install.sh | sh

# install to a custom prefix
curl -fsSL https://raw.githubusercontent.com/anomalyco/eye/main/scripts/install.sh | EYE_PREFIX=~/.local sh
```

See [`scripts/install.sh`](scripts/install.sh) for the full install script.

## CI/CD

| Job               | Description                                         |
| ----------------- | --------------------------------------------------- |
| `lint`            | `cargo fmt --check` + `cargo clippy -D warnings`   |
| `test`            | Full test suite on Linux, macOS, Windows             |
| `msrv`            | Minimum supported Rust version (1.85.0) check        |
| `bench`           | Criterion benchmark compilation + smoke execution    |
| `fuzz`            | Build + 2s smoke test of every fuzz target           |
| `docs`            | `cargo doc --document-private-items -D warnings`     |
| `release`         | Builds binaries for 4 targets, creates GitHub Release |

Cross-platform testing, benchmark regression gating, and fuzz smoke tests
run on every pull request. Release binaries are automatically built and
uploaded when a `v*` tag is pushed.

## Cleaning up

```sh
# blow away ./target (does not touch generated C/binaries in eyesrc/)
cargo clean
```
