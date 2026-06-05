# FFI: the C seam

**Status: built 2026-06-11, verified end to end** (`eyesrc/programs/file.eye`,
`eyesrc/programs/bubblesort.eye`, `eyesrc/ffi/caesar.eye`, e2e + parser + HIR
unit tests). This is the Horizon 0 C-seam item
([HORIZON0.md](../design/HORIZON0.md) item 5, [KERNEL.md](../design/KERNEL.md)):
variadic externs, opaque FFI types, and the removal of the auto-included
`<stdio.h>`. It restored the `bubblesort`/`file` corpus programs rejected at
the MIR cutover ([DEFER.md](../planning/DEFER.md)).

## The model

Rust-style: the `extern` block is the **sole prototype** for every C function
the program calls. No header is included for the user; whatever the block
declares is what the C translation unit sees, so a declaration can use Eye's
own types (an opaque `FILE*`, `string`) without colliding with a header's
spelling of the same function.

```
extern {
    type FILE;                                  -- opaque FFI type
    printf(string fmt, ...) -> int32;           -- variadic
    fopen(string path, string mode) -> FILE*;
    fclose(FILE* f) -> int32;
    fgets(ptr buf, int32 n, FILE* f) -> ptr;
}
```

## Variadic externs (`...`)

`...` as the last entry of an extern signature marks the C-ABI variadic
convention. It is a marker, not a parameter:

- **Calls** may pass any number of extra trailing arguments after the named
  parameters; they lower as ordinary operands (C's default argument promotions
  apply, e.g. `float32` widens to `double`).
- **The prototype** gains `, ...` (`int32_t printf(const char* fmt, ...);`).
- **Eye has no varargs access** (`va_list` is not modeled), so `...` is
  rejected outside an `extern` block (`G4 VariadicOutsideExtern`) - a defined
  function could never read the extra arguments.
- It must be the **last** parameter (`G5 VariadicNotLast`) and needs at least
  one **named parameter before it** (`G6 VariadicNeedsNamedParam`, the C99
  calling-convention rule).

Mechanics: `...` is one token (`Ellipsis`), a `Variadic` node in the
`ParamList` (the parser takes a `variadic_ok` flag - true only from
`extern_fn`), `Function::variadic: bool` in HIR, and a `", ..."` tail in the
emitted prototype. No MIR change: call arity was never checked against the
param list (pre-typeck floor), so extra operands flow through `RValue::Call`
unchanged.

## Opaque FFI types (`type Name;`)

`type FILE;` inside an extern block declares a named C type whose layout Eye
never sees. It emits exactly one line of C - a forward typedef, no definition:

```c
typedef struct FILE FILE;
```

- Legal **behind a pointer or reference** (`FILE*`, `&FILE`): the C side owns
  the layout; Eye only passes the address around. `0 as FILE*` gives a null of
  the right type.
- A **value-position** use (`let FILE f = ...`, a `FILE` field, `sizeof(FILE)`)
  is a C-side incomplete-type error. The floor has no HIR type-name resolution
  pass (type refs stay `Path(name)` until codegen), so an Eye-side diagnostic
  for this waits on the typeck split - the same posture as an undeclared type
  name.
- The name lives in its own namespace (`HIR::opaques` arena,
  `ItemScope::opaques`); redeclaring a struct/union/enum name is a
  `DuplicateItem` error.

The tag spelling `struct FILE` is Eye's own: with no `<stdio.h>` in the unit
there is nothing to collide with, and the linker only ever sees pointers.

## No auto-included `<stdio.h>`

The emitter previously included `<stdio.h>` unconditionally (the `print`
intrinsic lowered to `printf`). That header's prototypes conflicted with any
user extern that re-declared a stdio function with Eye types - the reason
`fopen` could not be declared. Now:

- The prelude includes only `<stdint.h>`, `<stddef.h>`, `<stdbool.h>` (type
  spellings the emitter itself uses).
- The `println` intrinsic still lowers to `printf` calls. A program that uses
  `println` without declaring `printf` gets one emitter-supplied prototype:
  `int printf(const char *, ...);`. A program that declares its own
  `extern printf(string fmt, ...) -> int32` suppresses it - the user
  declaration emits `int32_t printf(const char* fmt, ...)`, the same ABI
  (`string` renders as `const char*`).
- Every other libc call must be declared in an `extern` block, as `strlen` in
  `caesar.eye` always was. The declaration is the prototype; the linker
  resolves the symbol.

`println` itself remains a kernel intrinsic, reclassified 2026-06-11: with
`printf` reachable through a variadic extern it is sugar over an exposed
primitive, no longer load-bearing. It is kept because its `{}` placeholders
are formatted type-directed at codegen, which hand-written `%` specifiers
cannot replace safely and no Eye function can express yet (no variadics,
generics, or comptime). Its eviction target is the prime-era stdlib
([HORIZON0.md](../design/HORIZON0.md) Component 5 update,
[ledger.md](../planning/ledger.md), [MASTERPLAN.md](../planning/MASTERPLAN.md)).

## Restored corpus

- `eyesrc/programs/bubblesort.eye` - variadic `printf` (e2e `bubblesort_runs`).
- `eyesrc/programs/file.eye` - opaque `FILE`, `FILE*` signatures, variadic
  `printf`, `calloc`/`free`/`exit`; reads and prints itself.
- e2e: `variadic_extern_printf_runs`, `opaque_extern_type_fopen_roundtrip`,
  `variadic_misuse_is_rejected`; parser and HIR unit tests for the parse
  shapes, the three rejections, the `variadic` flag, and the opaque
  namespace.

The tree-sitter grammar (`eye-tools/treesitter/grammar.js`) was ported in the
same change, and the parity gate (`scripts/check-grammars.sh`) was fixed to
walk the nested corpus - its flat glob had been matching nothing since the
`eyesrc/{lang,programs,ffi}` split, letting five earlier surface additions
(block-scope const, struct destructure, guards, literal patterns, repeat
arrays) drift silently. All ported; the gate is green and non-vacuous (it now
fails if it finds zero corpus files).
