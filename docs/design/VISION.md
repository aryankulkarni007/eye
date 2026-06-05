# Eye - Design Vision Brief

Context handoff. Captures the language vision, not the current implementation state.

## What Eye is

A systems language whose core is an **ergonomic, modern C**: capable of expressing most things with the fewest features. Subtractive design - rank features by capability-unlocked per feature-weight, cut anything that doesn't earn its place.

## The central thesis: composable core

The core is **maximally simple**. The core developer experience is delightful because cognitive overhead is near zero. Most real features do NOT live in the core - they live in the **stdlib**.

The mechanism that makes this possible is **supermacros**.

## Supermacros

Supermacros are **compiler extensions**, not text substitution. They can add new _fundamental_ features to the language from the syntax up. Not sugar - they reach into compiler internals.

Worked example: an **OOP stdlib**. It is NOT a fake class emulation built from structs plus function pointers. The OOP lib **defines a vtable into the compiler internals**. Classes feel native because the extension hooks the compiler, not because it papers over C idioms.

Consequence: features most languages bake into core syntax (OOP, generics, sum types, containers) become stdlib supermacros in Eye.

## Rings of safety

Privilege boundary model:

- **Kernel** (the core language) is maximally protected and **unoverwriteable**.
- **stdlib** is likewise protected.
- Outer rings (user supermacros) get progressively less privilege.

The unoverwriteable kernel is the load-bearing constraint: anything placed in the kernel is permanent, with no deprecation path. So kernel inclusion must be conservative.

## AST tooling

Supermacros require **excellent AST-manipulation tooling**. The output of a supermacro must be _integrated_ - native-feeling, indistinguishable from hand-written core code, not bolted-on. The AST is the API surface supermacros live on, so it must be clean, orthogonal, hygienic, and stable.

## Horizon

This is the **~v10 vision**, not v0.4. The supermacro engine is far-future. But the vision shapes near-term decisions: do not bake into the unoverwriteable kernel any feature that should later be a supermacro.

---

## The kernel/stdlib line (derived, to be ratified)

Discriminating test: **a feature belongs in the kernel iff a supermacro provably cannot synthesize it.** Not "hard as a macro" - _cannot_.

**Kernel (irreducible):**

- functions, calls, struct (product type), raw pointers, `if`, `loop`+`break` - already exist
- **union / overlapping storage** - macros cannot fake overlapping memory layout
- **FFI `extern`** - macros cannot synthesize the C ABI / linker seam; the kernel bottoms out at the machine here (IO, alloc)
- **sized/unsigned ints + `as` casts** - macros cannot invent machine-width types

**Stdlib (derivable, therefore not kernel):**

- sum types / payload enums = union + tag + extensible match
- generics = comptime + AST instantiation
- OOP/vtables = the stated example
- Vec, Option, Result, iterators, owned strings
- `while`, `for` = over `loop`+`if`+`break`

## Two open hinges

1. **Match extensibility.** If `match` is a closed kernel construct, sum types are forced into the kernel forever. If stdlib can register pattern lowerings, sum types stay stdlib. This decision outranks the sum-types decision itself.

2. **Bootstrapping sequence.** Supermacros are themselves kernel (chicken/egg). Until the macro engine exists, should-be-stdlib features have nowhere to live. Two paths: (a) temporary kernel builtins migrated later - contradicts "unoverwriteable"; (b) build the macro engine earlier, slower feature delivery but everything lands as stdlib from day one. Feature choices diverge by path, so pick the path first.

## Discipline that applies now

The AST node set is forever-API. Ship substrate (union, FFI, machine types, casts), not features. Do **not** ship `for`, payload-enum syntax, or class syntax into v0.4-v0.9.

For **near-term compiler scope, limitations, and decision forks** (match extensibility vs closed kernel, modules vs substrate hardening), see [`FUTURE.md`](planning/FUTURE.md) - especially _Roadmap - v0.5_ and _Future forks_.
