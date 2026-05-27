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
- Items: `structure`, `fn` (named via call-form), `enum` (waterfall `| A | B`).
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
