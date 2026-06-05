# Maintenance Guide

Checklist of things that must be updated when the compiler evolves, ordered by
how often they break.

---

## 1. IR dump flags (`--dump-*`)

**Files:** `src/main.rs`, `src/dump/`, `src/cli.rs`, `tests/snapshots.rs`

Every IR change (new arena, new variant, renamed field) can change dump output.

| Flag | Maintainer action |
|------|-------------------|
| `--dump-hir` / `--dump-hir-raw` | Update `src/dump/hir.rs` when HIR structs/enums/variants change. |
| `--dump-mir` / `--dump-mir-raw` | Update `src/dump/mir.rs` when `MirBody`/`MirStmt`/`RValue` change. |
| `--dump-ast` | Update `src/dump/ast.rs` when AST node kinds or accessors change. |
| `--dump-cst` | Auto-maintained (prints `{:#?}` of the green tree). |
| `--dump-symbols` | Auto-maintained. |

### Updating snapshot tests

When dump output changes intentionally:

```sh
cargo insta review            # review pending snapshots interactively
# or, to accept all:
cargo insta accept            # accept all pending snapshots
```

Pending snapshots appear as `.snap.new` files next to the existing `.snap` files.
The four snapshot tests live in `tests/snapshots.rs` and cover:

- `mir_dump_snapshot` - `--dump-mir-raw` on a minimal program
- `c_codegen_snapshot` - `--dump-c` on a minimal program
- `hir_dump_snapshot` - `--dump-hir` on a minimal program
- `hir_raw_dump_snapshot` - `--dump-hir-raw` on a minimal program

---

## 2. Insta snapshots (parser, lexer)

**Files:** `crates/parser/src/snapshots/`, `crates/lexer/src/snapshots/`

The CST snapshot (`crates/parser/src/lib.rs`, test `cst_snapshot`) and the token
stream snapshot (`crates/lexer/src/lib.rs`, test `token_stream_snapshot`) capture
a specific input program. If the test program or the CST/token representation
changes, run `cargo insta review` to accept the new output.

---

## 3. MIR unit tests

**File:** `crates/mir/src/tests.rs`

11 tests assert structural properties of lowered MIR (stmt counts, variant kinds,
local counts). When `mir::lower::lower_function` changes its output shape:

- Update count assertions if the lowering strategy produces more/fewer statements.
- Add tests for any new `MirStmt` or `RValue` variant.
- Remove tests for deleted variants.

Run: `cargo test -p eye-mir`

---

## 4. Doctests

**Files:** `crates/hir/src/core.rs`, `crates/hir/src/core/types.rs`, `crates/lexer/src/lib.rs`

6 doctests document and test public APIs:

| API | File |
|-----|------|
| `decode_string_literal` | `crates/hir/src/core.rs` |
| `decode_char_literal` | `crates/hir/src/core.rs` |
| `TypeRef::inner_ref_ptr` | `crates/hir/src/core/types.rs` |
| `TypeRef::as_array` | `crates/hir/src/core/types.rs` |
| `TypeRef` (Display) | `crates/hir/src/core/types.rs` |
| `Interner` | `crates/lexer/src/lib.rs` |

Update when the function signature, behaviour, or output format changes.
Run: `cargo test --doc -p eye-hir -p eye-lexer`

---

## 5. Grammar document

**File:** `docs/grammar.md`

Formal EBNF of the language. Must be kept in sync with the parser:

| Grammar change | Update section |
|----------------|----------------|
| New item kind | §2 Items |
| New expression form | §6 Expressions |
| New operator or precedence change | §6, operator precedence table |
| New keyword or token | §1 Lexical Grammar |
| New pattern form | §6.9 Match Expression |

---

## 6. Benchmarks

**File:** `benches/compile.rs`

5 Criterion benchmark groups (lex, parse, hir-lower, mir-lower, full-pipeline)
use a fixed `COMPLEX_PROGRAM` (`eyesrc/programs/raytracer.eye`) and a minimal
inline program.

| Event | Action |
|-------|--------|
| New pipeline stage | Add a benchmark group |
| `COMPLEX_PROGRAM` deleted/renamed | Update `include_str!` path |
| Criterion version bump | Update `Cargo.toml` workspace dependency |

Run: `cargo bench`

---

## 7. E2E test fixtures

**File:** `tests/e2e.rs`, `tests/common/mod.rs`

Every `.eye` program that is executed at test time is either inlined or pulled
from `eyesrc/` via `include_str!`. When a source file under `eyesrc/` changes:

- Update expected stdout in the corresponding test.
- If the program no longer compiles, either fix it or move the test to the
  failure-expected path (`compile_expect_failure`).

The shared helpers in `tests/common/mod.rs` should be extended when a new test
pattern emerges (e.g. `run_driver_dump` for snapshot tests).

---

## 8. Summary: what to run before every PR

```sh
# Full test suite (fast, ~8 s)
cargo test --workspace

# Doctests (separate binary, ~1 s)
cargo test --doc -p eye-hir -p eye-lexer

# Benchmarks (compiles only; run on demand)
cargo bench --no-run

# Review pending snapshots
cargo insta review
```

If any snapshot has a `.snap.new` file, review and accept before committing.
