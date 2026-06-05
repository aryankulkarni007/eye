> **EXPERIMENTAL** - this document captures an in-progress design discussion. Nothing here is settled or implemented. It serves as a shared reference for conversation, not a specification.

# Memory Model

## Philosophy

Three commitments guide Eye's memory model:

1. **Freedom** - no forced paradigm, no borrow checker, no ownership school. The programmer can express any pattern: graphs, self-referential structures, arenas, custom allocators.

2. **Guardrails, not rules** - the compiler catches common footguns with simple, cheap, intraprocedural analysis. It never asks permission; it warns after the fact.

3. **No magic** - the compiler never allocates, never deallocates, never resizes, never inserts runtime instrumentation without being told to. Every byte on the heap is placed there by explicit user code. There is no "implicit arena" and no hidden cost.

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

This makes `let` actually immutable, unlike C's `const` (const char* can point to mutable memory).

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

| Not done | Why |
|----------|-----|
| Borrow checking | Too constraining, not inline with freedom principle |
| Lifetime annotations | See above |
| Implicit arena creation | Magic - violates explicitness |
| Arena resizing | Magic - violates explicitness |
| Automatic reference counting | Post-kernel feature, not in compiler core |
| Garbage collection | Post-kernel feature, not in compiler core |
| `&mut T` type | `&T` + `*T` covers both read and write paths |
| Interprocedural alias analysis | Too expensive for too little gain in practice |
| Substructural type system | Violates freedom - restricts what you can express |

## Safety return

| Analysis | LOC | What it eliminates |
|----------|-----|--------------------|
| G1 - `let` enforcement | ~50 | Write-through-alias on immutable bindings |
| G2 - Stack escape | ~200 | Dangling `&T` from `return &local` |
| G3 - Const-index OOB | ~50 | `a[100]` on `[int32; 5]` |
| G4 - Zero-init debug | ~20 | Uninitialized reads (deterministic panic vs UB) |
| G5 - Provenance | ~300 | Use-after-free via `*T` (compile error) |
| G6 - Debug runtime | ~50 | OOB on variable indices and `offset` |
| **Total** | **~670** | Major footgun categories removed |

The remaining safety gaps (data races, interprocedural use-after-free with `usize` round-trip, heap fragmentation) are either caught in debug mode or are the programmer's explicit responsibility.

## Relationship to other languages

| Language | Our take |
|----------|----------|
| **Rust** | Full safety at great complexity. We aim for comparable practical safety with radically less compiler surface by staying intraprocedural and using provenance tracking instead of borrow checking. |
| **Hylo** | Pure value semantics with no reference types. We add `*T` + provenance for the expressiveness escape hatch. |
| **Vale** | Generational references with runtime overhead and per-allocation metadata. We have no per-object overhead; our checks are compile-time. |
| **Zig** | Similar escape analysis, similar `let`/`var` immutability. We differ by banning raw pointer arithmetic and tracking provenance. |
| **Jai** | Spiritual alignment - tools over rules, explicit allocation, context-based allocators. We add the guardrail analyses. |
| **Odin** | Similar allocator philosophy. We add escape analysis, `let` enforcement, and provenance. |
| **C** | Same `*T` freedom. We add the `&T` + value semantics safe default and provenance tracking. |
