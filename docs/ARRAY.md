# Arrays in the Eye kernel

**Status: implemented in the v0.7 working tree, verified end to end, not yet
committed or tagged.** Const/named-length arrays (A6) are the one v0.7 array
deliverable still outstanding (see the end of this document). Everything else
below is built and covered by tests.

## The thesis

A first-class array is a **value** whose length is part of its type. It copies
on assignment, passes and returns by value, and knows its own length at compile
time. Before v0.7 the array was C array decay - value-stored as a local,
reference-passed (losing its length), and UB-returned - which is exactly "not
first-class". v0.7 makes it a value.

## Model: value semantics

- `let b = a;` and `a = b;` **copy** the whole array (independent storage).
- An array **parameter** is passed by value (a copy); an array **return** is
  returned by value (no dangling stack pointer).
- `&[T; N]` is a **reference**: a pointer to the array that still carries `N` in
  its type. Use it for the no-copy path.

### `&[T; N]` is a reference, not a slice

| | What it is | Length | C representation | In kernel? |
|--|-----------|--------|------------------|-----------|
| `&[T; N]` | reference to a fixed array | static, in the type | `T(*)[N]` - thin pointer | yes |
| `&[T]` (slice) | length-erased view | dynamic, at runtime | `{T* ptr; usize len}` - fat pointer | no (stdlib) |

A reference keeps `N` in its type, so it is a one-word pointer-to-array with no
runtime length. A **slice** discards the static length and carries it at runtime
in a fat pointer; that is a container and belongs in stdlib, not the kernel. v0.7
ships `&[T; N]`, not slices. See [DEFER.md](DEFER.md).

#### A reference auto-follows for the obvious operation (ratified)

`r[i]` and `len(r)` on a `&[T; N]` reach through the reference automatically -
you do not write `(*r)[i]` or `len(*r)` (though both remain valid and explicit).
This is deliberate. It matches Rust, Go, and Zig; Go's spec is explicit ("if `a`
is a pointer to array, `a[x]` is shorthand for `(*a)[x]`"). Only C makes you
hand-deref a pointer-to-array, and `r[i]`-as-pointer-arithmetic is exactly the C
footgun the no-footgun principle rejects - for a pointer-to-array there is no
competing sane reading. The same auto-follow applies to field access on any
reference (`r.field`), which is the general reference model, not array-specific.

This does **not** reintroduce array-to-pointer decay (which the language still
forbids). Decay is an array silently *losing* its length; auto-follow keeps `N`
in the type the whole time. The two are opposite directions: decay erases the
length, a reference preserves it.

## Surface

| ID | Deliverable | State |
|----|-------------|-------|
| A1 | Value semantics: copy on init/assign, pass-by-value, return-by-value | Done |
| A2 | `&[T; N]` reference - pointer-to-array, length preserved; index via the reference | Done |
| A3 | `len(a)` intrinsic - a compile-time `usize` constant (works on `[T; N]` and `&[T; N]`) | Done |
| A4 | Literal out-of-bounds index is a hard Eye error | Done |
| A5 | Multi-dimensional arrays correct everywhere, including boundaries | Done |
| A6 | Const / named-length arrays `[T; N_const]` | Not yet - lowest-priority v0.7 deliverable |

### Bounds

A literal index out of range is a hard error: past the length (`xs[9]` on
`[T; 4]`) or negative (`xs[-1]`), including through one `&`/`*`. A **dynamic**
index (`xs[i]` for a variable `i`) is unchecked: runtime safety is deferred
because the language has no abort/trap mechanism yet ([DEFER.md](DEFER.md)).

### Operations on whole arrays

- **Binary operators** (`==`, `<`, `+`, ...) do not apply to a whole array (it
  is a struct in the backend); they are a hard error. Operate on elements.
- **`print`** is a primitive-only intrinsic (not a trait or macro yet), so a
  whole array - like any struct or union - is a hard error. Print its elements.
- **`[T; 0]`** (zero length) is a hard error: it has no value use and lowers to
  a nonstandard C zero-length array.

## Representation (C backend only)

Every `[T; N]` is lowered to a wrapper `struct { T data[N]; }`. C cannot pass,
return, or assign a bare array by value, but it can for a struct - so copy,
by-value passing, return, and multi-dimensional nesting all fall out of C struct
semantics for free. `&[T; N]` is a pointer to that wrapper.

This is a backend detail, **not** an Eye language concept. The language never
mentions `.data`; codegen rewrites indexing onto it (`a.data[i]`, or
`r->data[i]` through a reference) and `&a[0]` lowers to `&a.data[0]`. A future
Cranelift backend would emit a stack slot and a memcpy with no wrapper type at
all. There is deliberately no implicit array-to-pointer decay in the language: a
pointer comes from `&a` or `&a[0]`, through the existing reference model, which
is portable across backends.

Wrapper names come from an **injective** mangle of the element type: type names
are length-prefixed (`ref_int` -> `7ref_int`) and the `&`/`*`/array constructors
start with a letter, so a user type can never collide with a constructed type.
Two distinct Eye array types therefore never share one typedef (a collision
would dedup them to a single wrapper and miscompile one). The mangle lives in
`crates/codegen/src/core/arrays.rs` with injectivity unit tests.

## Latent gaps cleared alongside arrays

- **L1** - a value-position `match` inside a ternary-shaped `if` branch is now a
  clear diagnostic ("match in a conditional (ternary) expression is not
  supported yet; bind it to a `let` first") instead of the old broken
  `/* UNHOISTED MATCH */` C.
- **L3** - format strings now preserve UTF-8 byte-for-byte; the previous
  byte-wise `as char` corrupted any multibyte character.
- **L2** - the `int32` match-temp fallback (used only when no arm carries a
  type) is unchanged: it is gated on type inference, which is on hiatus (T1).
  Not independently fixable, documented as such.

## Known limitations

- **Arrays as struct/union fields are rejected** with a clear diagnostic. The
  wrapper typedef is emitted after the nominal types, so a struct holding an
  array would reference an undeclared type. Lifting this needs a codegen
  type-dependency topological sort. Tracked in [DEFER.md](DEFER.md).
- **Returning `&local` is not caught.** A reference to a stack-local array (or
  struct) returned from a function is a dangling pointer; clang warns but Eye
  does not, because there is no borrow/lifetime analysis. Tracked in
  [DEFER.md](DEFER.md).
- **A heterogeneous array literal is silently accepted.** `[1, true]` takes its
  element type from the first element with no uniformity check; a correct check
  needs coercion rules and so depends on the deferred typecheck pass (T1).
  Tracked in [DEFER.md](DEFER.md).
- **A bracket literal cannot be indexed directly.** `[1, 2, 3][i]` does not
  parse: the parser returns the array literal before the postfix chain runs
  (`grammar.rs`, the early `return array_lit(p)`), so no `[i]` index attaches.
  Bind the literal to a `let` first. Not yet built; not a ratified deferral.
- `len(a)` is a kernel intrinsic recognized by name (like `print`), so a
  user-defined `len` shadows it. It folds to a compile-time `usize` constant read
  from the type (emitted as `(size_t)N`, so `%zu` printing is well-typed) and
  never evaluates its operand (like C's `sizeof`). To keep a
  side-effecting operand from being silently discarded, the operand must be a
  **place** - a variable, field, index, or deref; `len(f())` and `len([1, 2, 3])`
  are rejected. A place can still hide a call in an index (`len(grid[f()])`),
  which is not yet rejected; that residual matches C's `sizeof` and Go's pre-rule
  (Go evaluates the operand when it contains a non-constant call). The `.len`
  field/method form is reserved for a future `.len()` array method (needs a real
  backend) and currently diagnoses, steering to `len(x)`.

## Out of scope for v0.7 (deferred)

Runtime bounds traps, slices `&[T]`. Const/named-length arrays (A6) stay in
v0.7 scope but are the lowest priority and not yet built. All in
[DEFER.md](DEFER.md).
