# Review Notes: Dump Overhaul & Test Refactor

## Changes (`src/main.rs`)

- **`read_to_string`**: replaced `SourceText::from_mmap` → `SourceText::new(read_to_string(...)?)`.
- **HIR dump split**: `--dump-hir` now prints a readable summary (names, types, counts); `--dump-hir-raw` prints `{:#?}`.
- **Consistent headers**: all dumps use `--- NAME ---` (printed from `main.rs`), all go to `stdout`.
- **Status messages**: added `"lowering AST to HIR..."`, `"lowering HIR to MIR..."`, `"generating c code..."`.
- **`render()` calls**: fixed to pass `Some(input_path)`.

## CLI (`src/cli.rs`)

- Added `--dump-hir-raw` flag.

## Dump modules (`src/dump/`)

- `hir.rs`: rewritten with `dump_hir` (readable) + `dump_hir_raw` (Debug). Both print to `stdout`.
- `mir.rs`: removed duplicate headers (now printed by `main.rs`), fixed `dump_mir_raw` header.
- `ast.rs`: removed duplicate `--- AST ---` header.
- `symbols.rs`: unchanged (its header works differently).

## Test infrastructure (`tests/`)

- **`tests/common/mod.rs`**: shared helpers extracted from `e2e.rs`: `fixture_dir()`, `write_source()`, `run_program()`, `compile_expect_failure()`, `run_driver_dump()`.
- **`tests/snapshots.rs`**: new file containing 4 snapshot tests:
  - `mir_dump_snapshot` (moved from `e2e.rs`)
  - `c_codegen_snapshot` (moved from `e2e.rs`)
  - `hir_dump_snapshot` (new)
  - `hir_raw_dump_snapshot` (new)
- **`tests/e2e.rs`**: refactored to use `common::*` helpers. Removed duplicate fixture setup. Dropped unused `PathBuf` import.

## Snapshot files (`tests/snapshots/`)

- `e2e__mir_dump.snap` → moved to `snapshots__mir_dump.snap`
- `e2e__c_codegen.snap` → moved to `snapshots__c_codegen.snap`
- `snapshots__hir_dump.snap` (new)
- `snapshots__hir_raw_dump.snap` (new)
- Accepted pre-existing `parser__tests__cst_snapshot.snap.new` (drift from `print` → `println` in test source).

## Test counts

- e2e: 55 → 53 (2 snapshot tests moved out)
- snapshots: 0 → 4 (2 moved + 2 new HIR)
- Total integration: 57 (was 55)
- Full workspace: 242 tests, all passing.
