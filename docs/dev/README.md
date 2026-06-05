# Eye documentation index

A map of `docs/` by category and status. Read top-down: current state first,
vision last, historical at the end. The root [`../README.md`](../README.md) covers
building and running the compiler.

Status legend: **current** (describes the tree as built), **designed** (planned,
not built), **aspirational** (long-term vision, not built), **historical**
(superseded, kept for provenance).

## Start here

| Doc | Status | Purpose |
|-----|--------|---------|
| [CAPABILITIES.md](CAPABILITIES.md) | current | What compiles and runs today, and the mechanism behind each feature. The present-tense overview. |
| [MASTERPLAN.md](planning/MASTERPLAN.md) | roadmap | The strategic map from the current tree to the vision: horizons, gating, the critical path. |
| [../README.md](../README.md) | current | Build, run, test, and the pipeline at a glance. |

## Status ledgers

| Doc | Status | Purpose |
|-----|--------|---------|
| [FUTURE.md](planning/FUTURE.md) | current | Version-by-version ledger (v0.1-v0.7): shipped surface, limitations, oversights, roadmap, future forks. |
| [KERNEL.md](design/KERNEL.md) | current | Kernel-completeness gap analysis: what remains before the unoverwriteable kernel can freeze. |
| [DEFER.md](planning/DEFER.md) | current | Deferral ledger: what was consciously deferred, why, and the condition to revisit. |
| [ledger.md](planning/ledger.md) | current | Working log of issues moved from deferred to in-progress; some items resolved inline. |

## Kernel design (per-topic, built)

| Doc | Status | Purpose |
|-----|--------|---------|
| [ARRAY.md](features/ARRAY.md) | current | Fixed arrays as a first-class value type (copy semantics, `&[T; N]`, `len`). |
| [MUT.md](features/MUT.md) | current | Immutable-by-default bindings: `let` vs `mut`, the deep-write rule, the raw-pointer escape. |
| [MATCH.md](features/MATCH.md) | current | Why kernel `match` stays a minimal discriminant dispatch, not rich pattern matching. |
| [PRINT.md](features/PRINT.md) | current | The `print` intrinsic: format-specifier selection and open points. |
| [FNPTR.md](features/FNPTR.md) | current | Function pointers: surface syntax, HIR/MIR/codegen representation, scope. |
| [TOPOLOGY.md](features/TOPOLOGY.md) | current | Type-declaration ordering and the value-recursion check (the object-topology pass). |

## Pipeline / compiler architecture

| Doc | Status | Purpose |
|-----|--------|---------|
| [adding-features.md](adding-features.md) | current | How to extend the compiler end to end: lexer -> HIR -> MIR -> codegen. |
| [REDESIGN.md](design/REDESIGN.md) | partly built | The 3-track pipeline refactor plan. Track 1 + Track 2 built; Track 3 designed. |
| [LIMITS.md](design/LIMITS.md) | current | Compiler architecture limitations: batch pipeline, fused passes, no incrementality. |
| [QUERY.md](design/QUERY.md) | designed | Query-driven compiler architecture (the response to LIMITS.md): Db trait, query decomposition, memoization. |
| [DIAGNOSTICS.md](features/DIAGNOSTICS.md) | current | The error model: 8 classes, typed per-crate kinds (Track 1, built). |
| [MIR.md](features/MIR.md) | current | The mid-level IR and HIR -> MIR lowering (Track 2, built). |
| [TYPECK.md](features/TYPECK.md) | designed | Type checking as a separate pass over frozen HIR (Track 3, not built). |
| [M5.md](planning/M5.md) | historical | v0.3 match-codegen hoist design brief; superseded by the MIR cutover. |

## Vision (aspirational, not current)

| Doc | Status | Purpose |
|-----|--------|---------|
| [VISION.md](design/VISION.md) | aspirational | The canonical kernel/stdlib thesis, supermacros, and the two open hinges. |
| [FARFUTURE.md](planning/FARFUTURE.md) | aspirational | Far-future execution brief: extensibility engine, effect system, the Cranelift jump. |
| [DESIGN.md](design/DESIGN.md) | aspirational | The forensic meta-platform strand: effect tracking, interaction bridges, meta.dev. |

## Reference / tooling

| Doc | Status | Purpose |
|-----|--------|---------|
| [LSP.md](features/LSP.md) | current | `eye-lsp` capability audit (semantic tokens, parser diagnostics, limits). |
| [editor-setup.md](editor-setup.md) | current | Configure `eye-lsp` in VS Code / Cursor. |

## Testing infrastructure

| Doc | Status | Purpose |
|-----|--------|---------|
| [../README.md](../README.md#property-based-tests) | current | Property-based tests: 7 proptest invariants (lexer/parser survival, span coverage, token invariants, full-pipeline survival). Run with `cargo test --test proptest`. |
| [../README.md#fuzz-testing](../README.md#fuzz-testing) | current | Fuzz targets for lexer, parser, and full pipeline via `cargo-fuzz`. Three targets in `fuzz/fuzz_targets/`. Run with `cargo fuzz run --fuzz-dir fuzz <target>`. CI smoke-tests each for 2s per PR. |

## CI / release

| Doc | Status | Purpose |
|-----|--------|---------|
| [../.github/workflows/ci.yml](../.github/workflows/ci.yml) | current | CI pipeline: lint, cross-platform test (Linux/macOS/Windows), MSRV check, benchmark smoke, fuzz build+smoke, doc build. Runs on every PR and push to main. |
| [../.github/workflows/release.yml](../.github/workflows/release.yml) | current | Release workflow: builds binaries for 4 targets on `v*` tag, creates GitHub Release with checksums. |
| [../scripts/install.sh](../scripts/install.sh) | current | Curl-to-sh install script: `curl -fsSL https://raw.githubusercontent.com/anomalyco/eye/main/scripts/install.sh \| sh`. Auto-detects platform, downloads latest release, extracts to `/usr/local/bin`. |

## Historical (superseded, kept for provenance)

| Doc | Status | Purpose |
|-----|--------|---------|
| [NOTES.md](planning/NOTES.md) | historical | Pre-refactor scratch and external dump that became DIAGNOSTICS / MIR / REDESIGN. |
| [AUDIT.md](design/AUDIT.md) | historical | Point-in-time architecture audit snapshot taken around the MIR cutover. |
| [M5.md](planning/M5.md) | historical | Listed under pipeline above; the v0.3 match-codegen brief. |

## Grammar source

The typed AST is generated from [`../crates/ast/eye.ungram`](../crates/ast/eye.ungram);
run `cargo run -p xtask -- codegen` after editing it. See
[adding-features.md](adding-features.md).
