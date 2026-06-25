# Horizon 0: freeze the kernel - complete design

This is the end-to-end design for the remaining kernel-completion work. When the
checklist at the end clears, the kernel can freeze (become unoverwriteable, per
[VISION.md](VISION.md)). Status: **Component 1 (const) is BUILT (2026-06-06)** -
see [CONST.md](features/CONST.md) for the as-built notes; Components 2-5 remain DESIGNED.

For the strategic context (why this is the bottleneck) see
[MASTERPLAN.md](planning/MASTERPLAN.md); for the per-item gap ledger see
[KERNEL.md](KERNEL.md). This doc supersedes neither; it is the design those two
point at.

## What "freeze" means and how the gap was derived

The kernel is complete iff **every primitive a supermacro provably cannot
synthesize is present, and nothing else is** ([VISION.md](VISION.md)). VISION
also hands us the derivation identities directly:

- `sum types = union + tag + extensible match`
- `generics = comptime + AST instantiation`
- `OOP / vtables = struct + fn-ptr + raw-ptr`
- `while / for = loop + if + break`

Running the discriminating test on each stdlib feature and expanding to its
kernel dependencies produces this trace:

| Stdlib feature | = kernel deps | Unmet dep |
|----------------|---------------|-----------|
| `while` / `for` | `loop` + `if` + `break` | none (built) |
| OOP / vtables | struct + fn-ptr + raw-ptr | none (built) |
| iterators | struct + fn-ptr + `loop` | none (built) |
| sum types / Option / Result | `union` + int tag + extensible match | match seam (designed, inert) |
| Vec / containers | `malloc` (FFI) + raw-ptr + `sizeof` + comptime | **sizeof**, **comptime** |
| generics | comptime + AST instantiation | **comptime**; AST-inst is the engine (H2) |
| owned strings | Vec\<byte\> + string-literal seed | **string literals** |

Deduplicating the unmet column: the whole remaining kernel-*freeze* gap is **const
plus two leaves plus one already-decided seam**. All finite. The thing that
*unlocks generics* (compile-time execution: CTFE, the prime VM, the macro engine)
is **not** a freeze item - it is far-future ([PRIME.md](features/PRIME.md)). So generics do
not land at the freeze regardless, and the freeze itself is small.

## The shape of the gap

- **const.** A finite kernel-floor item: compile-time constant values plus a
  bounded const-expr fold. It does **not** run code at compile time and does
  **not** unlock generics; that is prime execution, far-future
  ([PRIME.md](features/PRIME.md)). The real *large* build after the freeze is the typeck
  surgery (MASTERPLAN), not const.
- **Leaf - `sizeof` / `alignof`.** A macro cannot compute a type's layout
  portably. Independent of const; ships standalone.
- **Leaf - addressable static data** (globals + string literals as one
  primitive). A macro cannot manufacture a static data address. Independent of
  const; ships standalone.
- **Seam - minimal `match` skeleton + lowering hook.** Already resolved (B2). The
  runtime skeleton exists; the deliverable is a documented seam, not new
  behavior.

The C-seam plumbing (variadic `extern ...`, opaque FFI pointer types, drop the
auto-`#include`) and the subtractive eviction of `print` are real kernel-freeze
items but low-identity, sequenced last and lazily.

## Component 1: const (compile-time constant values)

> **Status: BUILT 2026-06-06.** Top-level scalar `const` with a bounded
> const-expr fold, const-of-const (cycle-checked), and const-length arrays (A6)
> all land; references inline the folded value (a value has no address, so it is
> substituted, not emitted as a C symbol). Deferred from the floor: local
> (block-scope) const, aggregate const values, const type-checking, and the
> `sizeof`-tainted path (needs Component 2). See [CONST.md](features/CONST.md) and
> [DEFER.md](planning/DEFER.md).

This is const, **not** comptime execution. const provides compile-time constant
*values* plus a bounded const-expr fold; it does not run code at compile time and
does not unlock generics. Compile-time *execution* (CTFE, the prime VM, the macro
engine) is a separate, far-future layer designed in [PRIME.md](features/PRIME.md); see the
const-vs-prime distinction there.

### Ratified surface (2026-06-05)

The type follows the binding keyword (`<kw> <type> <name>`), matching the rest of
Eye (`let string x`). Three top-level forms, cleanly separated by *value vs
storage*:

```
const int32 MAX  = 100;            // compile-time VALUE  - no guaranteed address
const usize SIZE = sizeof(Point);  // const-expr may call sizeof
const int32 DBL  = MAX * 2;        // const-expr may reference other consts

let int32 origin  = 0;             // immutable STORAGE - addressable (Component 3)
mut int32 counter = 0;             // mutable   STORAGE - addressable (Component 3)
```

The model and the decisions it settled:

- **`let` / `mut` are storage bindings; `const` is a value.** `let`/`mut` are not
  overloaded between scopes - they mean one thing (a storage binding) and *scope
  sets storage duration*: stack if local, static if top-level. `const` is the
  genuinely different thing: a value, not a location.
- **`&const` is illegal.** A value has no guaranteed address. To point at data, use
  a top-level `let` (static storage) and take `&TABLE`. `const` is for scalars
  like `const int32 MAX = 100`; static `let` is for addressable aggregates.
- **`const` works at any scope.** A local `const int32 N = 4` is fine (e.g. a local
  array length). It is a value, so it is scope-free.
- **`const` is the only form usable in const-contexts.** `[int32; N]` requires `N`
  to be a compile-time value, so `N` must be a `const`, never a `let`.
- **Scalar-only at the floor.** const holds a scalar value. Aggregate const values
  (`const [int32; 3] xs = [1,2,3]`) are deferred; addressable aggregates use a
  top-level `let` static instead.
- **Explicit types at the floor.** `const int32 MAX = 100`, not `const MAX = 100`.
  Consistent with Eye's current type-explicit surface. Type *inference* (omitting
  the type on `let` / `mut`, and eventually `const`) is a payoff of the later
  typeck surgery ([PRIME.md](features/PRIME.md) D2), not a const-floor concern.

### Surface and grammar

- **token** (`crates/token/src/lib.rs`): add `#[token("const")] Const`. The
  keyword list today is let, mut, structure, enum, union, extern, if, else,
  loop, break, continue, return, match, as.
- **grammar** (`crates/parser/src/grammar/items.rs`, `item()`): add a `T![const]`
  arm producing `const_def := 'const' type Ident '=' expr ';'`, plus top-level
  `let` / `mut` arms for globals (Component 3).
- **AST** (`crates/ast/eye.ungram` + regenerate `generated.rs`): add a `ConstDef`
  node (type, name, body expr).

### HIR

- **collect** (`crates/hir/src/core/lower/collect.rs`): collect const items into a
  `ConstId` arena alongside fns / structs.
- **resolution**: add `Resolution::Const(ConstId)`. A const reference in expr
  position resolves to it; a scalar const folds to its value (or emits its C
  symbol).
- **const-expr evaluator** (new pass): the bounded folder. It folds a restricted
  expr subset to a scalar value. Reuse the cycle-detection pattern already in
  `crates/hir/src/core/typegraph.rs` for const-references-const cycles. Extend the
  existing `ConstError` (`crates/hir/src/core/errors.rs`) for non-const operands.

### The explicit upper bound (ratified - do not skip this)

This is const, not prime execution. AST instantiation (what generics need) **is**
the engine (far-future, [PRIME.md](features/PRIME.md)). So generics do not land at the
freeze no matter how far const is pushed; the const-expr evaluator is deliberately
narrow. It handles exactly:

- integer / float / bool literals (**scalar only** - no aggregate const values);
- the operator set (arithmetic, bitwise, comparison, logical) over const operands;
- references to other consts (cycle-checked);
- `sizeof(T)` (Component 2).

It explicitly does **not** handle: runtime values, **function calls (that is CTFE
= far-future)**, type-as-value beyond `sizeof`, or any AST manipulation. Stating
this bound is load-bearing: without it, const silently grows into CTFE and
type-reflection, which belong to the prime layer, not the freeze.

**The `sizeof` fold boundary** (where this meets Component 2). A pure int / bool /
float const-expr folds to an actual Eye value. A const-expr that transitively
contains `sizeof` does **not** fold to an Eye integer, because `sizeof` has no
Eye-side value (Component 2 leans on C for layout). Instead the whole expression
is emitted verbatim as a C constant expression and C folds it:

```
const int32 DBL  = MAX * 2;      // pure -> folds to Eye value 200
const usize PAIR = sizeof(T) * 2; // contains sizeof -> emits C `sizeof(ctype) * 2`, unfolded in Eye
```

This is the same "lean on C, build no layout model" instinct (a real Eye-side
layout model is still Horizon 3). The evaluator simply tracks whether a const-expr
is sizeof-tainted; tainted ones are passed through to C as constant expressions,
pure ones are folded.

**Consequence for A6 const-length arrays.** const unblocks `[T; N]` with a const
`N` ([DEFER.md](planning/DEFER.md)), but the two fold classes split here too. `array_len`
(`crates/hir/src/core/lower/types.rs` ~L62) wants a `u64` count: a pure-const `N`
folds to that `u64` and works directly. A sizeof-tainted length such as
`[T; sizeof(U)]` cannot - Eye never learns the count - so it must lower to a C
array `T[sizeof(ctype)]` with the dimension carried as an unfolded C constant
expression. Both are coherent; the implementer must handle the two paths.

### MIR and codegen

- MIR is per-function bodies today (`MirBody`). Add a module-level section
  (`globals`, `consts`) so consts and globals exist outside any `MirBody`.
- A scalar const emits as C `static const T NAME = <folded>;` (or inlines its
  folded value). `crates/codegen/src/core/mir_emit/` gains a pre-pass that
  prints these before the function bodies.

## Component 2: sizeof / alignof (leaf)

> **Status: BUILT 2026-06-06** ([SIZEOF.md](features/SIZEOF.md)). `sizeof(T)` is a
> callee-name-sniffed, user-shadowable `usize` intrinsic that lowers to C
> `sizeof(ctype)` (no Eye layout model). Floor accepts a bare named type;
> compound-type arguments, `alignof`, and the sizeof-tainted const-expr path are
> deferred ([DEFER.md](planning/DEFER.md)).

Mirror the `len` intrinsic exactly. `len` and `print` are name-sniffed in callee
position in `crates/hir/src/core/lower/expr.rs` (~L122-128) and are
user-shadowable; `sizeof` adds a third name and a `lower_sizeof_intrinsic`. It
returns `usize`, is user-shadowable like the others.

**Lean on the C backend.** Because the backend is a C printer, `sizeof(T)` in Eye
lowers to `sizeof(ctype)` in C. C is the portable layout authority, so **no
Eye-side layout model is needed at the floor**. A const-context use
(`const usize N = sizeof(T)`) emits `static const usize N = sizeof(ctype);` and C
folds it. This is the transpiler dividend: the hardest part of `sizeof` is free
today.

**The arg is a type, not a value.** `len(arr)` takes a value; `sizeof(T)` takes a
type. Because the intrinsic is recognized after parsing (by name), the argument
has already parsed as an expression. So the floor supports `sizeof(NamedType)`
(builtin / struct / union / enum), resolved by treating the path-expr as a type.
Compound-type arguments (`sizeof(&T)`, `sizeof([T; N])`) need type-in-argument
parsing and are deferred; none of the floor's container math requires them.

`alignof` is the same mold (emit C `_Alignof`) and is optional; defer until a
container needs it.

**Marked for termination.** `sizeof` is only needed in its intrinsic form *while
the backend is C*. Two later forces retire it: (a) when Eye owns its backend it
must compute layout itself (a target data-layout model), and (b) once prime makes
**types first-class values** ([PRIME.md](features/PRIME.md) D8), `sizeof` is just an
accessor on a type-value, not a kernel intrinsic. So `sizeof` is "free now via C,
real work at the own-backend transition, and ultimately not an intrinsic at all."
Correct and complete for the freeze regardless. See *Intrinsics are interim*
below.

## Component 3: addressable static data (leaf - the collapse)

> **Status: BUILT 2026-06-06** (both parts). Part A - top-level `let`/`mut`
> globals (`eyesrc/lang/global.eye`): const-evaluable initializer folded by
> `eval_globals`, `let` read-only / `mut` writable (immutable-by-default
> enforced, `&G` legal), `Place::Global` in MIR, emitted as file-scope C statics
> in a codegen pre-pass. Part B - string literals as `&[uint8; N]`
> (`eyesrc/lang/string.eye`, `eyesrc/ffi/caesar.eye`, [STRING.md](features/STRING.md)):
> NUL-terminated byte statics, wrapper-pointer value reusing the array machine,
> `print` `%s` over `->data` (closing the `%d` bug), escapes decoded so `N` is
> the decoded byte count. The length-polymorphism resolution is built: a
> `&[T; N]` **decays** to `&T`/`string` (a pointer cast) at let-init / argument /
> return, so strings pass to functions and FFI (`extern strlen(string s)`); the
> fat-pointer slice stays stdlib. caesar runs on this path.

Resolving the open design question (are const / globals / strings one primitive
or several?): **two clusters, not three.**

- **const scalar value** = `const` (Component 1). A value; no guaranteed address.
- **Addressable static data** = globals + string literals, **one primitive**. A
  global is a named static; a string literal is an anonymous static byte array
  whose value is its address. Both are "static storage with an address, which a
  macro cannot manufacture," and both bottom out on the same C mechanism
  (file-scope storage / `.rodata` / `.data`).

### Globals

Top-level `let` / `mut` bindings are global storage - the same storage-binding
meaning as a local `let` / `mut`, with static (not stack) duration. `let` is
read-only, `mut` is mutable; both initializers must be const-evaluable (a `mut`
with no initializer is zero-init). No runtime global constructors at the floor (C
requires constant static initializers). Grammar: add top-level `let` / `mut` item
arms; HIR collects a `GlobalId`; `Resolution::Global(GlobalId)`; codegen emits
file-scope C variables in the same module pre-pass as consts.

### String literals (ratified)

Today `Literal::String` exists in HIR (`crates/hir/src/core/body.rs`) but is typed
`TypeRef::Path("string")` with no real string type, so `print` renders it `%d`
(the KERNEL.md "string literals PARTIAL" row, and a live bug).

**A string literal is `&[uint8; N]`** - a reference to a fixed byte array, the
only choice consistent with Eye's existing array model and the subtractive kernel
line (a length-erased fat-pointer `str` type would be a slice, which VISION puts
in stdlib, not kernel). Mechanics:

- `"hello" : &[uint8; 5]`. `N` is the **visible byte count, excluding NUL**, so
  `len("hello") == 5` (no footgun). It reuses the whole array machine already
  hardened (auto-deref `r[i]`, `len(r)`, OOB checks).
- codegen emits the static byte array **with a trailing NUL** for C interop, but
  Eye never counts the NUL.
- **decays to `&uint8` / ptr** when passed to an FFI pointer param - the same
  decay rule as `&[T; N]` to `&T`.
- `char` = `uint8` at the floor (UTF-8 bytes); a real codepoint / grapheme type is
  stdlib.

This closes the `%d` bug, gives FFI real pointers, and seeds owned strings - all
three KERNEL.md string goals with zero new type machinery. **Revisit the string
primitive once Eye owns its backend** (a native string representation can be
reconsidered then; `&[uint8; N]` is the C-backend-era choice).

## Component 4: match - full over primitive domains, extensible over user domains

B2 (extensible match) is resolved ([KERNEL.md](KERNEL.md)), but the cut was
refined (2026-06-05): **B2 was about not baking *sum types* into the kernel, not
about crippling primitive matching.** So kernel `match` is **full-featured over
the kernel's own discrete domains**, and extensible to user-defined domains via
the seam. Making the user import a library to get guards or to match an integer
would be the wrong kind of minimal.

### Ratified kernel match surface

- **Domains** (scrutinee types): enum, int (all widths), char, bool.
- **Patterns**: enum-variant, int / char / bool literal, **range** (`a..b`
  exclusive, `a..=b` inclusive - pattern-only, Rust-style), **or-pattern**
  (`p | p`), wildcard `_`, binding `x`, **guard** (`pat if expr`).
- **Exhaustiveness**: finite domains (bool, enum) are proven (all cases or a
  default); unbounded domains (int, char) require an explicit `_`.
- **Deferred to the seam** (user-defined / composite domains only): sum-type
  payload destructure (`Some(x)`), struct / tuple destructure (`Point{x, y}`).

### The seam (built now, opened later)

Decompose any arm into TEST + BINDINGS + GUARD + body. The registration unit:

```
lower_pattern(pat, scrutinee: Place) -> PatternMatch {
    test:     Option<Predicate>,        // None = irrefutable (wildcard / binding)
    bindings: Vec<(Name, Projection)>,  // each name = a Place projected from scrutinee
}
```

A pattern kind registers its lowering. **The seam-shaped machinery is built now**
(the test / bindings / projection model and the assembly below); the kernel
*internally* registers all the primitive-domain patterns above. What is **deferred
to the prime engine** is *external* registration - stdlib `prime fn`s adding
pattern kinds over user-defined domains (a payload pattern lowers to a union-field
projection binding; a struct pattern to field projections). So the architecture is
the seam from day one; the engine merely opens registration to stdlib. The
standing negative commitment narrows to: the kernel bakes in no patterns over
*user-defined composite* shapes.

### Assembly - reuses existing MIR, no new nodes

```
if every arm.test is `tag/value == const_i`, distinct consts, no guards:
      -> MirStmt::Switch        (the fast path, already exists; works for enum/int/char)
else:
      -> if / else-if decision chain   (MirStmt::If nesting, already exists)
```

Guards, ranges, or-patterns, and any non-constant test force the if-chain backend.
Bindings become MIR locals initialized from the scrutinee projection, scoped to
the arm body. Union-payload projection (the future destructure case) is already
expressible in MIR `Place`. So match needs **zero new MIR** - it is a HIR-lowering
architecture feeding `Switch` + `If`.

### Amendment 2026-06-06: bare-ident rule, struct destructure, boundary redraw

Three refinements ratified in a design session, superseding the relevant lines
above. The first two expand the surface; the third redraws a VISION-level line to
keep the expansion principled.

**1. Bare-ident pattern rule (no sigil, no casing, footgun-free).** A bare ident
in a pattern resolves against the *scrutinee type*: for an enum scrutinee it is a
variant (a non-variant bare ident is a hard error, which the lowering already
emits as `UnknownVariantInPattern` / `NoSuchVariant`); for an int / char / bool
scrutinee it is a binding (no variant namespace exists to resolve against, so it
is unambiguous). The Rust footgun - "meant a variant, silently got a binding" -
cannot occur: over an enum a misspelled variant errors rather than binding, and
over a primitive there are no variants to shadow. This is the minimal extension of
the type-directed resolution the kernel already performs; a leading-dot `.Variant`
form was considered and rejected as less clean. Consequence: a whole-value binding
over an enum scrutinee (`match e { x -> }`) is an error, not a binding - whole-value
binding has no use when the scrutinee is already a named expr; the real binding use
is struct fields (below).

**2. Struct destructuring is a chosen kernel primitive.** `let Point { x, y } = p;`
(sugar for `let x = p.x; let y = p.y;`), the rename form `let Point { x: px } = p;`,
and the match-arm form `match p { Point { x, y } if x > 0 -> ... }`. It desugars to
field projections plus bindings. Strictly it is macro-synthesizable (pure projection
sugar), so it fails the "provably unsynthesizable" kernel test - but it is adopted
as a *chosen* ergonomic primitive, the same class as early return (see
[KERNEL.md](KERNEL.md): `return` is subsumable by labeled-break-with-value yet
adopted as a peer primitive). Ergonomics for primitive syntax earns a kernel slot.
This pulls bindings into the freeze, which is where they gain a real home (struct
fields) rather than the speculative whole-value binding otherwise deferred.

**3. The boundary redraw (the load-bearing call).** Pulling struct destructure in
crosses the standing negative commitment "the kernel bakes in no patterns over
user-defined composite shapes." That line is redrawn, not erased, along the
refutability axis:

- **Kernel:** *irrefutable* destructure of a statically-known composite (struct
  fields). Introduces **no test, only bindings** - a `Point` is always a `Point`.
  Pure multi-assignment sugar; it does not bake an ADT into the kernel.
- **Seam / prime:** *refutable* patterns over user-defined sum shapes (`Some(x)` -
  a projection valid only after a tag test). This is the part that would bake ADTs
  into the frozen kernel, and it stays out (far-future, [PRIME.md](features/PRIME.md)).

So the negative commitment narrows from "no composite patterns" to "no *refutable
sum* patterns." Struct field sugar is `a = p.x` written shorter, not an ADT; what
VISION wanted kept out of the frozen kernel (sum types) stays out.

### Build segments (as-sequenced 2026-06-06)

The codegen already renders `MirStmt::Switch` as an `if`/`else-if` chain, not a C
`switch` (so a match-arm `break` binds to the enclosing loop) - so the "Switch
fast-path vs if-chain" split above is moot at the C layer; MIR `Switch` is already
an ordered test-chain. The build generalizes its arm from `{variant}` to a general
arm test, computed in HIR-lowering. Order:

- **S0** - **BUILT 2026-06-06.** seam refactor (no behavior change): MIR
  `SwitchArm` carries an extensible `ArmTest` (`Variant`; S1 added `Const`, S4
  adds `Range`/`Or`). Bindings and a per-arm guard slot are added by S2 / S3.
- **S1** - **BUILT 2026-06-06.** literal patterns (`LiteralPat` = int / char /
  bool; float and string excluded) + int / char / bool scrutinee domains.
  `ArmTest::Const` lowers to `scrut == <const>`. Exhaustiveness: enum + bool
  proven, int / char require `_`; a literal whose domain disagrees with the
  scrutinee is a `PatternDomainMismatch` (int and char cross-match as C integer
  comparisons; bool and enum stay strict). Bare ident over a non-enum scrutinee
  stays the interim unknown-variant error (binding lands in S2).
- **S2** - **BUILT 2026-06-06.** `let` struct destructuring (`let Point { x, y } = p`,
  rename `let Point { x: px } = p`) - exhaustive (every field bound; no `..`/ignore
  yet), `HIR Pat::Struct` expanded in MIR into one field-projection `Let` per binding
  (value-control-flow init spilled to a temp first). Plus the type-directed
  whole-value bare-ident binding in match over a primitive scrutinee
  (`match n { x -> x + 1 }`): an irrefutable named wildcard, lowered by binding the
  scrutinee to a per-arm local. **Deferred to S3:** struct patterns *in match arms*
  (need scrutinee-as-place projection + guard nesting).
- **S3** - **BUILT 2026-06-07 (guards only).** guards (`pat if expr`) - arm-level,
  bindings live when the guard runs. Struct patterns in match arms deferred
  (need scrutinee-as-place projection).
- **S4** - ranges (`a..b` / `a..=b`) + or-patterns (`p | p`).
- **S5** - exhaustiveness / usefulness pass (prove bool + enum, `_` for int / char,
  flag unreachable arms).

## Component 5: the C-seam cluster + print eviction

> **Status: BUILT 2026-06-11** ([FFI.md](../features/FFI.md)). Variadic `...`
> (extern-only, last position, one named param required), opaque
> `extern { type FILE; }` (forward typedef, no definition), and the
> auto-`#include <stdio.h>` dropped - the `println` intrinsic self-declares
> `int printf(const char *, ...);` when the program declares no `printf`,
> exactly the interim below. `bubblesort`/`file` restored. Two deltas from
> the ratified sketch: fixed extern params are *not* yet type-checked at the
> call (no typeck pass exists for any call), and a by-value opaque use is a
> C-side incomplete-type error rather than an Eye diagnostic - both land
> with the typeck split. `println`'s eviction remains post-typeck.

Low-identity but necessary kernel-freeze plumbing (the kernel bottoms out at the
machine via FFI). All ratified 2026-06-05.

- **Variadic `extern ...`** - grammar: a trailing `...` in an extern param list.
  `extern { printf(&uint8 fmt, ...) -> int32; }`. Fixed params are type-checked;
  variadic arguments pass through unchecked (the C ABI is inherently unsafe here -
  an `ffi` effect, [PRIME.md](features/PRIME.md) D6). Unblocks `printf` and the
  `bubblesort` / `file` corpus.
- **Opaque / named FFI pointer types** - `extern { type FILE; fopen(...) -> &FILE; }`.
  An opaque named type, usable **only behind a pointer** (unknown size, so by-value
  use is an error). Emits a C opaque typedef.
- **Drop the auto-`#include`** - the `extern` block becomes the sole prototype
  source (Rust-style FFI). This kills the `void*` / `FILE*` clash that the blanket
  `<stdio.h>` caused.

### print: interim, marked for termination

Full eviction of `print` (to a stdlib that composes `printf`) is now *unblocked*
(first-class strings + variadic FFI both exist). But the replacement home - a
stdlib / prelude - does not exist yet (no module mechanism until post-typeck), so
evicting hard now would be a UX regression with nothing to receive it.

Interim (decided): **keep `print` as an intrinsic for now, but make it self-declare
its own `printf` prototype inline** instead of relying on the blanket include.
That lets the auto-`#include` be dropped, keeps ergonomic `print` working, and
unclashes opaque `FILE` - all at once. `RValue::Print` stays the isolated,
clean-deletion node it already is.

**`print` is marked for termination.** The subtractive intent stands: `print`
leaves the kernel the moment a prelude can host it. This is not a permanent
keep - it is a dated interim.

**Update 2026-06-11 (reclassification).** The interim above is built (see the
Component 5 status banner), and the intrinsic in question is now `println`
(`RValue::Println`; the old `print` was evicted earlier). Two facts changed
with the C-seam:

- **`println` is no longer load-bearing.** `printf` is directly reachable
  (`extern { printf(string fmt, ...) -> int32; }` compiles and runs), so the
  intrinsic is sugar over a primitive the language already exposes. The
  subtractive criterion - deletable without losing expressive power - is
  satisfied today, whether or not the deletion happens.
- **Deleting it now is rejected as a footgun regression.** `{}` placeholders
  resolve type-directed at codegen (`spec_for_type` selects the C conversion
  from the argument's type); hand-written `%` specifiers reintroduce the
  wrong-specifier/wrong-width UB that selection prevents. No Eye-level
  replacement can exist yet: an Eye function has no variadics, no generics,
  and no comptime, so a prelude alone cannot host `{}` formatting. The real
  receiver is the prime layer ([PRIME.md](features/PRIME.md)), not merely
  post-typeck modules.

So the eviction's meaning narrows: `println` stays in the frozen kernel as a
ratified dated interim, and "evict" means "move to the prime-era stdlib", not
"delete".

## Intrinsics are interim

A standing principle (ratified 2026-06-05): **the kernel's call-intrinsics
(`println`, `len`, `sizeof`) are interim, not permanent.** Each is a placeholder for
a more principled mechanism and is marked for eventual termination as the language
grows the means to express it:

- `println` (formerly `print`) -> stdlib composition over `printf`, once the
  prime layer can express type-directed `{}` formatting (a prelude alone
  cannot - see the 2026-06-11 reclassification under Component 5).
- `sizeof` -> an accessor on a first-class type-value, once prime lands
  ([PRIME.md](features/PRIME.md) D8); also reworked at the own-backend transition.
- `len` -> reconsidered once arrays / slices have a stdlib representation.

The kernel keeps only what a supermacro provably cannot synthesize; an intrinsic
that a future stdlib or reflection layer *can* express does not belong in the
frozen kernel. None of these blocks the freeze - they are tracked so the kernel
does not ossify around a convenience.

## The two-layer honesty

"Compose everything on top" is two layers; do not let the short answer conflate
them.

- **Substrate.** const + `sizeof` + static-data are ~3 primitives from freezable.
  Finishing them makes vtables, sum types, and containers **hand-writable** on the
  kernel.
- **Mechanism.** Auto-composition via supermacros is Horizon 2, and **prime
  execution is its floor** (not const). Per the resolved bootstrap hinge
  ([KERNEL.md](KERNEL.md), [PRIME.md](features/PRIME.md)), all of it stays hand-written
  until the engine arrives.

So: three primitives buy a freezable kernel and a fully hand-writable substrate.
They do not buy auto-composition; that is later and gated on prime execution, not
on const.

## Sequencing and the freeze checklist

Identity-ordered (whose language each item serves), not ease-ordered:

1. **const** - `const` + const-expr fold (bounded as above). Finite, not the
   keystone. Also unblocks A6 const-length arrays ([DEFER.md](planning/DEFER.md)).
   **BUILT 2026-06-06** ([CONST.md](features/CONST.md)); the rest of the checklist remains.
2. **`sizeof`** - leaf, independent, leans on C; interim intrinsic.
   **BUILT 2026-06-06** ([SIZEOF.md](features/SIZEOF.md)).
3. **addressable static data** - globals + string literals as `&[uint8; N]`. Leaf,
   independent. **BUILT 2026-06-06** ([STRING.md](features/STRING.md)); also closed
   the `print` `%d` bug.
4. **match** - full over primitive domains (enum / int / char / bool, with
   literals, guards; ranges / or-patterns remain S4), seam-shaped machinery built,
   external registration deferred to the prime engine. **S0-S3 BUILT 2026-06-06/10**
   ([MATCH.md](features/MATCH.md)).
5. **C-seam cluster** - variadic `extern ...`, opaque FFI pointer types, drop the
   auto-`#include`. Low-identity. **BUILT 2026-06-11** ([FFI.md](../features/FFI.md));
   restored the `bubblesort` / `file` corpus.
6. **`println`** - interim self-declaring intrinsic in place (Component 5, built
   with item 5); reclassified 2026-06-11: no longer load-bearing (`printf` is
   reachable via variadic extern), eviction target is the prime-era stdlib, not
   a bare prelude. Marked for termination, does not block the freeze.

Items 2, 3, and 4 are independent of item 1 (const) and of each other; const is
the only item the others might reference (sizeof in a const-expr). The kernel
freezes when 1-5 land and 6's interim is in place (its eviction is post-freeze,
gated on a prelude). As of 2026-06-11 items 1-5 are built and 6's interim is in
place; what stands between here and declaring the freeze is the residual
checklist audit (e.g. match S4 ranges / or-patterns - decide in or out of the
frozen surface).

After the freeze, the next large structural build is the typeck split (Horizon 1,
[MASTERPLAN.md](planning/MASTERPLAN.md)), then the extensibility engine (Horizon 2); the
Cranelift backend (Horizon 3) is parallel and may start any time after the MIR
boundary, which already exists.
