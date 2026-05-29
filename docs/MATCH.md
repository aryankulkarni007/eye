# Match kernel scope

`match` should stay small in the Eye kernel.

The kernel feature is not rich pattern matching. It is discrete control-flow
selection: given a scrutinee that can be reduced to a discrete discriminant,
choose one arm, optionally produce one value, and enforce the local correctness
rules that make that operation coherent.

Today this is effectively an enum-backed `switch`. That is not a bug; it is the
right starting point. The long-term kernel shape is a generalized discriminant
match, not a payload sum-type system.

## Current shape

The implemented v0.3 surface is:

```eye
enum Shape = Circle | Rectangle | Triangle;

main() {
    let Shape sh = Circle;
    let int32 code = match sh {
        Circle -> 1,
        Rectangle -> 2,
        Triangle -> 3,
    };
}
```

HIR resolves the scrutinee as a known enum, lowers arms to variant or wildcard
patterns, checks exhaustiveness for the enum's finite variant set, and codegen
emits a C `switch`.

Current supported pattern forms:

- exact enum discriminant
- wildcard `_`

Current semantic checks:

- non-enum scrutinee diagnostic
- duplicate arm diagnostic
- unreachable arm after wildcard diagnostic
- enum exhaustiveness diagnostic when no wildcard is present

Current codegen forms:

- statement-position match lowers directly to `switch`
- value-position match hoists into a temporary and assigns it inside `switch`

## Kernel contract

The kernel contract should be:

```text
match <discrete-scrutinee> {
    <discriminant> -> <expr>,
    _ -> <expr>,
}
```

Where:

- the scrutinee has a discrete domain
- an arm pattern resolves to one discriminant value, or wildcard
- duplicate discriminants are rejected
- arms after wildcard are unreachable
- exhaustiveness is checked when the compiler has a known finite universe
- value-position matches have one result type

This keeps `match` as a low-level dispatch primitive. Higher-level pattern
features can lower into this form later.

## Discrete domains

The generalization target is not "enum" specifically. It is a domain that can
provide discriminants.

Supported now:

- enum variants

Reasonable kernel domains:

- enum discriminants
- `bool` (`false`, `true`)
- integer literal labels
- `char` / ASCII byte labels
- integer or char ranges, if ranges are admitted as a compact label syntax

The important distinction is finite-known versus merely discrete:

- enum and bool have small known universes, so exhaustiveness is meaningful
- full-width integers are discrete but huge, so practical exhaustiveness should
  require `_` unless the type is later narrowed by a distinct finite-domain
  type
- ASCII or byte-sized char domains are bounded, but range coverage should be
  implemented deliberately, not accidentally through ad hoc arm checks

This suggests an internal abstraction along these lines:

```text
DiscreteDomain:
    representation type for codegen
    pattern label -> discriminant value
    optional finite universe for exhaustiveness
```

Enums are just the first implementation of that abstraction.

## Match result type

Value-position match needs one result type. This is kernel-worthy because it is
part of what makes a value-producing dispatch operation coherent.

This does not require a full typechecker. A local rule is enough for the
trivial case:

1. If the match appears in an explicitly typed context, use that as the
   expected result type.
2. Otherwise use the first arm body with a known type as the provisional result
   type.
3. Check every other arm body with a known type against that result type.
4. Do not cascade on unknown arm types; leave them unknown until typechecking
   exists, or emit a narrow diagnostic only when codegen needs a concrete type.
5. Record the match expression type as the resolved result type.

Example:

```eye
let int32 code = match sh {
    Circle -> 1,
    Rectangle -> 2,
    Triangle -> 3,
};
```

The explicit `int32` binding gives the match its expected type.

This should be rejected even before full type inference exists:

```eye
let int32 code = match sh {
    Circle -> 1,
    Rectangle -> "bad",
    Triangle -> 3,
};
```

The rule is local: all arms in a value-position match must produce the match's
result type when their types are known.

Statement-position match has no result type requirement.

## Exhaustiveness

Exhaustiveness belongs in the kernel for known finite domains.

Good targets:

- all enum variants covered
- both bool values covered
- all members of a small finite domain covered, if such a domain exists later

For large primitive domains, require wildcard for totality:

```eye
match n {
    0 -> ...,
    1 -> ...,
    _ -> ...,
}
```

Range coverage should be explicit. If integer or char ranges are added, coverage
should be computed over normalized intervals, not through string or syntax
special cases.

## Out of kernel scope

These should not be kernel `match` features:

- payload enum syntax
- payload destructuring
- `Some(x)`-style binding
- struct, tuple, array, or slice patterns
- guards
- extractor patterns
- custom user pattern protocols
- match ergonomics
- general sum types

Those features can be stdlib or supermacro features if the extension substrate
can lower them into kernel discriminant match plus ordinary expressions.

## Near-term implementation path

The next useful kernel work is not richer pattern syntax. It is tightening the
existing primitive:

1. Add match arm result type resolution.
2. Use explicit typed `let` context when available.
3. Diagnose mismatched known arm types.
4. Keep first-known-arm fallback for untyped contexts until inference exists.
5. Keep enum-only discriminants for now.
6. Later generalize HIR from enum-only matching to a `DiscreteDomain` model.
7. Add bool matching before integer/range matching; it is the smallest useful
   proof that match is no longer enum-specific.

Only after that should integer labels, char labels, and range arms be considered.

## Design line

Kernel `match` is a glorified `switch`, deliberately.

The kernel should own:

- discriminant dispatch
- local arm validity
- exhaustiveness for known finite domains
- one result type for value-producing dispatch

The kernel should not own rich pattern matching. Rich patterns are a higher
level language facility that should eventually lower into this primitive.
