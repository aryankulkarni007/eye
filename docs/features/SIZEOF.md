# `sizeof` in the Eye kernel

**Status: built 2026-06-06, verified end to end (`eyesrc/lang/sizeof.eye`, e2e
tests).** This is Horizon 0, Component 2 ([HORIZON0.md](design/HORIZON0.md)). The
deferred pieces are listed at the end and in [DEFER.md](planning/DEFER.md).

## The thesis

`sizeof(T)` is a compile-time `usize` equal to a type's target layout size. Eye
does not model layout: the value is the platform's, computed by the C backend.
This is the transpiler dividend - the hardest part of `sizeof` (portable layout)
is free because the backend is a C printer.

```
sizeof(int32)   -- 4
sizeof(Point)   -- struct layout, e.g. 8
count * sizeof(Point)   -- the malloc-argument shape
```

`sizeof` is the container substrate the way function pointers are the vtable
substrate: `malloc(n * sizeof(T))` cannot be written without it, and a macro
cannot compute a type's size portably.

## Surface

`sizeof(T)` where `T` is a **bare named type** - a builtin (`int32`, `usize`,
`char`, ...), or a declared `structure` / `union` / `enum`. It is recognized by
callee name like `print` / `len`, **after** parsing, so the argument has already
parsed as an expression; the lowerer reads the type name straight from that AST
node rather than evaluating it as a value. A user-defined `sizeof` (which
resolves to a function) shadows the intrinsic.

The type name is **not validated** at the floor (lenient, like every other
type-name site - see `lower_type_ref`); the C backend is the layout authority and
flags an unknown type.

## Pipeline

`sizeof` threads through as a dedicated node carrying a type, because its value is
not an Eye integer (it leans on C) and so cannot fold the way `len` does:

| Stage | What `sizeof` adds |
|-------|--------------------|
| HIR expr | recognized in callee position before arg-lowering (the argument is a *type*, so lowering it as a value would emit `UnresolvedName`/`StructNameAsValue`); produces `Expr::SizeOf(TypeRef)`, typed `usize` |
| diagnostics | `T` class: `SizeofArity` (not one argument), `SizeofNotAType` (argument is a value or compound type) |
| MIR | `RValue::SizeOf(Type)` - a dedicated rvalue (not an `Operand`), so the trivial-operand invariant holds; spills to a temp where an operand is wanted |
| codegen | emits `sizeof(ctype)` via the existing `CType` renderer; no Eye-side size is ever computed |

## Marked for termination

`sizeof` is an interim intrinsic ([HORIZON0.md](design/HORIZON0.md), *Intrinsics are
interim*). Two later forces retire it: when Eye owns its backend it must compute
layout itself (a target data-layout model), and once prime makes types
first-class values ([PRIME.md](PRIME.md) D8) `sizeof` becomes an accessor on a
type-value, not a kernel intrinsic. Correct and complete for the freeze
regardless.

## Deferred from the floor

Ratified but not in this build (see [DEFER.md](planning/DEFER.md)):

- **Compound-type arguments** (`sizeof(&T)`, `sizeof([T; N])`, `sizeof(T*)`) -
  need type-in-argument parsing; rejected with `SizeofNotAType`. No floor
  container math requires them.
- **`alignof`** - the same mold (emit C `_Alignof`); deferred until a container
  needs it.
- **The sizeof-tainted const-expr path** (`const usize N = sizeof(T)`,
  `[T; sizeof(U)]`) - a const-expr that transitively contains `sizeof` cannot fold
  to an Eye value; it must emit a C constant expression unfolded, which requires
  `const` to emit a C symbol rather than inline ([CONST.md](CONST.md)). Today
  `sizeof` in a const-expr is rejected as `NotAConstExpr`.
