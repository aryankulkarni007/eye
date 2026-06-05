# `println` in the Eye kernel

**Status: current (verified end to end 2026-06-10).** `println` is a built-in
intrinsic, not a trait or macro. It is the sole print intrinsic in the kernel -
there is no bare `print`. This document records what it does and every open point
still on it.

## The model

`println("...{}...", a, b, ...)` lowers to a single C `printf`. Each `{}`
placeholder in the format string is replaced by a printf conversion specifier
chosen from the corresponding argument's HIR type (`spec_for_type` in
`crates/codegen/src/core/types.rs`); the arguments are then forwarded in order.
A trailing newline is always appended. Recognition is by name, so a user-defined
`println` shadows the intrinsic (same rule as `len` and `sizeof`).

The format string is parsed by `char`, not by byte, so multibyte UTF-8 in the
literal (`é`, `→`) is preserved; `{`, `}`, and `%` are ASCII and still detected.
A literal `%` in the format string is escaped to `%%`.

## Type-to-specifier mapping

| Eye type | printf spec | Note |
|----------|-------------|------|
| `int8` / `int16` / `int32` | `%d` | default-promote to `int` |
| `int64` | `%lld` | assumes LP64/LLP64 |
| `uint8` / `uint16` / `uint32` | `%u` | |
| `uint64` | `%llu` | |
| `usize` | `%zu` | so `len(x)` prints well-typed |
| `isize` | `%td` | |
| `float32` / `float64` | `%f` | printf promotes float to double |
| `bool` | `%d` | prints `0` / `1`, not `true` / `false` |
| `char` | `%c` | one byte |
| `string` | `%s` | |
| `&T` / `*T` / `&[T; N]` | `%p` | prints the address |
| unknown nominal (e.g. enum) | `%d` | prints the integer tag |

## Compound-argument rejection

`println` is primitive-only: it has no format for a compound value.
`check_print_args` (`crates/hir/src/core/lower/expr.rs`) rejects a whole
**array**, **struct**, or **union** argument with a hard error
(`` `println` cannot format an array`` etc.). This is the H2 fix; before it, a
struct argument emitted `%p` over UB.

The rejection is type-driven and only covers those three. See open points below
for what still slips through.

## Open points

| # | Point | State / why |
|---|-------|-------------|
| P1 | **Enums print as `%d` (the integer tag).** An enum is a `TypeRef::Path` not in the struct/union maps, so `check_print_args` does not reject it; `spec_for_type` falls through to `%d`. | Ratified by precedent (v0.3 printed an enum as its tag). Open only if "primitive-only" should also exclude enums. |
| P2 | **References and `&[T; N]` print as `%p` (the address).** `check_print_args` rejects the array *value* but not a *reference* to one; `spec_for_type` maps every `Ref`/`Ptr` to `%p`. So `println("{}", &arr)` prints a pointer. | Read as a non-compound primitive (the pointer word). This is the user veto still open: if "primitive-only" means refs/`&[T; N]` are rejected too, tighten `check_print_args`. |
| P3 | **Too few arguments for the placeholders is UB.** A `{}` with no matching argument emits a `%d` spec with no value pushed (`print.rs`, `.unwrap_or("%d")`); `printf` then reads a missing vararg. Not caught in Eye. | No arity check. Needs `println` to diagnose a placeholder/argument count mismatch. |
| P4 | **Too many arguments is only a C-compile warning.** An argument with no matching `{}` is forwarded raw so libc surfaces the arity warning at C-compile time; Eye itself does not flag it. | Other half of P3. Same fix (arity check) closes both. |
| P5 | **A non-literal format string skips substitution entirely.** If the first argument is not a string literal (e.g. a variable holding a string), no `{}` is substituted and the remaining args are forwarded raw to `printf`. The format-spec checker and P3/P4 logic are bypassed. | Dynamic format strings reach `printf` unchecked. Classic format-string vector, though Eye has no untrusted input yet. Open. |
| P6 | **Always appends a newline; no no-newline form.** The trailing `\n` is hardcoded. The intrinsic was renamed `print` -> `println` to be honest about this, but there is still no no-newline `print` counterpart. | Design limitation, not a bug. A no-newline variant needs a second entry point or a flag. |
| P7 | **No formatting flags.** Only bare `{}`; no width, precision, or radix (`{:.2}`, `{:x}`). Floats are always `%f` (6 decimals), never `%g`. | `println` is an intrinsic with a fixed spec table, not a format mini-language. Needs a real formatting design (likely with a Display/Debug trait). |
| P8 | **No user-defined formatting.** `println` is recognized by name and emits `printf` directly; there is no Display/Debug equivalent a user type can implement. | Tied to P7; both wait on a trait/macro formatting design. |

## Out of scope (deferred)

A formatting mini-language, a Display/Debug trait, and a no-newline variant all
wait on `println` becoming a real macro or trait rather than a name-recognized
intrinsic. The compound-rejection scope (P1/P2) is a pending user ruling, not a
deferral. The vision ultimately evicts `println` from the kernel entirely,
composing `printf` in the stdlib once variadic FFI lands ([KERNEL.md](../design/KERNEL.md)).
