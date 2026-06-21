> **EXPERIMENTAL** - this document captures an in-progress design discussion. Nothing here is settled or implemented. It serves as a shared reference for conversation, not a specification.

# Memory Model

## Philosophy

Three commitments guide Eye's memory model:

1. **Freedom** - no forced paradigm, no borrow checker, no ownership school. The programmer can express any pattern: graphs, self-referential structures, arenas, custom allocators.

2. **Guardrails, not rules** - the compiler catches common footguns with simple, cheap, intraprocedural analysis. It never asks permission; it warns after the fact.

3. **No magic** - the compiler never allocates, never resizes, never inserts runtime instrumentation without being told to. Every byte on the heap is placed there by explicit user code. There is no "implicit arena" and no hidden cost.

   ~ reframed 2026-06-19: "never deallocates without being told" is superseded by
   auto-drop (see Value semantics). the compiler frees an owning value at scope
   exit because the owning *type told it to* via its drop; what stays sacred is no
   implicit allocation, no hidden arena, and LSP-visible drop points. deallocation
   defaults to managed; manual control is the opt-in.

   ~ reframed again 2026-06-21: "no magic" is not the bar - **predictable magic**
   is. I do not require the absence of magic (Eye already has auto-drop, implicit
   error propagation, auto-deref, inference); I require that any magic follow a
   uniform, learnable rule so the user always knows what to expect.
   surprising / dataflow-dependent behavior is the footgun, not magic per se. so
   implicit error propagation is fine (one uniform rule, LSP-painted) even though
   it is invisible in plain text. ([ERRORS.md](../features/ERRORS.md) D3.)

## The Three Tiers

### Tier 1: Stack values (`let`, `mut`)

Pure value semantics. No references, no pointers, no aliasing possible.

```eye
let int32 x = 5;   -- immutable binding
let int32 y = x;   -- copy - x and y are independent
mut int32 z = 10;  -- mutable binding
z = 20;            -- fine
```

- `let` is truly immutable -- the compiler rejects any write path through a `let` binding, including through `&` or `*` (see `let` enforcement below).
- `mut` allows mutation through the binding itself.
- All values are copied on assign/init/pass/return.
- Zero analysis needed. Always safe.

### Tier 2: `&T` references

Shared read-only references. Auto-deref for field access and indexing.

```eye
let &int32 r = &x;      -- borrow from x
let int32 v = *r;       -- explicit deref
let int32 f = r.field;  -- auto-deref
```

The compiler enforces two rules, both intraprocedural:

**Rule 1 - No escape.** A `&T` must not outlive the binding it was taken from.

```eye
bad() -> &int32 {
    let int32 x = 5;
    return &x;  -- COMPILE ERROR: &x escapes its scope
}
```

This covers `return &local`, assigning `&local` to a longer-lived variable, and storing `&local` in a global. It is a single forward pass over block scopes in the HIR.

**Rule 2 - No write-through.** `&T` is read-only. Writing through a `&T` is a compile error (see also: `*T` + `mut` below).

```eye
let &int32 r = &x;
*r = 10;   -- COMPILE ERROR: cannot write through &T

-- To write through a reference:
mut int32 x = 5;
let int32* p = &x;
*p = 10;   -- OK - mut + *T path
```

These two rules eliminate the most common dangling-reference footgun with no borrow checker, no lifetime annotations, and no complex analysis.

### Tier 3: `*T` pointers

Raw, unrestricted pointers. Total freedom. The compiler tracks provenance (see G5) for compile-time use-after-free detection, but imposes no constraints on pointer casting, aliasing, or indirection.

```eye
let int32* p = &x;       -- take address of a mut binding
let Node* q = alloc(Node); -- heap allocation via explicit call
*q = 5;                  -- write through pointer
let int32 v = *q;        -- read through pointer
```

- `*T` is a single address. No length, no fat-pointer metadata.
- `mut` is required to get a `*T` to a stack binding. `let x; let p = &x;` is a compile error -- the compiler does not allow `*T` derivation from immutable bindings.
- **No pointer arithmetic on `*T`.** Arithmetic requires the `offset` builtin (see below), which preserves provenance and is bounds-checked in debug builds.
- The programmer controls all allocation and deallocation.

**The compiler provides no safety guarantees for `*T` operations beyond provenance tracking.** This is the freedom hatch: the programmer explicitly takes responsibility here.

## Value semantics: copy, move, and drop (design agreed 2026-06-18 pair session, not built)

**Ratified direction (2026-06-19 pair session): auto-drop is the default, manual
deallocation is the opt-in.** this is the nudge ([PHILOSOPHY.md](PHILOSOPHY.md)):
the safe path is silent and free, the unsafe path is reachable but uphill. Eye's
stance is **safe-by-default-with-escape-hatches**, not Rust's safe-by-proof - holes
accepted (see Honest scope). ownership is a **type property** - an owning type
auto-drops, a raw `*T` never does - **not** an inferred tag that flows through
pointers; inferred "managed-ness" was considered and set aside as a confusing-rules
footgun (the rule for whether your memory frees would depend on dataflow you can't
see). the manual escape must be **ergonomic** (e.g. allocate from an arena = bulk
manual, one drop), not painful, or programmers route around the safe path entirely.

still open (parked until there is real Eye code to design against): the opt-out
granularity and spelling (arena vs a per-binding `disown`), what seeds an owning
type's `drop`, and whether a forgotten manual free is a warning or silent. a
leading candidate for the arena spelling is an `arena(a) { }` **modifier block**
([MODBLOCK.md](MODBLOCK.md)) - allocations inside the region come from arena `a`
and are bulk-freed with it, which is the "opt into manual = allocate from an arena"
ruling expressed as a lexical region (the nudge). the
subsections below are the exploratory design that fed this ruling; where they
assume a `defer`-first or no-auto-drop model they are superseded by the line above.

architectural note (corrects an earlier overstatement): sealed-body
([TYPECK.md](../features/TYPECK.md)) bounds *type-inference facts* from crossing fn
boundaries - it does **not** forbid interprocedural passes. the effect SCC fixpoint
is already whole-program (TYPECK.md:48). a richer ownership/escape analysis could
likewise run after types+effects - but it is not signature-bounded or union-cheap
the way effects are, so it sits in the deferred escape/lifetime class (G2), and a
heavy global pass spends the salsa incrementality sealed-body was bought for. the
guardrails stay intraprocedural by *choice* (cheapness), not by necessity.

The three tiers above govern _references_. This governs _values_: what happens
when a value is assigned, passed, returned, or goes out of scope. Today every
value copies (Tier 1) and nothing runs at scope exit. That is correct until a
value **owns a resource** (heap memory, a `FILE*`) - then copying it makes two
owners and dropping both double-frees. The use-after-`reset(&arena)` bug is this
class. The model that closes it:

### The discriminator: does the type have a destructor?

- **POD** (no destructor, every field POD) -> **copies**, silently, as today.
  Scalars, and structs/arrays/unions built only of POD. No ownership, no cleanup,
  zero ceremony. This is the common case and stays invisible (silent-safety).
- **owning** (declares a destructor, or contains an owning field) -> **move-only,
  with automatic drop**. Declaring a destructor is the explicit opt-in that flips
  a type from copy to move. This is the Rust rule distilled to one structural
  fact - _has a destructor => not copyable => moves_ - with no traits and no
  `derive`; copyability is inferred from the type's structure, not declared.

### Move semantics

Passing, assigning, or returning a move-only value **transfers ownership**: the
source binding is marked moved-from, and any later use of it is rejected
(`UseAfterMove`). This is exactly the use-after-`reset` fix - `reset(arena)`
taking `arena` by value consumes it, so the later `println(arena.start)` is a
compile error, not luck.

- this is **body-local, flow-sensitive dataflow** - and that is why it _fits_
  sealed-body inference (TYPECK.md): moves never cross a function boundary as
  _inference_. A by-value parameter's signature already declares "I consume
  this"; the move-checking is intra-body, the one place sealed-body permits
  flow analysis. (Contrast generics, which require _inter_-procedural type flow
  and so fight sealed-body - see TYPECK.md.)
- references borrow, they do not move: `&T` (shared) and `&mut T` (mutable, see
  [MUT.md](../features/MUT.md)) leave ownership with the original binding. The
  reference trinity is the full menu: `&T` borrow / `&mut T` mutable borrow /
  by-value move.

### Drop (destructors)

A live (not-moved-from) owning value's destructor runs at scope exit; a
moved-from value does not drop (ownership left). Early `return`/`break` run the
drops on the path out. This is drop-elaboration, codegen machinery in MIR (it
interacts with the CFG-MIR item A7 - structured MIR makes drop-on-every-exit
fiddlier than a CFG would).

Open: Eye has **no methods** (functions are free), so "the destructor _for_ type
`T`" needs a binding mechanism - a recognized free-function form, a `drop T =
fn;` association, or a type attribute. Undecided; the mechanism is kernel (the
compiler must insert the calls + enforce moves), the destructor body is user code.

### `defer` - the simpler stepping stone

`defer <stmt>;` runs a statement at scope exit (Zig/Go). Simpler than full
destructors: no type-level binding, no move tracking, explicit at each use.

```eye
let arena = handle_file("..");
defer free(arena);    -- runs at scope exit, on every path
```

Trade: `defer` guarantees cleanup _runs_ but does **not** prevent use-after-free
(you can still touch `arena` after a manual free) and does not compose with move.
Full destructors+move give cleanup **and** use-after-move rejection silently;
`defer` is the explicit-ceremony escape hatch and a buildable-sooner first step.

### Borrowing and the aliasing-xor-mutation invariant (agreed 2026-06-18)

References are the _preferred_ way to share data - not raw-pointer aliasing. Two
borrow modes:

- `&T` - shared, immutable. Many may be live at once.
- `&mut T` - exclusive, mutable. At most one, and no `&T` live alongside it.

The invariant: **many `&T` XOR one `&mut T`**. Derived from first principles -
aliasing alone is harmless; _shared mutation_ is the bug (a writer changing data
others are reading). Forbidding that one combination is the whole safety win, and
it supersedes the older Tier 2 framing above ("`&T` read-only, write via `mut` +
`*T`"): the write path is now `&mut T`, not the raw-pointer escape.

Enforcement is an **intraprocedural guardrail, not a borrow checker**:

- flow-sensitive, within a single function body, cheap, no lifetimes, no
  interprocedural proof.
- catches the common shared-mutation footguns where they are written: two `&mut`
  live together, `&mut` while `&` live, write through `&T`, and conflicting
  borrows passed as arguments of one call.
- the holes are **accepted, not closed**: borrows that escape via a callee, a
  return value, or persistent storage - which is _exactly_ the set lifetimes
  exist to track. unix "good enough": ship the guardrail that catches the
  street-level threats; the determined cross-function alias is the Joker case -
  the user's responsibility, with debug provenance (G5) as a runtime backstop.
- this is the deliberate "not Rust" line: comparable practical safety, no
  lifetime annotations, no cognitive tax.

The model is **kernel-robust on its own** - references + move + destructors + the
guardrail need nothing from the stdlib. `Rc` (shared ownership) and a `Box`-like
owning pointer are optional stdlib conveniences built on the kernel mechanism,
never load-bearing for the core safety story.

### Honest scope

Move + drop gives RAII, no double-free, and use-after-_move_ rejection; the
borrow guardrail gives intraprocedural no-shared-mutation. It does **not** give
full memory safety: a dangling `&T` to an already-dropped value, and the
interprocedural borrow holes above, still need escape/lifetime analysis (G2, the
separate deferred axis). This is a large, coherent feature - the affine-ownership
core minus lifetimes, by deliberate choice.

## Allocation

### Stack

Stack allocation is implicit via `let` / `mut` bindings. The compiler generates C locals.

### Heap

All heap allocation goes through explicit functions, provided by libraries or the user. The compiler provides no built-in allocator, no `new` keyword, no implicit heap.

```eye
-- stdlib arena (bump allocator)
let arena = BumpArena.new(4096);
let Node* p = arena.alloc(Node);
-- arena is freed when it goes out of scope (if it implements a dtor)
-- or explicitly via arena.free()
```

The compiler does not understand arena internals. It does not resize arenas. It does not create arenas implicitly.

## Pointer arithmetic

Raw `p + n` on `*T` is a compile error. Pointer arithmetic requires the `offset` builtin:

```eye
let [int32; 10] buf = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
let int32* p = &buf[0];
let int32* q = offset(p, 3);   -- q points to buf[3]
let int32* r = offset(p, 20);  -- caught in debug (bounds check), UB in release
```

`offset` is:

- **Provenance-preserving** - G5 tracks `r` back to `buf`, same as it tracks `p`.
- **Bounds-checked in debug** - the compiler knows the object's extent (for locals and `[T; N]`), so `offset` past the end is a deterministic runtime error.
- **Grep-able** - `offset` is visually distinct from `*` deref, making pointer arithmetic searchable.
- **Not allowed on opaque pointers** - `offset(void* p, n)` is only valid inside `extern` functions.

Without `offset`, `*T` is a single-object pointer. To iterate, use array indexing (`a[i]`), which is bounds-checked at debug and infallible at compile time for constant indices (G3).

## Static Analyses (the guardrails)

All analyses are:

- **Intraprocedural** - no cross-function inference, no generics analysis
- **Predictable** - the same input always produces the same result
- **Cheap** - O(n) or O(n log n) in the function body
- **Opt-out** - a compiler flag `-Xno-safety-warnings` silences them

### G1 - `let` immutability enforcement

**Cost:** ~50 lines in HIR lowering / typeck.

**Checks:**

- `&x` where `x: let` -> error (cannot get a mutable pointer to an immutable binding)
- `*r = v` where `r: &T` -> error (cannot write through read-only reference)
- `a[i] = v` where `a: let` -> error (cannot mutate through an immutable binding, even element-wise)

This makes `let` actually immutable, unlike C's `const` (const char\* can point to mutable memory).

### G2 - Stack escape detection

**Cost:** ~200 lines - one forward pass over HIR block scopes.

**Checks:**

- `return &local` -> error
- `g = &local` where `g` outlives the local's scope -> error
- `*p = &local` where the write target outlives the local -> error

Covers the pattern `f() -> &int32 { let int32 x = 5; &x }` and all intraprocedural variants.

### G3 - Constant-index bounds

**Cost:** ~50 lines in typeck.

**Checks:**

- `a[100]` where `a: [int32; 5]` -> error when index is a known literal or constant
- Variable indices are unchecked at compile time (caught at runtime in debug builds)

### G4 - Zero-init (debug only)

**Cost:** ~20 lines in codegen.

**Behavior:** In debug builds (`-debug`), all stack variables are zero-initialized. This turns uninitialized reads (which the compiler should already prevent but might miss) into deterministic behavior rather than UB.

### G5 - Pointer provenance tracking

**Cost:** ~300 lines - optional dataflow pass.

**Behavior:** The compiler tracks where every `*T` value originates (an address-of, an `alloc` call, an `offset`, a cast from another pointer with provenance). If a pointer is read or written after its source arena/local has been freed or gone out of scope, a compile error is emitted.

**Hard error, not warning.** Unlike C's the-compiler-can't-know approach, Eye's `*T` does not support un-trackable transformations:

- No pointer arithmetic on `*T` (requires `offset`, which preserves provenance)
- No `void*` outside FFI declarations
- No XOR lists or tagged pointers

This means the compiler can produce false positives only if the user performs an operation that deliberately obscures provenance. The available escape hatches:

- `-Xprovenance-safety` disables G5 entirely for the compilation unit
- Casting to `usize` and back severs provenance (the resulting pointer has no provenance and may not be dereferenced on the safe path)

**Why `-Xprovenance-safety` exists:** the compiler does not track provenance across the `usize` cast boundary. If the user needs to round-trip a pointer through an integer (e.g., for a tagged-union FFI layer), they opt out of G5 for that file and take responsibility.

### G6 - Debug runtime checks

**Cost:** ~50 lines in codegen (bounds checks on variable indices and `offset`).

**Behavior:** In debug builds, every variable index and `offset` call is preceded by a bounds assertion. Out-of-bounds access becomes a deterministic runtime error (panic), not UB.

## What the compiler does NOT do

| Not done                                | Why                                                                                                                                                                                                                                                                                               |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Borrow checking                         | Too constraining, not inline with freedom principle                                                                                                                                                                                                                                               |
| Lifetime annotations                    | See above                                                                                                                                                                                                                                                                                         |
| Implicit arena creation                 | Magic - violates explicitness                                                                                                                                                                                                                                                                     |
| Arena resizing                          | Magic - violates explicitness                                                                                                                                                                                                                                                                     |
| Automatic reference counting            | Post-kernel feature, not in compiler core                                                                                                                                                                                                                                                         |
| Garbage collection                      | Post-kernel feature, not in compiler core                                                                                                                                                                                                                                                         |
| ~~`&mut T` type~~ SUPERSEDED 2026-06-18 | this row is reversed: [MUT.md](../features/MUT.md) now designs `&mut T` as the safe mutable borrow. `&T`+`*T` left a gap - mutating through a reference forced the raw-`*T` (unsafe) escape, so there was no _safe_ mutable borrow. the reference trinity is now `&T` / `&mut T` / by-value-move. |
| Interprocedural alias analysis          | Too expensive for too little gain in practice                                                                                                                                                                                                                                                     |
| Substructural type system               | Violates freedom - restricts what you can express                                                                                                                                                                                                                                                 |

## Safety return

| Analysis               | LOC      | What it eliminates                              |
| ---------------------- | -------- | ----------------------------------------------- |
| G1 - `let` enforcement | ~50      | Write-through-alias on immutable bindings       |
| G2 - Stack escape      | ~200     | Dangling `&T` from `return &local`              |
| G3 - Const-index OOB   | ~50      | `a[100]` on `[int32; 5]`                        |
| G4 - Zero-init debug   | ~20      | Uninitialized reads (deterministic panic vs UB) |
| G5 - Provenance        | ~300     | Use-after-free via `*T` (compile error)         |
| G6 - Debug runtime     | ~50      | OOB on variable indices and `offset`            |
| **Total**              | **~670** | Major footgun categories removed                |

The remaining safety gaps (data races, interprocedural use-after-free with `usize` round-trip, heap fragmentation) are either caught in debug mode or are the programmer's explicit responsibility.

## Relationship to other languages

| Language | Our take                                                                                                                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Rust** | Full safety at great complexity. We aim for comparable practical safety with radically less compiler surface by staying intraprocedural and using provenance tracking instead of borrow checking. |
| **Hylo** | Pure value semantics with no reference types. We add `*T` + provenance for the expressiveness escape hatch.                                                                                       |
| **Vale** | Generational references with runtime overhead and per-allocation metadata. We have no per-object overhead; our checks are compile-time.                                                           |
| **Zig**  | Similar escape analysis, similar `let`/`var` immutability. We differ by banning raw pointer arithmetic and tracking provenance.                                                                   |
| **Jai**  | Spiritual alignment - tools over rules, explicit allocation, context-based allocators. We add the guardrail analyses.                                                                             |
| **Odin** | Similar allocator philosophy. We add escape analysis, `let` enforcement, and provenance.                                                                                                          |
| **C**    | Same `*T` freedom. We add the `&T` + value semantics safe default and provenance tracking.                                                                                                        |

# Memory (External Auditor's view) for Eye

## The three pointer types

| Type        | Ownership           | Mobility                    | Use                    |
| ----------- | ------------------- | --------------------------- | ---------------------- |
| `T` (value) | Owns                | Copy if POD, move if owning | Default — stack values |
| `&T`        | Borrows             | Copy                        | Shared read            |
| `&mut T`    | Borrows exclusively | Move-only                   | Exclusive write        |
| `*T`        | Unowned             | Copy (any)                  | Freedom hatch          |

`&T` is the safe default for sharing. `*T` is the opt-out for when you need
unrestricted pointer access (allocators, intrusive data structures, FFI). The
compiler never forces you into one or the other — you choose at each declaration.

---

## Ownership regime

### Destructor by convention — `drop(T: *T)`

No traits, no methods, no `derive`. A type is **owning** (move-only) if a
function named `drop` exists in the current scope whose first parameter is
`*T`:

```eye
drop(*Arena arena) {
    free(arena.start);
    arena.start = NULL;
}
```

A type without a `drop` in scope is **POD** (copyable). This is:

- **Findable** — `drop(` is grep-able
- **Consistent** — matches Eye's "functions are free" philosophy
- **Scoped** — bring a different `drop` into scope to shadow the default (opt-out
  without a compiler flag)

### Move semantics

- Passing, assigning, or returning an owning value **transfers ownership**
- The source binding is marked moved-from
- Use-after-move → compile error
- Body-local, flow-sensitive dataflow (not interprocedural)

### `defer` is move-aware

```eye
let a = handle_file("..");
defer free(a.start);    -- armed
let a2 = a;             -- a moved to a2 — defer disarmed
                        -- a2 dropped at scope exit
```

`defer` carries an armed flag: if the binding is moved or goes out of scope
prematurely (early return), the flag is cleared. Codegen emits:
`if (defer_armed) { free(a.start); }`. A single boolean per `defer`, no extra
cost in the common path.

### Array/slice access: no partial moves

```eye
let arr = [file_handle(); 3];   -- array of owning types
let f = arr[0];                  -- COMPILE ERROR: cannot move out of array

let arr2 = arr;                  -- OK: moves the entire array
let f = ptr::read(&arr[0]);      -- OK: explicitly reads element (leaves hole)
```

Arrays are moved as a whole. Individual element extraction requires an explicit
`ptr::read` / `ptr::swap` call. This avoids per-element state flags.

---

## Borrow rules (intraprocedural only)

### `&T` — shared, immutable, Copy

Multiple `&T` references to the same binding may be live simultaneously.
No mutation through `&T`.

### `&mut T` — exclusive, mutable, linear

`&mut T` follows normal move semantics — it is consumed on use. At most one
`&mut T` to a given binding may be live at any point.

Take a fresh `&mut` at each call site:

```eye
mut x = 5;
inc(&mut x);    -- fresh &mut, consumed by the call
inc(&mut x);    -- fresh &mut again — OK, previous one is gone
```

If you need multiple operations through one binding:

```eye
let r = &mut x;
*r += 1;        -- use r
foo(r);         -- r consumed here
-- r gone, x available for reborrow
```

### The one coercion: `&mut T` → `&T` in function arguments

```eye
fn reader(&int32 x) -> int32 { *x }

let mut x = 5;
let v = reader(&mut x);   -- coerces: &mut T → &T implicitly
```

This is the 95% case — passing a mutable reference to a function that only needs
read access. No other implicit reborrowing exists. `&mut T` is just a
pointer-that-can-only-have-one-copy, and the coercion is a widening conversion.

### Enforcement

The borrow guardrail is flow-sensitive, intraprocedural, and cheap:

- No `&mut T` while any `&T` to the same binding is live
- No two `&mut T` to the same binding live simultaneously
- No write through `&T`
- Holes are **accepted**: cross-function borrow escapes, returned `&mut T` that
  aliases an argument. This is the deliberate "not Rust" line.

---

## Pointer rules

### `*T` — raw, unrestricted

- No aliasing rules, no borrow semantics
- No pointer arithmetic without the `offset` builtin
- `offset` preserves provenance, bounds-checked in debug
- Dereferencing a `*T` after its provenance source is freed/moved → compile error
  (G5)

### `void*` → typed `as` preserves provenance

```eye
extern { malloc(usize) -> void*; }
let p = malloc(64) as int32*;     -- provenance: malloc call
let q = offset(p, 4);             -- provenance: same malloc call
*q = 10;                          -- OK (if in bounds)
free(p);
*q = 20;                          -- ERROR: provenance source (malloc) freed
```

The only way to break provenance:

```eye
let opaque = p as usize;
let p2 = opaque as *int32;   -- provenance severed — compiler NOTE
```

---

## Guardrails (G1–G6)

All are cheap, intraprocedural, and opt-out via `-Xno-safety-warnings`.

### G1 — `let` immutability

`&mut x` where `x: let` → error. No mutation through any path from an immutable
binding. `&x` on `let` is always fine (shared read).

### G2 — Stack escape detection

`return &local`, `g = &local` where `g` outlives the local → error. Single
forward pass over block scopes. Catches the dangling-reference class.

### G3 — Constant-index bounds

`a[100]` where `a: [int32; 5]` → error when index is a known literal or constant.

### G4 — Zero-init (debug)

All stack variables zero-initialized in debug builds. Uninitialized reads become
deterministic, not UB.

### G5 — Provenance tracking (all pointer-like types, not just `*T`)

`&T`, `&mut T`, and `*T` all carry a provenance tag — the allocation or binding
they were derived from. Dereferencing after the source is freed, moved, or out
of scope → compile error. This is ~50 lines of dataflow on top of the existing
`*T` provenance pass, and it closes the gap the original design doc left open.

### G6 — Debug runtime checks

Variable array indices and `offset` calls preceded by bounds assertions in debug
builds. OOB → deterministic panic, not UB.

---

## Self-referential structs: `*T` only

```eye
struct SelfRef {
    int32 data,
    *int32 ptr,    -- OK: raw pointer
    -- &int32 ref, -- ERROR: G2 catches "& to self" as escape (false positive)
}
```

The escape detection treats "address of own field" as an intra-scope escape.
This is a false positive, but the fix is `*T` — which is correct anyway, because
self-referential fields need raw semantics, not borrow semantics. `*T` +
provenance (G5) still catches use-after-free on the self-ref.

---

## What this buys you

| Category                                  | Caught?        | Mechanism                             |
| ----------------------------------------- | -------------- | ------------------------------------- |
| Use-after-move on value                   | Yes            | Flow-sensitive move tracking          |
| Use-after-move on `&T`/`&mut T`           | Yes            | G5 extended to all pointer-like types |
| Use-after-free on `*T`                    | Yes            | G5 provenance tracking                |
| Shared mutation (`&T` + `&mut T` aliased) | Yes, intraproc | Borrow guardrail                      |
| Self-referential struct invalidation      | Yes            | G5 on the `*T` field                  |
| Array bounds error                        | Yes, debug     | G6 runtime check                      |
| `return &local`                           | Yes            | G2 escape detection                   |
| Interprocedural borrow escape             | No             | Accepted — user responsibility        |
| Data race across threads                  | No             | Outside scope                         |
| Heap fragmentation                        | No             | Outside scope                         |

All caught patterns are either intraprocedural or cheap dataflow: no lifetime
annotations, no borrow checker, no trait system. ~670 LOC of guardrails total.

---

## Comparison with the original design

| Dimension                | Original                                                    | This version                                            |
| ------------------------ | ----------------------------------------------------------- | ------------------------------------------------------- |
| Reference types          | `&T` + `*T` + `&mut T` designed                             | Same three, linear `&mut T`                             |
| Destructor binding       | Undecided                                                   | `drop(T: *T)` convention                                |
| Reborrowing              | Undefined                                                   | One coercion (`&mut T` → `&T`), no implicit reborrowing |
| G5 provenance scope      | `*T` only                                                   | `&T` + `&mut T` + `*T`                                  |
| `void*` cast provenance  | Undefined                                                   | Preserves provenance                                    |
| `defer` + move           | Undefined                                                   | Move-aware (armed flag)                                 |
| Partial array moves      | Undefined                                                   | Not allowed (explicit `ptr::read` required)             |
| Self-referential structs | Undefined                                                   | `*T` only (G2 false positive forces correct type)       |
| Accepted holes           | 3 (use-after-move-on-ref, interprocedural escapes, threads) | 2 (interprocedural escapes, threads)                    |
