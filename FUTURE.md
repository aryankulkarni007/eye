# Eye compiler roadmap

Audit of next steps after v0.1. v0.1 ships the `main.eye` subset: structs, fns
(no params/ret), typed `let`, struct literals (with shorthand desugar), field
access, binops, calls, and a `print` builtin that lowers to `printf`. Parser is
resilient, HIR is arena-allocated with a source map, codegen emits C compiled
by clang.

Transpile pipeline (C backend via clang) stays through v0.3. Revisit only when
separate compilation, GC/closures, or a clang-free dependency story forces a
swap. The HIR/infer boundary above codegen is what keeps that option open.

## v0.2 - cover all of `design.eye`

Ordered by dependency. Each step builds on the prior.

### 1. Fn params, return types, tail expression

- parser: extend empty `param_list` to `( type_ref Ident (, ...)* )`. Add tail
  expr to `block` (`stmt* expr?`); grammar.rs only reads stmts today.
- ast: `Param`, `ParamList::params`, `FnDef::ret_type`, `Block::tail`. Edit
  `eye.ungram` and regen.
- hir: `Function.params` field exists but is never populated
  (`core.rs:545`). Populate it. `body.tail` is always `None`
  (`core.rs:571`). Lower the tail expression.
- codegen: param emission already present (`core.rs:84`). Tail expr lowers to
  `return <expr>;` when a return type is set.

Enables `add(int32 a, int32 b) -> int32 { a + b }`.

### 2. Assignment

- New `SyntaxKind::AssignExpr` (or `AssignStmt`). LHS restricted to lvalues:
  Path, FieldExpr, Deref. Today `=` only appears inside `let_stmt`.
- hir: `Expr::Assign { target, value }` or a new stmt variant. Mutability
  check: locals already carry `mutable`; struct fields and references need
  the same.
- codegen: trivial `lhs = rhs`.

### 3. `if`/`else` expression

- Tokens `If`/`Else` already lexed. Add `IfExpr` SyntaxKind. Parser wires it
  into `lhs()` at atom level. The worked example in
  `docs/adding-features.md:197` is exactly this.
- hir: `Expr::If { cond, then, else_ }`.
- codegen: statement position emits `if (c) { ... } else { ... }`. Expression
  position lowers to a temp + branch hoisted above the host statement (avoid
  GCC statement-expressions for portability).

### 4. Block expression with tail value

- HIR already has `Expr::Block(Vec<StmtId>)`, but codegen prints
  `BLOCK EXPRS DEFERRED` (`core.rs:211`). Add the tail-value path and share
  the temp-hoist machinery with if-expr.

### 5. `loop` / `break` / `continue`

- Tokens exist. Add `LoopExpr`, `BreakExpr`, `ContinueExpr` syntax kinds.
- hir: `Expr::Loop { body }`, `Expr::Break(Option<ExprId>)`,
  `Expr::Continue`. Labels deferred.
- codegen: `for (;;) { ... }`. `break <value>` reuses the temp-hoist trick.

### 6. References and pointers

- token: `&` is not in `define_tokens!`. Add `#[token("&")] Amp`.
- Type syntax: `&T` becomes a `RefType` node. `type_ref` today only accepts
  `Ident` (`grammar.rs:99`); add a prefix `&` form.
- Expressions: `&x` prefix and `*x` deref, or auto-deref on `.` (design.eye
  uses `pt_ref.y = 30`, which implies auto-deref). Pick one and document it.
- hir: `TypeRef::Ref(Box<TypeRef>)`, `Expr::Ref`, `Expr::Deref`.
- codegen: `T*`, `&x`, `(*x)` or `x->field` depending on the chosen deref
  rule.

### 7. Enums (waterfall syntax)

- token: `|` is not in `define_tokens!`. Add `#[token("|")] Pipe`.
- Parse `enum N = | A | B | C ;` into `EnumDef` with `EnumVariant` children.
- hir: `Enum { name, variants: Vec<VariantId> }`. Decide on flat namespace
  vs `Enum::Variant` qualification. Match expressions are out of scope for
  v0.2 since `design.eye` does not use them.
- codegen: `typedef enum { N_A, N_B, ... } N;`.

### 8. Doc comments (`---`)

`Dcomment` is already lexed. Attach it as leading trivia on the following
item in AST. No semantics yet.

## Bugs to fix before v0.2 work

- `codegen/src/core.rs:188-194`: call-arg emission has `if i > 0` guarding
  both the separator and the `gen_expr`, so arg 0 is never emitted. The
  `print` path works only because `gen_print` handles its arguments
  separately. Any non-print call with arguments is broken today.
- `codegen/src/core.rs:127`: inferred `let` falls back to `int32_t`. Fine
  for v0.1, replaced by real inference in v0.3.
- `hir/src/core.rs:14-15`: `collect_items` silently overwrites duplicate
  names. Emit a diagnostic.
- `hir/src/core.rs:419`: field-name extraction uses
  `children().filter_map(NameRef).nth(1)`. Fragile against trivia position;
  add a labelled accessor instead.

## v0.3 - bidirectional type inference

Lets users drop type annotations on `let`. Prereqs:

1. `Ty` arena with ids, separate from the syntactic `TypeRef`. HIR keeps
   `TypeRef`; a new `infer` pass produces `ArenaMap<ExprId, TyId>`.
2. Builtin type table: `int32`, `bool`, `f64`, etc.
3. Unification with inference variables (`?T`). A small Hindley-Milner
   subset; the bidirectional layer adds the check-mode / synth-mode split.
4. Lvalue and mutability checks ride on this pass.
5. Codegen consumes `TyId`, not `TypeRef`. The `int32_t` default goes away.

Algorithm sketch:

- `synth(e) -> Ty` for literals, paths, calls of typed fns, binops.
- `check(e, expected)` propagates expected types down through let with
  annotation, fn argument, struct field, and return position.
- Inference variables unify at join points. Ambiguous integer literals (a
  bare `const x = 0`) default to `int32` at the end of the pass.

Placement: a new `crates/infer` between `hir` and `codegen`. Codegen reads
inferred types, never `TypeRef` directly.

## Recommended v0.2 ordering

1. Fix the codegen call-arg bug.
2. Fn params, tail expr, return types.
3. Assignment plus mutability check.
4. `if`/`else` expression and block expression (share temp-hoist).
5. `loop` / `break` / `continue`.
6. References (add `&` token, ref types, decide deref rule).
7. Enums (add `|` token).
8. Duplicate-item diagnostic.

Each step lands with: parser unit test, HIR test, a `.eye` sample that
compiles and runs.
