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

Function parameters are currently mutable: there is no `mut`-parameter syntax, so
a default-immutable parameter would reject in-body reassignment with no way to
opt out. When the grammar grows a `mut` parameter marker, parameters can become
immutable by default like `let` bindings. Until then, reassigning a parameter is
allowed.

## Where it lives

- Enforcement: HIR lowering, in the assignment arm
  (`crates/hir/src/core/lower/expr.rs`, `immutable_assign_target`). The binding's
  `mutable` flag is recorded on the `Local` at let/param lowering.
- Codegen emits no C `const` for immutable bindings. Immutability is fully
  enforced in HIR before codegen, so the MIR-to-C printer makes no mutability
  decision. (An earlier `const` emission also mis-rendered an immutable pointer
  binding as pointer-to-const, which wrongly rejected the write-through-pointer
  escape; dropping it fixed that.)

## Not in scope

- `mut` parameters (needs grammar; see above).
- `&`/`&mut` reference split and escape analysis (a runtime-safety axis,
  [[DEFER.md]]).
- `const` / compile-time constants - a separate kernel item ([[KERNEL.md]]);
  immutable bindings are a runtime concept, not compile-time constants.
