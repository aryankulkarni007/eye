# Adding features to the eye pipeline

How to extend the compiler when growing `main.eye`'s v0.1 subset toward the
full language. Read this before touching the grammar.

## The pipeline

```
source bytes
  │  SourceText          src/lexer.rs   — mmap/owned holder, line table
  ▼
tokens + Interner        src/lexer.rs   — Lexer::tokenize() -> Lexed
  │  TokenKind            src/token.rs   — flat token kinds
  ▼
event stream             src/parser.rs  — Parser emits Copy POD events
  │  grammar rules        src/grammar.rs — recursive-descent, resilient
  ▼
green tree (CST)         src/parser.rs  — build_tree drives rowan, lossless
  │  SyntaxKind           src/syntax.rs  — unified leaf+node kind enum
  ▼
typed AST                src/ast.rs     — zero-cost typed views over the CST
  │
  ▼
HIR → C transpile        (not yet built)
```

Two invariants hold at every stage and must keep holding:

1. **Losslessness.** The CST reproduces the source byte-for-byte. `build_tree`
   asserts `green.to_string() == source.as_str()`. Trivia (whitespace,
   newlines, comments) lives in the tree — never drop it.
2. **Resilience.** The parser never bails. Malformed input yields an
   `ErrorNode` plus a diagnostic; a tree always comes out. Every accessor in
   `ast.rs` returns `Option`/iterator so a partial parse is still walkable.

## The shape of every node kind

A grammar construct touches the same set of layers in the same order. There is
no single place to "add a feature" — it is a slice through all of them.

| Layer | File | What to add |
|-------|------|-------------|
| Token kind | `src/token.rs` | variant in `define_tokens!` (only for new lexemes) |
| Lexing | `src/lexer.rs` | recognise it in `next_token` / a `lex_*` helper; keyword? add to `keyword()` |
| Syntax kind | `src/syntax.rs` | variant in `syntax_kinds!`; map it in `From<TokenKind>`; `T!` macro arm for punctuation/keywords |
| Grammar | `src/grammar.rs` | a parse rule that opens a marker, parses, completes with a `SyntaxKind` node kind |
| Typed view | `src/ast.rs` | `ast_node!`/`ast_enum!` wrapper + typed accessors |
| Tests | each file's `mod tests` | unit tests; refresh insta snapshots |

The reason it is mechanical: `syntax.rs` holds the *exhaustive* `From<TokenKind>`
match — a new `TokenKind` will not compile until it is mapped. That compiler
error is the checklist.

## Adding a new token / lexeme

Only when the surface syntax has bytes the lexer cannot already produce.

1. **`token.rs`** — add a `TokenKind` variant in `define_tokens!` with its
   display string. Group it with its neighbours (keyword, operator, …).
2. **`lexer.rs`** —
   - **keyword**: add one arm to `keyword()`. Nothing else; `lex_ident`
     already routes idents through it.
   - **operator/punctuation**: add a `match` arm in `next_token`. Multi-char
     operators disambiguate on `peek(1)` — see the `=`/`<`/`&` arms. **Never
     eagerly `peek(2)`**: peeking past a not-yet-consumed byte can index the
     middle of a multi-byte UTF-8 char and panic. Consume one byte, then peek
     again (see `lex_minus_or_comment`).
3. **`syntax.rs`** — add the matching `SyntaxKind` variant in the *token kinds*
   block of `syntax_kinds!`, then add its arm to `From<TokenKind>` (the build
   breaks until you do). Add a `T!` arm if grammar code will name it.
4. Test in `lexer.rs::tests` — extend `keywords_vs_idents` /
   `operators_and_delimiters`, or add a focused test.

Interning: `end_token` interns `Ident` and `String` text into the lexer-owned
`Interner`. A new identifier-like or string-like token should be interned the
same way; a pure operator/keyword should not.

## Adding a new grammar construct (the common case)

Most features — `if`, `loop`, real params, binary operators, more types — need
no new token, only a new *node*.

1. **`syntax.rs`** — add a `SyntaxKind` variant in the *node kinds* block of
   `syntax_kinds!` (after `SourceFile`, before `ErrorNode`). Node kinds are not
   in `From<TokenKind>` — they are produced only by `marker.complete(...)`.
2. **`grammar.rs`** — write the parse rule. The pattern, every time:
   ```rust
   fn my_thing(p: &mut Parser) {
       let m = p.open();
       p.advance();                          // a token you already know is there
       p.expect(T![...], "expected ...");    // a token that should be there
       child_rule(p);                        // a sub-node
       m.complete(p, SyntaxKind::MyThing);
   }
   ```
   - A `Marker` carries a `DropBomb` — it *must* be `complete`d or
     `abandon`ed, or it panics. That catches unbalanced grammar code.
   - Lookahead is `p.nth(n)` / `p.at(kind)`; it skips trivia automatically.
   - Recovery: at a decision point with no valid token, call
     `p.error_and_advance("expected ...")` — it wraps the stray token in an
     `ErrorNode` and *guarantees progress*. Loops must always make progress or
     the `FUEL` guard panics ("non-progressing loop").
   - Hook the new rule into its parent rule's `if p.at(...)` dispatch.
3. **`grammar.rs` doc comment** — update the EBNF block at the top of the file.
   It is the source of truth for the grammar; keep it exact.
4. **`ast.rs`** — add the typed view:
   ```rust
   ast_node! { MyThing = MyThing }

   impl MyThing {
       pub fn child(&self) -> Option<ChildNode> { child(&self.syntax) }
       pub fn name(&self)  -> Option<SyntaxToken> { token(&self.syntax, SyntaxKind::Ident) }
   }
   ```
   - Three helpers cover almost everything: `child::<N>` (first castable child
     node), `children::<N>` (all of them, in order), `token(parent, kind)`
     (first *direct* child token of a kind).
   - Alternatives (a node that is one of several kinds) use `ast_enum!` — see
     `Item`, `Stmt`, `Expr`.
   - Accessors recompute on each call and never cache; that is intentional.
5. **Tests** — unit-test the rule in `grammar.rs`/`parser.rs` and the accessors
   in `ast.rs`. Refresh snapshots: `INSTA_UPDATE=always cargo test`, then
   review the `.snap` diff before committing.

## Precedence — when expressions get operators

`grammar.rs::expr` is currently atom + postfix only (`call`, struct-literal).
Adding binary/unary operators means a **Pratt parser**: a binding-power table,
a loop that consumes an operator only while its left binding power exceeds the
caller's, and `CompletedMarker::precede` to retroactively wrap the LHS in a
`BinExpr` node. `precede` is the existing mechanism — `expr`'s postfix loop
already uses it. Do not switch to a tree-rewriting parser; precede keeps the
event buffer append-only.

## Worked example — adding `if`/`else`

The tokens (`If`, `Else`) and their `SyntaxKind`s already exist. So:

1. `syntax.rs` — add node kind `IfExpr` (`if` is an expression in eye).
2. `grammar.rs` — `fn if_expr(p)`: open marker, `advance` the `if`, parse the
   condition `expr`, parse a `block`, then `if p.eat(T![else])` parse another
   `block` (or a nested `if_expr`). `complete(p, SyntaxKind::IfExpr)`. Wire it
   into `atom` (so it nests in expressions) and update the EBNF.
3. `ast.rs` — `ast_node! { IfExpr = IfExpr }` with `condition()`, `then_block()`,
   `else_branch()` accessors; add `IfExpr` to the `Expr` `ast_enum!`.
4. Tests for parse + accessors; refresh the CST snapshot.

No lexer or `token.rs` change at all — the lexemes were already there.

## Gotchas

- **Exhaustive matches are the guardrail.** `From<TokenKind>` in `syntax.rs`
  has no `_` arm. A new token won't compile until mapped — follow the error.
- **UTF-8 in the lexer.** Index bytes for ASCII; for anything multi-byte go
  through `peek`/`advance` which decode properly. `advance_by` debug-asserts a
  char boundary. The regression test `minus_before_multibyte_char_no_panic`
  guards this class of bug.
- **Trivia is not the parser's concern but is the tree's.** Grammar code never
  sees trivia (`nth`/`at` skip it); `build_tree` re-interleaves it. A new
  trivia kind must be added to `SyntaxKind::is_trivia`.
- **Snapshots.** `src/snapshots/*.snap` are committed. Any grammar/lexer change
  shifts them — regenerate with `INSTA_UPDATE=always cargo test` and eyeball
  the diff; a snapshot diff *is* the review of your tree shape.
- **Defer semantics to HIR.** Escape-sequence decoding, name resolution, type
  checking — none belong in lexer/parser/AST. The CST and AST are syntactic and
  lossless; meaning is HIR's job.
- **`lib.rs` vs `main.rs`.** Pipeline modules are public in the `eye` lib crate
  so forward-looking API is not dead-code-flagged. New public stage API goes
  through `lib.rs`; `main.rs` stays a thin driver.

## Checklist

```
[ ] token.rs    — new TokenKind variant            (only for new lexemes)
[ ] lexer.rs    — recognise it; keyword() if a kw
[ ] syntax.rs   — SyntaxKind variant + From<TokenKind> arm + T! arm
[ ] grammar.rs  — parse rule, wired into its parent, EBNF updated
[ ] ast.rs      — ast_node!/ast_enum! wrapper + typed accessors
[ ] tests       — unit tests in each touched module
[ ] snapshots   — INSTA_UPDATE=always cargo test, review the diff
[ ] cargo build — 0 warnings
```
