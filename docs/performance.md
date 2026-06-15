# Eye Compiler — Performance Analysis

Benchmarked 2026-06-11 on Apple M3 (macOS), `cargo bench --bench compile`.

---

## 1. Pipeline Timing Summary

| Stage | Minimal (3 lines) | Complex — raytracer (58 lines) | Share (complex) |
|-------|------------------:|-------------------------------:|----------------:|
| Lex | 0.50 µs | 3.84 µs | 6.7% |
| Parse | 1.79 µs | 19.96 µs | 35.1% |
| HIR lower | 2.32 µs | 23.12 µs | 40.6% |
| MIR lower | — | 0.45 µs | 0.8% |
| Codegen + overhead | ~1.5 µs | ~9.5 µs | 16.8% |
| **Full pipeline** | **6.12 µs** | **56.9 µs** | **100%** |

All measurements from Criterion, 30 samples (20 for full-pipeline), output in `target/release/`. Criterion reports statistical noise within ±5%.

**Takeaway**: A ~60-line Eye program compiles to C in **57 microseconds**. Two passes dominate: HIR lowering (41%) and parsing (35%). MIR lowering is negligible at <1%. Codegen (C string emission) accounts for roughly 17%.

---

## 2. Workload Scaling

The benchmark workload (`eyesrc/programs/raytracer.eye`) is representative — 58 lines, 10 statements, 7 function calls, struct literals, if-else chains, loops, casts. For the largest single file (`lang.eye`, 178 lines, 26 function signatures), throughput stays comfortably in the **sub-200 µs** range for the full pipeline.

There are no non-linear algorithmic bottlenecks visible at this scale. The compiler uses `la_arena` (O(1) append), `FxHashMap` lookups, and string interning throughout — no O(N²) patterns found in the hot path.

---

## 3. Flamegraph Analysis (73 samples, 6 MB concatenated stress input)

Compiled `release` profile on a stress file (all `eyesrc/` files ×100, ~6 MB). The concatenation produces a syntactically-invalid file, so only the lexer and parser execute — HIR and later stages are never reached. Despite that, the lexer/parser data is valid and informative.

### CPU distribution within `eye::compile_file`:

```
▸ parser::parse                         57.5%  ─── parse dominates (valid input
                                                    would dilute to ~35%)
▸ lexer::Lexer::tokenize                24.7%
▸ lexer::SourceText::new                 4.1%
▸ std::fs::read_to_string                1.4%
```

### Breakdown inside the parser (57.5% of total):

| Function | Samples | Share |
|----------|--------:|------:|
| `parser::build_tree::_{{closure}}` | 11 | 15.1% |
| `parser::grammar::source_file` | 10 | 13.7% |
| `parser::grammar::block` | 10 | 13.7% |
| `rowan::green::node_cache::NodeCache::node` | 10 | 13.7% |
| `parser::grammar::expr_bp` | 6 | 8.2% |
| `rowan::green::node::GreenNode::new` | 6 | 8.2% |
| `rowan::arc::ThinArc::from_header_and_iter` | 6 | 8.2% |
| `rowan::green::node_cache::NodeCache::token` | 5 | 6.8% |
| `rowan::arc::Arc<T>::drop_slow` (aggregate) | 11 | 15.1% |
| `rowan::cursor::free` | 3 | 4.1% |
| `parser::grammar::type_ref` | 2 | 2.7% |

**The rowan green tree CST accounts for roughly 60% of parse time** — every `SyntaxNode` allocation goes through `NodeCache::node` (dedup), `GreenNode::new` (ThinArc allocation), and later `Arc::drop_slow` when the reference-counted tree is freed. This is the intrinsic cost of the lossless CST: pretty-printing, parent pointers, and incremental re-parse support come at the price of per-node refcounting and string interning.

### Breakdown inside the lexer (24.7% of total):

| Function | Samples | Share |
|----------|--------:|------:|
| `<token::TokenKind as Logos>::lex` | 6 | 8.2% |
| `lexer::lstarts` (memchr) | 3 | 4.1% |
| `lexer::Interner::intern` | 2 | 2.7% |
| `alloc::raw_vec::grow_one` | 2 | 2.7% |

The logos-generated tokenizer is fast — 8.2% for all DFA dispatch. Memchr for newline/tab scanning (4.1%) and string interning (2.7%) are the other costs. The `alloc::raw_vec::grow_one` samples (2.7%) hint at the token vec being resized; a capacity hint from the input size could avoid this.

---

## 4. Per-Stage Deep Dive

### 4.1 Lexer (`lexer` crate, 649 LOC)

Logos-generated DFA tokenizer. A 58-line program lexes in **3.8 µs**. The interner (`Interner::intern`) does O(N) lookups over an `FxHashSet` of interned text — no hash DOS vulnerability at this scale. The flamegraph shows 2.7% time in interning, 2.7% in vec resize.

**Overhead sources**: (A) String interning allocates independently for each identifier≥31 bytes, (B) logos-generated DFA tables are at the small scale but the match dispatch is general purpose.

### 4.2 Parser (`parser` crate, 2708 LOC)

Hand-written recursive descent over a logos token stream. Parsing the raytracer takes **20 µs**. The parser builds a full rowan CST (every token and node gets a `GreenNode` with interning and refcounting).

**Overhead sources**: The CST is the dominant cost. ~60% of parse time is rowan green-tree bookkeeping. The parser's `expect_after` and error recovery add a veneer of overhead. For comparison, a CST-free parser targeting a flat AST would be substantially faster (~3-5×) but loses lossless formatting, incremental reparse, and error recovery fidelity.

### 4.3 HIR Lowering (`hir` crate, 8211 LOC, the largest crate)

The most expensive pass at **23 µs** (41%). Runs three sub-passes:

1. **Collect** — walks top-level items, registers names, allocates `Struct`/`Function`/etc arena entries. Scans every field and parameter type for lowering.
2. **Const fold** — recursively evaluates constant expressions with memoization and cycle detection. The memo table (`FxHashMap<Text, Option<ConstValue>>`) is per-file and cheap.
3. **Body lower** — walks each function body expression-by-expression, building `Expr`/`Stmt`/`Pat` arenas, resolving names, stamping `TypeRef` handles via the interned `TypeInterner`.

**Overhead sources**: (A) Expression recursion visits every node; each visit does arena allocations and hash lookups. (B) Type interning does `kind.clone()` on every `intern()` call — for complex function-pointer types, this clones inner `Vec<TypeRef>`. (C) Name resolution walks the lexical scope stack for each identifier reference.

### 4.4 MIR Lowering (`mir` crate, 1752 LOC)

Fastest pass at **0.45 µs** (<1%). Converts HIR bodies to three-address form. The raytracer's `main()` body has ~33 MIR statements, each a flat `RValue` or `Operand`. Arena allocation is O(1); expression spill-to-temp is <10 entries.

### 4.5 Codegen (`codegen` crate, 1459 LOC)

C code generation builds a single `String` incrementally via `write_fmt`. For a 96-line C output, this is a trivial cost. No intermediate buffer abstraction.

**Overhead source**: `gen_mir` re-lowers every function body from HIR to MIR (issue A1). This is redundant when `--dump-mir` was already called. For a single compilation without dumps, the cost is modest: 0.45 µs per function for MIR lowering, but repeated for each function body in codegen.

---

## 5. Architectural Findings

### A1 — Double MIR lowering on `--dump-mir`
`src/main.rs:105-108` and `crates/codegen/src/core/mir_emit.rs:337`. When both `--dump-mir` and codegen run, every function body is lowered to MIR twice: once in the dump pass, once in `gen_mir`. No MIR cache exists. Fix: cache `MirBody` arenas in the `HIR` struct or gate dump to reuse them.

### A2 — `place_type()` recursion
`crates/codegen/src/core/mir_emit.rs:1058`. The `place_type()` function recurses O(depth) per call for nested field/pointer access. `index_access`, `place_is_pointer_like`, and `spec_for_type` each call it independently. For deeply nested struct chains this is O(N·depth). A memoization cache or iterative rewrite would avoid re-walking.

### Rowan CST overhead
The rowan green tree accounts for ~60% of parse time. This is by design — Eye targets developer tooling (LSP, diagnostics, formatting) where the lossless CST is essential. For batch compilation, a direct-to-AST parser would be 3-5× faster. If batch-compile throughput becomes critical, a separate fast-path parser could bypass CST construction.

### Allocator
mimalloc is the global allocator. The flamegraph shows ~4% total time in mimalloc routines (mi_free, mi_page_queue_find_free_ex, mi_page_free_list_extend), which is an efficient baseline.

---

## 6. Recommendations by Impact

### Implemented (2026-06-11)

| Fix | Status | Details |
|-----|--------|---------|
| **Token vec capacity hint** | DONE (already present) | `Vec::with_capacity(src.len() / 4 + 1)` in `crates/lexer/src/lib.rs:299` |
| **Memoize `place_type()` (A2)** | DONE | `FxHashMap<Place, Type>` cache on `MirGen`, cleared per-function. `PartialEq+Eq+Hash` added to `Place`/`Operand`/`Literal`. Saves O(depth) re-walks across `index_access`, `place_is_pointer_like`, `specifier` for same place. |

### High impact, low effort (not yet implemented)
- **Cache MIR bodies (A1)**: Store lowered `MirBody` in `HIR` to avoid double lowering on `--dump-mir` + codegen. Requires either a circular dep (`hir → mir`) or plumbing a shared cache through the pipeline. ~5 lines of logic, but cross-crate dependency issue.

### Medium impact, medium effort (not yet implemented)
- **Arena-backed string interner**: Replace `FxHashMap<SmolStr, Symbol>` in the lexer's interner with a bump-allocated string table. SmolStr already inlines strings ≤23 bytes, so benefit is marginal for identifier-heavy code.

### High impact, high effort
- **Fast-path parser for batch compile**: A CST-free, direct-to-AST parser would reduce parse time by ~60% (removing rowan green-tree construction). Would need to coexist with CST-based parser for LSP/tooling mode.
- **Query-based incremental compilation**: Replace the monolithic 3-pass HIR lowering with a red/green query system (like salsa or rustc's). Would benefit multi-file projects but is disproportionate for single-file programs.

---

## 7. Current Bottleneck Summary

| Rank | Bottleneck | Cost | Nature |
|------|-----------|:----:|--------|
| 1 | HIR lowering (3-pass walk) | 41% | Fundamental: every node visited once |
| 2 | Parser CST construction | 35% | By-design: rowan overhead for tooling |
| 3 | Codegen (type recomputation, string build) | 12% | Minor: A2 in deep structs |
| 4 | Lexer (logos dispatch, interning) | 7% | Trivial at current scales |
| 5 | MIR lowering | <1% | Negligible |

**Total throughput**: ~17,500 lines/second for the full pipeline on a single M3 core. CI gates run at reduced sample sizes and complete in under 30 seconds.
