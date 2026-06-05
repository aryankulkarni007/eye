# String literals in the Eye kernel

**Status: BUILT 2026-06-06** (`eyesrc/lang/string.eye`, e2e tests). Part of Horizon 0,
Component 3 (addressable static data) - see [HORIZON0.md](design/HORIZON0.md). This doc
records the representation and, in particular, the *length-polymorphism*
resolution that keeps slices out of the kernel. As-built notes and the remaining
deferrals are at the end.

## The representation

A string literal is `&[uint8; N]`: a reference to a fixed byte array, where `N`
is the **visible byte count, excluding the NUL**, so `len("hello") == 5`. `char`
= `uint8` at the floor (UTF-8 bytes). This reuses the hardened array machine
(auto-deref `s[i]`, `len(s)`, OOB checks) with zero new type machinery, and
closes the live `print` `%d` bug (today the literal carries a fake `string` type).

## The length-polymorphism problem (the crux)

Two string literals of different lengths are **different types**: `"hello" :
&[uint8;5]`, `"hi" : &[uint8;2]`. A function written over `&[uint8;N]` is
monomorphic in `N` - it accepts one length only. So "a function that takes any
string" needs one of exactly three mechanisms:

| Mechanism | What it is | Where it lives |
|-----------|-----------|----------------|
| **Monomorphize** `f<N>(&[uint8;N])` | instantiate per length | comptime + AST instantiation = the **prime engine**, far-future ([PRIME.md](PRIME.md)) |
| **Slice** `&[uint8]` = `{ptr, len}` | length-erased fat pointer, length carried at runtime | **stdlib** ([VISION.md](design/VISION.md)) |
| **Raw-pointer decay** `&[uint8;N] -> &uint8` | drop the static length, keep the byte pointer | **kernel** - the minimal C primitive |

### The kernel answer is decay, not slices

The kernel takes door 3, and it is **built**. A function that consumes a string
takes `&uint8` / `string` (a raw byte pointer); any `&[uint8;N]` **decays** to it
at the call/assign/return boundary - the same rule as `&[T;N] -> &T`. The length
is lost at the boundary and recovered the C way: NUL termination, or an explicit
`usize n` parameter. No monomorphization, no slice, no new kernel type. The decay
is lowered as a plain pointer cast (the wrapper's `data` is at offset 0, so
`(T*)wrapper_ptr` is the element pointer), inserted at four coercion sites:
let-init, call argument, explicit `return`, and the block-tail return.

Slices stay out of the kernel by the discriminating test ([VISION.md](design/VISION.md),
[KERNEL.md](design/KERNEL.md)): a slice is `struct { &uint8 ptr; usize len; }` - struct +
raw pointer + usize, all kernel primitives - so a stdlib supermacro can synthesize
it, so it must **not** be frozen into the kernel. A slice is the canonical
composable abstraction: ergonomic, length-preserving, and stdlib's job. The
kernel provides the irreducible substrate (fixed-length array + raw pointer +
decay); stdlib composes the fat-pointer slice on top.

### What `&[uint8;N]` is for

Exactly where the length **is** statically known - the literal/binding site:
`len("hello")`, `"hello"[2]` with OOB checks, assignment to a same-length
binding. The instant length-polymorphism is needed, the value decays to `&uint8`
(kernel floor) or is wrapped in a stdlib slice (later). That is the honest split.

## Codegen (the NUL / wrapper resolution)

Eye's `[uint8; N]` is the wrapper `struct { uint8_t data[N]; }`, so `&[uint8;N]`
is `Arr_uint8_N*`. The visible length `N` lives in that wrapper; the NUL does
**not** - it lives in the backing static storage, one byte past the wrapper's
logical extent but inside the allocated object:

```c
static const uint8_t s0[6] = {104,101,108,108,111,0};  // "hello" + NUL, byte-exact
// value of "hello":  (const Arr_uint8_5*)s0
```

`Arr_uint8_5` is `struct { uint8_t data[5]; }` - layout-identical to `uint8_t[5]`
(first member, offset 0, no padding), so `->data[i]` reads `s0[i]`. Reading
`->data` decayed to `uint8*` up to the NUL (strlen / `%s`) is in-bounds and
defined. The initializer is emitted **byte-exact from the decoded bytes** (not by
re-emitting a C string literal and trusting C to re-parse escapes), so `"\n"` is
`N == 1` and no Eye/C escape drift can corrupt `len`.

The backing statics are file-scope and emitted by the same module-level
static-emission pass as globals (Component 3, Part A) - which is why globals are
built first.

## The `print` `%d` fix

Retyping the literal alone does **not** fix `print`: `spec_for_type` maps
`TypeRef::Ref(_)` to `%p`. The fix is a new `spec_for_type` arm for a byte-array
reference `&[uint8;N]` -> `%s`, and the emitted value must be `s->data` (the byte
pointer), not the wrapper pointer (which would print as `%p`). The direct-literal
print case (`Operand::Const(Literal::String)`) already emits `%s` and stays.

## As built

- `literal_type` types a string literal `&[uint8; N]`, `N` the **decoded** byte
  count (`crates/hir/src/core/lower/types.rs`).
- `decode_string_literal` (`crates/hir/src/core.rs`) is the single decoder:
  `\n \t \r \0 \\ \" \'` expand to bytes; an unrecognized escape keeps both
  bytes. It feeds *only* `N` and the byte static - the stored `Literal::String`
  keeps the raw spelling, so the `print`/format paths still emit a C string
  literal and let C decode (a decoded newline inside `"..."` would be a C error).
- The codegen emitter (`mir_emit.rs`) interns unique literal contents
  (`collect_strings`) and emits one `static const uint8_t __eye_str{id}[N+1] =
  {bytes, 0};` per literal before the functions, using the *same* decoder for the
  bytes and `N`. `gen_literal` emits a string as `(Arr_uint8_N*)__eye_str{id}`
  (the wrapper-pointer cast); `len`, indexing, and OOB reuse the array machine;
  the `Arr_uint8_N` wrapper is auto-generated because `collect_type_nodes` walks
  `expr_types`.
- `print` of a string emits the byte pointer with `%s`: a literal value is its
  raw C string (`"..."`); a string place is `place->data` (`gen_print_value`).
  `spec_for_type` maps a `uint8`-array reference to `%s`.
- **Decay** (`maybe_decay`, `crates/hir/src/core/lower/expr.rs`): a `&[T; N]`
  value meeting a `&T`/`T*`/`string` context is wrapped in a cast to that type.
  Inserted at let-init (`stmt.rs`), call arguments (`expr.rs`), explicit `return`
  (`expr.rs`), and the block-tail return (`fn_body.rs`). The cast's type *is* the
  target, so the existing type check passes with no `types_compatible` change.
  `string` is the decay target for `&[uint8; N]` (so it is no longer orphaned).
  `eyesrc/ffi/caesar.eye` runs again on this path (`let string`, `encode(string s)`,
  `extern strlen(string s)`, string indexing, libc FFI).

## Deferred / known limits

- **Empty string storage.** `""` prints fine, but binding it needs the type
  `&[uint8; 0]`, which the `[T; 0]` zero-length-array rule rejects (`ArrayLenZero`).
- **Embedded NUL.** A `\0` in a literal embeds a NUL byte; `strlen`/`%s` over the
  C-string backing truncate there. Inherent to the C-string representation;
  `len` (type-level `N`) still counts the full decoded length.
- **Owned / growable strings** = `Vec<uint8>` + a string-literal seed - stdlib,
  far-future.
- **A real codepoint / grapheme `char`** - stdlib; `char = uint8` at the floor.
- **Native (non-C-backend) string representation** - revisit once Eye owns its
  backend; `&[uint8;N]` is the C-backend-era choice.
