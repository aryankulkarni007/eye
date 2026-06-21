# Mutability: immutable by default

Eye bindings are immutable unless declared `mut`. This is a no-footgun rule in
the [[FUTURE.md]] F-series mold: silent mutation of a binding the author meant
to be fixed is a class of bug the compiler can rule out for free, so it does.

```eye
let int32 x = 5;
x = 6;          -- rejected: `x` is immutable

mut int32 y = 5;
y = 6;          -- ok
y += 1;         -- ok
```

`let` and `mut` are the two binding keywords; both require an initializer
(valid-by-construction, no uninitialized binding). The keyword is the only
difference: an explicit type is optional after either.

## What "immutable" forbids

A binding's mutability governs **writes whose target roots in that binding**.
The check walks the assignment's left-hand side down to the local it ultimately
writes:

- `x = v` writes `x` directly.
- `s.f = v` and `a[i] = v` write the local the projection roots in (`s`, `a`).
- `*p = v` writes *through a pointer*, not the binding `p`. This is not tracked
  (see the escape below).

If the rooted local is an immutable `let`, the write is rejected with
`TypeError::AssignToImmutable` (class `T`). The rule is deep, not shallow:
mutating a field of a `let`-bound struct is rejected, because the struct binding
is immutable.

```eye
structure P { int32 a, };
let P p = P { a: 1 };
p.a = 9;        -- rejected: the write roots in immutable `p`
```

Both plain `=` and every compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`,
`&=`, `|=`, `^=`, `<<=`, `>>=`) go through the same check.

## The raw-pointer escape

A write through a pointer is deliberately **not** tracked:

```eye
mut int32 x = 5;
let int32* p = &x;
*p = 99;        -- allowed; `x` is now 99
```

`let int32* p` makes the *binding* `p` immutable - `p = &y` is rejected - but the
memory `p` points at is not part of `p`'s mutability. Writing through it is
allowed. This is consistent with Eye's runtime model: a raw pointer grants total
machine-level freedom at runtime ([[FARFUTURE.md]]), and the compiler tracks the
binding, not the reachable memory.

Two consequences of the same principle, both intentional and both currently
unchecked:

- `*p = v` through a `let`-bound pointer, as above.
- Taking `&x` of an immutable binding yields a pointer you can write through.
  Eye has no `&`/`&mut` split, so an immutable binding does not make `&x` a
  pointer-to-const.

These are escapes, not oversights. Lifetime / escape analysis that would close
them is a separate runtime-safety axis, deferred ([[DEFER.md]]), not part of the
binding-mutability rule.

## Parameters

! Current state: function parameters are **mutable**, the one place the
immutable-by-default rule is not applied. There is no `mut`-parameter syntax, so
a default-immutable parameter would reject in-body reassignment with no way to
opt out. Until the grammar grows the marker, reassigning a parameter is allowed.

This is a half-built model, not a decision: bindings and globals are
immutable-by-default with a `mut` opt-in, but parameters - which are bindings -
are not. The completion is designed below; nothing here is built yet.

### Const-by-default parameters (designed, not built)

- A parameter with no marker is **immutable**, the same rule as `let`. Its
  `Local.mutable` flag is `false`, and in-body reassignment (or a write rooted in
  it) is rejected with the existing `AssignToImmutable` (`T`).
- `mut` before a parameter opts into a mutable local copy, exactly as `mut` does
  for a `let`:

  ```eye
  square(int32 n) { ... }       -- n immutable
  accum(mut int32 n) { ... }    -- n is a mutable local copy
  ```

- Grammar: a `mut` marker on `Param` (the `ParamList` arm), mirroring the
  `let`/`mut` binding keywords. `Param` grows a `mutable: bool`; collection sets
  it; the `AssignToImmutable` check already fires on the `Local`.

This is the silent-safety rule from [[PHILOSOPHY.md]] applied to the call
boundary: protection with no keyword, the dangerous direction (`mut`) opt-in.

### FFI const-correctness (the concrete motivator)

Const-by-default parameters also close an FFI defect. clang knows libc functions
as builtins with const-qualified pointer parameters - `memcpy` is
`void *memcpy(void *dest, const void *src, size_t n)`. An Eye `extern` that
declares the signature emits a **non-const** prototype for `src`, which conflicts
with the builtin and warns ("incompatible redeclaration of library function").

With const-by-default parameters, an unmarked `extern` parameter emits a `const`
C parameter, so the Eye prototype matches the builtin and the warning is gone; a
`mut` parameter emits the non-const form. The qualifier the user never writes is
the correct one for the common (read-only) case - silent safety reaching into the
generated C.

```eye
extern {
    memcpy(ptr dest, ptr src, usize n) -> ptr;   -- src should emit `const void*`
}
```

## Mutable references (`&mut`) - designed, not built

Eye has `&T` (a shared, immutable reference - [[KERNEL.md]]) and the raw pointer
`T*` / `ptr`. There is no `&mut T`. The consequence: **mutating through a
reference forces the raw-pointer escape** (`T*` plus `*p = v`), which is the
`ffi`/unsafe boundary ([[EFFECT.md]]). There is no *safe* mutable borrow.

`mut` parameters make the gap visible: once a parameter can be an immutable
borrow `&T`, the natural opt-in for "I intend to mutate the caller's value" is a
mutable borrow `&mut T`, not a drop to a raw pointer. The two opt-ins are the
same rule at two scopes:

- `mut x` = mutate this binding (a local).
- `&mut x` = a borrow through which the pointee may be mutated.

`&mut T` would be a checked, safe mutable borrow (no `ffi` effect), reserving the
raw-pointer escape for genuine machine-level freedom. This is the directional,
dangerous-direction-gated model: `&T` (safe, default, silent), `&mut T` (explicit
opt-in), `T*`/`ptr` (the unsafe escape). Closing the dangling-`&local` hole
([[DEFER.md]] escape analysis) is the safety axis that would let `&mut` be fully
sound; until then `&mut` would carry the same runtime freedom as `&` does today.

### Aliasing model (decided 2026-06-21, option B)

I ship `&mut` as an **honest mutable borrow** and defer the aliasing rule, rather
than ship a partial one. Two axes, decided separately:

- **mutability axis - enforced now.** `&T` is read-only (write-through rejected);
  `&mut T` permits writes; the raw pointer remains the unsafe escape. The type
  states mutation capability and the compiler holds the user to it. Predictable.
- **aliasing axis (uniqueness / shared-XOR-mutable) - deferred.** Nothing stops a
  `&mut` overlapping another `&mut` or `&`; mutating through overlapping borrows is
  the user's problem until escape analysis ([[DEFER.md]]) can enforce uniqueness
  whole-program. I rejected an intraprocedural-only guardrail: it would reject a
  pattern within a body and allow the same pattern across a call - an inconsistent,
  unpredictable rule, which is the real footgun (predictable magic over no magic,
  [[MEM.md]] commitment #3). One consistent rule when it can be sound beats a holey
  one now.

Consequences of deferring the aliasing axis: no `restrict`/noalias optimization on
`&mut` parameters yet (minor, deferrable), and `&mut` uniqueness is what
concurrency safety will later require - but concurrency is far-future, so the
timing is free.

## Where it lives

- Enforcement: HIR lowering, in the assignment arm
  (`crates/hir/src/core/lower/expr.rs`, `immutable_assign_target`). The binding's
  `mutable` flag is recorded on the `Local` at let/param lowering.
- Codegen emits no C `const` for immutable bindings. Immutability is fully
  enforced in HIR before codegen, so the MIR-to-C printer makes no mutability
  decision. (An earlier `const` emission also mis-rendered an immutable pointer
  binding as pointer-to-const, which wrongly rejected the write-through-pointer
  escape; dropping it fixed that.)

## Status and scope

- + immutable-by-default bindings (`let`/`mut`), deep, with the raw-pointer
  escape - built.
- - const-by-default parameters + `mut` marker - designed above, not built.
- - `&mut T` mutable-reference split - designed above (option B, 2026-06-21:
  mutability axis enforced, aliasing deferred to escape analysis), not built.
- ! escape / lifetime analysis (dangling `&local`) - the runtime-safety axis
  that makes `&mut` fully sound; deferred ([[DEFER.md]]), tracked separately.
- = `const` / compile-time constants are a separate kernel item ([[KERNEL.md]]);
  immutable bindings are a runtime concept, not compile-time constants.
