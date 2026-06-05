# Function pointers in the Eye kernel

**Status: BUILT 2026-06-05, verified end to end (hir 66, e2e 29, codegen 48),
not committed.** This document specifies the surface syntax, the
HIR/MIR/codegen representation, and the scope boundary; the design below was
implemented as written, on top of the object-topology pass
([TOPOLOGY.md](TOPOLOGY.md)) - a function-pointer typedef is a node in that
graph, so function pointers in struct fields fall out. One addition beyond this
proposal: calling a non-function value (`let int32 x = 5; x(3);`) is rejected as
`TypeError::CallNonFunction` (no-footgun: it would otherwise leak a clang error).
Deferred as written: closures, generic function pointers, pointer equality. The
remainder of this document is the as-built design (sections use present tense).

## Why this is kernel

[KERNEL.md](design/KERNEL.md) classes a function pointer as genuinely-missing kernel
substrate, on the same line as the raw data pointer: a code address is the
code-side analog of `&T`, and a supermacro cannot manufacture one. It is the
floor that vtables, iterators, and callbacks bottom out on - the OOP/iterator
stdlib the vision describes cannot be written without it. Today `let x = f;`
(where `f` is a function) is a hard `ResolveError::FnAsValue` error
(`crates/hir/src/core/lower/expr.rs`, "Eye has no function pointers"). This
removes that wall.

A function pointer needs zero macro engine: it is a plain machine value, built
directly into the kernel now and used hand-written until the far-future engine
arrives ([KERNEL.md](design/KERNEL.md), bootstrap hinge = far-future).

## The thesis

A function pointer is a **code address**: a value whose type is a function
signature. It is a bare pointer with no captured environment - **not** a
closure. Closures (code + captured data) are a container and belong in stdlib,
not the kernel; they are out of scope here and below.

- A function name in value position **is** a value of its function type.
- That value may be stored in a `let`, passed as an argument, returned, held in
  a struct field or array element, and called.
- Calling through such a value is an indirect call; calling a function by its
  name stays a direct call. The two are distinct at the IR level and identical
  in source.

## Surface syntax

### The function type

A function type is the function declaration with the names and body removed:

```
(int32, int32) -> int32      -- takes two int32, returns int32
(int32) -> bool              -- takes one int32, returns bool
() -> int32                  -- takes nothing, returns int32
```

Keyword-free, by design. An Eye function declaration carries no leading keyword
(`add(int32 a, int32 b) -> int32 { ... }`); a function _type_ is that same form
minus the parameter names and body. Adding a `fn(...)` prefix would introduce a
keyword that appears in no Eye declaration anywhere, which is both inconsistent
and anti-subtractive. The function type reuses only tokens Eye already has: `(`,
`)`, `,`, and the `->` already used for return types in `FnDef` and `ExternFn`.

Parameters in a function _type_ are bare types with no names, unlike a `FnDef`
`Param` (`ty name`). A name there would be dead syntax.

This is unambiguous. A `(` can begin a type only as a function type: Eye has no
tuple types and no parenthesized-grouping in type position (grouping parens are
an _expression_ form, `ParenExpr`). Wherever the grammar expects a `TypeRef` and
sees `(`, it is a function type, with no lookahead.

### Void return spelling

The return arrow is optional, exactly as in `FnDef` and `ExternFn`
(`('->' ret)?`). A function type with no arrow returns nothing:

```
(int32)                      -- takes one int32, returns nothing
()                           -- takes nothing, returns nothing
```

This is the one spot worth challenging directly, because `(int32)` in isolation
reads like a parenthesized type. It is accepted for two reasons. First, there is
no parenthesized-type form for it to collide with: in type position `(int32)` can
_only_ be a function type, so the reading is never ambiguous to the parser, only
potentially to a first-time human reader. Second, it is the consistent choice -
every other Eye signature form spells "returns nothing" by omitting `-> ret`,
and inventing a unit type (`-> ()`) purely for function types would be a new
kernel concept carried for one use. The omitted-arrow rule keeps the function
type a strict subset of the declaration syntax already in the language. The
nullary-void case `()` is the natural reading of "no parameters, no return".

The alternative - a mandatory arrow with an explicit void marker, `(int32) -> ()`

- is rejected: it adds a unit type the kernel does not otherwise have.

### Taking a function as a value

A bare function name in value position decays to a function-pointer value, the
same way a bare array name decays and the same way `&x` is _not_ required to read
`x`:

```
let (int32, int32) -> int32 op = add;    -- `add` decays to its address
```

`&add` is **not** required and is **not** accepted. For a data value, `&x` is
what forms the pointer because `x` also has a non-pointer reading (its value);
for a function there is no non-pointer reading - a function is only ever
referenced by address - so the decay is unconditional and `&` would be noise.
This matches C (where a function name decays to a pointer), Rust, and Go. The
no-footgun position is that there is exactly one spelling, not two synonyms.

### Calling through a value

A function-pointer value is called with ordinary call syntax:

```
let int32 r = op(2, 3);      -- indirect call through `op`
```

No dereference operator is written. `op(args)` where `op` is a function-pointer
local reads identically to a direct call; the distinction is recovered by the
compiler, not the programmer (see HIR below).

### Mutability

`mut` is the existing binding keyword (the mutable sibling of `let`, not a
modifier on it) and is orthogonal to the function type. The binding keyword
leads, then the type; `mut` controls whether the **binding** can be repointed -
not the pointed-to code, which is immutable:

```
let (int32) -> int32 op = inc;   -- fixed: op always calls inc
mut (int32) -> int32 op = inc;   -- swappable: op may be repointed
op = dec;                        -- legal only under mut
```

This is the same `let` vs `mut` meaning as for any other type. There is no new
interaction and no parse ambiguity (after the binding keyword a `(` still
unambiguously begins a function type). Because a function type lowers to a
typedef name, the `const` emit for a `let` binding is the uniform
`const <typedef> <name>`, with none of C's function-pointer const-placement
awkwardness.

### Precedence and associativity

Three compositions need a single defined parse:

| Source                       | Parse                          | Meaning                                        |
| ---------------------------- | ------------------------------ | ---------------------------------------------- |
| `&(int32) -> int32`          | `&((int32) -> int32)`          | reference to a function pointer                |
| `(int32) -> int32*`          | `(int32) -> (int32*)`          | function returning `int32*`                    |
| `(int32) -> (bool) -> int32` | `(int32) -> ((bool) -> int32)` | returns a function pointer (right-associative) |

The rules: the postfix `*` (`PtrType`) binds tighter than the return arrow, so a
trailing `*` belongs to the return type, not the whole function type; to point
to a function pointer, parenthesize (`&(...)->R`, `((...)->R)*`). The return
arrow is right-associative, so a function returning a function needs no inner
parentheses. A reference/pointer _to_ a function type is always parenthesized
because the function type's own parens do not extend leftward over a prefix `&`
or `*`.

### Relationship to tuples

In type position `(int32, int32)` is a parameter type list, not a tuple. With the
return arrow present (`(int32, int32) -> R`) there is no overlap with anything.
The one honest collision is the void case: bare `(int32, int32)` (omit-arrow) is
also the spelling a tuple _type_ would want, since a tuple type is likewise a
parenthesized list of types.

This is accepted, not worked around, because tuples are **not kernel**: a tuple
is a container, stdlib via supermacros, in the same NOT-kernel bucket as `Vec`
and `Option` ([KERNEL.md](design/KERNEL.md)). The kernel will never spell a built-in
`(A, B)` tuple type, so the function type is not displacing a kernel feature. It
does occupy the `(A, B)` type-position spelling a future _stdlib_ tuple sugar
might prefer; the subtractive thesis says not to reserve clean kernel syntax for
a feature already decided to be stdlib, so if the far-future engine introduces
tuples it picks a non-colliding spelling. The forfeiture is recorded here
deliberately.

### Alternatives considered

- `fn(int32, int32) -> int32` (Rust/Go-style keyword prefix). Rejected: adds a
  keyword that exists in no Eye declaration; anti-subtractive.
- `int32 (*)(int32, int32)` (C declarator). Rejected: the split-declarator form
  is the canonical C readability footgun Eye exists to remove.

## HIR representation

Add one `TypeRef` variant (`crates/hir/src/core/types.rs`):

```rust
pub enum TypeRef {
    Path(Text),
    Ref(Box<TypeRef>),
    Ptr(Box<TypeRef>),
    Array { elem: Box<TypeRef>, len: u64 },
    Fn { params: ThinVec<TypeRef>, ret: Option<Box<TypeRef>> },   // new
    Error,
}
```

`ret: None` is the void-returning case. `Display` renders
`(p0, p1, ...) -> ret` (arrow omitted when `ret` is `None`), the inverse of the
surface syntax. Parsing adds an `ast::TypeRef::FnType` node and a `FnType` arm in
`lower_type_ref` (`crates/hir/src/core/lower/types.rs`).

### A function name becomes a value

`ResolveError::FnAsValue` is **removed**. The arm in `lower_expr` that rejects
`Resolution::Fn` in non-callee position
(`crates/hir/src/core/lower/expr.rs`) instead types the name as a function
pointer: its `expr_type` is the `TypeRef::Fn` built from the resolved function's
`params` and `ret`. Every function name is now a valid value; what was a resolve
error becomes, at most, a later type mismatch (assigning a function to a slot of
the wrong type is a T-class `LetTypeMismatch`, not an R-class misuse). The
`is_callee` branch is unchanged: a name in callee position still lowers to a
direct call.

This is the single behavioral removal in the proposal. The exhaustive
`Resolution` match in `lower_expr` (REDESIGN I2: a `Path` reaching codegen always
denotes a value) stays exhaustive - `Resolution::Fn` simply moves from the
"not a value" set to the value set.

## MIR representation

Two additions to `crates/mir/src/core.rs`, both anticipated by the existing
comment on `RValue::Call` ("an indirect call through a function-pointer value
would add a separate variant; Eye has no function-pointer type today").

```rust
pub enum RValue {
    // ... existing ...
    /// A function symbol used as a value (its address). Emits the bare C
    /// function name, which decays to a function pointer in value context.
    Func(FnId),
    /// An indirect call through a function-pointer value. `callee` is the
    /// pointer operand; the result type comes from the callee's `Fn` type, not
    /// from a resolved `FnId`.
    CallIndirect { callee: Operand, args: ThinVec<Operand> },
}
```

`Func` is an `RValue`, not an `Operand`, for the same reason `Variant` is: the
trivial-operand invariant is "a constant or a place", and a function symbol is
neither, so where an operand is needed it spills to a temp like any other
rvalue. The existing direct `Call { func: FnId, args }` is unchanged.

Lowering picks the path by the HIR callee shape:

- callee is `Expr::Path(Resolution::Fn(id))` -> direct `Call { func: id }`,
  result type `functions[id].ret`.
- callee is any other expression yielding a function-pointer value (a local, a
  field, an index, the result of another call) -> `CallIndirect { callee }`,
  result type taken from that expression's `TypeRef::Fn` `ret`.

The `print` intrinsic path (an unresolved callee named `print`) is unaffected; it
still lowers to `RValue::Print`.

## Codegen

C's function-pointer declarator is split (`R (*name)(params)`), the same shape
problem arrays already solved with a generated wrapper typedef. Function-pointer
types reuse that machinery rather than special-casing the declarator.

### Typedef per signature

For each distinct function type, emit one typedef:

```c
typedef int32_t (*__eye_fn_i32_i32__i32)(int32_t, int32_t);
```

Then `CType` for a `TypeRef::Fn` is just that typedef name, and `CDeclarator`
stays the uniform `<type> <name>` with no split (`crates/codegen/src/core/types.rs`).
The name comes from extending the injective `array_mangle`
(`crates/codegen/src/core/arrays.rs`) with a `Fn` arm over the parameter and
return mangles, so names stay collision-free.

The non-obvious integration point: the typedef emission-ordering pass must
**unify** function-pointer typedefs with the array wrappers _and the nominal
types_ (structs/unions) into one topologically-ordered emission, not independent
passes. They cross-reference - a function type can take or return an array
(`([int32; 4]) -> bool`), an array can hold function pointers
(`[(int32) -> int32; 8]`), and a struct field can be either - so a single
dependency order over all three is required to avoid emitting a typedef before
one it names.

This is the same topological sort [DEFER.md](planning/DEFER.md) names as the fix for
**arrays as struct/union fields** (before the pass a hard error: the array wrapper
typedef was emitted after the nominal types, so a struct field of array type
referenced an undeclared type). Function-pointer struct fields hit the identical
ordering problem - the `structure Ops { (int32) -> int32 step }` in the worked
example below needs the `step` typedef emitted before `Ops`. The one pass
([TOPOLOGY.md](TOPOLOGY.md)) unblocked both: it shipped for function pointers and
lifted the array-struct-field deferral at the same time, as planned.

### Emit

- `RValue::Func(id)` emits the bare function name (`add`); C decays it to a
  pointer in value context.
- `RValue::CallIndirect { callee, args }` emits `callee(args)` - C calls through
  a function-pointer value with ordinary call syntax, no explicit deref needed.
- `spec_for_type` (`crates/codegen/src/core/types.rs`) gains a `Fn` arm
  returning `%p`, so `print(f)` on a function pointer formats as an address
  rather than falling through to the `%d` default.

## No-footgun posture

Eye has no null today, so a function-pointer value is **valid by construction**:
it is always the address of a real function, there being no syntax to produce a
null or dangling one (no null literal, no uninitialized `let`). This is the
no-footgun position - the C hazard of a null/wild function pointer is absent
because the values that would create it do not exist in the surface. Address
equality and ordering of function pointers are not specified here and are
deferred; the motivating uses (callbacks, dispatch tables) do not need them.

## Scope

**In scope (this proposal):**

- The `(params) -> ret` function type, including void return.
- Function name as a value (decay), with `FnAsValue` removed.
- Storing function pointers in `let`, parameters, and returns - these fall out
  of "a function type is a regular `TypeRef`" once the typedef exists.
- Function pointers in **struct fields and array elements** - these fall out of
  the same regular-`TypeRef` treatment, but only once the unified typedef
  topological sort (codegen, above) is built. That sort is the shared cost: it is
  the same machinery [DEFER.md](planning/DEFER.md) defers arrays-as-struct-fields on, so
  the two land together rather than either being free.
- Direct and indirect calls.
- Passing Eye functions to `extern` C functions (the qsort case below) and
  declaring `extern` parameters of function-pointer type.

**Deferred / out of scope:**

- **Closures** (code + captured environment). A container, stdlib, far-future.
  A function pointer has no environment.
- **Generic function pointers**. Blocked on generics, which are themselves
  stdlib (comptime + instantiation), not kernel.
- **Function-pointer equality / ordering**. Unspecified above; revisit if a use
  appears.

## Worked example

A comparator passed to C `qsort`, and a hand-written dispatch table (the vtable
this substrate exists to enable), exercising extern interop, a function-pointer
parameter, a struct field of function-pointer type, and an indirect call:

```
extern {
    qsort(ptr base, usize n, usize size, (ptr, ptr) -> int32 cmp);
}

cmp_int(ptr a, ptr b) -> int32 {
    -- ... compare two int32 through the pointers ...
}

-- a 2-entry dispatch table: a struct of function pointers, hand-written here,
-- machine-generated by the supermacro engine later.
structure Ops {
    (int32) -> int32 step,
    (int32) -> bool  done,
};

inc(int32 x) -> int32 { x + 1 }
at_ten(int32 x) -> bool { x == 10 }

run(Ops ops, int32 start) -> int32 {
    mut int32 v = start;
    loop {
        if ops.done(v) { break v; }    -- indirect call through a struct field
        v = ops.step(v);               -- indirect call through a struct field
    }
}
```

Type-checking this on paper through `TypeRef::Fn` + the typedef emission +
`CallIndirect` is the validation gate for the design: `cmp_int` decays to a
`(ptr, ptr) -> int32` value matching `qsort`'s declared parameter; `Ops`'s fields
are two distinct function-pointer types, each one typedef; `ops.done(v)` and
`ops.step(v)` lower to `CallIndirect` with the result type read from the field's
`Fn` type. If that holds, the design composes.

## Implementation checklist

Roughly in dependency order. Each layer is mechanical once the one above lands.

1. **Grammar** (`crates/ast/eye.ungram`, then `cargo xtask codegen`): add
   `FnType = '(' (TypeRef (',' TypeRef)*)? ')' ('->' ret:TypeRef)?` to the
   `TypeRef` alternation; parser support for `(` in type position.
2. **HIR type** (`types.rs`): the `TypeRef::Fn` variant + `Display`;
   `FnType` arm in `lower_type_ref`.
3. **HIR value** (`lower/expr.rs`): remove the `FnAsValue` rejection; type a
   function name as its `TypeRef::Fn`. Delete `ResolveError::FnAsValue`
   (`errors.rs`) and its test.
4. **MIR** (`mir/src/core.rs` + lowering): `RValue::Func`, `RValue::CallIndirect`;
   callee-shape branch in call lowering; result-typing for the indirect path.
5. **Codegen** (`codegen/src/core/`): `Fn` arm in `array_mangle`; unified
   typedef emission ordering; `CType`/`CDeclarator` via the typedef; emit for
   `Func` and `CallIndirect`; `Fn` arm in `spec_for_type`.
6. **Tests**: function-as-value let/param/return; direct vs indirect call;
   struct field and array element of function-pointer type; the qsort extern
   path; a type-mismatch case proving the former `FnAsValue` is now a clean
   T-class diagnostic.
