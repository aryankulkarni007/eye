# DIAGNOSTICS: Error model

Status: BUILT (2026-05-31). Track 1 of `REDESIGN.md`, sequenced first. The model
below is implemented: the `diagnostics` crate carries the shared trait/`Span`/
`Severity`/`Code`/`Sink`/`Diag`; each producing crate (lexer/parser/hir) owns its
typed kind enums; the binary renders with `ariadne` (`src/diagnostics.rs`).
Message-text test assertions were migrated to structural `matches!` on concrete
variants. The "Verified starting point" table below is the pre-refactor record.

## Goal

Replace the three untyped, string-based diagnostic types with one homogeneous,
typed model. Errors are partitioned by class, with the typed kind as the single
source of truth and the rendered message derived from it. Codegen and
MIR-lowering emit no diagnostics (REDESIGN invariant I2).

## Verified starting point

| Layer  | Type                | Payload                 | Span            | Where                  |
| ------ | ------------------- | ----------------------- | --------------- | ---------------------- |
| Lexer  | `token::Diagnostic` | `msg: Cow<'static,str>` | `TextRange`     | `token/src/lib.rs:29`  |
| Parser | `ParseDiagnostic`   | `msg: &'static str`     | `TextRange`     | `parser/src/lib.rs:63` |
| HIR    | `HirDiagnostic`     | `msg: String`           | `SyntaxNodePtr` | `hir/src/core.rs:64`   |

Reporting is three near-identical per-layer macros in `src/diagnostics.rs`. No
severity, no codes, no class partition. Roughly 45 test sites assert on message
text (for example `d.msg.contains("do not chain")`, `parser/src/lib.rs:492`), so
rewording a message breaks tests.

## Locked decisions

- Homogenise all layers. A new crate `diagnostics` holds the shared contract
  (the `Diagnostic` trait, `Span`, `Severity`, `Code`, `Sink`, `Diag`). It sits
  just above `syntax` (it needs `SyntaxNodePtr` for `Span::Ptr`, so it cannot be
  the rock-bottom leaf where `token` is) and below lexer, parser, and hir. The
  typed kind enums live in their producing crates, not in `diagnostics`: the
  orphan rule blocks implementing `Display`/`Diagnostic` for a foreign kind, and
  keeping the kinds out of `diagnostics` avoids a `token -> diagnostics ->
  syntax -> token` cycle. The lexer's lexeme errors keep a payload-free
  `token::LexErrorTag` (set by the logos callbacks), which the `lexer` crate
  maps to the typed `LexError`, so `token` carries no diagnostics dependency.
- Source of truth is a typed kind enum per layer (`LexError`; `SyntaxError` /
  `GrammarError`; `HirError` over `Resolve`/`Type`/`Pattern`/`Const`/`Unsupported`
  sub-enums). The prose message is derived from the kind via `Display`, never
  stored as the truth.
- Cross-crate carrier is a trait object: `Box<dyn Diagnostic>`. The type stays
  available up to the future Diagnostic Bus, which routes on `code()` or by
  downcast. Error-assertion tests match concrete variants inside the producing
  crate, before boxing, which removes the message-text fragility.
- Span is homogenised: `enum Span { Range(TextRange), Ptr(SyntaxNodePtr) }`. HIR
  needs `Ptr` because ranges shift across edits; lexer and parser use `Range`.
- Severity: `enum Severity { Error, Warning }`.
- Emission API is homogeneous: `Sink::emit(primary_span, kind)`. The sink
  accumulates and never halts mid-pass.
- Display and render via `thiserror` per variant (`#[error("...")]`). Caveat:
  thiserror routes through `String`, so the `Cow` static-borrow saving is lost
  for static messages. Acceptable on the error path. Hand-write `Cow::Borrowed`
  render arms only if zero-alloc static messages are ever wanted.
- Renderer is `ariadne`, isolated at the binary edge (`src/diagnostics.rs`
  only). Core crates carry no render dependency and emit typed data. The
  renderer is a pure consumer of the typed `Diagnostic`, so it can be swapped,
  wrapped, or replaced later without touching core crates. Theming and
  customisation are deferred.

## Aesthetic growth

The trait is shaped so rich, multi-span errors are reachable without API churn.
Secondary labels, notes, and help default to empty and are overridden by richer
variants later. Secondary-label spans live in the variant payload.

The primary message is carried by a required `Display` bound rather than a
custom `render` method. `thiserror`'s `#[error("...")]` already generates
`Display`, so the locked thiserror-per-variant decision produces the primary
message for free; a separate `render` method would be a second, redundant path.
The `Debug` bound gives structural assertions in tests at no cost and matches the
"match concrete variants" test plan. The lost nuance (`render` returned
`Cow<'static, str>` for zero-alloc static messages) was already conceded above:
thiserror routes through `String`, so the `Cow` saving is gone regardless.

```rust
pub trait Diagnostic: std::fmt::Display + std::fmt::Debug {
    fn code(&self) -> Code;
    fn severity(&self) -> Severity { Severity::Error }

    fn secondary_labels(&self) -> Vec<(Span, Cow<'static, str>)> { vec![] }
    fn notes(&self) -> Vec<Cow<'static, str>> { vec![] }
    fn help(&self) -> Option<Cow<'static, str>> { None }
}
```

The primary message is `self.to_string()` (via `Display`); `emit` keeps the
primary span as an argument because it is the common case; extras come from the
variant payload through the default-overridable methods.

## Class partition (locked)

Eight classes across the whole compiler. Partitioned by concern and by feature
(that is why Const is separate from Type: arrays are a distinct feature, and a
shape/constant error is a different concern than a type-rule violation).

| Phase  | Class       | Prefix | Covers (grounded in current messages)                                                                                 |
| ------ | ----------- | ------ | --------------------------------------------------------------------------------------------------------------------- |
| Lexer  | Lex         | `L`    | malformed lexeme: unterminated string, empty char literal, invalid char in literal, invalid utf-8                     |
| Parser | Syntax      | `S`    | expected token / node: missing `;`, unclosed delimiters, missing pattern/field/name                                   |
| Parser | Grammar     | `G`    | deliberate rejections (footgun rules): comparison chaining, `&&`-as-type disambiguation                               |
| HIR    | Resolve     | `R`    | name lookup: duplicate item, duplicate variant decl, unknown enum/variant, name used as value                         |
| HIR    | Type        | `T`    | type-rule violation: let/arm/return mismatch, struct field unknown/missing, union one-field, op-on-array, `len` arity |
| HIR    | Pattern     | `P`    | match analysis: duplicate arm, unreachable-after-wildcard, non-exhaustive                                             |
| HIR    | Const       | `C`    | array shape / CTFE: length not a literal, length zero, length too large, literal index OOB, negative index            |
| HIR    | Unsupported | `U`    | deferred feature: arrays as struct/union fields; unhoisted match (transient, see below)                               |

Each class is a typed kind enum implementing the `Diagnostic` trait. None is a
codegen class, so invariant I2 holds by construction.

The Type class is expected to gain a member for an undeterminable expression or
match result type. This replaces codegen's current `int32_t` type fallback: an
unknown type becomes a type-check error rather than a silent guess, so MIR may
assume complete types. See `MIR.md` "Types and the inference gap" and the Track
3 type-check doc. This is an open boundary decision, recorded here for
visibility.

Code scheme: per-class letter prefix plus number (`R001`, `T001`, ...).
Self-documenting and sorts by class. Lower priority than the partition; numbers
assigned during implementation.

`Unsupported::UnhoistedMatch` is transient: it codifies today's HIR ban
(`check_unhoisted_matches`), and is deleted when MIR lands (REDESIGN I3). One
variant, one emit site. Expected, not rework.

## Scope and ordering (locked)

- All three layers (lexer, parser, HIR) migrate in this track. Not staged.
- This track runs first, before MIR (REDESIGN Track 2) and before the
  typeck/lowering split (Track 3). MIR is not a prerequisite: the error refactor
  is entirely upstream of MIR, and MIR + codegen emit no diagnostics (I2), so
  there is nothing in the control-flow "tumors" the error work depends on. The
  tracks are independent; errors-first is the low-risk cleanup that makes the
  high-risk MIR work debuggable.

## Open

- Error code numbering (the digits after each class prefix).
- Migration of the roughly 45 tests that assert on message text to match
  concrete variants instead.

  Instead of writing string matches, write structural assertions. If you are testing the HIR pattern pass, your test helper should look like this:

```rust
// In hir/src/pattern_tests.rs
let diagnostics = check_patterns(source_code);
  assert!(matches!(
        diagnostics.first(),
        Some(PatternError::UnreachableAfterWildcard { .. })
        )); -> just an example not representative of actual compiler code
```

## Span anchoring (implemented 2026-05-31)

A diagnostic's span must cover exactly the offending source, with no surrounding
whitespace, newlines, or comments. The parse tree is lossless: the parser
attaches leading trivia as a node's first children and trailing trivia as its
last children, so a node's full range routinely includes a leading space and a
trailing newline plus the next line's indentation. A node that ends a line thus
spills its range into the following line, which made `ariadne` bracket two lines
and label the wrong one.

Two independent properties produce a correct span; both are required, and they
live at different layers.

1. Granularity (per emit site). The span passed to an emit anchors on the most
   specific element that is the subject of the error, not an enclosing one. A
   `let` initializer mismatch anchors on the initializer expression, not the
   whole `LetStmt`; a non-enum match scrutinee on the scrutinee, not the whole
   `match`; a `print`/`len` argument error on the argument, not the whole call;
   an array struct field on its `[T; N]` type node, not the whole field; an item
   name conflict on the name token, not the whole declaration. Body sites use
   `LoweringCtx::expr_ptr(id, default)` to recover a lowered child's pointer;
   item collection uses `collect::name_span` for the name-token-or-node choice.
   A span is a `SyntaxNodePtr` when it names a node and a `TextRange` when it
   names a bare token (a name has no wrapping node); both are carried by `Span`.

2. Tightness (one shared trim, applied at each rendering edge). `Span::trimmed_range(root)`
   is the single definition of the trim. Given the parse tree root it resolves a
   `Span::Ptr` to its node and trims leading and trailing trivia via
   `syntax::trimmed_text_range`, which walks tokens in source order and keeps the
   span from the first to the last non-trivia token (`SyntaxKind::is_trivia`, so
   whitespace, newlines, and all comment kinds are removed). A `Span::Range` is
   already a tight token range and passes through unchanged, so `root` may be
   `None` before a tree exists.

Putting the trim on `Span` rather than at each emit means every emit path - HIR
body lowering, item collection, and any added later - is tight without repeating
it, and a new emit site cannot forget it. Granularity cannot substitute for the
trim: even the correct node carries edge trivia when it ends a line (a match arm
body before the closing brace, a struct field before the next field). The trim
cannot substitute for granularity: it narrows a span to real tokens but never
changes which node was chosen.

HIR spans stay `Span::Ptr` in storage and are trimmed only when rendered, so a
non-rendering consumer keeps the pointer's edit stability. There are two
rendering edges, and both must call `trimmed_range`. The CLI renderer
(`src/diagnostics.rs`) does, passing the parse root for HIR diagnostics and
`None` for the pre-parse lexer phase. The language server (`crates/lsp`)
currently surfaces only parser diagnostics, whose spans are already tight
`Span::Range`, so it shows no broad spans today; when it begins surfacing HIR
diagnostics it must route them through `trimmed_range` as well, or it will render
the untrimmed node ranges.

## Invariant tie-in

REDESIGN I2 requires codegen and MIR-lowering to emit zero diagnostics. The
aggregate of kind enums must contain no codegen error class; if one is ever
needed, the layering has leaked.

## Architectural Recommendation: RATIFIED

Manually added last session, flagged for review. Reviewed and accepted
2026-05-31. The trait now requires `std::fmt::Display + std::fmt::Debug` instead
of a custom `fn render(&self) -> Cow<'static, str>`; the "Aesthetic growth"
section above carries the ratified trait definition. Rationale recorded there:
thiserror already generates `Display`, so requiring it removes a redundant render
path, and the `Cow` zero-alloc nuance was already conceded under the locked
thiserror decision.
