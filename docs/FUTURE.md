# Eye — language and compiler status

What the compiler ships today, known limitations, and where work is headed.
For how to extend the pipeline, see [`adding-features.md`](adding-features.md).
For long-term language vision, see [`VISION.md`](VISION.md).

## Pipeline

```
.eye source → lexer → rowan CST → typed AST → HIR → C → clang → native binary
```

- Lossless CST, typed AST, arena HIR, C transpile, clang link.
- Source-mapped diagnostics at lexer, parser, and HIR. The driver exits before
  codegen if any stage reports errors.
- Per-file output: `<file>.c` next to source, native binary alongside.
- Optional `clang-format` on generated C.

HIR lowering lives in [`crates/hir/src/core/lower/`](crates/hir/src/core/lower/).
Codegen lives in [`crates/codegen/src/core/`](crates/codegen/src/core/). Both are
split by concern (same pattern as rust-analyzer-style crates).

**Not implemented:** separate typechecker pass, multi-file modules, optimizations,
incremental compilation, non-C backends.

## Editor support (`eye-lsp`)

| Area | Shipped | Limitations | Oversights |
|------|---------|-------------|------------|
| Transport | stdio LSP via `lsp-server` | — | — |
| Highlighting | Semantic tokens (lexer + CST classification) | Legend slots `METHOD`, `FALLBACK` reserved; not all contexts use distinct types | No `semanticTokens/range` |
| Diagnostics | Parser errors on `didOpen` / `didChange` | No HIR diagnostics yet | — |
| Protocol | Shutdown, `didClose`, `MethodNotFound` for unknown requests | No completion, hover, goto-def | — |
| Docs | [`editor-setup.md`](editor-setup.md) | Client-specific JSON varies by extension | — |

Build: `cargo build -p eye-lsp`. Debug: `EYE_LSP_LOG=1`.

---

## Completed features by version

### v0.1 — core surface

| Area | Shipped | Limitations | Oversights |
|------|---------|-------------|------------|
| Items | `structure`, `fn` (call-form name), fields | Single file; flat `ItemScope` | — |
| Lets | `let` / `mut`, optional type, struct literal shorthand | **No type inference** — untyped `let` leaves `ty: None`; codegen emits `/* EXPLICT TYPE MISSING */` until v0.5 | No HIR test for untyped `let` + enum variant |
| Control flow | `if` / `else`, `loop` / `break` / `continue` | `break` / `continue` store no optional value | — |
| Expressions | literals, paths, calls, fields, binops, blocks, tail expr | Call results have no `expr_types` entry | — |
| `print` | Builtin → `printf`, format from HIR type or literal | Builtin only | — |
| Driver | `eye <file.eye>`, dump flags | Hard stop on HIR diagnostics | — |

**Samples:** `eyesrc/main.eye`, `design.eye`, `physics.eye`  
**E2E:** `main_eye_compiles_runs_and_prints_expected_output`, `arithmetic_expression_evaluates_correctly`, `print_eye_covers_every_format_specifier`

### v0.2 — references and parameters

| Area | Shipped | Limitations | Oversights |
|------|---------|-------------|------------|
| Types | `&T`, `T*` | `TypeRef` remains `Path(name)` in HIR; codegen resolves by name | — |
| Functions | `ParamList`, return types | — | `T*` in let-type position not fully disambiguated in parser |
| Expressions | `&`, `*`, assignment, ref parameters | One level of auto-deref in field lookup | — |

**Sample:** `eyesrc/particle.eye`

### v0.3 — enums and match

| Area | Shipped | Limitations | Oversights |
|------|---------|-------------|------------|
| Enums | `enum X = A \| B ;`, flat variant index, cross-enum name collision error | Tagless C enums; bare variant names are global in C output | — |
| Variant access | `Shape.Circle` and bare `Circle` when unique | Enum type in value position → diagnostic + `Expr::Missing` | — |
| `match` | Parse, lower, exhaustiveness, duplicate / unreachable arms | No payloads, guards, or-patterns, or bindings in patterns | Match inside ternary-`if` not hoisted (see [`M5.md`](M5.md)) |
| Codegen | Statement `switch`; value-position `_matchN` hoist | Match temp falls back to `int32` when first arm has no recorded type | Block-bodied match arms documented but not required for M6 fixture |
| | | Non-enum scrutinee: diagnostic; exhaustiveness skipped | Match-in-ternary intentionally untested |

**Spec fixture:** `eyesrc/v03.eye`  
**E2E:** `v03_eye_lowers_match_and_prints_expected_output`  
**Detail:** milestone archive at [Archive — v0.3 milestones](#archive--v03-milestones) below.

### v0.4 — kernel substrate

Aligned with [`VISION.md`](VISION.md): machine types, casts, FFI, union, arrays —
not sum types, `for`, or class syntax.

| Area | Shipped | Limitations | Oversights |
|------|---------|-------------|------------|
| Integers | `int8`…`int64`, `uint8`…`uint64`, `usize` / `isize` | `usize` width is platform-defined | — |
| Casts | `expr as Type` | C cast semantics; no Eye-side cast safety | — |
| FFI | `extern { ... }`, `ptr` → `void*` | Linker binds symbols; `uint64` vs `size_t` on libc can warn — use `usize` for `size_t` params | `ptr` misuse diagnosed by clang on `void*`, not in Eye HIR |
| Union | `union X { ... }`, one field per literal | Overlapping storage — second field in literal is a lowering error | — |
| Arrays | `[T; N]`, `[...]` literal, `base[i]` rvalue and lvalue | Length must be integer literal (no const expr yet); **1D local arrays** are the supported path; in cast / return / param positions arrays decay to `elem*` | Multi-dim, whole-array assign, pointer arithmetic, indexing a bare array literal: not specified |

**Samples:** `eyesrc/v04.eye`, `ffi.eye`, `arrays.eye`  
**E2E:** `v04_eye_lowers_primitives_and_casts`, `cast_expr_compiles_and_truncates`, `sized_integer_types_compile_and_print`, `ffi_eye_links_libc_and_lowers_union`, `arrays_eye_lowers_fixed_arrays_and_indexing`

---

## Cross-cutting limitations

These apply across the versions above and motivate v0.5.

| Topic | State |
|-------|--------|
| Type inference | No inference for untyped `let`; annotate types or wait for v0.5 |
| Call types | `expr_types` does not record call return types (affects match hoist temps) |
| HIR types | `TypeRef::Path(name)` only; no `StructId` on types until codegen |
| Semantics | Checks live in lowering, not a separate typecheck pass |
| Scope | One source file per compile; duplicate names → diagnostic and shadow |
| Match | See v0.3 row; sum types belong in stdlib per vision, not kernel syntax yet |

---

## Roadmap — v0.5 (active)

**Theme:** typing hygiene and documentation accuracy — no new surface syntax.

| ID | Deliverable |
|----|-------------|
| D1 | This doc set accurate (FUTURE, adding-features, README, M5 banner) |
| T1 | `let` inference from initializer when type is omitted |
| T2 | Call return types — user `fn` from `Function::ret`, `extern` from signature |
| T3 | Match hoist temp uses real type; `int32` fallback only when unavoidable |
| T4 | Codegen: fix `EXPLICT` → `EXPLICIT`; drop placeholder for inferred lets |
| T5 | Tests: untyped `let` + variant; typed match temp; optional `eyesrc` fixture |

**Out of scope for v0.5:** modules, payload enums, `for` / `while` syntax, supermacros.

---

## Future forks

No default path is chosen. Pick a fork before opening the next version scope.
See also [`VISION.md`](VISION.md) hinges on match extensibility and supermacro bootstrap.

### Fork A — Substrate hardening (vision-aligned, low syntax risk)

- Enforce the documented narrow array surface in HIR **or** extend array ABI with explicit milestones.
- Const array lengths (literals → named constants → minimal const-eval).
- Eye-side `ptr` restrictions before codegen emits `void*`.

### Fork B — Match and sum types (vision hinge 1)

| Option | Tradeoff |
|--------|----------|
| **B1 Closed kernel** | Richer `match` in core — fast, but locks match in the unoverwriteable kernel |
| **B2 Extensible match** | Stdlib registers pattern lowerings — enables stdlib sum types; large design |
| **B3 Stdlib-only sum types** | No new kernel syntax; manual union + tag until macros exist |

Defer until decided: payload enums, guards, or-patterns, match bindings.

### Fork C — Compiler scale-out

| Option | Tradeoff |
|--------|----------|
| **C1 Multi-file modules** | Real programs; multiplies scope and tests |
| **C2 Separate typecheck pass** | Cleaner `lower/`; refactor cost |
| **C3 Early supermacro engine** | Stdlib-first features; very large (hinge 2) |

### Fork D — Control-flow polish (low priority)

- Block-bodied match arms, match-in-ternary hoist, `break` with value.
- `while` / `for` as syntax vs stdlib over `loop` + `if` + `break` (vision prefers stdlib).

### Fork E — Horizon (~v10)

Supermacros, privilege rings, stable AST API for extensions — vision only.

**Suggested sequence:** finish v0.5 → Fork A → decide Fork B before payload-enum or match-syntax work.

---

## Working sample programs

| File | Exercises |
|------|-----------|
| `eyesrc/main.eye` | struct, let, field access, print |
| `eyesrc/design.eye` | loops, if, assignment, mutation through ref |
| `eyesrc/particle.eye` | reference parameter, field mutation |
| `eyesrc/physics.eye` | nested structs, conditionals, mixed `print` |
| `eyesrc/print.eye` | every `print` format specifier |
| `eyesrc/v03.eye` | enums, match (statement + value position) |
| `eyesrc/v04.eye` | sized / unsigned ints, `as` casts |
| `eyesrc/ffi.eye` | `extern`, `ptr`, `union`, libc link |
| `eyesrc/arrays.eye` | `[T; N]`, literals, indexing, `mut` arrays |

## Test map

| Layer | Location | Notes |
|-------|----------|-------|
| Parser | `crates/parser` snapshots + unit tests | CST round-trip |
| HIR | `crates/hir/src/core/tests.rs` | 21+ lowering tests |
| Codegen | `crates/codegen/src/core/tests.rs` | match hoist, arrays, print |
| LSP | `crates/lsp` lib tests | Semantic tokens, CST roles, document store, parser diags |
| E2E | `tests/e2e.rs` | 10 driver build-and-run tests |

**Documented gaps:** untyped `let` + enum variant; match-in-ternary; call-typed match arm body.

---

## Archive — v0.3 milestones

Historical checklist for v0.3 (all complete). Algorithm detail for match hoist:
[`M5.md`](M5.md).

**Goal:** variant access, `match` with exhaustiveness. No payloads, guards, or-patterns, bindings.  
**Spec:** `eyesrc/v03.eye`

- [x] **M1** — `enum X = A | B ;` grammar, parser, AST.
- [x] **M2** — variant access: `Resolution::Variant`, flat index, `FieldExpr` shortcut for `E.V`, collision diagnostics.
- [x] **M3** — `match` parse: `match` / `_` tokens, `MatchExpr` / patterns in ungrammar, struct-lit suppression on scrutinee.
- [x] **M4** — HIR `Expr::Match`, `lower_match_pat`, exhaustiveness bitmap, duplicate / unreachable arms.
- [x] **M5** — codegen Strategy A: statement `switch`, value hoist `_matchN`, `core/*` split by concern.
- [x] **M6** — e2e `v03_eye_lowers_match_and_prints_expected_output`.
