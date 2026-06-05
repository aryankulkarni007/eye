# Object topology: type-declaration ordering

**Status: built 2026-06-05, verified end to end (hir 64, e2e 26, codegen 48),
not committed.** Replaces the fixed-category-order prelude. The foundation
function pointers in structs build on ([FNPTR.md](FNPTR.md)).

## The problem

C requires a type to be declared before it is used by value. The old codegen
prelude emitted type declarations in fixed category order - all structs, then all
unions, then enums, then array wrappers - and as anonymous `typedef struct {...}
Name;` (no forward-declared form). That broke a whole cluster from one root
cause: a struct could not hold a field whose type was declared later, a union, an
array, or a pointer to itself.

## The model

Type declarations form a dependency graph. The shared graph
(`crates/hir/src/core/typegraph.rs`) is consumed by **both** the value-recursion
check (HIR) and the emission order (codegen), so they classify every edge
identically - a divergence would either reject a valid program or emit
unorderable C (a raw clang error).

A **node** is a nominal type (struct/union, by name) or a fixed-array value
wrapper (`[elem; len]`). The single edge rule:

- **Embedded by value** -> hard edge: the embedded type's definition must precede
  this one. A struct field of type `T`, a union field, an array wrapper's element.
- **Behind a pointer/reference** (`T*`, `&T`) -> soft edge: no edge. A pointer to
  an incomplete type is legal in C once the type has a forward declaration.

Enums have no out-edges (integer tags) and nothing depends on them incompletely,
so they are emitted first, outside the sort.

`hard_deps` is the one function encoding this. `cyclic_nodes` (HIR) and
`topo_order` (codegen) both run over it.

## Emission

1. **Enums** - no dependencies.
2. **Forward declarations**, named tags, for every struct, union, and array
   wrapper: `typedef struct Name Name;` / `typedef union Name Name;` /
   `typedef struct __eye_arr_N_T __eye_arr_N_T;`. Every pointer field, every
   self-reference, and `&[Self; N]` resolves against these.
3. **Definitions** in topological order of the value (hard) edges, so every
   value-embedded type is complete first: `struct Name { ... };`, and the array
   wrappers `struct __eye_arr_N_T { T data[N]; };`.

Kahn's algorithm, seeded and tie-broken in node order (arena order for nominal
types, innermost-first discovery for wrappers), so the generated C is
deterministic.

## Value recursion

A type that embeds itself by value - directly (`structure A { A a }`), mutually
(`A { B b }; B { A a }`), or through an array (`structure A { [A; 4] xs }`) - has
infinite size and is a hard clang error. The HIR pass `check_value_recursion`
(`crates/hir/src/core/lower/recursion.rs`) detects it via the shared graph and
rejects it as `TypeError::RecursiveValueType` before codegen, anchored on the
type name. The fix is a pointer, which is a soft edge.

Because pointers (including `&[Self; N]`, a pointer to the named-tag wrapper) are
soft edges, `structure Node { int32 v, Node* next }`,
`structure Node { [&Node; 4] kids }`, and `structure Node { &[Node; 4] kids }`
are all finite and compile. Only a genuine by-value cycle is rejected, and the
diagnostic only ever claims infinite size when the type truly is.

## What it unblocks

Nested structs in any declaration order; unions, arrays, and (with
[FNPTR.md](FNPTR.md)) function pointers as struct fields; self-referential and
mutually-recursive pointer structs (linked lists, trees). Supersedes the old
`collect_program_arrays` array-only pass and removes the `ArrayField` rejection.

## Not a language concept

This is a C-backend ordering detail. The struct-wrap array wrapper, the named
tags, and the forward declarations are all C-emission artifacts. A backend that
is not C (a native code generator) computes layout directly and needs none of
this; the dependency graph stays useful only as the value-recursion check.
