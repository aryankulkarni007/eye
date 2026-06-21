# main: the program entry contract

`main` is an ordinary Eye function with one special role: the program entry
point. the C backend emits it under an internal name (`__eye_main`) and
generates the `int main(...)` entry shim the C runtime requires. this is the
sole place the C entry convention lives; a non-C backend omits it entirely.

## current behavior (built)

+ the user `main` lowers to `__eye_main`; codegen emits an `int main(void)` shim
  (`crates/codegen/src/core/mir_emit/mod.rs`, `gen_all`).
+ an integer return type forwards as the process exit code:
  `int main(void) { return (int)__eye_main(); }`. so `main() -> int32 { return
  1; }` already exits 1 - exit codes are not stuck at 0. the cast to `int` makes
  a wider integer return well-defined.
+ every non-integer return (void / bool / float / struct / array) runs main for
  its effect and exits 0: `__eye_main(); return 0;`.
! parameters are rejected (`MainHasParams`/T, `crates/hir/src/core/lower/collect.rs`):
  the shim calls `__eye_main()` with no arguments, so argc/argv are unreachable.
! main is callable recursively from within the program - nothing forbids it.

## designed (not built)

the entry contract finished per silent-safety ([PHILOSOPHY.md](../design/PHILOSOPHY.md)):
the common case free, the dangerous or explicit case opt-in.

- main is the one function whose **omitted return type defaults to `int32`**
  (every other function defaults to void / unit). falling off the end of main is
  an implicit `return 0` (C99 semantics), so a bare `main() { ... }` still exits
  0 with no `return` ceremony, and `return 1` sets the exit code naturally - no
  `exit(1)` call needed.
  - this is a main-only completeness exception: a non-main `-> int32` body that
    falls off the end is still a missing-return error (the unit/never sweep).
- **argc/argv**: the signature `main(int32 argc, string* argv)` (with or without
  `-> int32`) is accepted, and the shim forwards them:
  `int main(int argc, const char** argv) { return __eye_main(argc, argv); }`
  (`string*` = `const char**`). `MainHasParams` relaxes to accept exactly this
  shape; any other parameter list stays rejected with a clear diagnostic.
  - a slice-typed `argv` waits on slices ([DEFER.md](../planning/DEFER.md));
    `string* argv` is the honest C-ABI form today.
- **recursive main is rejected**: a call expression resolving to the main
  function is a diagnostic. a program entry point is not a callable routine;
  allowing the call makes no sense and is a footgun the frontend can rule out.

## where it lives

- shim + return forwarding: `crates/codegen/src/core/mir_emit/mod.rs` (`gen_all`,
  `main_ret_is_integer`, `c_fn_name`).
- the parameter rule: `crates/hir/src/core/lower/collect.rs` (`MainHasParams`).

open items tracked in [ledger.md](../planning/ledger.md) "main entry contract".
