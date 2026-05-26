# Adding features to the Eye pipeline

How to extend the compiler end-to-end: lexer through HIR lowering and C codegen.
Read [`FUTURE.md`](FUTURE.md) for what is already shipped and known limitations.
Read [`VISION.md`](VISION.md) before adding kernel syntax that might belong in stdlib.

## Workspace layout

The compiler is a Cargo workspace - one crate per pipeline stage, wired by the
`eye` binary at the repo root. Dependencies flow one way:

```
token ──┬──▶ lexer ──┐
        └──▶ syntax ──┴──▶ parser ──▶ ast ──▶ hir ──▶ codegen ──▶ eye (bin)

xtask  ──▶ generates crates/ast/src/generated.rs   (off to the side)
```

| Crate          | Path                                                      | Owns                                                                 |
| -------------- | --------------------------------------------------------- | -------------------------------------------------------------------- |
| `eye-token`    | `crates/token/src/lib.rs`                                 | `Token`, `TokenKind` (+ its `logos` rules), `Diagnostic`             |
| `eye-lexer`    | `crates/lexer/src/lib.rs`                                 | `Lexer` (the `logos` driver), `SourceText`, `Interner`, `Lexed`      |
| `eye-syntax`   | `crates/syntax/src/lib.rs`                                | `SyntaxKind`, rowan binding, `T!`                                    |
| `eye-parser`   | `crates/parser/src/lib.rs` + `grammar.rs`                 | event stream, grammar, `build_tree`                                  |
| `eye-ast`      | `crates/ast/src/lib.rs` + `generated.rs` + `eye.ungram`   | typed views over the CST                                             |
| `eye-hir`      | `crates/hir/src/core/` + `core/lower/`                    | `HIR`, `lower_source_file`, name resolution, `expr_types`, diags     |
| `eye-codegen`  | `crates/codegen/src/core/` + `core/{expr,stmt,matches,…}` | `CGen`, HIR → C string                                               |
| `xtask`        | `crates/xtask/src/main.rs`                                | `cargo xtask codegen` - regenerates `ast/generated.rs`             |
| `eye`          | `src/main.rs`                                             | driver: lex → parse → lower → codegen → clang                        |
| `eye-lsp`      | `crates/lsp/src/`                                         | `eye_lsp::run` — semantic tokens + parser diagnostics over LSP       |

Each crate's lib name is the short form (`use lexer::Lexer`), the package name
is `eye-*`. A stage only sees the crates below it - `ast` cannot reach into the
`parser`, `syntax` cannot reach the `lexer`. That boundary is deliberate: keep
it. `xtask` is a build tool, not part of the compiler - nothing depends on it.

## External crates the front-end leans on

| Crate       | Role                                                                                |
| ----------- | ----------------------------------------------------------------------------------- |
| `logos`     | DFA lexer. The lex rules live in `#[token]`/`#[regex]` attributes _on_ `TokenKind`. |
| `rowan`     | The lossless CST (green/red trees).                                                 |
| `ungrammar` | The grammar file format (`eye.ungram`) `xtask` reads to generate the typed AST.     |
| `smol-str`  | Inline string storage for the `Interner` - short identifiers never heap-allocate.   |
| `text-size` | `TextSize`/`TextRange` - the one byte-range type, shared with rowan.                |

## The pipeline

```
source bytes
  │  SourceText          lexer  - mmap/owned holder, line table
  ▼
tokens + Interner        lexer  - Lexer::tokenize() drives logos -> Lexed
  │  TokenKind            token  - flat token kinds, logos rules attached
  ▼
event stream             parser - Parser emits Copy POD events
  │  grammar rules        parser/src/grammar.rs - recursive-descent, resilient
  ▼
green tree (CST)         parser - build_tree drives rowan, lossless
  │  SyntaxKind           syntax - unified leaf+node kind enum
  ▼
typed AST                ast    - generated views over the CST
  │
  ▼
HIR                      hir    - lower_source_file (collect → lower bodies)
  │  lower/ split by concern: types, collect, expr, stmt, pat, matches, …
  ▼
C source                 codegen - CGen::gen_all (core/ split by concern)
  │
  ▼
native binary            eye driver - clang link, optional clang-format
```

Two invariants hold at every stage and must keep holding:

1. **Losslessness.** The CST reproduces the source byte-for-byte. `build_tree`
   asserts `green.to_string() == source.as_str()`. Trivia (whitespace,
   newlines, comments) lives in the tree - never drop it.
2. **Resilience.** The parser never bails. Malformed input yields an
   `ErrorNode` plus a diagnostic; a tree always comes out. Every accessor in
   the `ast` crate returns `Option`/iterator so a partial parse is still
   walkable.

## The shape of every node kind

A grammar construct touches the same set of crates in the same order. There is
no single place to "add a feature" - it is a slice through all of them.

| Layer       | Crate                    | What to add                                                                                |
| ----------- | ------------------------ | ------------------------------------------------------------------------------------------ |
| Token kind  | `token`                  | variant in `define_tokens!` _with_ its `logos` rule (only for new lexemes)                 |
| Syntax kind | `syntax`                 | variant in `syntax_kinds!`; map it in `From<TokenKind>`; `T!` arm for punctuation/keywords |
| Grammar     | `parser` (`grammar.rs`)  | a parse rule that opens a marker, parses, completes with a `SyntaxKind` node kind          |
| Typed view  | `ast`                    | a rule in `eye.ungram`, then `cargo xtask codegen`                                         |
| Tests       | each crate's `mod tests` | unit tests; refresh insta snapshots                                                        |

The reason it is mechanical: the `syntax` crate holds the _exhaustive_
`From<TokenKind>` match - a new `TokenKind` will not compile until it is
mapped. That compiler error is the checklist.

## Adding a new token / lexeme

Only when the surface syntax has bytes the lexer cannot already produce.

1. **`token` crate** - add a `TokenKind` variant in `define_tokens!`, with its
   display string _and_ its `logos` rule as an attribute:
   - a fixed lexeme (operator, punctuation, keyword): `#[token("…")]`.
   - a pattern (identifier-like, numeric): `#[regex(r"…")]`.
   - a lexeme that needs a diagnostic on a malformed/unclosed form: a
     `#[token("…", callback)]` whose callback `bump`s the token to its true
     end and pushes a `Diagnostic` into `lex.extras` - see `lex_string`,
     `lex_char`, `lex_block_comment`.

   `logos` resolves overlaps by **longest match first**, then `priority` as
   the tie-breaker - a keyword `#[token("loop")]` outranks the `Ident` regex
   automatically. If the derive reports a rule conflict, give the rules an
   explicit `priority = N`. `Eof` and `Illegal` carry _no_ rule: the lexer
   driver synthesizes `Eof` at end of input and `Illegal` from a lex error.

2. **`syntax` crate** - add the matching `SyntaxKind` variant in the _token
   kinds_ block of `syntax_kinds!`, then add its arm to `From<TokenKind>` (the
   build breaks until you do). Add a `T!` arm if grammar code will name it.
   A new _trivia_ kind must also be added to `SyntaxKind::is_trivia`.
3. Test in the `lexer` crate's `mod tests` - extend `keywords_vs_idents` /
   `operators_and_delimiters`, or add a focused test.

Interning: the lexer driver interns `Ident` and `String` text into the
`Interner` (a string literal without its quotes). A new identifier-like or
string-like token should be interned the same way in `Lexer::tokenize`; a pure
operator/keyword should not.

## Adding a new grammar construct (the common case)

Most features - `if`, `loop`, real params, binary operators, more types - need
no new token, only a new _node_.

1. **`syntax` crate** - add a `SyntaxKind` variant in the _node kinds_ block of
   `syntax_kinds!` (after `SourceFile`, before `ErrorNode`). Node kinds are not
   in `From<TokenKind>` - they are produced only by `marker.complete(...)`.
2. **`parser/src/grammar.rs`** - write the parse rule. The pattern, every time:
   ```rust
   fn my_thing(p: &mut Parser) {
       let m = p.open();
       p.advance();                          // a token you already know is there
       p.expect(T![...], "expected ...");    // a token that should be there
       child_rule(p);                        // a sub-node
       m.complete(p, SyntaxKind::MyThing);
   }
   ```

   - A `Marker` carries a `DropBomb` - it _must_ be `complete`d or
     `abandon`ed, or it panics. That catches unbalanced grammar code.
   - Lookahead is `p.nth(n)` / `p.at(kind)`; it skips trivia automatically.
   - Recovery: at a decision point with no valid token, call
     `p.error_and_advance("expected ...")` - it wraps the stray token in an
     `ErrorNode` and _guarantees progress_. Loops must always make progress or
     the `FUEL` guard panics ("non-progressing loop").
   - Hook the new rule into its parent rule's `if p.at(...)` dispatch.
3. **`grammar.rs` doc comment** - update the EBNF block at the top of the file.
   It is the source of truth for the grammar; keep it exact.
4. **`ast` crate** - add the typed view by editing the **grammar file**, not
   Rust:
   - Add a rule to `crates/ast/eye.ungram` mirroring the new node. Label any
     token that needs an accessor (`name:'ident'`); label one of two
     same-type children so they get distinct accessors. A node that is one of
     several others is an alternation of node names (`Expr = A | B | …`) and
     generates a typed `enum`.
   - Run `cargo xtask codegen` - it rewrites `crates/ast/src/generated.rs`
     (committed) with the struct, its `AstNode` impl, and child accessors.
   - Only a _semantic_ accessor - one that reads meaning out of a token kind,
     like `LetStmt::kind()` (`const`/`var`) or `BinExpr::op()` - is
     hand-written, as an extra `impl` block in `crates/ast/src/lib.rs`. The
     structural accessors are always generated.
5. **Tests** - unit-test the rule in the `parser` crate and the accessors in
   the `ast` crate. Refresh snapshots: `INSTA_UPDATE=always cargo test
--workspace`, then review the `.snap` diff before committing.

## The typed-AST generator

`crates/ast/eye.ungram` is the grammar; `cargo xtask codegen` turns it into
`crates/ast/src/generated.rs`. The generator (`crates/xtask/src/main.rs`)
handles the rule shapes the v0.1 grammar uses:

- `A = B | C` (all node refs) → a typed `enum`.
- a node with child nodes → `support::child` / `support::children` accessors.
- a labelled token → a `support::token` accessor.
- two children of the same type → positional accessors (`lhs`/`rhs`).

It deliberately does _not_ try to be a general ungrammar code generator - it
targets the shapes in `eye.ungram`. The hand-written half lives in
`crates/ast/src/lib.rs`: the `AstNode` trait, the `support` helpers,
`AstChildren`, and the four semantic accessor impls. `generated.rs` is
committed and must stay in sync - re-run `cargo xtask codegen` after any
`eye.ungram` edit; CI can verify with `git diff --exit-code`.

## Precedence - the expression parser

`grammar.rs::expr` is a **Pratt parser**: `expr_bp(p, min_bp)` parses an LHS,
then folds in infix operators while their left binding power is at least
`min_bp`; `infix_binding_power` is the precedence table. Prefix-unary `-` and
postfix forms (call, struct-literal) live in `lhs`. Every operator wraps its
operand with `CompletedMarker::precede`, which keeps the event buffer
append-only - no tree rewriting. To add an operator: a `TokenKind`/`SyntaxKind`
if the lexeme is new, an arm in `infix_binding_power`, and a `BinOp` variant in
the `ast` crate (`crates/ast/src/lib.rs`, the hand-written half).

## Worked example - adding `if`/`else`

The tokens (`If`, `Else`) and their `SyntaxKind`s already exist. So:

1. `syntax` crate - add node kind `IfExpr` (`if` is an expression in eye).
2. `grammar.rs` - `fn if_expr(p)`: open marker, `advance` the `if`, parse the
   condition `expr`, parse a `block`, then `if p.eat(T![else])` parse another
   `block` (or a nested `if_expr`). `complete(p, SyntaxKind::IfExpr)`. Wire it
   into `atom` (so it nests in expressions) and update the EBNF.
3. `ast` crate - add `IfExpr = condition:Expr then:Block else_branch:Block?`
   to `eye.ungram` and add `IfExpr` to the `Expr` alternation; run
   `cargo xtask codegen`.
4. Tests for parse + accessors; refresh the CST snapshot.

No `lexer` or `token` change at all - the lexemes were already there.

## Adding semantics (HIR)

After the AST parses, meaning lives in `eye-hir`. Entry point:
`lower_source_file` in `crates/hir/src/core/lower/mod.rs`.

| Submodule | Path | Typical change |
| --------- | ---- | -------------- |
| Item collection | `lower/collect.rs` | new top-level item kinds |
| Types / literals | `lower/types.rs` | new `TypeRef` shapes, literal typing |
| Expressions | `lower/expr.rs` | new `Expr` variants, `expr_types` rules |
| Statements | `lower/stmt.rs` | `let`, blocks, scopes |
| Patterns | `lower/pat.rs` | match patterns (not `let` bindings) |
| Context | `lower/ctx.rs` | resolve, alloc, field-type lookup |

`LoweringCtx` and `Scopes` are defined in `lower/mod.rs` so split `impl` blocks
in child files can access private fields (same pattern as `CGen` in codegen).

Rules:

- Name resolution and exhaustiveness belong here, not in the parser.
- Populate `body.expr_types` when codegen or match lowering needs a type.
- Add unit tests in `crates/hir/src/core/tests.rs`.
- Record user-facing limitations in [`FUTURE.md`](FUTURE.md).

## Adding codegen (C backend)

`CGen` in `crates/codegen/src/core.rs`; methods split across `core/*.rs`.

| File | Owns |
| ---- | ---- |
| `types.rs` | `map_type_ref`, `get_expr_type`, `c_declarator`, `print` specifiers |
| `items.rs` | struct, union, enum, function prologue |
| `stmt.rs` | `let`, expression statements, match hoist prelude |
| `expr.rs` | expression emission, ternary `if` |
| `matches.rs` | `switch`, `_matchN` hoist |
| `print.rs` | `print` intrinsic |

Value-position `match` is hoisted from `gen_stmt` before the use site is emitted
(see [`M5.md`](M5.md)). If you add an expression form that can contain a
match inline, update the hoist walk in `matches.rs`.

Add regression tests in `crates/codegen/src/core/tests.rs`. For externally
visible behaviour, add or extend a test in `tests/e2e.rs` and an `eyesrc/*.eye`
fixture.

## Extending `eye-lsp`

Crate layout mirrors the compiler split: [`crates/lsp/src/`](../crates/lsp/src/)

| Module | Change |
|--------|--------|
| `legend.rs` | New semantic token type — keep indices in sync with `SemanticTokensLegend` |
| `highlight/cst.rs` | Classify new AST nodes (accurate name colors) |
| `highlight/token_kind.rs` | New lexer keyword / operator mapping |
| `diagnostics.rs` | New diagnostic sources (e.g. HIR after v0.5) |
| `server/` | New LSP methods |

Tests live in each module’s `#[cfg(test)]` block. See [`editor-setup.md`](editor-setup.md).

## Gotchas

- **Exhaustive matches are the guardrail.** `From<TokenKind>` in the `syntax`
  crate has no `_` arm. A new token won't compile until mapped - follow the
  error.
- **The lexer is `logos`, the rules are data.** There is no hand-written
  scanner to edit - a new lexeme is a new attribute on a `TokenKind` variant.
  Unicode identifiers ride on the `\p{XID_Start}…` regex; an unclosed literal
  stays a real token (not an error) because its callback `bump`s and
  diagnoses rather than failing to match.
- **Ranges are `TextRange`.** `Token`, `Diagnostic` and `ParseError` all carry
  a `text-size` `TextRange`; it is the same type rowan uses, so there is no
  conversion at the CST boundary. `SourceText::line_col` takes a `TextSize`.
- **Trivia is not the parser's concern but is the tree's.** Grammar code never
  sees trivia (`nth`/`at` skip it); `build_tree` re-interleaves it.
- **`generated.rs` is generated.** Never hand-edit it - edit `eye.ungram` and
  run `cargo xtask codegen`. Hand-written AST code goes in `lib.rs`.
- **Snapshots.** `crates/lexer/src/snapshots/` and
  `crates/parser/src/snapshots/` hold the committed `*.snap` files. insta names
  each `<crate>__<module>__<name>.snap`. Any grammar/lexer change shifts a
  snapshot; regenerate with `INSTA_UPDATE=always cargo test --workspace` and
  eyeball the diff - a snapshot diff _is_ the review of your tree shape.
- **Defer semantics to HIR.** Escape-sequence decoding, name resolution, type
  checking - none belong in lexer/parser/AST. The CST and AST are syntactic and
  lossless; meaning is HIR's job.
- **Crate boundaries.** A pipeline stage is `pub` API of its crate, so
  forward-looking API is not dead-code-flagged. An item used across a crate
  boundary must be `pub` - `pub(crate)` stops at the crate edge. New stages get
  a new crate under `crates/`; `src/main.rs` stays a thin driver.

## Checklist

```
[ ] token crate     - new TokenKind variant + its logos rule  (only for new lexemes)
[ ] syntax crate    - SyntaxKind variant + From<TokenKind> arm + T! arm
[ ] grammar.rs      - parse rule, wired into its parent, EBNF updated
[ ] eye.ungram      - new rule; then `cargo xtask codegen`
[ ] ast lib.rs      - hand-written semantic accessor, only if one is needed
[ ] hir lower/      - lower the new construct; expr_types / diags as needed
[ ] hir tests       - crates/hir/src/core/tests.rs
[ ] codegen core/   - emit C; update match hoist walk if expr trees change
[ ] codegen tests   - crates/codegen/src/core/tests.rs
[ ] e2e             - tests/e2e.rs + eyesrc fixture when behaviour is user-visible
[ ] FUTURE.md       - shipped surface, limitations, oversights
[ ] snapshots       - INSTA_UPDATE=always cargo test --workspace, review the diff
[ ] cargo clippy --workspace --all-targets -- -D warnings
```
