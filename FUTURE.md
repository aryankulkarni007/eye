# Eye - completed features

Tracks what the compiler can already do. Roadmap and scoping for the next
version live elsewhere (PR descriptions, branch READMEs).

## Pipeline

- Lossless rowan CST -> typed AST -> arena HIR -> C transpile -> clang.
- Source-mapped diagnostics at lexer, parser, and HIR layers.
- Per-file output: `<file>.c` written next to source, native binary alongside.
- Auto-format generated C through `clang-format` when available.

## Driver

- `eye <file.eye>` builds and writes the binary.
- Internal dumps gated behind clap flags: `--dump-cst`, `--dump-ast`,
  `--dump-symbols`, `--dump-hir`.

## Lexer

- Logos-backed, byte-range tokens.
- Interned identifiers and string literals (`Interner` / `Symbol`).
- Line/column resolution for diagnostic spans.
- Trivia preserved: whitespace, `--` line comments, `---` doc comments.

## Parser

- Pratt expression parser; error-resilient with synthesised holes.
- Items: `structure`, `fn` (named via call-form), `enum` (assignment form
  `enum X = A | B ;`; leading `|` on the first variant is always optional
  (accepted inline or multi-line, stylistic only); `|` is mandatory
  between subsequent variants; empty variant list rejected).
- Statements: `const`/`var` let with optional type, expression statements,
  trailing tail expression in blocks.
- Expressions: literals, paths, calls, field access, struct literals
  (positional, named, shorthand, mixed), binops, prefix `-` / `!` / `*` / `&`,
  `if`/`else`, `loop` / `break` / `continue`, assignment, block expressions.
- Types: identifier path, `&T` reference, `T*` raw pointer.

## AST

- Generated from `eye.ungram` via `xtask`.
- Typed wrappers over rowan green nodes; preserved trivia.

## HIR

- Arena-allocated structs, enums, fields, functions, locals, pats, exprs,
  stmts, blocks (la-arena).
- Item scope with duplicate-name diagnostics.
- Lexical scope stack for locals; name resolution to
  `Local` / `Fn` / `Struct` / `Enum` / `Unresolved`.
- `expr_types` side table populated for struct literals, refs, derefs, locals
  via let-type, fn params - enough to drive codegen decisions today.
- Source map from HIR nodes back to syntax pointers for diagnostics.

## Codegen (C backend)

- `int32`, `float32`, `float64`, `bool`, `char`, `string` map to fixed-width
  C types; refs/pointers lower to `T*`.
- Structs lower to `typedef struct { ... } Name;`.
- Enums lower to `typedef enum { ... } Name;`.
- Functions: params, return types, tail expression as implicit `return`.
- Statements: `const`/`var` let, expression statements.
- Expressions: literals, paths, calls (every arg emitted), struct literals
  (compound literal form), field access with auto-deref to `->` on `&T`/`T*`
  bases, binops, prefix unops, blocks with tail value, `if`/`else` (both
  statement and ternary-expression forms), `loop` lowered to `while(true)`,
  `break`, `continue`, `&x`, `*x`.
- `print(fmt, args...)` builtin lowers to `printf` with type-directed format
  specifiers: `%d` / `%f` / `%s` / `%c` / `%p` selected per arg from the HIR
  type (or literal kind when the arg has no recorded type).

## Tests

- Parser snapshot test plus targeted unit tests per construct.
- HIR lowering tests for scoping, items, expressions, tail expressions,
  references.
- Codegen regression tests for call args, nested field access, references,
  and printf format specifiers.
- End-to-end build-and-run tests over the `eyesrc/` sample programs.

## Working sample programs

- `eyesrc/main.eye` - struct, let, field access, print.
- `eyesrc/design.eye` - loops, if, assignment, mutation through a ref.
- `eyesrc/particle.eye` - reference parameter, field mutation via auto-deref.
- `eyesrc/physics.eye` - nested structs, conditional expressions, mixed
  primitive `print` formatting.

## Roadmap - v0.3 (enums + match)

Goal: variant access (qualified and bare-in-typed-context), match
expressions with exhaustiveness. No payloads, no guards, no or-patterns,
no bindings.

Authoritative source spec: `eyesrc/v03.eye`.

### Locked design

- Variant access: `Shape.Circle` qualified form always works. Bare `Circle`
  also works anywhere as long as the name resolves uniquely to one variant
  across every enum in the module. Two enums claiming the same variant
  name is a hard decl-time error (collision is also unsafe for the C
  backend since enum constants are global).
- Match: Rust-shaped block, comma-separated arms, `->` arrow, `_` wildcard.
  Match is an expression. Assignment is a statement. Trailing comma optional.
- Exhaustiveness check in HIR; hard error on missing variant unless `_`
  catches it.
- Codegen Strategy A: any non-statement-position match is hoisted - emit
  `Type _matchN;` decl and `switch(scrut) { case ...: _matchN = ...; break; }`
  at the nearest enclosing statement; substitute `_matchN` at the use site.
  Uniform rule, no GCC statement-expression extension.

### Milestones

- [x] M1 - enum decl shape `enum X = A | B ;`. Leading `|` on the first
      variant is always optional (stylistic only, accepted inline or
      multi-line). `|` mandatory between subsequent variants. Empty
      variant list rejected. Grammar + parser + AST regen.
- [x] M2 - variant access codegen.
  - HIR: `Resolution::Variant { enum_id, idx }` added. `ItemScope.variants`
    flat index registers every variant by bare name at enum-decl time. A
    collision across enums emits a hard error diagnostic; the first enum
    wins the slot so codegen can keep emitting bare C enum constants
    safely.
  - HIR: `resolve()` checks the variant index after locals/fns/structs/
    enums, so bare `Rectangle` resolves to `Resolution::Variant` anywhere
    in expression position without an expected-type hint.
  - HIR: `Shape.Circle` short-circuits in `FieldExpr` lowering by
    AST-inspecting the base before `lower_expr` runs, so the qualified
    form still works and bypasses the bare-enum NameRef diagnostic.
  - HIR: bare enum name in expression position (`Shape` alone) raises an
    "enum type, not a value" diagnostic and lowers to `Expr::Missing`.
    Unknown variant after `Enum.X` raises "enum `E` has no variant `X`".
  - Codegen: `Resolution::Variant` emits the C enum constant name.
    `Resolution::Enum` arm is now `unreachable!()` (HIR rejects it).
  - **Known limitation:** untyped `const x = Rectangle;` still trips the
    existing "EXPLICIT TYPE MISSING" codegen placeholder; v0.3 does not
    add inference for untyped `let`. Workaround: annotate the type.
- [x] M3 - match expression parse.
  - Lexer: `match` keyword token; bare `_` is its own `Underscore` token
    (logos `priority = 3` wins the tie with the ident regex, while `_foo`
    still lexes as a single `Ident`).
  - Syntax: `Match`, `Underscore` token kinds; `MatchExpr`, `MatchArmList`,
    `MatchArm`, `PathPat`, `BareIdentPat`, `WildcardPat` node kinds;
    `T![match]` and `T![_]` arms added.
  - Ungrammar: `MatchExpr = 'match' scrut:Expr arm_list:MatchArmList`,
    `MatchArmList = '{' arms:MatchArm* '}'`,
    `MatchArm = pat:Pat '->' body:Expr ','?`,
    `Pat = PathPat | BareIdentPat | WildcardPat`.
    Path-shaped patterns wrap `NameRef` children (`qualifier`/`name`) so
    the structural generator can emit positional accessors.
  - Parser: `match_expr` in `lhs`; scrutinee parses under `set_no_struct_lit(true)`
    so `match sh { ... }` does not absorb the arm block as a struct literal,
    mirroring `if_expr`. Arm list clears the gate so arm bodies may be
    struct literals. `match` joins `if`/`loop` in the block-like statement
    rule so a statement-position match needs no trailing `;`. `,` between
    arms is mandatory; only the final arm before `}` may omit it - the
    diagnostic recovers and keeps parsing.
  - AST: regenerated via `cargo xtask codegen`. `ast::Expr::MatchExpr`
    added to the lowering switch in HIR as a `Missing` stub - real
    lowering and codegen land in M4 and M5.
  - Tests: nine new parser tests cover every pattern form, scrutinee
    struct-lit suppression, match-as-let-value, block-like statement
    placement, trailing-comma both shapes, arm-body struct literals, empty
    arm list, missing-arrow recovery, and missing-comma recovery. All CST
    round-trip byte-for-byte.
- [x] M4 - match HIR + exhaustiveness.
  - HIR: `Expr::Match { scrut, arms: Vec<MatchArm> }` with
    `MatchArm { pat, body }`. `Pat` gains `Variant { enum_id, idx }` and
    `Wildcard` siblings alongside the existing `Bind`/`Missing` let-pat
    forms; match arms never route through let-pat lowering so no stray
    `Local` is allocated.
  - Pattern lowering (`lower_match_pat`): `WildcardPat` -> `Pat::Wildcard`.
    `PathPat` resolves the qualifier against `items.enums`, errors out on
    cross-enum patterns when the scrutinee enum is known, then looks up
    the variant. `BareIdentPat` resolves strictly against the scrutinee
    enum's variant list when known (spec: no bindings); falls back to the
    global variant index only when scrutinee type is unknown. Any
    unresolved pat becomes `Pat::Missing` so coverage cannot be silently
    satisfied by a typo.
  - Scrutinee type comes from `expr_types`; if it isn't a known enum the
    pass emits "match scrutinee type is not a known enum" and skips
    exhaustiveness (arms still lower so the user keeps typing).
  - Exhaustiveness: tracks a `covered[]` bitmap over the scrutinee enum's
    variants and a `saw_wildcard` flag. After all arms, if no wildcard
    and any variant uncovered, emits a single diagnostic listing every
    missing name. Duplicate arms emit "duplicate match arm for variant
    `X`". Arms after `_` emit "unreachable match arm after `_` wildcard".
  - Match expression type mirrors `if`: the first arm body's recorded type.
  - Codegen + driver: `Expr::Match` carries an "M5" placeholder in
    codegen so the build stays green; `src/main.rs` dump rendering
    already had the AST-side arm from M3.
  - Tests: eight HIR unit tests cover qualified + bare + wildcard
    lowering, scrutinee type pinning, missing-variant exhaustiveness,
    duplicate-arm, unreachable-after-wildcard, cross-enum pattern,
    unknown-variant, and non-enum scrutinee diagnostics.
- [x] M5 - match codegen hoist.
  - Strategy A: `hoist_matches` walks a statement's inline expression subtree
    in post-order, and for each value-position match synthesises a
    `Type _matchN;` decl + assigning `switch` at the nearest enclosing
    statement, then substitutes `_matchN` at the use site. The walk stops at
    block boundaries (`if`/`loop`/block bodies, match arms) so nested matches
    hoist into their own scope.
  - Match-in-statement-position lowers directly to a `switch` with no temp and
    no trailing `;`. Wildcard arm -> `default:`; variant arm -> `case <name>:`.
  - Temp type comes from the HIR-recorded first-arm-body type; absent (e.g. a
    call-typed arm) falls back to `int32_t` with a visible comment, never
    `void*`. Counter resets per function so names stay `_match0`, `_match1`.
  - Codegen split by concern to match HIR: `core.rs` keeps the `CGen` aggregate
    + `gen_all`; `core/{types,items,stmt,expr,print,matches,tests}.rs` hold the
    method groups.
  - Tests: five codegen unit tests pin the four layouts (statement-position,
    value-position hoist + read order, wildcard -> default, two-matches
    counter) plus the per-function counter reset.
- [x] M6 - v0.3 end-to-end test.
  - `eyesrc/v03.eye` exercises a statement-position match plus two
    value-position matches (one exhaustive, one with a wildcard) returning into
    typed `let`s.
  - `tests/e2e.rs::v03_eye_lowers_match_and_prints_expected_output` `include_str!`s
    the fixture and asserts stdout `0\n1\nboxy\n4\n0\n`.
