# Ledger

The single tracking document for the Eye compiler. Every open bug, design
question, and backlog item lives here or in a source comment at the issue's
location (`FIXME:` in `.rs`/`.eye` files; each is cross-referenced in the
register below). No other document carries open items:

- `docs/design/CLEAK.md` is the frozen 2026-06-11 audit record (its open
  rows are mirrored here; new status changes land here, not there).
- `docs/planning/DEFER.md` records ratified deferrals - decisions, not
  open issues.
- `docs/todo.md` was deleted 2026-06-12; everything open in it was
  absorbed here (see the 2026-06-12 completed entry for what was closed
  as stale during the absorption).

**Sequencing (decided 2026-06-12):** tie up loose ends, then freeze the
kernel, then the typeck split (Horizon 1) - freeze before typeck, within
reason. Rows tagged **[typeck]** are deliberately post-freeze: the correct
fix is a type judgment and patching it into lowering is the wrong layer
(CLEAK class T). At freeze time every non-typeck row below must be fixed
or explicitly accepted; the typeck-class gaps are accepted as documented
limitations of the frozen kernel and become the typeck pass's scope.

---

## Phased implementation plan (sequencing decided 2026-06-19, expanded with P0 + the move/drop reorder 2026-06-21)

Ratified: **harden the current pipeline first, then add features in dependency
order.** Consistent with the safety quadrant (silent-unsafe first, PHILOSOPHY.md)
and the harden-pipeline-first directive. Each P0 item already has a row elsewhere in
this ledger (named here, not duplicated). Design homes: KERNEL.md (arithmetic,
ADTs), MODBLOCK.md (wrapping / modifier blocks), MUT.md (mutability, `&mut`),
ERRORS.md (errors), EFFECT.md (S7-payload), MEM.md (move/drop).

### P0 - current pipeline: correctness + hardening (no new language surface)

fix / harden what already ships. build-ready checklist (audit 2026-06-21). source
scan is clean - no `FIXME`/`TODO`/`XXX`/`HACK` in the crates, the 39 panic-family
calls are invariant guards / checked-`unreachable!`; the only in-source FIXMEs are 3
in `eyesrc/programs/lang.eye` (readable-C mode, field/arg types CLOSED, tail-expr
enforcement = T5 below). ordered cheap -> keystone:

**T1 - cheap lints / hygiene (independent, fast):**
- [ ] dead / pure expression statement lint (`-Wunused-value`) [typeck/lint]
- [ ] unsigned `<= 0` tautology lint [lint]
- [ ] unused-variable / unused-parameter lint [lint]
- [ ] unused generated array-wrapper global - emit only when referenced [codegen]
- [ ] dangling-`&local` return warning (`-Wreturn-stack-address` surface) [the free
      warning, ahead of the deferred escape analysis]

**T2 - frontend-vs-clang flags + class A:**
- [ ] add `-Wall` to the prod clang invocation (not `-pedantic`); opt-in
      `eye build --strict` [driver]
- [ ] class A: frontend owns clang's hard errors with a clean diagnostic [typeck]
- [ ] variadic argument types unchecked (args past a variadic) [typeck]

**T3 - C-attribute stamping (free clang enforcement) [codegen]:**
- [ ] `nonnull` on `&T` / `string` params
- [ ] `format(printf, m, n)` on the printf prototype
- [ ] FFI const-correctness: unmarked `extern` param emits `const` C (fixes the
      `memcpy` builtin-redecl warning). standalone here; the general
      const-default-param rule is P1. (`warn_unused_result` / `returns_nonnull`
      stays blocked on a new attribute surface - deferred, not P0.)

**T4 - pointer / usize-ptr correctness [typeck]:**
- [ ] non-offset pointer arithmetic unchecked -> reject
- [ ] pointer-type contamination propagates -> contain
- [ ] implicit `usize -> ptr` at a return -> reject / require explicit cast

**T5 - index / judgment completeness [typeck]:**
- [ ] non-integer array index (T47) -> reject
- [ ] one-operand-checked -> two-operand judgment sweep (audit every binary judgment)
- [ ] tail-expression type enforcement (the lang.eye FIXME / open typeck-scope gap)
- [ ] verify untyped heterogeneous array literal (`let xs = [1, true]`) is rejected
      under let-from-init; close the uniformity check if open (DEFER row)

**T6 - parser bug:**
- [ ] block-bodied match arms (VERIFIED broken) - fixed by **blocks-as-expressions**
      (wire `{...}` into pratt expr parsing); small, bundled into P0 because it is
      the fix [parser]

**T7 - arithmetic UB-kill + the abort/panic keystone:**
- [ ] abort / panic mechanism (keystone) - the trap primitive, bare `abort()` now
      [codegen/runtime]
- [ ] `-fwrapv` in the clang invocation (signed overflow -> defined wrap) [driver]
- [ ] div / mod by zero + `INT_MIN/-1` runtime check -> abort [codegen]
- [ ] shift `>=` bit width -> define (trap or mask) [codegen]
- [ ] dynamic out-of-bounds index -> bounds trap (rides the keystone; DEFER "runtime
      bounds traps" unblocks here)

**T8 - main entry contract:**
- [ ] main return-type default (`-> int32`) + implicit `return 0` [typeck/feature]
- [ ] argc/argv: `main(int32 argc, string* argv)` [feature]
- [ ] recursive-main ban [typeck]

(the trap-default *flip* + `wrapping { }` escape is P1, not P0 - P0 ships the UB-kill
+ the mechanism; P1 flips the default and adds the modifier-block surface.)

NOT in P0: the performance backlog, the code-clarity / DRY backlog, and the
architecture-analysis items - separate tracks, not behavior-correctness.

### P1 - footgun-fix surface + cheap independents (first new surface)

- **trap-default arithmetic flip + `wrapping { }`** (the first modifier block; needs
  the P0 abort keystone). [MODBLOCK.md](../design/MODBLOCK.md), KERNEL.md option Y.
- **const-default parameters + `mut` marker** (completes the mutability model).
  MUT.md.

### P2 - ownership keystone (large; the reorder)

move/drop is the prerequisite for owning-payload ADTs and error-path drops - the
2026-06-19 order buried it at the tail of "mutability completion"; corrected
2026-06-21 to sit before ADTs / errors.

- **move + drop + use-after-move** (the auto-drop machinery). gates P3b, P4, the
  resource model. MEM.md.
- **`&mut T`** (mutability axis enforced, aliasing-xor-mutation deferred to escape
  analysis - option B). MUT.md.

### P3 - ADTs / sum types

- **3a non-owning**: `struct { tag; union }` desugar + B2 refutable payload-match
  seam + bare variant resolution (qualify only when forced). no drop dependency, so
  it can overlap / precede P2. KERNEL.md.
- **3b owning-payload drop glue** (tag-dispatched; needs P2). recursive ADTs (`Box`)
  deferred to P5 (comptime).

### P4 - error handling

- **S7-payload effects** (`set<TypeRef>` on `fail`; lattice extension over the built
  S4 fixpoint). EFFECT.md.
- **try / catch + implicit-propagation lowering** (hidden result-channel +
  early-return) + error-path drops (P2) + C3 nominal closed set + catch
  exhaustiveness. needs P3 (error values = ADTs) + S7. ERRORS.md. (raise word: last
  micro-pick.)

### P5+ - far future

- **comptime / prime VM** -> generics (monomorphization, subsumes row-poly effects)
  + recursive ADTs (`Box`) + generic containers (Vec / Option / Result / Box) +
  user-defined modifier blocks (the engine). PRIME.md.
- **escape / lifetime analysis** -> `&mut` aliasing-xor-mutation + full memory safety
  (dangling `&local`) + `restrict` / noalias optimization + `arena(a) { }` /
  `unchecked { }` modifier blocks (with their features). DEFER.md.

- [ ] **ADTs / sum types with payloads** [feature, Phase 2] - ratified 2026-06-19
      as the first **compiler-blessed desugaring** (KERNEL.md "The freeze,
      precisely"): `enum Opt = Some(int32) | None` -> `struct { tag; union
      payload; }`, match payload-patterns desugar through the B2 seam to kernel
      tag-check + union-extract + bind. **no new kernel primitive** (lowers to the
      frozen kernel). pulls forward match payload-patterns (S3+, DEFER). the value
      side of error handling ([ERRORS.md](../features/ERRORS.md) D2); also makes
      plain enums far more useful.

## Open bugs (correctness)

- [x] **M2 - mixed-width arithmetic narrows** [typeck] - CLOSED 2026-06-15
      (S3, typeck's first judgment customer). A binary's result type is now the
      operands' common integer type via `infer::arith_result_type`: an integer
      literal adopts the other operand's concrete width (Rust-style literal
      inference), so `7 - usize` and `usize - 7` both type `usize` and the MIR
      temp no longer truncates the C `size_t` result. Both operand orders fixed
      (the prior LHS-only rule was asymmetric). Regression:
      `judgments::mixed_width_arith_adopts_concrete_width`. (Was: CLEAK M2,
      VERIFIED 2026-06-11.) Follow-up out of scope: two _distinct concrete_
      widths (neither a literal, e.g. `int8 + int64`) still keep the LHS type;
      tracked as M2b below.

- [x] **L4 residue - cross-element array-literal types unchecked** [typeck] -
      CLOSED 2026-06-16 (S3). Each array-literal element is now checked against
      the declared element type in `coerce_array_literal` (after the per-element
      coercion, the same `site_assignable` the arg/field judgments use):
      `ArrayElementTypeMismatch`/T42. Regression:
      `array_element_type_mismatch_is_rejected`. (Was: CLEAK L4 PARTIAL.)

- [x] **U2 - const-eval value unchecked against declared type** [typeck] -
      CLOSED 2026-06-16 (S3). `const int8 X = 200` (and a global initializer)
      now range-checks the folded integer against the declared type:
      `ConstError::ConstValueOutOfRange`/C13, in `const_eval::check_const_range`.
      Composes with U4: an explicit `as` to the type is the blessed truncation
      (in range), a bare out-of-range value is rejected. Regression:
      `const_value_out_of_range_is_rejected`.

- [x] **U4 - const-eval `apply_cast` silently truncates** [typeck] - CLOSED
      2026-06-16 (S3). `apply_cast` now reproduces the C cast: an integer target
      truncates to its width (`200 as int8` folds to `-56`, two's-complement,
      via `wrap_int`), so a folded const equals its runtime value. int->float
      widens to f64. Paired with U2 so the truncation is the explicit-cast
      escape, not a silent loss.

- [x] **M2b - distinct concrete integer widths** [typeck] - CLOSED 2026-06-16.
      A binary on two _non-literal_ operands of different concrete integer widths
      (`int8 a + int64 b`) silently narrowed (took the LHS width); now rejected
      with `TypeError::MixedIntegerWidths`/T44 (the Rust strict-width rule),
      applied to arithmetic, bitwise, and comparison. The literal-adoption case
      (M2) and equal widths stay legal. Built per the correctness-over-breakage
      directive - but the corpus turned out to need _no_ changes (it already uses
      `usize` consistently for lengths/indices and literals elsewhere; sweep =
      0 new rejections). Regressions: `mixed_integer_widths_are_rejected`,
      `matching_width_and_literal_binaries_accepted`.

- [x] **string literal -> `char*` decay** - CLOSED 2026-06-16. `array_ref_decays_to`
      now accepts the `char`<->`uint8` byte pun (helper `byte_pun`), so a string
      literal (`&[uint8; N]`) decays into a `char*` slot - the scalar
      `let char* s = "hi"`, an array element `let [char*; N] = ["a", ...]`, and a
      `char*` FFI argument. Codegen needed no change: the decay already lowers to
      `RValue::Cast(inner, target)`, emitting an explicit `(char*)` cast that is
      well-defined and silences `-Wpointer-sign`. Verified end-to-end (compiles,
      links, runs) + strict-C. Regression: `string_literal_decays_to_char_ptr`.

- [x] **A3 - `mir_type_of` fallback** [typeck] - CLOSED 2026-06-15 at S2C C5.
      `mir_type_of` now reads typeck's `expr_types` (the sole type source) and
      ICEs on a miss: codegen only runs on a diagnostic-free program where the
      walker is total (proven by `corpus_generates_no_error_type`), so a miss is
      a compiler bug, not a silent `int32`/`error_type`. (Was: a missing entry
      silently typed a temp - the amplifier under every typing gap.)

All non-typeck open bugs were closed 2026-06-12 (see the loose-ends
completed entry); the rows above are the typeck pass's scope and are
accepted as documented limitations of the frozen kernel.

---

## Typeck split scope (Horizon 1, post-freeze)

The consolidated requirements list for the pass design. No fixes here until
the pass exists. **Design ratified 2026-06-12**: sealed-body inference,
docs/features/TYPECK.md (strategy, judgments, migration segments S0-S6) +
docs/features/EFFECT.md (the second lattice); see the completed entry for
the rulings.

- [x] Struct-literal field **value** types unchecked - CLOSED 2026-06-16 (S3
      first judgments, `StructFieldTypeMismatch`/T38). `P { x: "hello" }` with
      `int32 x` is now rejected at the typeck `StructLit` arm via `site_assignable`.
- [x] Call argument **types** unchecked - CLOSED 2026-06-16 (S3 first judgments,
      `ArgTypeMismatch`/T37). Swapped/wrong-type args are rejected at `infer_call`'s
      defined-fn arm (arity stays T026). Int-family + pointer-into-`ptr` stay lenient
      (defers M2b; FFI escape). Source FIXME at `generate_lang` updated.
- [x] `as` casts unrestricted any-to-any - CLOSED 2026-06-16 (S3). The cast
      lattice (CAST.md) is built: scalar<->scalar, pointer<->pointer,
      pointer<->integer convert; an aggregate (array/struct/union) on either side
      is rejected (`CastNotAllowed`/T43). `cast_allowed`/`cast_class` in the
      walker's `Cast` arm.
- [x] `const` declared type vs folded value unchecked - CLOSED 2026-06-16. U2
      range-checks integer consts/globals; the value-_kind_ check (`check_const_kind`,
      `ConstError::ConstTypeMismatch`/C14) closes the non-integer cases: a folded
      value's scalar kind must match the declared type's kind, so `const bool B = 5`,
      `const char C = 65`, `const int32 X = true` reject - the const analogue of the
      cast lattice (no implicit `int -> bool`/`int -> char`). `int -> float`
      widening (`const float64 R = 0x7fffffff`) and the `ptr <- int` address idiom
      (`const ptr NULL = 0 as ptr`) stay legal. Regressions:
      `const_value_kind_mismatch_is_rejected`, `const_matching_and_widening_kinds_accepted`;
      corpus sweep 0 new rejections.
- M2 operand unification (open-bug row above).
- Integer-literal typing: the `int32` default replaced by expected-type
  propagation (the T030 range sweep already guards the values).
- [ ] **let-init type check is Call-only** [typeck] - `check_explicit_let_init_type`
      (infer/judgments.rs) returns on any non-`Call` initializer, so a known-typed
      init through a field / variable / index read skips the declared-type
      judgment. observable footgun: a raw `ptr` field read into a typed-pointer
      slot (`let int64* d = vec.start;`) compiles, while the same value from a
      `malloc()` call rejects (T002). the dangerous `void*` -> `T*` reinterpret is
      gated for call inits, branch tails (T41) and args (`site_assignable`), but
      not for non-call let inits; integer narrowing through a non-call init leaks
      the same way. same directional rule as the integer widen/narrow note above.
      fix: route every known-typed init through `site_assignable`, pending the
      let-init width ruling. found dogfooding a Vec in dev/ds (raw `ptr` field).
- A3 `mir_type_of` fallback flipped to a hard error (open-bug row above).
- [x] `types_compatible` integer-family leniency (any int matches any int) -
      REMOVED 2026-06-16. Compatibility is now exact `TypeRef` equality (plus Error
      poison): the boundary analogue of M2b, so a non-literal `int64` argument to an
      `int8` parameter (and the same at fields/returns) rejects. Required a spine
      slice - `site_coerce` now forwards the expected type into value-position
      `if`/`match` branches, so a literal in a branch adopts the declared width
      (`let int64 x = match k { 0 -> 1, _ -> 2 }`); the only corpus program affected
      (match.eye) is fixed by the adoption, 0 net new rejections. Regressions:
      `mismatched_width_argument_is_rejected`, `value_position_branch_literals_adopt_width`.
- [x] Assignment is non-value - CLOSED 2026-06-16 (S3, `AssignInValuePosition`
      T39). A value-position `x = y` / `x += y` (let init, argument, condition,
      operand, value-producing branch tail) is rejected; statement position and
      discarded (void) tails stay legal.
- `ptr` duality: magic `Path("ptr")` vs `Ptr(inner)` appears at every type
  dispatch; give `ptr` a real representation.
- Fn-type variadic flag: `TypeKind::Fn` carries none, so indirect calls
  through a fn-pointer value skip the arity check (L3 residue).
- [x] U2/U4 const-eval range and cast checks - CLOSED 2026-06-16 (open-bug rows
      above).
- [x] L4 cross-element judgment - CLOSED 2026-06-16 (open-bug row above).
- [x] Tail-expression type enforcement in value-position blocks - CLOSED
      2026-06-16. A value-position `if`-branch tail is now checked against the
      declared type (`expect_branch_type`, after `site_coerce` adopts its literals):
      a `malloc()` tail (raw `ptr`) in an `int32*`-typed `if` rejects with
      `IfBranchTypeMismatch`/T41, cutting the implicit raw-`ptr` -> typed-pointer
      footgun per the kernel philosophy (a `void*`-style escape stays only for the
      _safe_ direction, typed -> `ptr`; the dangerous direction needs `as int32*`).
      match arms were already covered by `check_match_arm_consistency` (each arm vs
      the recorded match type). Regressions: `raw_ptr_into_typed_pointer_branch_is_rejected`,
      `raw_ptr_into_typed_pointer_with_cast_is_accepted`. 0 net corpus rejections
      (only lang.eye used the pattern - already XFAIL). The lang.eye `onset_head`
      FIXME is now an accurate rejection (it needs the `as int32*` casts).
- [x] **F1 - value-`if` branch types unchecked** - CLOSED 2026-06-16 (S3).
      `check_if_branch_consistency` runs the match-arm consistency judgment for a
      value-position `if`: `let int32 x = if c { 1 } else { true }` is rejected
      (`IfBranchTypeMismatch`/T41). Value position is the shared `discarded_set`
      (a statement-position or discarded `if` runs its branches for effect and
      stays legal). Regressions: `value_position_if_branch_mismatch_is_rejected`,
      `if_branch_consistency_clean_cases`.
- [x] **F2 - unary `-` on an unsigned type unchecked** - CLOSED 2026-06-16 (S3).
      `-u` on an unsigned value is rejected (`NegationOnUnsigned`/T40); a negated
      literal (a signed constant the range sweep bounds) is exempt. `~` (BitNot)
      stays legal - it is well-defined on unsigned (Rust parity), so the original
      `-`/`~` grouping was too broad; only `-` rejects. Regression:
      `negation_on_unsigned_is_rejected`.
- [x] **F3 - float literals do not adopt the expected float width** - CLOSED
      2026-06-16 (S3). `adopt_float_literal` (the float analogue of
      `adopt_int_literal`) retypes a float literal to a `float32` expectation at
      the coercion site, so `let float32 f = 1.5` and `[float32; N]` literals type
      consistently. Regression: `float_literal_adopts_expected_width`.
- [x] **Tier-2 expectation spine - BUILT 2026-06-17.** The headline remaining
      in-kernel inference work. `infer_expr(id, expected: Expectation)` (and
      `infer_block`) now thread a downward `Expectation` through every
      transparent node: a block to its tail, an `if`/`match` to each branch/arm
      (re-tagged `IfBranch`/`MatchArm` by `rebind`), a `return` to its value;
      imposing sites (let-init, call arg, struct field) start a fresh one; an
      operand passes `None`. One `expect` funnel - `infer_expr`'s tail - adopts a
      literal/divergent value to the expected width (`coerce_to`), files the
      array decay, and reports the cause-specific mismatch
      (`ArgTypeMismatch`/`StructFieldTypeMismatch`/`ReturnTypeMismatch`) dispatched
      on the `Cause`. This replaced `site_coerce` (one-level forwarding) and
      `expect_branch_type`: the scattered inline arg/field/return mismatch checks
      fold into the funnel; `check_if_branch_consistency` now compares each branch
      against the `if`'s settled type (subsuming `expect_branch_type`, so a
      both-branches-agree-but-wrong-vs-expected `fn -> int32 { if c { 1.0 } else
{ 2.0 } }` is still caught); the return checks are trimmed to arity-only.
      No net new rejections (behavior-preserving: 74 typeck + full workspace +
      corpus green). Open: the two-span render - the `Cause` selects the variant
      but the secondary span waits on the mismatch `TypeError` variants carrying
      a declaration `SyntaxNodePtr`. TYPECK.md Tier 1 / Tier 2 updated.

Acceptance corpus for the pass: lang.eye plus the CLEAK reproducers.

---

## Type-check + codegen gaps surfaced 2026-06-18 (arena allocator program)

A separate arena-allocator port exercised paths lang.eye does not. Six gaps
the frontend should catch before clang, queued for a dedicated session. The
headline: a `uint8*` accepts `+ usize` (and worse) with no cast, and the
resulting pointer type silently contaminates downstream integer arithmetic.
Gaps 1-3 are one root (pointer arithmetic leniency); 4-6 are independent.

- [ ] **pointer operand in non-offset arithmetic unchecked** [typeck] -
      `arena.start + arena.off` synthesizes a pointer, then `-addr` and
      `addr & mask` apply unary-neg and bitwise-and to that pointer. `ptr + int
      -> ptr` (offset) is the one legal pointer arithmetic; unary `-`, `~`, and
      the bitwise ops on a pointer operand must reject (need an explicit
      `as usize` first). the no-footgun rule for pointer arithmetic: only the
      offset form stays implicit, every other pointer-in-arith is the dangerous
      direction and is gated.
- [ ] **pointer type contamination propagates** [typeck] - once `addr` is typed
      pointer (gap 1), `(-addr) & mask` stays pointer and poisons every
      downstream use: `total = size + padding`, `next = arena.off + padding`.
      catching gap 1 at the source kills most of it; also verify a binary's
      result-type rule does not let a pointer operand leak into an
      integer-declared destination without the mismatch firing (the ptr/int
      analogue of M2b exact-width checking).
- [ ] **implicit `usize -> ptr` at a return** [typeck] - a fn declared `-> ptr`
      returns `arena.off + padding` (a `usize` offset), missing the
      `arena.start +` that would convert the offset to an address. typeck
      accepts the bare `usize` tail into a `ptr` return. `int -> ptr` is the
      dangerous direction (fabricating an address from a number) and must be
      gated behind `as ptr` - the int/ptr analogue of the `&T -> T*` widening
      already built (which gates only the safe direction). the blessed
      `const ptr NULL = 0 as ptr` idiom already casts; the offset case does not.
- [ ] **dead / pure expression statement** [typeck/lint] - a bare `NULL` (folds
      to `0`) as a statement after the if-body has no effect. flag an
      expression-statement whose value is pure and discarded (no call, no
      assignment, no side effect); or - if it was meant as an early exit - the
      missing `return` is the real bug. a purity check over the
      discarded-expression-statement set.
- [ ] **unsigned `<= 0` tautology** [lint] - `size <= 0` on a `usize` is always
      `== 0` (unsigned). readability lint: suggest `== 0`, and flag the
      always-false `< 0` / always-true `>= 0` family on an unsigned operand. not
      a correctness bug.
- [ ] **unused generated array-wrapper global** [codegen] - `__eye_arr_27_5uint8`
      is emitted into the C but never referenced (an empty-string `""` wrapper).
      dead-declaration elimination: suppress an emitted static/struct decl with
      no use site. codegen-side DCE, not typeck.

---

## Mutability model completion (surfaced 2026-06-18, lexer session)

The immutable-by-default rule (MUT.md) is applied to bindings + globals but not
parameters, and the reference model has `&T` only. Three linked items: the
silent-safety principle ([PHILOSOPHY.md](../design/PHILOSOPHY.md)) finishing a
half-done job. Designed in [MUT.md](../features/MUT.md), **not built** - build
nothing until ratified.

- [ ] **const-by-default parameters + `mut` marker** [feature] - parameters are
      currently mutable, the one immutable-by-default hole. design: an unmarked
      param is immutable (`Local.mutable = false`, the existing `AssignToImmutable`
      `T` check fires), `mut` opts into a mutable local copy. needs a `mut` marker
      on the `Param`/`ParamList` grammar + a `mutable: bool` on `Param` that
      collection sets. MUT.md "const-by-default parameters".
- [ ] **FFI const-correctness** [codegen] - clang knows libc fns as builtins with
      const-qualified pointer params (`memcpy` = `void *memcpy(void *dest, const
      void *src, size_t n)`); an Eye `extern` emits a non-const prototype for
      `src`, conflicting with the builtin ("incompatible redeclaration of library
      function"). closed for free by const-by-default params: an unmarked extern
      param emits a `const` C param (matching the builtin), a `mut` param emits the
      non-const form. MUT.md "FFI const-correctness".
- [ ] **`&mut T` mutable-reference split** [feature] - no safe mutable borrow
      exists; mutating through a reference forces the raw-pointer escape (`T*` +
      `*p`, the `ffi`/unsafe boundary). design: `&T` safe-default-silent, `&mut T`
      explicit opt-in (checked, no `ffi` effect), `T*`/`ptr` the unsafe escape -
      the same dangerous-direction-gated rule as `mut`. full soundness gated on
      escape analysis (dangling `&local`, DEFER); until then `&mut` carries the
      same runtime freedom `&` does today.

---

## Frontend-vs-clang safety audit (2026-06-18)

The silent-safety quadrant ([PHILOSOPHY.md](../design/PHILOSOPHY.md)) made
concrete: every place the Eye frontend leans on clang to catch a problem the
frontend should own. Two safety nets, a gap between them:

- production `eye build` (`src/backend.rs`) invokes clang **bare**: `clang
  file.c -o bin -O0`. no `-Wall`/`-Werror`/`-pedantic`/`-std`. clang hard-errors
  halt the build; clang *warnings* still build and run -> the binary ships with
  the issue. this is the silent-unsafe quadrant.
- the strict-C gate (`scripts/check-c-strict.sh`, CI corpus job) compiles the
  corpus's generated C under `-std=c11 -pedantic-errors -Wall -Wextra -Werror`.
  it proves **codegen cleanliness** for corpus programs; it does **not** catch a
  user's semantic error (it skips files Eye rejects, and only covers corpus
  inputs). it even suppresses `-Wno-unused-parameter`/`-Wno-unused-variable`
  because "Eye has no unused-binding lint yet" - two gaps recorded as
  suppressions.

three classes. class A = clang hard-errors (build halts); Eye already pre-empts
most (R-class unresolved names/unknown types, `typegraph` ordering, value
recursion, arity, the CLEAK "verified via strict gate" rows). largely owned, but
not entirely (the row below). class B = clang **default-on warnings** that still
build+run in production = silent today. class C = clang cannot catch even under
the strict gate (runtime UB). B and C are the work.

### class A - clang hard-errors the frontend should own (clean diagnostic)

- [ ] **non-integer array index** [typeck] - `index_judgments`
      (`crates/typeck/src/infer/judgments.rs`) checks the *base* type
      (`IndexOfNonIndexable`/T46, `IndexOnPtr`) and compile-time bounds, but never
      the *index operand's* type. `arr[3.5]` (float), `arr["x"]`, `arr[somePtr]`,
      `arr[aStruct]` all pass the frontend; clang then hard-errors ("array
      subscript is not an integer" / pointer+pointer). add an index-type judgment:
      the index must be an integer type. new `TypeError::NonIntegerIndex` (T47),
      the companion to T46. design call: integers only (`int*`/`uint*`/`usize`/
      `isize`); `char`/`bool` are C-integers but indexing with them is a footgun -
      lean Rust-strict (require `as usize`) per no-footguns. silent-safety: a clean
      T-class diagnostic instead of a cryptic clang error on generated C.

### class B - clang warns, production builds + runs anyway

- [ ] **no unused-variable / unused-parameter lint** [lint] - `-Wunused-variable`
      / `-Wunused-parameter`, suppressed in the strict gate. a dead local or
      param is silently legal Eye. frontend lint over `Body::locals` + params
      (reachability already known to the walker).
- [ ] **variadic argument types unchecked** [typeck] - extra args past a variadic
      extern's fixed params have no parameter to check against (`infer/mod.rs`
      stops at the declared arity), so `printf("%d", some_ptr)` passes a
      wrong-typed value to a `%d` slot - UB, and clang's `-Wformat` only fires for
      functions it recognizes as printf-family. a format-string-aware check (or at
      least a "variadic arg must be a scalar/pointer, not an aggregate" floor).
- [ ] dangling `&local` return - `-Wreturn-stack-address`. cross-ref DEFER escape
      analysis (the runtime-safety axis).
- [ ] implicit `usize -> ptr` at a return - `-Wint-conversion`. cross-ref arena
      gap 3 above.
- [ ] FFI const-correctness (builtin redeclaration) -
      `-Wincompatible-library-redeclaration`. cross-ref mutability section above.
- [ ] dead pure expression statement - `-Wunused-value`. cross-ref arena gap 4.
- [ ] unsigned `<= 0` tautology - `-Wtautological-compare`. cross-ref arena gap 5.
- [~] value-position uninitialized temp - `-Wsometimes-uninitialized`. the
      exhaustive-match case is FIXED (CLEAK M3, switch emits its last arm as
      `else`); the general value-position control-flow case waits on CFG MIR (A7).
- [?] incompatible pointer-type assignment - `-Wincompatible-pointer-types`. the
      array-element-invariance case (lang.eye `[char*; 10]` into `[ptr; 10]`) is a
      correct reject now, and scalar pointer mismatches mostly route through the
      cast lattice / ref-widening; verify no remaining slip path before closing.

### class C - clang cannot catch even strict (runtime UB)

these are the **defined-arithmetic-edge-semantics** kernel gap (KERNEL.md
"Genuinely-missing kernel substrate (2026-06-18 audit)"): the kernel ships the
operator set but not its behavior at the edges. **decided 2026-06-21 (pair
session, option Y): trap-by-default.** every edge with no correct value
(signed/unsigned overflow, neg `INT_MIN`, over-width shift, div/mod by zero,
`INT_MIN/-1`) traps at runtime (reserved `panic` atom, allowed in `pure`/prime) -
a bug per ERRORS.md D1, not a silent wrap. wrapping is opt-in via a lexical
**`wrapping { }` modifier block** (no per-op sigils), which doubles as the
auto-vectorization opt-out; saturating/checked are stdlib intrinsics, not syntax.
rejected: wrap-by-default + trap-debug/wrap-release (both ship a silent footgun in
the release build). sequencing: kill UB now (`-fwrapv` + define shift + div-zero
`abort()`), flip overflow to trap + ship `wrapping { }` when the abort path lands
(bare `abort()` works before the full panic theme). vectorization preserved by
clang check-elision + the region + a future MIR once-per-loop check-hoist + the H3
backend.

- [ ] **signed integer overflow** [kernel-semantics] - C UB; Eye emits `a + b`
      verbatim and inherits it. near-term near-free fix: compile the generated C
      with `-fwrapv` (clang/gcc make signed overflow two's-complement wrap =
      defined), pairing with the `-Wall` prod-build decision above. that removes
      the UB without the abort mechanism; the explicit-intent ops are the later
      ergonomic layer.
- [ ] **runtime division / modulo by zero** [kernel-semantics/runtime-safety] -
      only the *constant* case is caught (`ConstDivByZero`/C9, const_eval.rs). a
      runtime `a / b` with `b == 0` is UB; `-fwrapv` does NOT cover it. needs a
      runtime trap (abort mechanism) or a checked-div lowering.
- [ ] **shift amount >= bit width** [kernel-semantics/runtime-safety] - `x << 64`
      on a 64-bit type is UB; not covered by `-fwrapv`. runtime trap or a masked
      shift lowering.
- [ ] dynamic out-of-bounds index - cross-ref DEFER bounds traps (same theme).
- [ ] pointer arithmetic producing a bad address (typed `T* & mask`, `-ptr`) -
      cross-ref arena gaps 1-2 (frontend can gate the dangerous forms; the
      resulting address validity is still runtime).
- [!] raw-pointer deref of an invalid/garbage pointer - inherent to the `ptr`
      escape, `ffi` boundary by design ([EFFECT.md](../features/EFFECT.md)); not a
      gap to close, the explicit unsafe seam.

### threads 2 + 3 (same session)

- thread 2 (diagnostics too loud): swept the diagnostic surface - Eye does **not**
  over-narrate. messages are already terse (doc-voice) and the classes are small.
  the asymmetry runs the other way: Eye **under-catches** (the B/C gaps above),
  it does not over-explain. no loud-safe softening work found. revisit if a future
  pass adds chatty notes.
- thread 3 (missing-capability fork ranking, by real-program unblock): for the
  programs being written now (the lexer, lang.eye), the order is **sum types w/
  payloads > slices/growable buffers > generics > modules**. a lexer's `Token`
  wants `Ident(string) | Number(int) | ...`; today enums are tagless (no payload),
  so the token type cannot be expressed without a manual tagged union. sum types
  (Fork B2, the extensible-match engine) is the nearest wall. generics and modules
  unblock scale, not the current programs. ties to the design-question rows below.

### C-attribute stamping: free clang enforcement

the resolution mechanism for several class-B rows, and the cleanest expression
of the silent-safety quadrant: Eye does the analysis, the C backend stamps an
`__attribute__`, clang/gcc enforce it for free. the user writes nothing; a silent
fact becomes a loud-safe clang check. split by what Eye already knows:

- [ ] **`nonnull` on `&T` / `string` parameters** [codegen] - references and
      string values are non-null by construction (a `&place` of a real value, or
      a string literal); only raw `ptr` / `T*` is the nullable escape. stamp
      `__attribute__((nonnull(i, j, ...)))` with the 1-based positions of every
      `&T`/`string` param. maps exactly onto the existing ref-vs-ptr distinction,
      no new language surface. closes the "passing 0/NULL to a reference param"
      hole for free.
- [ ] **`format(printf, m, n)` on the printf prototype** [codegen] - Eye emits
      `int printf(const char*, ...)` for the `println` intrinsic; stamping
      `__attribute__((format(printf, 1, 2)))` makes clang check the varargs
      against the format string. directly closes the class-B "variadic argument
      types unchecked" row for the printf path. arbitrary user variadic externs
      need a format-position annotation (below) to know which param is the format
      string.
- [ ] **`warn_unused_result` / `returns_nonnull`** [feature, blocked] - need new
      language surface Eye lacks: a `must-use` annotation and a nullability /
      never-fail contract (no null concept yet). deferred until those annotations
      exist; the user's motivating cases (`align_alloc` never-null,
      `init` must-use) are annotation-driven, not inferable. record only.

decision: build the free tier (nonnull-on-refs, format-on-printf) when the
mutability/FFI work lands - they share the extern-emission path; defer the
annotation tier.

### main entry contract

design captured in [MAIN.md](../features/MAIN.md). three items; build nothing
until ratified. note: integer exit codes via `-> int32` already work (the shim
forwards `return (int)__eye_main()`); these are the remaining gaps.

- [ ] **main return-type default + implicit `return 0`** [feature] - main is the
      one function whose omitted return type defaults to `int32`; falling off the
      end is an implicit `return 0` (C99), so a bare `main() {}` exits 0 with no
      ceremony and `return 1` sets the code with no `exit(1)` call. a main-only
      completeness exception (non-main `-> int32` fall-off stays a missing-return
      error).
- [ ] **argc/argv** [feature] - accept `main(int32 argc, string* argv)` (with or
      without `-> int32`); the shim forwards `int main(int argc, const char**
      argv) { return __eye_main(argc, argv); }`. relax `MainHasParams` to this one
      shape, reject any other param list. slice-typed argv waits on slices.
- [ ] **recursive-main ban** [feature] - reject a call expression resolving to the
      main function (an entry point is not a callable routine). new diagnostic.

### decision: production build surfaces clang warnings (not pedantic)

ratified 2026-06-18 ("if Eye doesn't catch everything, at least clang should -
not too pedantic"). today prod `eye build` (`src/backend.rs`) is bare
(`-O0`, no warning flags), so even issues clang catches-by-warning ship silently.

- [ ] add `-Wall` to the prod clang invocation: warnings become **visible**
      (int-conversion, return-type, return-stack-address, uninitialized,
      incompatible-pointer-types, format are all in `-Wall` or default-on).
- [ ] do **not** add `-pedantic`/`-pedantic-errors` (GNU-extension noise = too
      pedantic, against the directive) and do **not** `-Werror` by default (a
      warning must not break the flow). the strict-C gate keeps
      `-pedantic-errors -Werror` for the corpus/codegen-cleanliness job only.
- [ ] optional `eye build --strict` opts a user build into the gate's full flag
      set for those who want loud-safe-as-error.

### use-after-invalidation evidence (lifetime/ownership theme)

a concrete motivator for the deferred escape/lifetime analysis (DEFER): reading
`arena.start` after `reset(&arena)` (which zeroes the fields) is currently safe
only by accident - `reset` happens to null them. printing after a free/reset is
UB the frontend should reject as "use of `arena` after it was invalidated".

- two layers exposed, both feed the mutability + ownership theme:
  1. `reset(&arena)` mutates through a `&` (shared, immutable) reference - it can
     only do so today via a raw-pointer write, the `&mut` gap (mutability section
     above). a `reset(&mut Arena)` would express the mutation honestly.
  2. "arena is dead after reset" needs a **consuming / move** parameter mode
     (the fn takes ownership; the caller's binding is marked moved-from; later
     reads reject) - Rust's affine model. this is the third reference mode beyond
     `&T` / `&mut T`: by-value-owned. the largest single feature here; the whole
     escape/lifetime/ownership theme, deferred. record as the motivating example.

---

## Blocks as expressions (decided 2026-06-18 pair session)

- [ ] **a block is a first-class expression everywhere** [feature] - Rust's rule:
      a block evaluates to its final expression when that expression has no
      trailing `;` (a trailing `;` makes it `()`). Eye already does this in
      dedicated slots (fn body, `if`/`loop` tails - the tail-expr mechanism); the
      decision generalizes it so a bare `{ ...; v }` is an expression in any expr
      position (`let x = { .. }`, a call arg, a match-arm RHS). same machinery,
      wider. interacts with the statement-boundary footgun fix (`if {} * p`,
      already handled) - keep block-like-in-statement-position parsing as-is.
- [ ] **block-bodied match arms** [parser] - VERIFIED broken 2026-06-18: `1 -> {
      let int32 y = 2; y }` fails with `[S045] expected an expression` at the `{`.
      arm RHS uses general pratt expr parsing (`grammar/expr.rs lhs`), which does
      not accept a `{` block. the AST already has `MatchArm::body() -> Option<Block>`
      (generated.rs:1295) - intended but never wired in the grammar. SUBSUMED by
      blocks-as-expressions above: once `{...}` is an expr, the arm RHS accepts it
      for free.

---

## Error handling (design agreed 2026-06-19 pair session, not built)

Full design in [ERRORS.md](../features/ERRORS.md). errors = first-class values;
fallibility = a tracked `fail` effect. decided: D1 bugs/recoverable split
(bugs -> panic/abort = the deferred abort theme; recoverable -> values), D2
inferred typed payload-carrying error union (the effect-lattice join = the
error-set union, no `From` plumbing, no `Result` signature noise), D3 implicit
propagation by default + explicit-check optional, visibility via the `fail`
signature + witness-driven LSP inlay hints. handling = a `catch` boundary (no
algebraic resume).

- [ ] **`fail` effect + payload (typed-effect upgrade)** [effect] - `fail` carries
      the error type; the lattice join unions error types. this is the
      typed/payload-effect upgrade (EFFECT.md S7-adjacent) - the one genuinely new
      machinery; today's atoms are payload-free `u8`.
- [ ] **implicit propagation + LSP visibility** [effect/lsp] - fallible calls
      auto-propagate inside a `fail` context (early-return-on-error lowering, no
      `setjmp`); the LSP paints potential-failure call sites inline from the
      witness data. explicit-check form optional.
- [ ] **`catch` boundary** [feature] - discharge the `fail` effect into a handled
      value; exhaustive match over the inferred union where matched. no resume.
- [ ] **drops on the error path** [codegen] - destructors/`defer` (MEM.md) must run
      as an error propagates out (the `errdefer` interaction).
- open sub-decisions (ERRORS.md "Open"): error value representation (gate on sum
  types vs a kernel-simple error first), keywords, catch exhaustiveness over an
  open union, dependency ordering vs sum types + EFFECT.md S7.

---

## Design questions (decide, not just fix)

Seven of the nine rows were ruled and (where code was needed) built
2026-06-12; see the completed entry. The two left open are post-freeze
features, not freeze blockers:

- [ ] CLI arguments: `main` takes no parameters, argc/argv unreachable.
      Post-freeze feature (needs a slice/string story, or a raw `ptr`
      form).
- [ ] Self-referential structs still impossible (no null, no two-phase
      init), so no linked list - lang.eye ports the arena instead. Needs
      the runtime-safety/null theme (DEFER: linked with slices).

---

## Architecture / infrastructure backlog

- [x] **Massive-file split** (maintainability; all 6 production files + both
      tier-2 test files DONE 2026-06-18). Split
      every oversized source file into concern-named module dirs, following the
      repo precedent (`crates/hir/src/core/lower/`, `crates/codegen/src/core/`):
      a concern dir + `mod.rs` (struct + `mod` decls + driver) and child files
      each carrying one `impl` block (or free items); cross-module items become
      `pub(crate)`, internal helpers stay private. Behavior-preserving (pure
      module moves), verify per file (`cargo test -p <crate>` + clippy). Recipe
      and gotchas: [[massive-file-refactor]] memory. - [x] `typeck/src/infer.rs` 2287 -> `infer/` (mod 748 / judgments 991 /
      coerce 280 / ty 302). 74 typeck green, clippy clean. UNCOMMITTED. - [x] `mir/src/lower.rs` 1288 -> `lower/` 2026-06-18 (mod 224 = entry +
      `Lower`/`ArmKind` + body/tail drivers + infra `collect_operands`/
      `terminated`/`map_local`/`mir_type_of` / stmt 442 = `lower_stmt`/
      `lower_expr_stmt`/blocks/`return` + match-arm machinery / expr 588 =
      rvalue+operand cores + `lower_into` family + const inlining / place
      87 = `lower_place`/`place_for_value`). 11 mir green, clippy clean.
      UNCOMMITTED. - [x] `codegen/src/core/mir_emit.rs` 1429 -> `mir_emit/` 2026-06-18 (mod
      664 = `gen_mir` entry + `MirGen` + `gen_all` driver + function/
      type-decl/global emission + `gen_stmt` + shared free helpers
      `c_fn_name`/`write_c_char_literal`/`local_name` / expr 446 = rvalue/
      operand/place + `place_type` recovery + `println` + literals / switch
      161 = `gen_switch`/`gen_guarded_switch`/`gen_arm_test` (module named
      `switch`, not the `match` keyword) / strings 209 = `collect_strings`/
      `gen_string_statics`/`string_id`). 1 codegen unit green, e2e 71,
      snapshots 4/4 (`c_codegen` byte-identical), clippy clean. UNCOMMITTED. - [x] `parser/src/grammar.rs` 1378 -> `grammar/` 2026-06-18 (items 407 /
      types 77 / expr 553 / pat 151 / stmt 156 + mod.rs re-globs siblings;
      all free fns blanket `pub(crate)` in the private `mod grammar`). 64
      parser green incl `cst_snapshot`, clippy clean. UNCOMMITTED. - [x] `parser/src/lib.rs` 1306 -> `event.rs` 2026-06-18 (Event +
      Marker/CompletedMarker + build_tree out; lib.rs keeps Parser/Parse/
      parse/tests). Marker fields -> `pub(crate)` (lib `open()` constructs
      it); `pub use event::{CompletedMarker, Marker}` keeps the API. 64
      parser green, clippy clean. UNCOMMITTED. - [x] `effect/src/lib.rs` 972 -> 2026-06-18 lattice.rs 121 (Atom/EffectSet + atom_index/LIVE_ATOMS + parse_effect_name/describe) + judge.rs 178
      (WitnessKind + EffectResult + EffectJudge observer + infer_body_effects) + lib.rs (EffectMap + fixpoint/SCC + contracts + witness trail). 16
      effect green, clippy clean. UNCOMMITTED. - [x] `lexer/src/lib.rs` 732 -> 2026-06-18 interner.rs 106 (Symbol/Interner +
      StringTable impl) + source.rs 173 (LineCol/SourceHolder/SourceText/
      SourceFile) + lib.rs (LexError/Lexed/Lexer). All moved items already
      `pub`; re-exported. 21 lexer green + Interner doctest, clippy clean.
      UNCOMMITTED. - [x] (tier-2) test files 2026-06-18. `hir/src/core/tests.rs` 2008 ->
      `core/tests/` (mod.rs = 3-line header + 5 helpers lower/diags/
      first_match/MAIN_EYE/SHAPE_DECL + 8 concern modules: arrays/consts/
      format/functions/matches/naming/pointers/structs; children `use
    super::*`). `typeck/tests/judgments.rs` 1856 -> `tests/judgments/`
      (cargo auto-discovers a `<name>/main.rs` dir as the same test
      target; main.rs = header + lower/diags + 7 modules branches/calls/
      casts/let_init/matches/range_arith/returns). hir 76 + judgments 74
      green, clippy clean. UNCOMMITTED. - x EXCLUDED: `ast/src/generated.rs` 1827 (xtask codegen output). - -> after the split: refresh TYPECK.md `infer.rs` path refs; then the
      let-from-init inference build (below) + the two-span render.
- [x] **Let-from-init inference** BUILT 2026-06-18 (the real annotation-omission
      feature, on top of the Tier-2 spine). An untyped `let x = <init>` now binds
      x to the initializer's bottom-up synthesized type (no inference variables -
      the type already exists). 4-point plumbing: (1) lowering `lower/stmt.rs` no
      longer emits T025 for an untyped let (typeck owns it); (2) typeck `infer/
  mod.rs` Let arm - when the annotation is absent and the pat is `Pat::Bind`,
      record `local_types[local]` from `infer_expr`'s returned type IF concrete
      (`is_inferrable`: not Error/Unit/Never); (3) MIR `lower/stmt.rs` normal Let
      reads the `local_types` fallback (mirroring `bind_local_to` for match
      bindings) - the ordering gate verified: lower (untyped) -> typeck (fills
      map) -> MIR (reads map) is already correct; (4) T025 reworded + relocated
      to typeck, firing only for a VALUE-LESS init (`()`/`!`, nothing to bind);
      an erroneous init (synthesizes nothing) stays silent (its own error covers
      it). init-less let is a parse error (`ExpectedEqInBinding`), so unreachable
      here. Tests: hir `untyped_let_requires_annotation` deleted; typeck
      `untyped_let_infers_from_init` + `untyped_let_value_less_init_rejected`;
      e2e `untyped_let_infers_and_runs` (21/9). hir 76 + judgments 76 + e2e 72,
      clippy clean. UNCOMMITTED. Detail: [[typeck-effects-design]]. NOTE follow-up
      found: an untyped `let xs = [1, "two"]` infers `[int32;2]` (heterogeneous
      array literal not element-checked without an expected type) - separate
      array-literal-synthesis hardening, ledgered below.
- [x] **Heterogeneous array-literal synthesis** FIXED 2026-06-18 (footgun
      surfaced by let-from-init). The `Expr::ArrayLit` synth arm
      (`typeck/src/infer/mod.rs`) now runs `check_array_homogeneous` (each
      element `site_assignable` to the first -> T42 `ArrayElementTypeMismatch`)
      when no array type is expected (`!self.expects_array(&expected)`); a
      declared element type still owns the check at the funnel
      (`coerce_array_literal`), so each bad element is reported exactly once (no
      double-report on the heterogeneous-AND-declared case). Test
      `untyped_array_literal_must_be_homogeneous`. typeck 77, clippy clean,
      corpus 0 new rejects (2 pre-existing XFAIL: lang.eye T024, linkedlist R008).
- [~] **Salsa structural backdating** (SALSA.md divergence 5). `Memo<T>`
  equality WAS `Arc::ptr_eq` (every re-executed query counts as changed).
  **Signature-firewall half BUILT 2026-06-16 (S5):** `Memo`'s `PartialEq`
  now delegates to a `MemoEq` trait (default conservative); `FileScope` and
  `LoweredFn` override it with a content digest (signature digest / body
  digest), so a body-only edit backdates `item_scope` and the unedited
  bodies' `typeck_fn` cache-hit - keystroke cost flat in project size.
  Verified both directions (`body_edit_backdates_the_sibling_typeck`,
  `signature_edit_reruns_every_body`). REMAINING (stays here, conservative):
  token-stream/`lex`/`parse`/whole-file backdating (a comment edit stopping
  at `lex`); and the end-state per-item signature queries
  (`fn_signature(StableFnId)`, with the multifile milestone).
- [ ] **A9 - LSP full-document sync, no incremental parsing.** Every
      keystroke re-runs the pipeline on the whole file. Debounce threshold
      plus incremental update when practical; salsa now memoizes between
      unchanged revisions but a changed file re-runs every phase.
- [ ] **A10 - typegraph walks every body expression per compilation**, even
      when type declarations are unchanged. A dirty-tracked or
      query-cached typegraph would skip redundant walks.
- [ ] **Readable-C mode.** Every subexpression spills to a `_tN` temp (MIR
      operand spilling), making the generated C hard to debug. Ruled
      2026-06-12: the spilled C is the ratified default (codegen quality,
      not semantics - does not block the freeze); a readable mode keeping
      nested expressions where legal, or gating full spilling behind a
      flag, is backlog. Source marker: `eyesrc/programs/lang.eye` (FIXME
      at the top of the file).
- [ ] **A7 - no CFG-based MIR.** Structured MIR (If/Loop/Switch) limits
      backend analysis; value-position control flow leaves uninit temps (no
      phi nodes). A CFG lowering pass would unlock dominance, liveness,
      SSA, dead-code elimination.
- [ ] **A6 - parser `no_struct_lit` is a single boolean, not a stack.**
      RAII save/restore works, but nested struct-literal ambiguity from any
      future grammar change must handle the interaction manually.
- [ ] **VFS / source manager.** Load source text once, serve every consumer;
      groundwork for multi-file compilation. (Salsa's `SourceFile` input is
      the seed; the manager would own path->input mapping and file
      watching.)
- [ ] **`main` shim debug UX.** User `main` emits as `__eye_main` plus a C
      `main` shim; debuggers see `__eye_main`. Consider
      `__attribute__((weak))` or a linker alias.
- [ ] **Evict `print`/`println` (and `len`) intrinsics to stdlib.**
      `println` is a dedicated `RValue::Println` sniffed by unresolved
      callee name; composing `printf` in Eye needs a pre-lowering rewrite
      pass. Post-typeck, non-blocking (the last KERNEL.md gap row).
- [ ] **`if` codegen refactor** - follow up on the recorded conversation
      about if-statement codegen (see 2.1.157).
- [ ] **Intelligent error spans**: reduce lexing-time calculations, trim
      spans at emit time, scan for smart spans only on the error path.
- [ ] **Parser sync mechanism re-evaluation**: resilient today, but should
      recover to the next valid code without producing rubbish diagnostics.
- [x] **Statement-boundary footgun: `if {} * p`** FIXED 2026-06-18 (ruled by
      kernel philosophy: adopt the Rust statement-expression boundary). In
      statement position the block driver (`grammar/stmt.rs`) now parses a
      block-like expr (`if`/`loop`/`match`) via `lhs` (which returns before the
      infix pratt loop) instead of the full `expr`, so a following `*`/`-`/`&`
      starts the next statement (a deref/neg/ref) rather than folding as an infix
      operator on the block's value. a pure-infix follow (`+`/`==`/...) becomes an
      `ExpectedStatement` (rust-correct - not a prefix op). expression position (a
      let initializer, a call arg) still uses full `expr`, where the block-like
      form is an operand and the operator binds. Test
      `block_like_in_statement_position_is_a_complete_statement` (asserts two
      statements + no `BinExpr` in stmt position; `BinExpr` present in expr
      position). parser 65, clippy clean, corpus 0 new rejects.
- [x] **Unit type `()` + Never type `!`** - CLOSED 2026-06-17. Was a cascading
      MIR-ICE gap: a value-less `Expr::If`/`Expr::Loop` typed `None`, which
      reached `mir_type_of` and panicked. Built the full Rust-style pair instead
      of a single sentinel: `TypeKind::Unit` (the value-less completing type, `()`)
      and `TypeKind::Never` (the bottom/divergent type, `!`), both pre-interned in
      `TypeInterner` (`unit_ty()`/`never_ty()`). `()` is now spellable in source
      (`f() -> () { }` - parser `UnitType` node, normalized to void in
      `InferCtx::new`). The walker is now total: a tail-less or all-statement block
      types `()`; a value-position `if`-without-else / bare `loop` (no break) /
      `return` / `break` / `continue` types `!` and coerces to any expected type
      (Never-absorbing `join`); a value-position expr that types `()` is rejected
      (`VoidValueInValuePosition`/T024 via `check_value_position_voids`). Wired at
      every `TypeKind` match site: display (`()`/`!`), walk leaf, lower, MIR/codegen
      (both -> C `void`), const-eval, mangle (`unit`/`never`), LSP, AST dump.
      Regressions: `explicit_unit_return_type_is_void_like`,
      `value_position_void_if_is_rejected`, `never_branch_coerces_to_value_branch`,
      e2e `value_position_loop_does_not_panic` (now a clean T024 reject, was a
      silent print-0 footgun).
- [x] **Ref (`&T`) / ptr (`T*`) auto-conversion** BUILT 2026-06-18, refined to
      DIRECTIONAL (the decided "convert automatically" was symmetric; kernel rule
      = dangerous direction gated, so only the safe direction is implicit).
      `ref_widens_to_ptr` (`typeck/src/infer/ty.rs`): a `&T` flows into a `T*`
      slot (same pointee) with no cast - both are a `T*` in C and using a valid
      reference as a raw pointer is lossless. The reverse (`T*` into `&T`,
      fabricating the safety guarantee) stays GATED behind an explicit cast.
      Wired into `site_assignable` (arg/field) and the `Cause::Return` policy in
      `cause_assignable`; no codegen change (Ref/Ptr already emit identical C).
      Tests: `ref_widens_to_typed_pointer`, `typed_pointer_does_not_narrow_to_ref`,
      e2e `ref_widens_to_pointer_and_runs`. typeck 79, clippy clean, corpus 0 new
      rejects. UPDATE 2026-06-18: lang.eye line-160 void-`if` (T024) FIXED at
      source (`if syl.str[i] == 'v' { seen_nucleus = true; }` - statement form,
      the `{ true; }` made the value-position `if` void; T024 was correct) + the
      call site is now `&lang, &arena` (ref widening), so lang.eye PASSES `--check`.
      Remaining XFAIL = a clang error from the program's own `[ptr; 10] words`
      hack (line 69): `lang.words = word_arr` assigns a `[char*; 10]` to a
      `[ptr; 10]` field. arrays are element-INVARIANT (char* -> ptr widens per
      scalar but not per whole-array; the C wrapper structs differ), which is the
      correct no-footgun rule - lang.eye needs consistent array element types
      throughout (its own data-model cleanup), not a compiler change.
- [x] **Deref / index of a non-pointer-non-indexable value** FIXED 2026-06-18
      (footgun surfaced by a design discussion on `&`/`*` semantics; lang.eye's
      `*arena` where `arena` is a value was the tell). typeck silently accepted
      `*x`/`x[i]` on a plain value (scalar/struct/...) and emitted invalid c. Now:
      T45 `DerefOfNonPointer` - the `Expr::Deref` arm (`infer/mod.rs`) rejects an
      operand that is not `&T`/`T*` (RawPtr stays its own T28 `DerefOfPtr`, Error
      stays silent). T46 `IndexOfNonIndexable` - `index_judgments`
      (`infer/judgments.rs`) rejects a base that is not an array / pointer /
      reference / `string` (the byte-pointer view, indexable like `char*`); a
      typed `T*`/`&T` still indexes c-style, RawPtr stays T27 `IndexOnPtr`.
      Directional `&`/`*` model confirmed: `&` = address-of (value -> ref), `*` =
      deref (ref/ptr -> value); pass a value into a `*struct` param via `&value`
      (covered by #372 `&T -> T*` widening). Tests
      `deref_of_non_pointer_is_rejected`, `index_of_non_indexable_is_rejected`.
      CAUGHT a regression in review: the first index cut rejected `string`
      (string.eye/caesar.eye) - fixed by allowing `Path("string")`. workspace all
      green (e2e 73, judgments 81), clippy clean, corpus 0 new rejects.

---

## Architecture analysis (2026-06-18)

Full expanded analysis of four architectural questions, noted for decision
after the full spec is built.

### 1. MIR — 75% mechanical, 25% essential

MIR is 1,346 lines of lowering + 317 lines of type defs + 1,482 lines of
mir_emit codegen = **3,145 lines total**. Every HIR Expr variant (24 of them)
maps to MIR.

**Essential MIR work** (adds structure HIR cannot express): **~350-400 lines**
- Temp/spill allocation (`lower_operand_raw`) — ~80 lines
- Short-circuit `&&`/`||` rewritten to control flow (~80 lines)
- Value-position `if`/`match` in-place lowering (`lower_into`) — ~70 lines
- Match arm classification + guard lowering — ~60 lines
- Destructure expansion — ~30 lines
- Decay materialization — ~20 lines

**Mechanical MIR work** (1:1 mapping HIR→C): **~900-1000 lines**
- Literal, Path, Binary, Unary, Call, Index, Field, Ref, Deref, Cast, SizeOf,
  ArrayLit, StructLit, Assign — all emit trivially from HIR
- Control flow (If, Loop, Break, Continue, Return) — 1:1
- Let/bindings — 1:1

A HIR→C codegen would absorb the mechanical work inline. Estimated size:
**~1,770 lines** vs current 3,145. Savings **~1,375 lines (~44%)**.

The cost: less testable (no separate MIR dump/diff), harder to add a second
backend, evaluation-order correctness inlines with C emit logic.

### 2. Dual database path — every body lowered and typed twice

On a single `eye build`, the pipeline is:

| Query | Fires? | Work |
|-------|--------|------|
| `lex` | yes | tokenize |
| `parse` | yes | parse |
| `item_scope` | yes | collect items (no bodies) |
| `lower_fn` (per-fn) | yes × N | lower every body |
| `typeck_fn` (per-fn) | yes × N | type-check every body |
| `lowered_file` | **yes** | **re-does item collection + every body lowering + every body typeck** |
| `mir_map` | yes | MIR-lower every body |
| `c_code` | yes | emit C |

**`lowered_file` is a full redundant pipeline.** It calls `lower_source_file`
(which calls `collect_file_scope` + per-body `lower_fn_body`) and `infer_file`
which re-types every body. On a body-only edit, every per-fn query correctly
cache-hits for siblings, but `lowered_file` re-executes everything from scratch.

**The architectural fix exists now:** With the S6 lock-free interner, per-fn
`lower_fn` and `typeck_fn` already intern into the *shared* `item_scope.types`.
The old "private interner clone" doc comment is stale — handles are comparable
across bodies. So the whole-file path's only technical reason for existing
(shared interner) is already guaranteed by the per-fn path.

Fix the query graph to:

```
SourceFileInput
└─ lex ─ parse ─ item_scope ─┬─ lower_fn ─ typeck_fn ── typeck_map (new)
                              │                              │
                              │                effect_fixpoint (new)
                              │                              │
                              └── mir_map ───────────────── c_code
```

Where `typeck_map` collects per-fn results (cache hits on unedited bodies),
`effect_fixpoint` reads per-fn atom data, and `c_code` reads them both. This
eliminates the entire `lowered_file` path from production.

### 3. AST elimination — not worth it

Every `ast::` accessor used in HIR lowering is a thin wrapper:

```
fn_ast.body() -> Option<Block>
  syntax.children().find(|n| n.kind() == SyntaxKind::Block)
    wraps in Block(syntax_node)
```

The AST is **1827 lines of generated code** + **633 lines of hand-written
lib.rs** = **2460 lines**. But:

- **For HIR alone**: eliminating it saves ~100 lines and adds ~100 lines of
  inline raw-rowan traversal. Net wash.
- **For total deletion**: saves ~2000 lines but every crate duplicates
  `EXPR_KINDS` slices, `TYPES_KINDS` slices, etc., losing compile-time
  exhaustiveness. Adding a grammar variant silently falls through raw
  `matches!` — the AST's `enum Expr { ... }` forces a match-arm to be added.

The rowan pattern (thin AST as typed lens over CST) is standard and correct.
**This change has negative ROI.**

### 4. Separate batch parser — highest impact, highest effort

A second `parser::parse_batch()` that skips rowan entirely and produces
`hir::core::HIR` directly would eliminate:
- The event stream (`Vec<Event>` with tombstone writes)
- The `GreenNodeBuilder` + rowan `GreenNode` allocation
- The `SyntaxNode::new_root` + AST extraction
- The entire `ast::` crate dependency for batch

Estimated saving: the ~60% parse-time allocation (existing perf doc figure).

The rowan parser stays for LSP/dumps unchanged. The batch parser shares the
same grammar functions but writes to HIR arenas instead of events. Since HIR
lowering currently reads AST (which reads CST), a direct batch parser would
write `hir::core::HIR` nodes (items + bodies) directly during recursive
descent.

**Cost:** two parsers to maintain. Every grammar change needs updates in both.
The LSP path always has the correct rowan parser; the batch parser is a second
correctness surface.

### 5. `TypeKind::Path("string")` representation — latent bug generator

`string` is interned as `TypeKind::Path("string")` — a flat name, not the
structural `TypeKind::Ref(uint8_ty)` that it semantically represents. Every
judgment that matches on structural pointer types (`Ref`/`Ptr`/`RawPtr`) must
also remember to handle `Path("string")` or silently reject valid code.

**Evidence:** two bugs found in one session (T046 indexing, T028 deref). The
same blind spot exists in every judgment that matches TypeKind and has a
catch-all error arm. The `cast_class` path is lenient-by-coincidence
(`Unknown` → `cast_allowed` returns `true`).

**Why it was done this way:** `TypeInterner::new()` interns all primitive names
uniformly as `TypeKind::Path(name)`. `string` is just one entry in the name
list. Making it structural would require either (a) a special case at intern
time, or (b) a post-intern normalization pass.

**Fix: intern `string` structurally.** At `TypeInterner::new()`, instead of
`this.intern(TypeKind::Path(Text::from("string")))`, compute `let uint8 =
this.intern(TypeKind::Path(Text::from("uint8"))); let string =
this.intern(TypeKind::Ref(uint8));`. The display name "string" would need
preserving — add a `display_overrides: FxHashMap<TypeRef, &'static str>` map
(or overload `Debug` for `TypeKind::Ref` when the pointee is `uint8`/`char`).
The `array_ref_decays_to` check in `ty.rs` already handles `Path("string")` as
a decay target; change it to match `Ref(uint8)` instead. This eliminates an
entire class of latent bug at its root.

**Cost:** the decay check changes, the `Path("string")` special case in
judgments disappears, and the `string` display name needs a fallback. Low
effort, moderate impact.

### 6. Lowering/typeck split — was the cutover worth it?

**Current state:** Lowering produces untyped HIR; typeck is a separate pass
that walks HIR and produces `TypeckResults` (expr_types, adjustments,
local_types). Communication is through a side table. This required the S1-S6
migration campaign (shadow oracle, C1-C5 coordinated flip, PARITY markers, A3
fallback concerns).

**Why this way:** The split enables incremental typeck (per-fn `typeck_fn`
salsa queries cache-hit on unedited bodies), clean separation of structural
lowering from semantic analysis, and the potential to swap type inference
strategy without touching lowering.

**Question the premise:** For a sealed-body inference model where every
function is checked independently, rerunning typeck on an unedited body is
~1µs. The incremental win of memoization is noise at this scale. The *real*
win of the split is that typeck owns the type judgments — not that it can be
memoized.

**What if lowering produced typed HIR directly?** Like rustc's AST→HIR
lowering, which stamps expression types during lowering. The HIR body would
carry `expr_types` as part of its construction. No `TypeckResults` side table.
No MIR `mir_type_of` indirection. No shadow oracle period.

**Counterpoint:** The split forced a clean API between lowering and type
checking. The `TypeckResults` table is the explicit, documented contract. If
types were stamped inline in lowering, the boundary would be blurred —
lowering would need access to the type checker, creating a circular dependency.
Rustc avoids this by... doing exactly that (rustc's lowering calls into type
inference). Rustc's HIR is typed.

**Verdict:** The cutover was necessary because the original design *did* have
inline stamping, and extracting it was the right call for correctness (the
stamping had bugs). But the cost was extreme relative to the benefit. The
lesson is architectural hygiene pays off, but deferred architectural work
compounds interest.

### 7. Salsa query database — necessary complexity or premature infrastructure?

**Current state:** The database crate wraps 6 file-level and N per-fn Salsa
queries. The CLI creates a fresh database per invocation (one-shot, no reuse).
The LSP keeps the database alive across requests for incremental recomputation.

**Why Salsa:** Incremental compilation — a keystroke in a body re-runs only
that body's typeck. LSP integration — Salsa's revision tracking maps directly
to LSP diagnostics changes. Memoization between requests.

**Question it:**
- **One-shot cost:** Every `eye build` allocates a Salsa `Database` (which is a
  Storage handle bump + per-query memo table allocations), registers source
  input, runs 6+N tracked queries, then drops everything. The memo tables and
  Arc-wrapped `Memo<T>` entries are written exactly once and never read again.
  The granular hot-path audit estimates ~5-10% overhead. A `compile_direct()`
  bypass (already in the performance backlog) addresses this, proving the
  design acknowledges the waste.
- **LSP benefit:** At ~57µs per full compile (58-line program), even 10x scale
  (~570µs) is within LSP response time budgets. The incremental granularity
  Salsa provides (per-fn vs per-file) saves microseconds on a milliseconds-scale
  operation. Is the complexity of Salsa — SALSA.md divergence document, `MemoEq`
  trait, signature firewall (S5), `Storage` handle threading, `#[salsa::query]`
  macro invocations across crates — worth sub-millisecond savings?
- **What's the alternative?** A simple compile cache: parse + lower + type +
  codegen into a struct. On change (detected by file modification time or
  content hash), recompile the whole file. LSP diagnostics come from the cached
  result. No query tracking, no revision counter, no memo tables. The per-fn
  granularity (which Salsa was needed for) is unnecessary at this scale.
- **SALSA.md divergences:** 5 documented divergences from idiomatic Salsa
  (structural backdating, shared type interner, per-database snapshots,
  fork-on-write diagnostics, the bypass path). Each divergence is a workaround
  for Salsa's assumptions that don't fit Eye's architecture. This is the
  strongest signal that Salsa is the wrong tool.
- **Growth path:** If the language goes multi-file, Salsa's per-file query
  granularity becomes valuable (a change in file A only re-types dependents).
  But multi-file is far in the future, and the VFS milestone is still
  unstarted. Eye is incurring Salsa complexity now for a future need that may
  or may not materialize.

**Verdict:** Salsa is over-engineered for the current single-file scale. The
complexity cost (SALSA divergences, S5 firewall, crate dependencies, query
macro overhead) exceeds the incremental benefit for a ~57µs compile. The batch
bypass path is an admission that the query infrastructure is overhead. The LSP
could use a simple content-hash cache with no query tracking.

### 8. MIR — what does it actually buy?

**Current state:** MIR is a structured IR (If/Loop/Switch, not CFG) sitting
between HIR and C codegen. 3,145 lines. MIR lowering handles spills,
short-circuit rewrite, value-position control flow lowering, match arm
classification, and decay materialization. Codegen reads MIR and produces C.

**Why MIR:** Abstraction layer — codegen doesn't know about HIR's match
expressions, string decay, array literal retyping, etc. Optimization surface —
MIR is where CFG analysis, constant folding, and dead code elimination would
live. Second backend boundary — if C is ever replaced, MIR is the shared
interface.

**Question every claim:**
- **Optimization surface:** MIR-OPT was built and **fully reverted**. CFG-based
  MIR (A7) is a backlog item with no scheduled implementation. No MIR
  optimization exists today. The "optimization surface" argument is entirely
  hypothetical.
- **Second backend:** No plans exist for anything other than C. WebAssembly,
  direct machine code, and LLVM IR are not on any roadmap. The "second backend"
  argument is also hypothetical.
- **Abstraction:** The abstraction is real — codegen uses `RValue::Use`,
  `RValue::BinaryOp`, `RValue::Cast`, not `Expr::Binary`, `Expr::Cast`. But
  75% of MIR lowering is mechanical 1:1 mapping. The 25% that is genuinely
  structural (spills, short-circuit, value-position lowering, match
  classification, decay) could be absorbed into HIR lowering or codegen
  directly.
- **What does the 25% buy that makes the 75% worth it?** Evaluation-order
  correctness (C respects left-to-right for comma operators but not for function
  arguments; MIR's temp spilling guarantees order). Match lowering (HIR's match
  with guards, exhaustiveness, and nested patterns is complex; MIR linearises
  it). These are real — but they could be done as a HIR→HIR transformation
  (match lowering pass) + codegen-side temp management, without a full MIR IR.
- **Testability:** MIR dump/diff is useful for debugging, but e2e tests (which
  run the compiled C and check stdout) catch the same bugs. The `mir_dump`
  snapshot tests are rarely the first line of defense — they're usually updated
  after a behavior-preserving refactor, not after a bug fix.

**HIR→C alternative:** A codegen that walks HIR directly, managing a temp
stack for evaluation-order spills and applying decays + match lowering inline.
Estimated size: ~1,770 lines vs current 3,145. Savings: ~1,375 lines (44%).
Cost: codegen knows about HIR's full complexity (match, decay, etc.).

**Verdict:** The 75% mechanical overhead is a real carrying cost. The 25%
essential work is valuable but doesn't require a separate IR. The optimization
and second-backend arguments are speculative. If the goal is to ship a working
compiler, HIR→C is simpler. If the goal is a platform for optimization
research, MIR is the right investment.

### 9. Sealed-body inference — principle or constraint?

**Current model:** No inference facts cross function boundaries. No inference
variables, no unification. The bidirectional expectation spine threads
top-down `Expectation` through transparent nodes; leaves synthesize types
bottom-up. Tier 2 adds `Cause`-chaining for diagnostic provenance.

**Why this model:** Embarrassingly parallel (S6 fan-out across bodies).
Incremental (body edit re-types one body). Simple — no constraint solving, no
HM variables. Precise diagnostics (Cause chains name the exact argument/field).

**Question the constraints:**
- **No inference variables means limited inference.** The only "inference" is
  literal adoption (int/float literals take the expected type). `let x = 1`
  gives x type `int32` (the int default). `let y = x` gives y type `int32`
  (from x, which happens to be known). `let z = id(x)` where `id` is
  `fn(T) -> T` would need generics (not in the language) **and** inference
  variables. The sealed-body + no-variable model means adding generics would
  require either inference variables (breaking the model) or fully explicit
  type annotations (like `fn id[T](x: T) -> T` — no inference at call sites).
- **The Expectation spine is ~500 lines of plumbing.** Every `infer_expr` call
  threads an `Expectation`. Every transparent node (block, if, match, return)
  rethreads it with `rebind`. The Cause enum has a variant for every diagnostic
  context. This displaces what would be a handful of constraint variables.
- **Sealed bodies prevent inter-procedural inference.** A function's return
  type is exactly its declared signature — never inferred from the body. A
  function's parameter types are never refined from call sites. This is
  consistent with Eye's philosophy (explicit types at boundaries) but it IS a
  constraint, not a law of nature.
- **Parallelism is genuine.** The S6 fan-out across bodies works because sealed
  bodies have no shared inference state. But how much does this matter? At the
  current scale (~50 functions, ~57µs compile), parallel typeck saves ~20µs.
  Even at 1000 functions, the savings is ~400µs. At what scale does this matter?
- **The Cause infrastructure is elegant but incomplete.** The ledger lists
  "two-span render" as open work — the secondary span isn't rendered yet. The
  elaborate Cause chain (Return → Field → Arg) is built but only the primary
  span is displayed. The payoff is still pending.

**Verdict:** The sealed-body model is a principled choice that simplifies
parallelism and incremental computation at the cost of inference power. It's
consistent with Eye's explicit-typing culture. The real question is whether
the inference constraints (no HM, no cross-function inference) will block
future language features (generics, higher-kinded types, etc.). If yes, the
model needs a Tier 3 (body-local unification) escape hatch — which is
explicitly mentioned in TYPECK.md but not built.

### 10. Effect system — differentiating feature or complexity sink?

**Current state:** Effect lattice (io/ffi/state), per-body atom collection
fused with typeck walk, whole-program fixpoint (Tarjan SCC + condensation),
exact-match annotation contracts, witness trails. Separate crate with 3 source
files + 16 tests.

**Why effects:** Eye's differentiating feature. Compile-time tracking of I/O,
FFI, and state mutation. Exact-match contracts mean a function declared
`pure fn foo()` is compiler-verified to perform no I/O, no FFI, and no state
mutation. Witness trails explain *why* a function has an effect.

**Question the ROI:**
- **What does exact-match buy?** If you declare `io fn foo()` and the compiler
  infers `{io}`, the annotation is redundant (both agree). If you declare
  `pure fn foo()` and the compiler infers `{io}`, the compiler rejects it.
  This is a safety net: annotations are enforced, not just documentation. But
  the enforcement is only as useful as the annotation coverage — unannotated
  functions are never checked (no default "must be pure" rule).
- **Whole-program fixpoint complexity.** The call graph, SCC condensation, and
  transitive effect propagation are ~200 lines of code. The fixpoint runs in
  ~1µs. This is not expensive. But it IS complexity that only exists for effects.
- **Witness trails.** "The `io` effect comes from a call to `println` (via
  `reporter`)" — this is genuinely useful for understanding why a function
  has an effect. But the witness trail is only shown on the error path (when a
  contract is violated). For correct programs (the common case), the trail is
  never displayed. The value is on the error path only.
- **Fusion with typeck** means the effect observer runs during the typeck walk.
  This is zero additional traversals. The observer API is minimal (3 call
  sites in the typeck walker). The fusion cost is in maintenance: every typeck
  change must consider the effect observer.
- **8 reserved atoms** (alloc, panic, diverge) are listed but unused. The
  lattice has 8 bits, only 3 are live. The reserved atoms add complexity to
  the display, parsing, and validation code without any operational value.
- **Growth path:** Row-polymorphic effects (S7) would add effect variables for
  precise higher-order effect tracking. This is the natural extension but adds
  significant complexity (effect variables, unification, row typing). S7 is
  explicitly listed as "not started" with an S6 dependency.

**Verdict:** Effects are genuinely differentiating and the implementation is
well-architected (fused walk, single traversal, fixpoint after). But the value
prop depends on annotation culture — if users don't annotate their functions,
the effect system is silent and provides no value. The 8 reserved atoms and
the unimplemented S7 extension suggest the system was designed for a future
that may not arrive. The costs (maintenance, complexity, crate dependency) are
real and ongoing.

### 11. Codegen as C string building — what's the ceiling?

**Current design:** `gen_mir` appends C text to a `String` via `write_fmt!`.
No LLVM, no assembly, no intermediate representation. The output is compiled
by clang/gcc with no optimization flags. The strict-C gate enforces standards
compliance.

**Why C:** Simplicity — string building is the most direct path to an
executable. Debuggability — the generated C is human-readable. Portability —
any platform with a C99 compiler works. No LLVM dependency — faster builds,
no version management.

**Question the ceiling:**
- **Performance:** Eye programs run at clang -O0 speed. For the raytracer,
  floodfill, and sieve benchmarks, this is ~2-10x slower than -O2. The compiler
  emits no `restrict`, no `inline`, no alignment hints, no vectorization
  pragmas. Every pointer access goes through a `_tN` temp. The user accepted
  spilled C as the default (readable-C mode is backlog), but the performance
  gap is semantic, not cosmetic.
- **Feature envelope:** Every language feature needs a C analogue. Features
  that don't map to C — guaranteed tail calls, garbage collection, stackful
  coroutines, exceptions, arbitrary-precision integers, dynamic dispatch — are
  either impossible or require a runtime library. The `extern` mechanism
  delegates to C for anything outside the envelope. This means Eye's growth
  is bounded by C's expressiveness.
- **Optimization wall:** Without an IR, there's no place for optimizations.
  Constant folding is done in HIR const-eval. Dead code elimination, inlining,
  loop unrolling, and alias analysis would need a new IR (LLVM or CFG-MIR).
  The CFG-MIR item (A7) is backlog with no timeline.
- **C is not a portable assembly.** C has implementation-defined behavior
  (int size, char signedness, struct padding, etc.), and Eye's generated C
  depends on clang's x86-64 interpretation. Porting to ARM, wasm, or a
  different C compiler would surface latent assumptions. The strict-C gate
  catches clang-specific issues but doesn't guarantee portability.
- **Build chain dependency:** The compiler doesn't produce executables — it
  produces `.c` files. Users need clang/gcc installed. This is a reasonable
  dependency for now but means Eye can't be a self-contained tool.
- **Growth path to LLVM:** If the project decides to use LLVM, it would need to
  either (a) emit LLVM IR from MIR (which doesn't exist in LLVM-compatible
  form), or (b) replace the codegen backend entirely. Both are large projects.
  The current C backend would become a fallback or be dropped.

**Verdict:** C codegen is the right choice for a kernel-phase language
exploration — it minimizes build dependencies and keeps the compiler simple.
But it creates a hard ceiling on performance (clang -O0), feature
expressiveness (C-compatible only), and optimization potential (no IR). The
question is whether Eye will ever need to break through this ceiling. If the
goal is a research/teaching language, C codegen is fine. If the goal is a
production language, LLVM or direct machine code is inevitable.

### 12. Single-file compilation — dead end or foundation?

**Current state:** Every `.eye` file compiles independently. No imports, no
modules, no linking. `extern` declarations interface with C. The VFS/source
manager is an unstarted backlog item.

**Why single-file:** Simplicity — no module system means no cross-file name
resolution, no build system, no linker invocations. The LSP only has one file
to track. Tests are self-contained `.eye` files.

**Question the sustainability:**
- **Is Eye viable without multi-file?** Real programs of any significant size
  need multiple files. A 5000-line file is unwieldy. Libraries (standard or
  third-party) need a module system. The entire corpus fits in single files
  only because the language is new and the programs are small.
- **Every architectural decision assumes single-file.** The Salsa database is
  keyed by `SourceFile`. The `HIR` struct is per-file. The `item_scope`
  collects items from one file. Type resolution looks in one `HIR.items`.
  Effect inference is a whole-program fixpoint over one file's functions.
  Multi-file changes every one of these.
- **The VFS backlog item is the prerequisite.** "Load source text once, serve
  every consumer; groundwork for multi-file compilation." It's listed under
  "Architecture / infrastructure backlog" with no priority and no assignee.
- **The `StableFnId` abstraction is already in place.** It was built for the
  S5 signature firewall and the per-fn `typeck_fn` query. The entity exists
  but the multi-file plumbing (how to resolve a `StableFnId` across files)
  doesn't.
- **Amount of work:** Multi-file touches every crate. The parser (multiple
  files to parse). The item collector (cross-file collections). Type resolution
  (cross-file type lookup). The effect system (cross-file fixpoint). Codegen
  (linking multiple `.c` files or producing a single output). The LSP (multiple
  open files, cross-file references). The CLI (multiple input files + output
  executable). This is the single largest feature the language could add.

**Verdict:** Single-file is fine for exploration but unsustainable for growth.
The multi-file milestone is the point where Eye either becomes a real language
or remains a toy. The current architecture (per-file queries, `StableFnId`,
the HIR/typeck abstraction) is reasonable scaffolding for multi-file — none of
the existing abstractions would need to be replaced. But the amount of work is
daunting and there's no clear plan or timeline.

### 13. Freeze-before-typeck sequencing — architectural debt lessons

**The sequence:** Build the language with inline type stamping in lowering.
Freeze the kernel (declare all non-typeck bugs closed). Then extract typeck
from lowering into a separate pass. This was Horizon 1: S1 (shadow oracle),
S2 (migrate judgments), S2C cutover (C1-C5), S3 (new judgments), S4 (effects),
S5 (firewall), S6 (parallel).

**Why this sequence:** The exploratory phase needed a working compiler fast to
validate the language design. Typeck was the most complex and risky change, so
it was deferred. The freeze created a stable base before the risky refactoring.

**Question the cost:**
- **The typeck cutover (C1-C5) was the most expensive single refactoring in the
  project's history.** Days of work, a shadow oracle (100+ lines of comparison
  logic), PARITY markers at every judgment site, a coordinated multi-step flip
  (each C1-C5 had its own revert risk), and the shadow oracle's eventual
  deletion. The granular audit mentions 4 source files modified across 5
  crates for the C5 irreversible flip alone.
- **What if typeck had been built first?** If lowering had never stamped types,
  the shadow oracle would never have been needed. The entire S1-S6 migration
  would have been unnecessary. The "typeck is the sole type authority" was the
  destination — the project spent enormous effort migrating from a design that
  was known to be temporary.
- **The counter-argument is about exploration speed.** Building a working
  compiler first validated that the language *could* compile to correct C.
  Finding bugs (M2 mixed-width narrowing, L4 array element type checking, the
  string decay gap, etc.) required a running compiler that produced real
  output. If the team had designed the perfect type system first, they would
  have discovered the same bugs later, against a more rigid architecture.
- **The real lesson is about deferral cost.** The typeck cutover was deferred
  from the initial design, and the cost of deferring was the S1-S6 migration.
  The question for future architectural decisions is: what is the next typeck-
  scale refactoring, and should it be deferred or done early? Multi-file is
  the obvious candidate. If it's deferred, the project will pay the same
  migration tax when it's eventually built.

**Verdict:** The freeze-before-typeck sequencing was a rational tradeoff
(exploration speed vs architectural purity) that incurred predictable
deferred cost. The total cost (S1-S6 + cutover) was probably worth it — the
exploratory phase produced a validated language design. But the experience
should inform future sequencing: multi-file (the next big architectural change)
should be done early if it's ever going to be done at all.

### 14. Diagnostic architecture — 3 sinks, 9 classes, manual codes

**Current state:** Three diagnostic sinks (lowering, typeck, effect) merged at
the driver level. 9 error classes (Lex, Parse, Resolve, Const, TypeError,
Effect, generic Error, etc.). Manual error code assignment (T037-T046,
E001-E002, C013-C014). Two-span render is designed (Cause enum) but not fully
implemented (secondary span SyntaxNodePtr not yet carried).

**Why this design:** Separation of concerns — each pass owns its diagnostics.
Explicit error codes make tests precise (assert on T046, not on a string
match). The Cause chain captures diagnostic provenance for two-span rendering.

**Question the patterns:**
- **Three sinks cloned at every merge.** The granular audit flags
  `database/lib.rs:409,413,421` — each sink is cloned before extending into
  the merged result. An `Arc<Sink>` or a `&Sink → Vec<Diag>` by-reference
  collect would eliminate these clones. The current pattern is known-bad and
  documented.
- **9 classes with overlapping domains.** A type error in a const context: is
  it T-class (TypeError) or C-class (ConstError)? The `ConstValueOutOfRange`
  (C013) is a type error (value doesn't fit declared type) but lives in C-class.
  The `ArrayElementTypeMismatch` (T042) is a type error in an array literal.
  The boundary between T-class and C-class is fuzzy.
- **Manual error code assignment is fragile.** Two developers adding
  diagnostics simultaneously could assign the same code. There's no central
  registry or assignment authority. The ledger's source-comment register
  cross-references FIXMEs but doesn't catalog error codes. The current
  highest T-code is T046 (indexing non-indexable), C014 (const type mismatch),
  E002 (effect mismatch). A collision is unlikely but unguarded.
- **Two-span render is designed but unbuilt.** The Cause enum carries the
  diagnostic provenance (`Cause::Field { field, cause }`, `Cause::Arg { index,
  cause }`, etc.) but the actual two-span rendering (primary span + secondary
  span pointing to the declaration) is listed as open work. The elaborate
  Cause infrastructure is built for a feature that doesn't fully exist yet.
- **The `Sink<T>` type is `Vec<T>` in a trench coat.** It's not arena-
  allocated (every diagnostic is a heap allocation). It's not `Copy`-cheap
  (clone clones every entry). It's not integrated with the type interner
  (diagnostic types are display strings, not TypeRef handles that could be
  resolved lazily).

**Verdict:** The diagnostic architecture is serviceable but has known
deficiencies (clone costs, unbuilt two-span render, fuzzy class boundaries)
that the ledger explicitly tracks. The Cause infrastructure is ahead of its
consumers. The cloning pattern is the most immediate fix (swap `clone()` for
by-reference collect).

---

## Performance backlog

- [ ] **Salsa bypass for batch compile** : The CLI driver creates a fresh
      `Database` per invocation and routes every phase through 6 salsa-tracked
      queries (`lex`, `parse`, `item_scope`, `lowered_file`, `mir_map`,
      `c_code`). Salsa per-query cost: revision tracking, input registration,
      memo-table insertions, `Memo<T>` Arc wrapping, dependency edge recording,
      change-detection comparison. In a one-shot compile every query executes
      exactly once : the tracking infra has zero reuse value. A
      `compile_direct()` path calling the underlying pure functions directly
      (`lexer.tokenize → parser.parse → hir::lower_source_file →
      effect::infer_file → mir::lower_all → codegen::gen_mir`) would bypass
      all tracking. The building blocks are already public; the sketch is
      `src/main.rs` routing around `crates/database`. Estimated saving: ~5-10%
      of pipeline time.
- [ ] **Pre-size codegen output string** : `gen_mir` builds the C output as a
      bare `String` with no capacity hint. Append via `write_fmt` may
      reallocate several times. Either estimate from input size or collect
      into a `String`-sized `with_capacity` once the C line count is known.
- [ ] **Rowan `NodeCache` allocation** is the main memory-pressure point
      (flame graph): vendor rowan or pre-reserve, or drop the cache.
- [ ] **`TypeKind::Fn` clones `Vec<TypeRef>`** at every hard-dep / node
      collection cycle in typegraph + topo-order. `TypeNode::Fn` stores
      `params: Vec<TypeRef>` and `Fn` is cloned each time it enters a
      `FxHashSet<TypeNode>`. Consider interning function-pointer types so the
      node is a single handle, or storing the `TypeRef` of the whole fn type
      (which `Function::fn_type` already holds).
- [ ] **`typegraph.rs` re-walks every body every compile** (A10). `topo_order`
      and `compute_scc` iterate `hir.bodies` looking for wrapper typedefs.
      A dirty-tracked or query-cached typegraph would skip redundant walks on
      body-only edits.
- [ ] **`effect::infer_file` re-runs the whole-program fixpoint on every
      compile.** For a single-file compile it is fast (~1 µs), but the SCC
      condensation and contract checks are pure overhead when the file compiles
      from scratch anyway. Liftable into the batch bypass.
- [ ] **Dense-integer-keyed maps -> `Vec`/arena indexing**: `local_map`
      (mir/lower.rs), `string_index` (codegen), `fn_names` (dump) are hash
      maps keyed by dense newtype ids; direct indexing is O(1) with no
      hashing and better locality. Pairs with typed arenas.
- [ ] **Parser CST always built** : Even on batch compile (no LSP, no dumps),
      the rowan green tree is constructed for every parse, accounting for ~60%
      of parse time. NOTE: `ast::SourceFile` is a typed lens over `SyntaxNode`
      (every accessor calls `support::children(&self.syntax)`), so the AST
      cannot exist without the CST. Eliminating the CST would require a
      parallel non-rowan AST type or changing `hir::core` to consume a
      different input format — both massive. The feasible batch saving here is
      the **Salsa bypass** above (skip query tracking, not the parse itself),
      plus fixing the `build_tree` tombstone writes (event.rs:129 overwrites
      every `Vec<Event>` slot before dropping it).
- [x] PARALLEL.md sharing: the type interner needs a concurrent structure -
      DONE 2026-06-16 (S6). `TypeInterner` is now lock-free (`boxcar::Vec` +
      `papaya::HashMap`, `&self` intern), so the whole-file per-body walk fans
      out across rayon with no clone. The global _symbol_ table (cross-file)
      still wants the same treatment at the multifile milestone (`lasso`).
- [ ] **Finer-grained database query graph** : `c_code` depends on
      `lowered_file` + `mir_map` + `parse` + `lex`. A body edit busts all of
      them because `lowered_file` has no per-fn granularity. The fix:
      `c_code` should depend only on `mir_map` + the expression-type seed
      (already separate). `mir_map` already reads per-fn `typeck_fn` results
      (memoized). Diagnostic gating should live in `mir_map`, not be re-read
      from `lex`/`parse` in `c_code`. This would mean a body edit busts only
      `typeck_fn` (that body) + `mir_map` + `c_code`, not `lowered_file`,
      `item_scope`, `parse`, or `lex`.
- [ ] **Remove whole-file `lowered_file` for codegen path** : Split into
      `lowered_hir` (whole-file, honest for item collection) + per-fn
      `typeck_fn` (already exist) + `effect_map` (whole-program fixpoint but
      cheap). `c_code` reads `typeck_fn` results per-fn + the fixpoint,
      avoiding the redundant whole-file typeck. Requires the typegraph's
      expression-type seed to work from per-fn `TypeckResults` (it already
      does: `typeck::expr_type_seed(&checked.typeck)` iterates the
      existing map).

## Code clarity / DRY / data-structure backlog

- [x] **`collect.rs` DRY violation** FIXED 2026-06-18 (clarity sweep). The 6
      duplicate-name checks (struct/function/union/opaque/extern-fn/enum) - which
      were NOT identical, each `||`-chaining a different namespace subset - now
      call one `ItemScope::name_in_use(&name)` (`items.rs`) that checks ALL item
      namespaces. Cleaner AND more correct (a name is unique across every
      namespace, no-footgun); verified behavior-safe (full suite + corpus, 0 new
      rejects). The `check_c_keyword + check_reserved_file_scope` pair (6 sites:
      global/struct/function/union/enum/variant, identical args) ALSO consolidated
      into one `check_file_scope_name`; the c-keyword-only sites (fields, opaque
      type, reserved-exempt extern fn) call `check_c_keyword` directly.
- [x] **`LoweringCtx` carries dead state** FIXED 2026-06-18 (clarity sweep).
      `fn_ret: Option<TypeRef>` was written (init + per-body set) but never read
      (return diagnostics moved to typeck at S2). Removed all 3 sites (field decl
      `lower/mod.rs`, init `ctx.rs`, assignment `fn_body.rs`).
- [ ] **`typegraph.rs` `HardDepsVisitor` uses a manual `pointer_stack` Vec**
      rather than a recursion counter or the natural call-stack. The
      `visit_ty`/`visit_ty_post` protocol forces storing `under_pointer` state
      per recursive depth. This is a pattern constraint from the `VisitTypeRef`
      trait, but pushing/popping auxiliary state on every Ref/Ptr visit is
      ceremony easily broken by a missing pop. Consider threading a depth
      parameter through the visit trait, or restructuring `hard_deps` as an
      iterative walk with an explicit stack of `(TypeRef, bool)` pairs.
      (DEFERRED from the 2026-06-18 clarity sweep: a visitor-trait restructure
      that touches codegen typedef ordering - not a mechanical/behavior-safe
      change, belongs with the typegraph rework, not a clarity pass.)
- [ ] **`TypeNode` clones on every graph operation** : `collect_type_nodes`,
      `topo_order`, and `compute_scc` all clone `TypeNode::Nominal(name)`,
      `TypeNode::Array { .. }`, and `TypeNode::Fn { params: .. }` repeatedly
      : into `FxHashSet<TypeNode>` for dedup, into `FxHashMap<TypeNode, usize>`
      for indexing, into the output `Vec<TypeNode>`. For `Fn` variants this
      clones `Vec<TypeRef>` (params) each time. A `TypeNode` internment or
      arena-id-based representation would eliminate these clones.
- [ ] **`flat_map` + `collect` chains** in the typeck inference walk
      (`infer_expr` call/array-literal/struct-literal arms) materialize
      intermediate `Vec`s via `.collect()` when only iteration is needed.
      Specifically: the call-arg arm collects `args`, `param_tys`; the
      array-literal arm collects `elems`; the struct-literal arm collects
      `fields`. All of these are immediately iterated once and dropped.
      These per-expression-shot allocations are the main allocation pressure
      in typeck (S2 perf pass fixed some, but this cluster survived).
- [ ] **`LoweringCtx::resolve` chains 7 sequential `FxHashMap::get` lookups**
      for every identifier reference. A `NameResolution` struct mapping each
      name to its resolved kind in one lookup would convert 7 hash probes into
      one (at the cost of pre-populating it per scope : worth measuring).
- [ ] **`BodySourceMap` is three separate `ArenaMap`s** (`expr`, `stmt`, `pat`)
      keyed by raw `Idx<Expr>` etc. These are always accessed together
      (e.g. source-map lookups after lowering consult all three). A single
      `enum SourceMapEntry { Expr(SyntaxNodePtr), Stmt(SyntaxNodePtr),
      Pat(SyntaxNodePtr) }` arena with a unified `Idx` type would simplify
      the API and improve cache locality.
- [ ] **`nominal_field_types` allocates a fresh `Vec<TypeRef>` on every call**
      by mapping `fields.iter().map(|f| hir.fields[f].ty).collect()`.
      Called from `node_deps` for every nominal-type node in the dependency
      graph. This vector is immediately iterated and dropped by `hard_deps`.
      Either cache field types on the `Struct`/`Union` struct, or return an
      iterator rather than an owned Vec.
- [ ] **`codegen::core::mir_emit::gen_all` builds a single `String` segment**
      by appending one function at a time alongside header includes, typedefs,
      string statics, and the main shim. The type-declaration order is
      produced by `topo_order` as `Vec<TypeNode>`, which is immediately
      pattern-matched and formatted. For large programs, pre-computing an
      estimated output size and using `write!` into a `BufWriter`-style
      abstraction instead of repeated `write_fmt!` into a growing `String`
      would reduce reallocation.
- [ ] **`infer_file` in `effect/src/lib.rs`** no longer needs to be a
      whole-file function. With the lock-free interner, effect inference
      can be per-body like typeck is. The fixpoint is the only cross-body
      step; factor so the LSP's `hir_diagnostics` path can skip it when
      no effect-relevant change occurred.
- [ ] **Systematic intermediate `Vec` removal.** Many `collect::<Vec<_>>()`
      calls allocate only to be immediately iterated once. Strategy per
      pattern:

      | Pattern | Instances | Fix |
      |---------|-----------|-----|
      | `.collect::<Vec<_>>()` then immediate `for`/`iter()` | `infer_expr` call/array/struct arms, `effect/lib.rs:72` (rayon), `codegen/mod.rs:219` (fn ordering) | Direct iterator, no collect |
      | `.collect::<Vec<_>>().join()` | `effect/lib.rs:224` | `Iterator::intersperse` (Rust 1.80+) |
      | Hash map keyed by dense arena ID | `codegen/strings.rs:33-34`, `mir/lower.rs` | `Vec` indexed by handle (O(1) no hashing) |
      | `.to_vec()` on slice | `typeck/coerce.rs:217` | `SmallVec<[T; 4]>` (small arrays avoid heap) |
      | Fresh `Vec` per hot-loop iteration | `typegraph.rs:317-318,406-407` | Allocate once, `clear()` each iteration |
      | `Vec<TypeRef>` from field iteration | `typegraph.rs:146` | Accept `&mut Vec` buffer parameter |
      | Per-field `Vec` in struct validation | `lower/expr.rs:297-332` | One `HashSet<&Text>`, derive both sets |
      | `Vec<Param>` -> `Vec<TypeRef>` per fn type | `collect.rs:394,522` | `ThinVec<TypeRef>` in `TypeKind::Fn` |
      | `Vec<String>` for witness trail | `effect/lattice.rs` | Fixed `[&str; 3]` array + counter |
      | `Sink` clones on every diagnostic path | `main.rs`, `database/lib.rs` passim | `Arc<Sink<T>>` or `&Sink<T>` -> `Vec<Diag>` by reference |

## Granular per-method hot-path audit (2026-06-18)

Every entry below is a specific method/expression-level waste found by
combing each crate. Ranked by estimated impact.

### A. src/main.rs + crates/database/src/lib.rs : CLI + query layer

- **`main.rs:30` : `text.clone()` double-allocates source text.** `text` is
  cloned before `SourceText::new(text)`, so the entire file exists as two
  owned `String`s. Fix: share via `Arc<String>` or restructure to consume.
- **`main.rs:39,62` : `.clone().into_diags()` on every diagnostic path.**
  Salsa's `Memo` returns `&Sink`, forcing `.clone()` before `into_diags()`
  (which takes `self`). Acceptable on error-exit (rare path).
- **`main.rs:91` : `hir.diagnostics.clone()` clones the entire typed `Sink`.**
  Could collect into `Vec<Diag>` by ref instead of cloning the typed sink.
- **`main.rs:96-97` : `checked.typeck[&fn_id].diagnostics.clone()` per fn.**
  `checked` is dead after line 107 : consume it via `.into_iter()` to avoid
  clones.
- **`database/src/lib.rs:287,294` : `file.text(db).to_owned()` clones source
  text in both `lex` and `parse` queries.** Same file text cloned 3× total
  (into `SourceFileInput`, then `SourceText` in lex, then again in parse).
  Fix: `SourceText` should share via `Arc<str>`.
- **`database/src/lib.rs:337-342, 375-380` : O(n) linear scan over `scope.fns`
  per fn, repeated in both `lower_fn` and `typeck_fn`.** For N functions,
  each per-fn query does O(N) work to find `FnId` by `SyntaxNodePtr`.
  Fix: store `FxHashMap<SyntaxNodePtr, FnId>` in `FileScope`.
- **`database/src/lib.rs:407-414` : two redundant passes over `scope.fns`.**
  First for lowering diagnostics, second for typeck diagnostics. Each
  constructs `StableFnId` anew. Merge into one pass.
- **`backend.rs:15` : unconditional `.to_owned()` of generated C string.**
  Only needed when `format` is true. Conditionalize.

### B. crates/parser/src/ : parser

- **`grammar/stmt.rs:98-107,163-169` : dead code: `#[allow(dead_code)]` on
  `fn stmt()` and `fn expr_stmt()`.** Neither is called; the `block` loop
  inlines the dispatch. Delete both (13 lines + two `#[allow]` attrs).
- **`grammar/expr.rs:103-108` : `p.nth0()` called twice in `expr_bp`.**
  The peeked token is re-read after binding-power extraction. Fix: store
  in a local `op` variable.
- **`grammar/expr.rs:112-129` : duplicated `matches!` on assignment ops.**
  `infix_binding_power()` already matched the same operator set. Return
  `InfixOp { l_bp, r_bp, kind: SyntaxKind }` from `infix_binding_power`
  to eliminate the second 11-arm match.
- **`lib.rs:161` : `self.tokens.len() - 1` underflows on empty input.**
  Latent panic. Guard with `if self.tokens.is_empty() { return TextRange::default(); }`.
- **`grammar/items.rs:277-287` : unnecessary `open()` + `abandon()` pair
  on empty enum body.** Writes two events then immediately tombstones one.
  Guard with `had_first_variant` check before opening.
- **`lib.rs:222` : `as u32` truncation on `diagnostics.len()`.** Use
  `u32::try_from(...).expect(...)`.

### C. crates/hir/src/core/lower/ : HIR lowering (hot path)

- **`expr.rs:261-268, 285-292` : `types.lookup(ty)` computed twice for the
  same `TypeRef`, name cloned twice.** Fix: compute once before the branch.
- **`expr.rs:679-682` : `Vec<ast::Expr>` collected only for `.len()` + `[0]`.**
  For call-arg arity check, every argument is heap-allocated into a `Vec`
  solely to check count ≠ 1 and index `args[0]`. Fix: use `.count()` + `.next()`.
- **`expr.rs:297-332` : struct-literal validation allocates `Vec<Text>` +
  two separate `FxHashSet`s.** One `Vec` of declared names, one `HashSet`
  for `field_names`, then a *second* `HashSet` from the same `Vec`. Fix:
  build one `FxHashSet<&Text>` and compute missing/unknown from it.
- **`fn_body.rs:65` : `lowered_block.stmts.clone()` clones entire `ThinVec`
  per function body.** The block was just allocated in the arena. Fix: write
  directly into `ctx.body.block` / `ctx.body.tail` without routing through
  a `Block` arena entry.
- **`collect.rs:319-321, 570-572` : `contains_key` + `insert` double hash
  lookup per field/variant.** Fix: `entry().or_insert()`.
- **`collect.rs:597` : `Vec<ast::Variant>` collected only for error-path
  indexing.** Fix: `zip` lowered variants with AST iterator.
- **`collect.rs:394, 522` : `Vec<TypeRef>` allocated from `SmallVec<Param>`
  per fn type.** `TypeKind::Fn` stores `Vec<TypeRef>`. Fix: change to
  `ThinVec<TypeRef>` to avoid the intermediate conversion.
- **`expr.rs:404-410, 474-480` : const-name extraction duplicated across
  `AssignExpr` and `RefExpr` arms.** Fix: extract into helper `fn const_name(&self, e: ExprId)`.
- **`body.rs:361` : `Hash` derived on `Literal` but never used as a key.**
  Remove `Hash` derive.
- **`collect.rs:86` : zero-capacity `FxHashMap` placeholder allocates map
  metadata.** Fix: use `&[]` sentinel or `Option`.

### D. crates/typeck/src/infer/ : typechecking walker

- **`infer/mod.rs:638-642` : `Vec<(ExprId, &str)>` collected then immediately
  iterated.** In the `println` argument check, `args.iter().skip(1).filter_map().collect()`
  creates a `Vec` only to loop over it. Fix: emit directly in the filter_map.
- **`infer/coerce.rs:216-219` : `elems.to_vec()` on array-literal coercion.**
  Clones every element of the array into a new `Vec` to break borrow on
  `self.body`. Fix: use `SmallVec<[ExprId; 4]>`.
- **`infer/coerce.rs:286-289` : `adopt_int_literal` does interner lookup
  before cheap body-expr check.** For *every* expression at coercion, it
  calls `types.lookup()` (hash + string match) when the expression isn't
  even a literal 99% of the time. Fix: check `self.body.exprs[id]` first.
- **`infer/mod.rs:345` : `n.clone()` of struct name per struct literal.**
  Clones the entire `Text` out of the interner just to read it. Fix: use
  `&Text` reference.
- **`infer/mod.rs:531` : `arm_expected.clone()` per match arm.**
  `Expectation::clone()` may clone `Cause::Field { name: Text }`. Fix:
  take `&Expectation` and clone only at the fork point.

### E. crates/effect/src/ : effect system

- **`lib.rs:72` : `hir.functions.iter().collect::<Vec<_>>().into_par_iter()`.
  Intermediate `Vec` from full function map.** Standard rayon pattern but
  allocates per file. Minor.
- **`lib.rs:221-225` : `Vec<String>` allocated only for `.join(" -> ")`.**
  3-element witness trail formatted into individual `String`s then joined.
  Fix: `Iterator::intersperse` or manual fold.
- **`lib.rs:291` : `chain.insert(0, name.clone())` is O(n) per call for
  deep call chains.** Fix: push to end, reverse at base case, or use
  `VecDeque::push_front`.
- **`lib.rs:311-321` : per-function `FxHashSet` for callee dedup in fixpoint.**
  For functions with few callees, HashSet allocation dominates the small
  work. Fix: sorted `SmallVec` + dedup.
- **`judge.rs:100-106` : `cx.scope.functions[*fid]` indexed twice.**
  Fix: bind to `let func = &cx.scope.functions[*fid];`.
- **`judge.rs:100` : duplicate `FnId` entries in `callees` Vec.** Same fn
  called N times stores N copies. Fix: use `FxHashSet<FnId>` internally.
- **`lib.rs:149-153, 305-309` : duplicate `FxHashMap<FnId, usize>` index
  built in both `check_contracts` and `run_fixpoint`.** Build once, pass
  as parameter.
- **`lattice.rs:106-120` : `describe()` allocates heap `Vec` for ≤3 static
  `&str` items.** Fix: match on bits directly or use fixed `[&str; 3]`.

### F. crates/mir/src/lower/ : MIR lowering

- **`stmt.rs:31-32` : `Vec<(Text, HirLocalId)>` collected only to hand to
  `lower_let_destructure`, which immediately iterates. Fix: pass iterator.**
- **`stmt.rs:116` : `base.clone()` inside per-destructure-field loop.**
  The base `Place` is loop-invariant. Fix: clone once outside loop.

### G. crates/codegen/src/core/ : C codegen

- **`mir_emit/mod.rs:219-235` : two intermediate `Vec<FnId>` allocations.**
  Collected extern fns and defined fns into separate `Vec`s then immediately
  iterated. Fix: inline the iteration without collect.
- **`mir_emit/expr.rs:421,423` : `n.clone()` of `&Text` from interner, then
  immediately borrowed via `&struct_name`. Fix: use `&Text` reference.**
- **`mir_emit/expr.rs:390` : `place.clone()` for cache key on every
  `place_type` call.** Deep `Place` tree cloned speculatively for memoization;
  if hit rate is low, the clone cost dominates. Verify hit rate.
- **`mir_emit/expr.rs:224-229` : `decode_string_literal(s)` called again
  in `gen_string_statics` for same strings.** Fix: cache decoded bytes in
  the string table.

### H. crates/codegen/src/core/typegraph.rs : type dependency graph

- **`typegraph.rs:146-162` : `nominal_field_types` allocates fresh `Vec<TypeRef>`
  per call.** Fix: accept `&mut Vec<TypeRef>` buffer and extend.
- **`typegraph.rs:317-318, 406-407` : per-node fresh `Vec` + `HashSet`
  allocations in `topo_order` and `compute_scc`.** For N nodes, 2N heap
  allocations. Fix: allocate once, clear each iteration.
- **`typegraph.rs:75-78` : `params.clone()` (entire `Vec<TypeRef>`) for every
  Fn type node.** `TypeRef` is `Copy` but the Vec allocation per node adds up.
  Fix: consider `ThinVec` or interning.
- **`typegraph.rs:508-518` : `cyclic_nodes` builds full graph then filters
  to a set.** Double work. Fix: add method on `SccInfo` yielding cyclic
  nodes as iterator.

### I. crates/diagnostics/src/ : diagnostic infrastructure

- **`lib.rs:203` : `into_diags` takes `Sink<T>` by value, forcing clones
  at every call site.** Change to accept `&Sink<T>` and push into caller's
  `Vec<Diag>` to allow zero-copy error paths. (Applies to `main.rs:39,62,91`
  and `database/lib.rs:408,414`.)

## Iterator / collection misuse patterns

All found by combing every file for non-idiomatic or wasteful iterator and
collection access patterns.

### `.nth(0)` instead of `.next()` - x FIXED 2026-06-18 (clarity sweep)

- x **`crates/xtask/src/main.rs`** : root-cause fixed - the `gen_struct`
  accessor template now emits `.next()` when `pos == 0`, `.nth(n)` otherwise.
  `cargo xtask codegen` regenerated `generated.rs`: a clean 6-line diff (the 6
  first-child accessors `nth(0)` -> `next()`), nothing else, grammar was in sync.
  Verified workspace + corpus green.

### `.len() == 0` / `.len() > 0` instead of `.is_empty()`

- **`crates/lexer/src/interner.rs:92`** : `self.len() == 0` used in
  `Interner::is_empty()`. The explicit method body is fine, but `self.len()`
  is O(1) here so this is stylistic.
- **`crates/lexer/src/source.rs:108`** : `self.source.len() == 0` in
  `SourceText::is_empty()`. Same as above (stylistic).

### `.to_vec()` on `&[T]`

- **`crates/typeck/src/infer/coerce.rs:217`** : `elems.to_vec()` on the
  slice of array-literal elements. Clones every `ExprId` into a new heap
  `Vec` only to break the borrow on `self.body`. For a large literal this
  allocates N elements. Fix: `SmallVec<[ExprId; 4]>` so small arrays avoid
  the heap.

### `for i in 0..xs.len()` (C-style index loop) vs `for x in xs`

- **`crates/parser/src/event.rs:129`** : `for i in 0..events.len()` then
  indexes `events[i]`. This is legitimate (the algorithm needs random access
  with `std::mem::replace`), so no change. Noted as the only C-style index
  loop in the hot path.

### `.collect::<Vec<_>>().into_par_iter()` : intermediate `Vec`

- **`crates/effect/src/lib.rs:72`** : `hir.functions.iter().collect::<Vec<_>>()
  .into_par_iter()`. The `collect` allocates a `Vec` of every function entry
  to fan out. Standard rayon pattern but pays the allocation. Consider
  `par_iter()` on the underlying collection if it supports it, or accept
  as a known cost.

### Redundant Text clones from the type interner

(x = cleaned 2026-06-18 clarity sweep; verified workspace + corpus green.)
- x **`crates/codegen/src/core/mir_emit/expr.rs`** : `field_type` is a read-only
  `&mut self`, so `&self.hir.types` and `&self.hir.items` coexist - the
  `struct_name` clone was pointless ceremony, now a `&Text`.
- x **`crates/typeck/src/infer/ty.rs`** : `byte_pun` closure returns
  `Option<&str>` (`n.as_str()`), comparing directly - no clone.
- x **`crates/hir/src/core/lower/expr.rs`** : the struct-literal arm computed the
  type name TWICE (`name_union`, `name_path`, identical) - merged into one
  `lit_name`, one clone (the clone is needed: it releases the `self.types`
  borrow for the `self.hir`/`self.emit` reads below).
- ~ **`crates/typeck/src/infer/mod.rs`** : the struct-literal `struct_name` clone
  is KEPT - the ledger mis-flagged it; the clone deliberately releases the
  `self.types` borrow so the later `&mut self` walk can run (a `&Text` would not
  outlive it).

### `.clone()` on every diagnostic emit path

- **`crates/database/src/lib.rs:409,413,421`** : `.extend(lower_fn(...)
  .lowered.diagnostics.clone())`, `.extend(typeck_fn(...).diagnostics
  .clone())`, `.extend(checked.effect_diagnostics.clone())`. Each clone
  copies every diagnostic entry. Fix: consume the results since they are
  not reused.

### `.collect::<Vec<_>>().join()` : intermediate `Vec<String>`

- **`crates/effect/src/lib.rs:224`** : `format!("\`{n}\`").collect::<Vec<_>>()
  .join(" -> ")`. Allocates a `Vec<String>` (heap per element) only to join.
  Fix: `Iterator::intersperse` or manual fold. (Already flagged.)

### `.to_owned()` on source text copied per query

- **`crates/database/src/lib.rs:287,294`**, **`crates/lsp/src/highlight/
  mod.rs:113,134,153,215`**, **`crates/lsp/src/server/notifications.rs:67`**,
  **`crates/lsp/src/server/requests.rs:35`** : all call
  `file.text(db).to_owned()` to produce an owned `String` for `SourceText`.
  The file text is already owned somewhere in salsa's storage. Share via
  `Arc<str>` to avoid the N+1 copies (one per query that reads it).

### Unnecessary `match` on known-constant values

- **`crates/effect/src/judge.rs:118-125`** : `atom_index(atom)` called
  inside a `for atom in [Atom::Io, Atom::Ffi, Atom::State]` loop, where
  `atom_index` is just a hardcoded `match { Io => Some(0), Ffi => Some(1),
  State => Some(2), _ => None }`. For the three live atoms the indices are
  known at compile time. Fix: use literal indices `[(0, Atom::Io), (1,
  Atom::Ffi), (2, Atom::State)]`.

### `unwrap_or(Default::default())` could be `unwrap_or_default()`

No instances found in the hot path (codebase already uses
`unwrap_or_default()` idiomatically).

### `.chars().next()` (fine : no simpler alternative)

- **`crates/token/src/lib.rs:308,325,334`** and **`crates/lexer/src/
  lib.rs:118`** and **`crates/hir/src/core/lower/const_eval.rs:384`** : all
  use `chars().next()` to peek the first char of a `&str`. This is the
  canonical API (no `str::first_char()` exists). No change needed.

### `.as_slice().last()` : unnecessary qualification

No instances found.

### `.map(|x| x.clone()).collect()` instead of `.cloned().collect()`

- **`crates/hir/src/core/lower/expr.rs:300`** : `.map(|&fid| self.hir
  .fields[fid].name.clone())`. Not a simple `.cloned()` case (the map
  dereferences `fid` and projects a field), so this is legitimate.

### Double `.clone()` chain

- **`crates/hir/src/core/lower/const_eval.rs:228`** : `self.memo
  .insert(name.clone(), value.clone())` clones both key and value.
  If `memo` is a `HashMap<Text, ConstValue>`, the insert will clone
  the key anyway if not found. The double clone is justified (need
  ownership for the insert, key/value not reused). Context-dependent.

### `.extend(collection.clone())` : cloning entire collections

- **`crates/database/src/lib.rs:409,413,421`** : see above.
- **`crates/typeck/tests/judgments/main.rs:29`** : `hir.diagnostics
  .extend(typeck[&fn_id].diagnostics.clone())`. In tests: acceptable.

### `use std::collections::HashMap` vs `FxHashMap`

No instances found (codebase consistently uses `FxHashMap`/`FxHashSet`
from `rustc_hash`).

### `Result::ok()` vs `match`

No idiomatic issues found (Result handling is clean).

## Industry cross-references (2026-06-18)

Research into production compiler techniques (TinyCC, rustc, GCC, Clang,
LCC, LLVM) cross-referenced against Eye's architecture. Each entry cites
the industry precedent and maps it to Eye's current design.

### CST-free parse for batch (NOT viable with current AST design)

- **TinyCC** generates machine code in a single pass. **LCC** (20 KLOC) uses
  its own AST, not a CST.
- **Eye constraint (important)**: The `ast::SourceFile` type is a *lens* over
  a rowan `SyntaxNode` — `ast::Item::FnDef(f)` calls `f.name()`, `f.body()`,
  etc., each of which walks `SyntaxNode` children via `support::children()`.
  No CST = no AST = HIR lowering has nothing to consume. Skipping the CST
  would require a parallel non-rowan AST type or changing `hir::core` to
  accept a different input format. Both are prohibitively large changes.
- **Revised verdict**: Keep rowan for the LSP (incremental reparsing needs
  green-tree diffs). For batch, the existing `build_tree` tombstone writes
  are a real cost (writes to every `Vec<Event>` slot before dropping it) but
  the CST itself is not avoidable without an AST redesign. The real batch
  parse saving is the **Salsa bypass** (entry above): skip the query tracking
  overhead, not the parse itself.

### Salsa query bypass for batch (rustc `-Z no-query-cache`)

- **rustc** has `-Z no-query-cache` for debugging, but always runs through
  the query system even for `cargo build --release`. rustc's queries are
  **much finer-grained** (per-item `type_of`, `predicates_of`) than Eye's
  (per-file `lowered_file`, per-fn `typeck_fn`), so the per-query overhead
  is proportionally smaller. Eye's 6 coarse queries mean the tracking
  infrastructure (revision counter, memo table, dependency edge record, Arc
  wrap, change-detect comparison) is pure overhead on every batch compile.
- **Verdict**: Eye's "bypass for batch" (already in performance backlog) is
  uniquely well-suited because queries are coarse. No production compiler
  has this exact problem because none uses Salsa-level tracking for 6
  queries. The fix is a `compile_direct()` function that calls the
  underlying pure functions.

### Arena allocation everywhere (compiler consensus)

- **LLVM, rustc, GCC, TCC, LCC** all use arena/bump allocation as the
  primary allocation strategy for IR nodes. Rustc's `ArenaAllocator` and
  `rustc_arena` provide bump-allocated `Vec`-like containers. LLVM's
  `BumpPtrAllocator` is used throughout. **Protobuf arenas** showed 2-5x
  allocation speed. **LCC** uses arenas for every tree node.
- **Eye analogue**: `TypedArena` in HIR/MIR is the right pattern and is
  already in use. The gap is that many *intermediate* data structures
  (diagnostics Sink entries, typegraph `Vec`s per node, inference
  per-expression `Vec` collects) still use heap `Vec`/`String`. Extending
  arena allocation to these would eliminate per-element allocation overhead.
- **Specific targets**: `Sink<T>` entries (diagnostic arena for the
  compile-session lifetime); `typegraph.rs` per-node `raw` Vecs (reuse
  arena); `infer_expr` throwaway `Vec`s (arena-allocate these or use
  `SmallVec` on the stack).

### Zero-copy diagnostic pipeline (Clang model)

- **Clang** builds `Diagnostic` objects incrementally without cloning.
  Diagnostics are stored in a `DiagnosticEngine` that owns the entries.
  The driver reads them by reference. No clone-from-memo pattern exists.
- **Eye analogue**: the pattern throughout the hot path (`main.rs:39,62,91`,
  `database/lib.rs:409,413,421`) is `.clone().into_diags()`. Every
  diagnostic sink is cloned behind a `Memo` reference. Fix: change
  `into_diags` to accept `&Sink<T>` and push into a caller-owned
  `Vec<Diag>`, or make the driver collect diagnostics by reference.
  Alternatively, make `Sink` `Copy`-cheap via an internal `Arc<Vec<T>>`.

### Preallocated codegen output (GCC/LLVM buffer model)

- **GCC** and **LLVM** both preallocate output buffers based on estimated
  size. LLVM's `raw_ostream` uses a `SmallVector<char, 256>` that grows
  geometrically but starts with a reasonable capacity. GCC's `print-tree`
  etc precompute line counts.
- **Eye analogue**: `codegen::core::mir_emit::gen_all` writes into a bare
  `String` with no capacity hint. For a 1000-line C output this reallocates
  ~10-12 times (doubling strategy). Pre-estimating from `hir.functions.len()
  * avg_lines_per_function + type_declarations` and calling
  `String::with_capacity` would eliminate these reallocation cycles.

### Name resolution caching (rustc Resolver pattern)

- **rustc** runs name resolution as a dedicated pass (`Resolver`) that
  produces a `ResolveResult` with every name mapped to its `DefId`. The
  HIR lowering context reads from this pre-computed map rather than doing
  per-name sequential lookups.
- **Eye analogue**: `LoweringCtx::resolve` (ctx.rs:151-176) does 7
  sequential `FxHashMap::get` calls per identifier. While Eye's name
  spaces are simpler than Rust's (no traits, no generics, no modules), the
  pattern still does 7 probes per ident reference. A pre-computed
  `FxHashMap<Text, Resolution>` per scope would reduce this to one probe.

### MIR bypass for simple functions (TCC/LCC direct codegen)

- **TCC** generates code per-statement without an IR. **LCC** uses a
  shared `IR` node tree but generates code for each function independently.
  Neither has a CFG-based MIR equivalent.
- **Eye analogue**: functions without control flow (simple getters,
  wrappers, const initializers) could skip MIR lowering entirely. The
  codegen already has `gen_function` which could directly consume HIR for
  simple cases. This is a micro-optimization but costs little to implement:
  a `fn is_trivial(&self) -> bool` on `Body` that checks for no branches,
  and a direct `hir_to_c` path in codegen.

### Effect inference fixpoint optimization (dataflow analysis precedent)

- **LLVM's `AAResults`** and **GCC's `dataflow`** both use worklist-based
  fixpoint algorithms with early-exit when the lattice stops changing.
- **Eye analogue**: `effect::run_fixpoint` already uses SCC condensation
  + worklist, which is the standard algorithm. The waste is in building
  the same `FxHashMap<FnId, usize>` index twice (`check_contracts` and
  `run_fixpoint` both compute it). Fix: compute once, pass as parameter.

### Generated AST accessors: nth(0) vs next() (Rust API convention)

- **Rust API guidelines** (and Clippy lint `iterator_nth_zero`) specify
  that `.next()` is preferred over `.nth(0)` because `.next()` is more
  idiomatic and may be slightly more efficient (avoids the `nth` default
  implementation's `nth` -> `next` delegation via `advance_by`).
- **Eye analogue**: `crates/xtask/src/main.rs:219-222` generates
  `support::children(&self.syntax).nth(#idx)` for all indexed fields.
  When `idx == 0`, this should emit `.next()` instead. 6 generated
  accessors currently emit `nth(0)`.

- [ ] Error-message tests: e2e only checks successful execution; add tests
      asserting specific diagnostics for malformed programs at the driver
      level (the HIR unit tests already assert many).
- [ ] Graphviz visualisation flag for internal structures (low priority;
      make it aesthetic).

---

## Roadmap markers

Tracked here as pointers; the substance lives in DEFER.md / MASTERPLAN.md.

- const completion: aggregate const values, sizeof-tainted const-symbol
  path, declared-type check (DEFER "const sub-deferrals"). When aggregate
  values land, add the U1 regression test: an array-wrapper typedef whose
  sole reference is a const/global type must appear in the C output.
- match S4/S5: or-patterns, ranges, struct-patterns-in-arms,
  usefulness/exhaustiveness (literal dups, out-of-range arms, negative
  literal patterns) - sequenced after typeck (DEFER "Match").
- `alignof` (DEFER "sizeof sub-deferrals").

---

## Source-comment register

Every in-source issue marker, so none float free of the ledger:

| location                                      | ledger row                                                                                         |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `eyesrc/programs/lang.eye` top-of-file FIXME  | readable-C mode (design questions)                                                                 |
| `eyesrc/programs/lang.eye` `off off` FIXME    | typeck scope: field/arg value types + cast lattice CLOSED 2026-06-16 (T37/T38/T43); tail-expr open |
| `eyesrc/programs/lang.eye` `onset_head` FIXME | typeck scope: tail-expression type enforcement                                                     |

---

## Completed

### 2026-06-18: T046 latent bug - string indexing rejected

The `string` type alias (`&uint8`) was interned as `TypeKind::Path("string")` —
a plain path name, not a structural `Ref(uint8)`. The typeck `index_judgments`
match only allowed indexing on `Array`, `Ref`, and `Ptr` structural variants;
the `_` catch-all emitted T046 (`IndexOfNonIndexable`) for any `Path` type,
including `string`. Two e2e tests (`string_eye_byte_array_refs`,
`caesar_eye_string_decay_and_ffi`) were silently broken.

Fix: `crates/typeck/src/infer/judgments.rs:937` — added
`TypeKind::Path(n) if n == "string" => {}` to the indexing-ok match arm.
Same blind spot found and fixed in the deref judgment at `infer/mod.rs:463`
— `string` now resolves to `uint8_ty()` on dereference. All tests green
(75 hir, 81 typeck, 73 e2e), clippy clean. Analysis in item 5 below.

### 2026-06-16: S6 parallel wave - lock-free interner + whole-file fan-out

The "parallelised" half of dual inference. Built on the validation spike below.

- **Lock-free interner.** `TypeInterner` internals: `Vec`+`FxHashMap`/`&mut self`
  -> `boxcar::Vec` (lock-free append, stable addresses) + `papaya::HashMap`
  (lock-free dedup), `intern(&self)`. Stable arena addresses keep
  `lookup(&self) -> &TypeKind` (a `Mutex`/`RwLock` interner could not). Tolerated
  race: concurrent same-kind interns elect one canonical index via
  `get_or_insert`; the loser's slot is dead, never a second handle for one type.
  Deterministic `Debug` (arena in handle order; the dedup map's lock-free
  iteration order is excluded - non-deterministic).
- **Clone elimination.** The two per-body interner clones are gone
  (`lower_fn_body`'s `scope.types.clone()`, `typeck_fn`'s
  `lowered.lowered.types.clone()`); bodies intern into the one shared scope
  interner via `&self`. `LoweredBody.types` / `FnLowerOut.types` deleted; the
  whole-file take/restore dance deleted (`check_file`, `collect_results`,
  `infer_file` take `&HIR`). `&mut TypeInterner` -> `&TypeInterner` across
  hir-lowering / typeck / effect. Interner stays homed in `HIR` (not lifted to
  the `Database` - a documented divergence; HIR-home suffices for sharing).
- **Whole-file fan-out (Wave 1).** `collect_results` runs the fused type+effect
  walk one task per body across `rayon` (`into_par_iter`), interning into the
  shared interner with no shared mutable state. Order-preserving collect keeps
  determinism law #1.
- **Determinism gate (law #2).** Corpus C is byte-identical across 8
  separate-process runs each for histogram/calculator/design/floodfill/raytracer
  (codegen emits typedefs in program-discovery order, never by handle value).
  Regression test `parallel_inference_is_deterministic` (6 fresh databases,
  equal C).
- **Scoped out (documented):** the per-fn `hir_diagnostics` fan-out (the S5
  firewall already makes that path one-body-incremental - parallelizing
  mostly-cache-hits is low value for high `&dyn Database` churn) and Wave 0
  parallel lowering (`lower_source_file` stays serial; body arena alloc needs
  `&mut`, and lowering is cheap next to the walk).

Verified: workspace 375/0 + the new determinism test, clippy clean, snapshot
re-accepted (cleaner handle-ordered interner dump). `rayon`+`boxcar`+`papaya`
added.

### 2026-06-16: S6 checkpoint 1 - parallel validation spike

The version-sensitive question (does salsa 0.27's parallel-snapshot API support
the per-body fan-out the wave needs?) answered empirically before any interner
rewrite. Result: `salsa::Database: Send` (not `Sync`), so the model is owned
database _clones_ - cheap `Storage` handle bumps onto the shared, internally
synchronized memo tables - moved into workers via rayon `into_par_iter`;
interned ids and salsa inputs are valid across clones of the same storage. The
per-fn `typeck_fn` query is already seal-isolated (its own interner clone), so
it parallelizes with **zero** interner change and is trivially deterministic
(no shared whole-file interner = no `TypeRef`-handle-order dependence).

- `rayon` added as a workspace + `eye-database` dependency.
- `database::tests::parallel_per_fn_typeck_matches_serial`: clones the db per
  body, fans `typeck_fn` out across rayon, asserts the parallel diagnostics
  (in collection order) equal the serial loop's - determinism law #1.
- Diagnostics are safe to compare across runs because they carry baked-in
  display strings + type _names_, not raw `TypeRef` handles.

Verified: `cargo test -p eye-database` 11/0 (+1 spike), clippy clean. No
production path changed yet - the spike is test-only.

REMAINING S6 checkpoints (not built): lock-free interner (`boxcar`+`papaya`,
`&self` intern, kills the per-fn clone + the whole-file take/restore), the
production per-fn fan-out (needs the concrete `Database` threaded or a fork on
the `&dyn` path), the whole-file fused fan-out (blocked on the lock-free
interner - shared `&mut hir.types`), and the corpus-diff-twice determinism gate.

### 2026-06-16: S5 signature firewall - structural backdating

The incremental half of dual inference: a body edit no longer re-checks sibling
bodies. `Memo`'s blanket `Arc::ptr_eq` `PartialEq` became a `MemoEq` trait
(default `false`, the old conservative behavior), overridden for the two
firewall results by a **content digest** rather than a deep `PartialEq` -
correct-by-construction because lowering is deterministic and `Text` is an owned
`SmolStr` (no interner-id drift across edits), and cheap.

- `database/lib.rs`: `MemoEq` trait + `impl<T: MemoEq> PartialEq for Memo<T>`.
  `FileScope` gains `sig_digest` (hash of every item with fn bodies excluded -
  stable across a body-only edit); `lower_fn` now returns a `LoweredFn { lowered,
digest }` whose `digest` = this body's text combined with `sig_digest`.
  `signature_digest`/`body_digest`/`hash_text_range` hash text _content_ (not
  byte offsets, so an edit shifting later items leaves their digest unchanged).
- Effect: edit one body -> `item_scope` backdates (signatures equal) -> the
  unedited bodies' `lower_fn` re-runs but backdates -> their `typeck_fn`
  cache-hits. Keystroke cost = reparse one file + recheck one body + effect
  fixpoint, flat in project size (was O(functions)).
- `database/tests.rs`: `body_edit_backdates_the_sibling_typeck` (alpha's
  `typeck_fn` returns the same `Arc` when only beta's body changed) +
  `signature_edit_reruns_every_body` (a const-initializer edit busts every
  body's cache - the staleness guard). `database_eq` switched to `Arc::ptr_eq`
  for the within-revision identity checks (Memo's `==` is now a digest test).

Scope: the _signature-firewall_ half of salsa structural backdating. Still
conservative (open, ledgered): token-stream/`lex`/`parse`/whole-file backdating,
and the end-state per-item `fn_signature` queries (multifile milestone).

Verified: `cargo test -p eye-database` 10/0 (+2 firewall, both directions);
full workspace green (e2e 71, hir 74, all suites 0-fail), corpus `--check` sweep
44/2-XFAIL (0 new rejections through the changed `hir_diagnostics` path),
strict-C 42/42, clippy clean. The firewall changes only _when_ queries re-run,
not their values, so codegen output is unchanged (e2e confirms).

### 2026-06-16: S3 complete - F1/F2/F3, L4, cast lattice, U2/U4

The remaining S3 judgments, closing the segment (only M2b stays open, ratified
as deferred until a corpus program needs the strict-width ruling). New error
codes T40-T43 and C13; CAST.md added for the lattice ruling.

- **F2** `NegationOnUnsigned`/T40 - unary `-` on an unsigned value (wraps in C)
  rejected; `~` stays legal (Rust parity); negated literals exempt. Unary arm.
- **F3** float-literal width adoption - `adopt_float_literal` in `site_coerce`,
  the float analogue of `adopt_int_literal`. No new rejection (a stamp); tested
  observably through L4 not false-flagging a `[float32; N]` literal.
- **F1** `IfBranchTypeMismatch`/T41 - value-position `if` branch consistency,
  the `if` analogue of `check_match_arm_consistency`. `discarded_set` factored
  out of the assignment check and shared (statement/discarded `if` stays legal).
- **L4** `ArrayElementTypeMismatch`/T42 - per-element value judgment in
  `coerce_array_literal`, the same `site_assignable` as arg/field. Surfaced the
  string->char* decay gap (lang.eye `[char*; 24]`); recorded as a follow-up.
- **cast lattice** `CastNotAllowed`/T43 - `as` no longer any-to-any
  (`cast_allowed`/`cast_class`, CAST.md), built to the ratified directional
  ruling: numeric<->numeric, pointer<->pointer, integer<->pointer, and the
  tagged scalars (`char`/`bool`/`enum`) widen OUT to an integer only. `_ -> bool`
  / `_ -> char` / `int -> enum` / float<->pointer / aggregate / fn reject;
  `Unknown` stays lenient. Corpus uses none of the rejected pairs (the two
  `as char` are `as char*`).
- **U2** `ConstValueOutOfRange`/C13 - const/global folded integer range-checked
  against the declared type (`const_eval::check_const_range`).
- **U4** `apply_cast` reproduces the C cast (`wrap_int`): an integer target
  truncates to its width, so a folded const equals its runtime value and
  composes with U2 (explicit `as` = blessed truncation).

Verified: `cargo test -p eye-typeck` 64/0 (8 new); corpus `--check` sweep 44
programs, 2 rejects (lang.eye + linkedlist.eye, the pre-existing XFAILs) - zero
new program rejections. Full workspace gate (U4 changes folded const values, so
codegen-affecting): workspace green (e2e 71, hir 74, judgments 64, all suites
0-fail), strict-C 42/42, clippy clean. The cast lattice was first built
symmetric, then tightened to the ratified directional ruling after confirming
the corpus uses none of the now-rejected pairs (re-verified: e2e still 71/0).

### 2026-06-16: S3 first judgments - call-argument + struct-field type checks

The S3 judgment pass's first two customers, unblocked by the C5 shadow-oracle
retirement (the oracle required walker types to mirror lowering's; with it gone,
the walker can reject). Both were headline typeck-scope holes: swapped/wrong-type
call arguments reached clang (only arity, T026, was checked) and `P { x: "hi" }`
with `int32 x` converted silently (only missing/unknown fields were caught).
**Verified: workspace 362/0, strict-C 42/42, grammar parity green; zero new
corpus rejections (driver `--check` over all 44 `.eye`: only lang.eye +
linkedlist.eye reject, both pre-existing XFAILs - the float-heavy
montecarlopi/statistics/raytracer all still compile).**

- `hir/errors.rs`: `TypeError::ArgTypeMismatch { index, expected, found }` (T37)
  - `StructFieldTypeMismatch { field, expected, found }` (T38) - variants,
    Display, Code.
- `typeck/infer.rs`: `site_assignable(expected, found, types)` - the
  coercion-site acceptance predicate. Accepts an equal/integer-family-compatible
  type, the `&[T; N] -> &T` / `string` decay, and any pointer-shaped value
  widening into the untyped `ptr` (the FFI escape). Wired into `infer_call`'s
  defined-fn arm (per argument, after `site_coerce`) and the `StructLit` arm
  (per field). Variadic-extra args (no parameter) and the `println`/intrinsic
  Unresolved arm stay unchecked.
- Lenient on int-family (defers the M2b strict-width rule) and pointer->`ptr`,
  matching every other coercion site, so the blast radius is bounded to genuine
  cross-family mismatches.
- `typeck/tests/judgments.rs`: 3 tests (arg mismatch incl. two-wrong-args, arg
  correct + `&[T;N]` decay + ptr-escape clean, struct-field mismatch).

### 2026-06-16: S3 assignment-non-value

The ruled assignment-non-value judgment (`AssignInValuePosition`, T39): a
value-position `x = y` / `x += y` - a `let` initializer, an argument, a
condition, an operand, a value-producing branch tail - is rejected (`if x = y`
is the canonical `==` typo footgun). Statement position and discarded (void)
tails stay legal. **Verified lean (a pure new diagnostic): `cargo test -p
eye-typeck` judgments 56/0, driver `--check` sweep over all 44 `.eye` rejects
only lang.eye + linkedlist.eye (the pre-existing XFAILs) - zero new rejections.**

- `typeck/infer.rs`: post-walk `check_value_position_assignments` +
  `mark_discarded`. The discarded set is seeded from the statement arena (every
  `Stmt::Expr` discards its expr, nested blocks included) plus a void fn's body
  tail, then propagated through the tails of discarded `if`/`block`/`match`
  expressions - so an `if c { x = 1 } else { y = 2 }` _statement_ keeps its
  branch-tail assignments legal while the same shape as a `let` initializer is
  rejected. The Assign arm drops its `PARITY(S3)` marker (still types as the rhs,
  unused for a rejected program).
- `typeck/tests/judgments.rs`: 2 tests (value-position let-init reject;
  statement-position bare/compound/if-statement-branch-tail clean).

Remaining S3: the cast lattice, M2b strict-width, F1 value-`if` / F2
unary-unsigned / F3 float-literal, L4 cross-element, U2/U4 const-eval range/cast.

### 2026-06-16: EXPERIMENTAL tag audit + cleanup

Repo-wide audit of `EXPERIMENTAL` markers; removed stale/junk, kept genuinely
provisional (all comment/doc-comment-only, no semantic change). Removed: 7
`vamous` junk markers (parser sync-sets + typeck ICE comments - settled code);
7 LSP module `//! EXPERIMENTAL` headers (the salsa-backed LSP shipped); A2
`place_type` memoization ratified (`DONE (EXPERIMENTAL)` -> `DONE`, kept the
`A2:` cross-ref). Demoted to plain doc-comments (shipped + load-bearing, not
trials): the typed-arena infra (hir + mir, 8 sites) and the StringTable /
query-pipeline trait (lexer + syntax + hir, 7 sites; the stability caveat
reworded "provisional"). Kept: U1 (untested, ledger-tracked), the
destructure-test caveat (accurate for the live S3-S5 work), the guard notes
(complex-guard redesign still open), MEM.md (in-progress design). `VFS.md`'s
`features = ["experimental"]` is a cargo feature name, not our tag.

### 2026-06-16: dev build profile - cranelift dropped, native knobs

The cranelift codegen backend cannot emit the `__mod_init_func` static-init
section salsa's macros generate on macOS, so it failed `eye-database` and every
downstream crate. Reverted; the dev profile now uses stable-toolchain-safe
knobs: `codegen-units = 256`, `debug = 1`, and `split-debuginfo = "unpacked"`
(skips the slow macOS `dsymutil` pack step). No backend swap, nothing for
salsa's macros to trip over.

### 2026-06-15 (later): D - `typeck_fn` per-fn salsa query (LSP hover deferred)

The per-fn type-check query the cutover left as the last inference step. Sealed-
body inference means no type fact crosses a function boundary, so a body types
independently - `typeck_fn(db, StableFnId)` runs `typeck::check_body` over one
`lower_fn` body on its own interner clone, keyed by `StableFnId` so a body edit
re-runs only that query (clean siblings cache-hit). `hir_diagnostics` now sources
type-judgment diagnostics per-fn from `typeck_fn` (replacing the whole-file
`lowered_file.typeck` loop), restoring per-body granularity on the diagnostics
path. **Verified: workspace 357 (+1) / 0, clippy clean, grammar green, strict-C
41/41**; new `typeck_fn_localizes_type_diagnostics` db test.

Scope notes: `lowered_file` stays whole-file for the codegen path (codegen
compares `TypeRef` across bodies, needs one shared interner; `typeck_fn`'s
handles resolve through the per-body interner and are not cross-body comparable).
Effect-contract diagnostics (`E` class) also stay whole-file - the effect verdict
is a whole-program fixpoint. So `hir_diagnostics` still calls `lowered_file` for
effect diagnostics (a transient double-typeck on that path); decoupling effects
into a per-fn atom query + a cheap fixpoint query, and wiring the **LSP hover**
handler (none exists yet), are the remaining D work, deferred to the LSP-latency
push per the user.

main fast-forwarded to the dual-inference work (`8ddf403`) before this step.

### 2026-06-15 (later): C5 cutover COMPLETE - lowering stops typing, shadow retired

The dual-inference cutover is finished. Lowering no longer types any expression

- the `Body.expr_types` field is **deleted** - and the typeck pass is the sole
  source of expression types for MIR/codegen. **Verified: workspace 357 / 0,
  c_codegen snapshot byte-identical, clippy clean, grammar parity green, strict-C
  41/41.** (357 < the prior 368 because the shadow-oracle tests + the lowering
  expr-type-stamp tests were removed/migrated, not regressions.)

Done as two increments (the recovery commit `fce7ec5` "Point of no return"
preceded the irreversible work):

- **Increment 1 (shadow stayed green):** relocated the last type-directed
  _structural_ read in lowering - `CallNonFunction` (an indirect call through a
  non-pointer value) - to typeck's `infer_call`.
- **Increment 2 (the irreversible cut):**
  - Deleted `Body.expr_types` (compiler-guided). Gutted `lower_expr`'s entire
    bottom-up type computation + the per-arm stamps; deleted `coerce.rs`
    (coerce + array-literal/int-literal re-typing) and its call sites,
    `record_match_result_override`, `block_tail_type`, the dead lowering helpers
    (`lookup_field_type`, `peeled_array_len`, `literal_type`).
  - Relocated the remaining type-directed judgments to typeck: `LenNotArray`
    (Expr::Len), `LenFieldOnArray` (`.len` on an array), `PrintCannotFormat`
    (a compound `println` arg). `LenNotAPlace` (structural) stays in lowering.
  - `mir_type_of`'s A3 fallback flipped from the error sentinel to an **ICE** -
    a miss is now a compiler bug, not bad input (codegen only runs on a
    diagnostic-free program, where the walker is total).
  - Deleted `crates/typeck/src/shadow.rs` + `tests/shadow.rs` + `pub mod shadow`
    - the corpus-wide parity oracle. `corpus_generates_no_error_type` (e2e) is
      the remaining completeness guard.
  - **Architecture amended** where the old design coupled lowering to types: the
    codegen type-declaration topology (`typegraph::topo_order`) no longer reads
    `body.expr_types`; it takes a seed of whole-file typeck expr types
    (`typeck::expr_type_seed`) threaded through `gen_mir`, so array/fn-ptr
    wrapper typedefs for intermediate values (a string literal's `&[uint8; N]`,
    an array-literal argument) are still discovered. `--dump-hir` now shows HIR
    structure only (no type column).
  - Tests reading `body.expr_types` migrated to `crates/typeck/tests/judgments.rs`
    or rewritten structural; the two HIR dump snapshots re-accepted (expr-type
    column / `expr_types` field gone).

Cutover C1-C5 DONE + S0 + effects + C2/C4. The walker still carries the
`PARITY(S3)` rules (binary = lhs type / M2, assign = rhs, etc.); with the shadow
oracle retired, the **S3 judgment pass** is now unblocked to fix them (it was the
oracle's whole payoff). Remaining inference work: S3 judgments, D (typeck_fn
salsa query + LSP hover, additive), S5 firewall, S6 PARALLEL wave.

### 2026-06-15 (later): C2 match analysis relocated BUILT (net-up, byte-identical)

Cutover step C2 done, by the rustc/rust-analyzer model rather than the ledger's
original "defer everything + pat-resolution table" sketch (the user ruled this
the more correct, footgun-free path). **Verified: workspace 368 / 0, c_codegen +
mir_dump + hir_dump snapshots unchanged, clippy clean, grammar parity green,
strict-C 41/41.**

Key reframing: bare-ident pattern classification is a NAME-resolution question,
not a type question. A bare ident is a variant iff the name is in the flat
item-scope variant index (`ItemScope::variants`), else a binding. Type-free, so
it stays in lowering and the ordering problem (a binding must be in scope for the
arm body, which lowers in the same pass) dissolves - lowering defines the binding
exactly when the name is not a variant. Only the judgments that genuinely need
the scrutinee type move to typeck.

- hir lowering: `lower_match_pat` is now structural (no `scrut_enum` param);
  bare ident -> `Pat::Variant` (known name) or `Pat::Bind` (else, local typed
  `None`). `lower_match_expr` gutted to structural arm lowering; `match_domain`,
  `MatchDomain`, `ArmPatShape`, `literal_pat_text`, `is_int_type_name` removed.
  `MatchArm` gained `ptr` (the arm span) so typeck anchors arm diagnostics
  byte-identically. Lowering's match code now reads zero `expr_types`.
- typeck: new `check_matches` post-walk pass owns MatchScrutineeNotEnum,
  PatternEnumMismatch (now also a bare variant of the wrong enum),
  PatternDomainMismatch, DuplicateArm, UnreachableAfterWildcard, NonExhaustive,
  NonExhaustivePrimitive. `local_types: FxHashMap<LocalId, TypeRef>` records a
  bind-arm local's type (the scrutinee's) during the walk; `path_type` falls
  back to it so arm-body references resolve.
- MIR: `Pat` classification unchanged (`arm_kind` reads the same shapes);
  `bind_local_to` falls back to `typeck.local_types` for the binding type. No
  threading, no codegen change.
- Behavior change (intentional, user-approved): a bare ident that is not a known
  variant is a BINDING, not an "unknown variant" error - even over an enum
  scrutinee (a catch-all). The qualified `Enum.Bad` form still errors in
  lowering. `ResolveError::UnknownVariantInPattern` is now never emitted.
- tests: the 11 type-directed match-diagnostic tests + the now-vacuous
  acceptance tests migrated from `crates/hir/src/core/tests.rs` to
  `crates/typeck/tests/judgments.rs` (lowering + typeck); the hir crate keeps
  the structural tests + a qualified-`NoSuchVariant` test; a new
  `match_bare_unknown_ident_is_binding_not_error` pins the behavior change.

Cutover state: C2 + C4 (net-up) DONE. Remaining: CHECKPOINT, then C5
(irreversible), then D (additive).

### 2026-06-15 (later): C4 decay flip BUILT (net-up, byte-identical)

Cutover step C4 done. Array-reference decay moved from a lowering-injected
`Expr::Cast` node to a typeck read adjustment MIR applies. **Verified:
workspace 367 / 0, c_codegen snapshot byte-identical, clippy clean, grammar
parity green, strict-C 41/41.** No user-visible change - its only purpose is
to unblock C5.

- typeck: `TypeckResults` gained `adjustments: ArenaMap<Idx<Expr>, Adjustment>`
  (`crates/typeck/src/lib.rs`); `site_coerce` now calls `record_decay`, which
  files `Adjustment::Decay(expected)` when `ty_of(id)` (a `&[T;N]`) decays to
  the expected `&T`/`string` (free fn `array_ref_decays_to`, ported from
  coerce.rs). The three mismatch checks (tail return, explicit return, let-init)
  now accept the decay pairing - they previously passed only because the
  injected `Cast` was typed `declared`.
- hir lowering: `coerce` no longer injects the cast (returns the expr
  unchanged); `maybe_decay` + coerce.rs's `array_ref_decays_to` deleted; the
  now-dead `alloc_expr_with_type` (ctx.rs) removed. coerce.rs module doc and
  `typeck::Adjustment` doc updated (decay is now load-bearing, not inert).
- MIR: `lower_rvalue`/`lower_operand` are adjustment-aware wrappers over
  `lower_rvalue_raw`/`lower_operand_raw`. A `Decay(target)` expr reads its
  undecayed inner via the `_raw` core and emits the same cast the old
  `Expr::Cast` arm did (direct `RValue::Cast` in rvalue position, a spilled
  `target _t = (target)<value>` temp in operand position). `lower_operand_raw`'s
  `_ =>` fallback calls `lower_rvalue_raw` so a decay is never applied twice.
  Shadow oracle stays valid: the cast node is gone from BOTH the walker's and
  lowering's view of the (now cast-free) body, and `adjustments` is invisible to
  the `expr_types` comparison.

Remaining cutover: C2 (net-up), then CHECKPOINT, then C5 (irreversible), then D.

### 2026-06-15: MIR-OPT reconstruction reverted + A3 divergent-expr gap closed

Two verified, net-up changes; the coordinated type-side cutover (C2/C4/C5)
was NOT started this session (handoff below). Baseline after this work:
**workspace 367 / 0, clippy clean, grammar parity green, strict-C 41/41.**

**MIR-OPT reconstruction pass FULLY REVERTED.** The `reconstruct_expressions`
pass (`Operand::Expr` nested-tree codegen for "readable C") broke the strict-C
gate on 7/39 files - all 7 were reconstruction artifacts: extraneous parens
around comparisons inlined into `if` (`functions`/`file`/`floodfill`), a lost
`(void*)` cast on a `%p` arg (`print`), a tautological `B == B` from an inlined
constant scrutinee (`guard_example`), and a `%lld`-vs-`int` mismatch from an
inlined bare literal (`integers`). The pass also contradicted the ratified
"spilled (flat three-address) C is the default" ruling. Removed: `Operand::Expr`
variant + its manual PartialEq/Hash arms + all the mir-opt-only derives
(`RValue` Clone/PartialEq/Hash, MirBody/MirBlock/MirStmt/SwitchArm/Guard/ArmTest
Clone, VariantRef Eq/Hash) in `crates/mir/src/core.rs`; `crates/mir/src/optimize.rs`
(deleted); `pub mod optimize` in mir/lib.rs; the `Operand::Expr` arm in
mir/lower.rs (`place_for_value`); 3 `Operand::Expr` arms + the Println
collect_strings change in codegen/mir_emit.rs; the `-O` CLI flag (cli.rs); the
clone+optimize driver path (main.rs, src/lib.rs - both restored to flat
`gen_mir(hir, mir_map)`, matching the salsa `c_code` query, which was already
flat). The c_codegen snapshot reverted to byte-identical-with-committed-HEAD,
proving typeck/S0/effects changed no generated C. The KEPT codegen changes in
mir_emit.rs (S0 `TypeNode::Fn { variadic }` -> `, ...`) were preserved (the
codegen files mix mir-opt with S0, so no git-checkout - surgical only).

**A3 divergent-expr completeness gap CLOSED** (a real C3 prerequisite the
reconstruction pass had MASKED). A value-position `loop`/`return`/`break`/
`continue` lowers to MIR poison `0`; its temp type came from `mir_type_of`,
which had no `expr_types` entry -> A3 fallback. At the freeze commit A3 fell
back to int32 (valid C); C3 flipped it to `error_type` -> `void* /* ERROR TY */`,
and reconstruction inlined the temp so the bad return never showed. The flat
path exposed `void* _t0 = 0; return _t0;` from an int32 fn (e2e
`value_position_loop_does_not_panic`, the MIR-OPT.md Q2 "Loop never typed"
case). Fix: walker `adopt_divergent` in `site_coerce`
(`crates/typeck/src/infer.rs`) stamps a value-position divergent expr with the
expected type, keeping `expr_types` complete. Shadow-safe (walker-extra stamp;
lowering left these untyped). This is the substantive prerequisite C5 needs to
flip A3 -> ICE.

**HANDOFF - remaining type-side cutover (precise scoping done this session).**
C2 + C4 are net-up (shadow oracle + corpus + strict-C verify, fully
reversible); C5 is irreversible (deletes the corpus-wide parity net). Safe
order: C2, C4 (net up), then CHECKPOINT, then C5, then D (D is additive,
independent of all). None is small - each is a multi-part change.

- **C2 (gating, large) - DONE 2026-06-15** (see the "C2 match analysis
  relocated" entry above; built name-based, not via a pat-resolution table).
  Original plan kept for reference:
  relocate the type-directed match analysis from
  lowering to a post-walk typeck pass. ROOT CAUSE: match bare-ident
  classification (binding-vs-variant) is type-directed via `match_domain(scrut)`
  (`crates/hir/src/core/lower/expr.rs:1198`), which reads
  `self.body.expr_types.get(scrut)` - lowering's OWN stamping. Lowering runs
  BEFORE typeck, so it can only do this because it stamps inline; C5 deletes the
  stamping and this breaks (every match -> domain `Other` -> MatchScrutineeNotEnum).
  The WHOLE type-directed match block hinges on that one scrutinee-type read:
  bare-ident classify, domain checks (PatternDomainMismatch), coverage
  (`covered`/`saw_true`/`saw_false`/`saw_wildcard`), exhaustiveness
  (NonExhaustive/NonExhaustivePrimitive), DuplicateArm, UnreachableAfterWildcard.
  Plan (rust-analyzer precedent): lowering stores an UNCLASSIFIED bare pattern
  (new Pat form) + structural arm lowering only; typeck (which types the
  scrutinee) classifies + runs domain/coverage/exhaustiveness in a post-walk
  pass like the existing `check_match_arm_consistency`, records arm
  classifications in a pat-resolution table on `TypeckResults`; MIR reads the
  table. Files: lower/expr.rs (lower_match_expr ~960-1194 + match_domain ~1198),
  lower/pat.rs (BareIdentPat ~74-106), typeck/infer.rs (new pass + table),
  mir/lower.rs (read classification). All PatternError variants already live in
  hir/errors.rs (shared), so typeck can emit them.

- **C4 (decay, net-up, BYTE-IDENTICAL output) - DONE 2026-06-15** (see the
  "C4 decay flip BUILT" entry above). Original plan kept for reference:
  move decay from a lowering-
  injected `Expr::Cast` node to a typeck `Adjustment::Decay` MIR applies.
  `Adjustment::Decay(TypeRef)` EXISTS (`crates/typeck/src/lib.rs:45`) but
  `TypeckResults` has NO adjustments field yet. Plan: (1) add
  `adjustments: ArenaMap<Idx<Expr>, Adjustment>` to TypeckResults; (2)
  infer.rs `site_coerce` (~990) records `Decay` when `ty_of(id)` (a `&[T;N]`)
  decays to `expected` (port `array_ref_decays_to` from coerce.rs:181); (3) the
  walker type checks (enforce_return_type / let / call-arg) must ACCEPT decay -
  today they pass because the injected Cast is typed `declared`, but after the
  node is gone the site expr is the `&[T;N]` original; (4) remove the
  `maybe_decay` call from `coerce` (`crates/hir/src/core/lower/coerce.rs:54`;
  `maybe_decay`+the element-rewrap in `coerce_array_literal` become dead); (5)
  MIR applies decay - refactor `lower_operand`/`lower_rvalue`
  (`crates/mir/src/lower.rs:824`/`583`) so a `Decay(target)` expr lowers like
  the old `Expr::Cast` arm (lower.rs:666) WITHOUT double-applying (lower_operand
  trivial arms - Const/place Copy - bypass lower_rvalue, so each path needs the
  cast exactly once; array-element decay rides lower_operand on each element).
  Verify string.eye/caesar.eye generate byte-identical C. C4 alone has no
  user-visible effect (byte-identical) - its only value is unblocking C5.

- **C5 (IRREVERSIBLE) - DONE 2026-06-15** (see the "C5 cutover COMPLETE" entry
  above). Went further than the original sketch: the whole `Body.expr_types`
  field was removed (not just the stamping), `CallNonFunction`/`LenNotArray`/
  `LenFieldOnArray`/`PrintCannotFormat` relocated to typeck, and the codegen
  typedef topology re-seeded from typeck (`expr_type_seed`).

- **D - query part DONE 2026-06-15** (see "D - `typeck_fn` per-fn salsa query"
  above): the `typeck_fn` query + `hir_diagnostics` per-fn type-diag wiring.
  REMAINING (deferred to the LSP-latency push, per the user): (1) the **LSP hover**
  handler (none exists - add one in crates/lsp reading `TypeckResults.expr_types`
  with a position->ExprId mapping). On an **expression** hover show its type; on a
  **function-name** hover show the inferred **effect set** from the `EffectMap`
  (e.g. `io`, `ffi`, `state`, or `pure`) alongside the signature - effects are
  inferred and otherwise invisible, so hover is their primary surfacing. (2)
  decouple effect diagnostics from the codegen `lowered_file` (a per-fn
  effect-atom query feeding a cheap fixpoint query) so the diagnostics path is
  fully per-fn incremental.

### 2026-06-12 (evening): typeck + effects design ratified - sealed-body inference

Kernel freeze declared (committed); Horizon 1 design session closed the
typeck/effects architecture. Full design: docs/features/TYPECK.md +
docs/features/EFFECT.md; PARALLEL.md's draft inference section replaced to
match. Build runs as segments S0-S6 (TYPECK.md migration plan); **S0 BUILT
2026-06-12** (`TypeKind::RawPtr` replaces the `Path("ptr")` magic at every
judgment site; `TypeKind::Fn`/`TypeNode::Fn` carry `variadic` through
mangle - `fn{n}v` - and C typedefs emit `, ...`; 324 tests + strict-C +
grammar parity + clippy green). **S1 BUILT 2026-06-12**: `crates/typeck`
(walker `infer.rs` faithful to lowering's stamping with `PARITY(S3)`
markers on the deliberately-kept-wrong rules; `InferObserver` fusion seam +
no-op impl; `TypeckResults`/`Adjustment`/`Expectation`/`Cause` skeletons);
shadow harness `shadow.rs` compares walker types against lowering stamps on
every visited expression (typed orphans of rejected exprs excluded by
construction; walker-extra stamps allowed, missed lowering stamps are
divergences). 10 shadow tests including whole-corpus parity; oracle
verified to bite (deliberate rule break failed 2 tests, reverted).
Workspace 334 green. S1 limitations recorded in TYPECK.md: adjustments
inert until S2 (coerce still mutates the tree), diagnostics still
lowering's, completeness contract starts at S2.

**S2 ~ in progress.** step A BUILT 2026-06-12: MIR reads `TypeckResults`
everywhere - `typeck::check_file(&mut HIR)` (take/restore interner, so
every result handle resolves through `hir.types`), `mir::lower_all(hir,
typeck)` + `lower_function` gain the side table, `mir_type_of` reads it
(A3 fallback unchanged until completeness), database `lowered_file` ->
`CheckedFile { hir, typeck }` with mir_map/c_code/driver/LSP/proptest/
bench call sites updated; mir dep typeck (chain hir <- typeck <- mir).
334 tests + strict-C green on walker-derived types.

step B ~ in progress: diagnostics infrastructure BUILT (TypeckResults
gains a `Sink<HirError>` + `emit_at` through the body source map; merge
points wired: driver render, `database::hir_diagnostics` (whole-file
interim until step D), `c_code` gate, `compile_file`, proptest).
x hir-tests-merge-typeck-diags via dev-dependency - cargo builds two
`hir` crate versions in that configuration (type identity breaks);
judgment tests migrate to `crates/typeck/tests/judgments.rs` with their
checks instead. first cluster MOVED: M1 int-literal range sweep
(`check_int_literal_ranges` + `int_type_range` now in the walker, run at
body end over visited expressions in arena order; the `len`-fold
synthesized literal is skipped by its tell - it shares its cast
wrapper's syntax pointer; poison orphans drop their range cascades by
construction). 335 green, clippy clean.
2026-06-13: binary/unary + index/deref-ptr clusters MOVED to the walker
(`binary_judgments` OpOnArray/ArithmeticOnPtr/ArithmeticOnEnum/ModuloOnFloat,
the unary opaque-enum arm, `index_judgments` IndexOnPtr/IndexOutOfBounds/
NegativeIndex, and the new `DerefOfPtr` arm). Lowering's dead remnants
removed (`bin_op_str`, orphan `is_comparison`, `const_uint_index`,
`expr_enum_name`). The 7 positive judgment tests + 2 clean controls
relocated hir->`crates/typeck/tests/judgments.rs` against the typeck
pipeline (judgments 9 green). Parity repair: the prior move had the walker
poison these exprs to `Error`, breaking the shadow oracle
(`diagnosed_programs_still_agree`: lowering leaks the left-operand type,
walker returned `<error>`); reverted to emit-but-keep-lowering-type with
`PARITY(S3)` markers - the poison flip is step C. `binary_judgments`
rewritten clone-free (matches each operand `TypeKind` by reference via
`binary_judgment_error`, no `TypeKind` clone). Workspace 345 green, clippy
clean.
2026-06-13 (cont.): value-position match-arm consistency MOVED to the walker
(`check_match_arm_consistency`, run post-walk; statement-position +
value-discarded-tail (`fn_ret.is_none()`) excluded exactly as lowering did;
`types_compatible` + `is_integer_path` + the `ContainsError` walk ported into
typeck - PARITY(S3): the integer leniency dies with literal adoption at the
cutover). Lowering's `check_value_position_match_arms` + its `fn_body` call
deleted (`enforce_fn_return_type`'s match-tail stamping stays - the walker
mirrors it in `run`). 7 match-arm tests relocated to typeck judgments (the
diagnosed + clean cases incl. the integer no-false-positive); hir keeps the
trimmed `match_wide_int_let_records_binding_type` (lowering stamping) and the
return-type-mismatch test (`enforce_fn_return_type` stays). Workspace 346
green, clippy clean.
2026-06-13 (cont. 2): RETURN enforcement MOVED to the walker. Block-ptr
threading solved by a new `Body.fn_block_ptr` field (set once at fn lowering)
rather than a `check_body` signature change - the walker reads it, no
salsa/driver churn. Walker gained `enforce_return_type` (post-walk tail
check: no-tail-no-`return val` -> ReturnMissingValue on the fn block; tail
yields-no-value -> VoidValueInValuePosition; tail match defers to the per-arm
check; else tail-vs-ret `types_compatible`), `check_explicit_return` (per
`Expr::Return`: ReturnValueInVoid / ReturnMissingValue / VoidValueInValue /
ReturnTypeMismatch), and ported `yields_no_value`/`block_yields_no_value`.
Lowering's `enforce_fn_return_type` + `check_explicit_return` DELETED (+ the
now-dead `types_compatible`/`is_integer_path` there; `type_ref_contains_error`
stays for the let check) and the `fn_block_ptr` LoweringCtx field removed; the
tail-match return-type re-record (stamping only) inlined into `fn_body`. 8
return tests relocated hir -> typeck judgments. PARITY preserved incl. the
latent double ReturnMissingValue on `return;` in a typed fn (both
check_explicit_return + the no-tail path fire, as lowering did). hir+typeck
GREEN (clippy clean); `hir_raw_dump` snapshot updated for the new Body field.
2026-06-13 (cont. 3) typeck PERF + soundness pass (user-requested): the
walker dodged the borrow checker with per-expression heap allocs - removed by
copying the shared `&Body`/`&HIR` into a local so the tree borrows at the
data's lifetime, not `self`'s. Killed: `infer_block` `stmts.clone()`,
`infer_expr` `args.to_vec()` / `elems.to_vec()` / struct-fields & match-arms
`collect()` / `Literal` & `Resolution` clones, `infer_call` `param_tys` Vec,
`lookup_field_type` `TypeKind` clone (all arms are `&self`, no clone needed),
plus a lits-first early-out in `check_int_literal_ranges`. Zero semantic
change - shadow oracle (corpus-wide parity) + all hir/typeck tests stay green.
2026-06-13 (cont. 4) ADJACENT FIX (concurrent mir-opt pass, not typeck): the
new `reconstruct_expressions` inliner (crates/mir/src/optimize.rs) emitted
`use of undeclared identifier '_t6'` for an array-index temp (bubblesort +
string e2e red). Root cause: `try_inline_operand` only rewrote
`Copy(Place::Local)`; a single-use temp used as an INDEX lives in
`Copy(Place::Index(base, idx))`, classified inlineable + its `let` removed,
but the index operand was never rewritten. Fix: descend projected places via
the existing `rewrite_place_in_rvalue` + regression test
`index_operand_temp_is_inlined`. c_codegen snapshot accepted (their temp
elimination). FULL WORKSPACE 350 green.
2026-06-13 (cont. 5) LET-CHECKS cluster MOVED to the walker (last S2 step-B
type judgment): `check_array_init_len` (ArrayInitLenMismatch) +
`check_explicit_let_init_type` (VoidValueInValuePosition on an else-less `if`
init; LetTypeMismatch on a wrong-typed call init; Error/array-len lenient),
called from `infer_stmt`'s Let arm against the explicit declared type, stmt
ptr as the anchor fallback. Lowering's two methods DELETED plus the now-dead
`yields_no_value`/`block_yields_no_value`/`type_ref_contains_error`/`ContainsError`
(all only fed the let-checks once return enforcement left); `record_match_result_override`
stays (stamping). 4 tests relocated hir -> typeck judgments. Pure diagnostics
(no stamping), so shadow oracle untouched. S2 step B DONE bar println arity
(structural, STAYS in lowering). remaining S2 steps: C delete coerce +
stamping + A3 ICE + shadow harness (len HIR node + match bare-ident
classification ride it), D per-fn `typeck_fn` query + LSP hover.
! two type-directed-lowering snags found, they scope B/C:

- `len(x)` folds to a literal whose VALUE comes from the operand's type;
  a pure-builder lowering cannot fold it. fix: keep a `len` HIR node and
  fold at MIR lowering from `TypeckResults` (type-directed lowering moves
  to MIR, where the types live).
- match bare-ident arms classify binding-vs-variant by the scrutinee's
  type (type-directed resolution). fix: lowering stores an unclassified
  bare pattern; typeck classifies and records it in the results (a
  pat-resolution table); MIR reads the classification. rust-analyzer
  precedent.

S2 STEP C - CUTOVER EXECUTION PLAN (code-grounded 2026-06-13, NOT STARTED).
goal: typeck becomes the SOLE type source; lowering stops stamping. it is a
coordinated, partly-irreversible segment (deletes the shadow oracle - the
corpus-wide parity net). safe order = build prerequisites with the net UP,
then one coordinated flip, delete the net LAST. surface as read:

- coerce.rs does 3 adjustments: array-lit retype + int-lit adopt (both
  already mirrored walker-side by `site_coerce`, stamp-only) + DECAY
  (injects a `Cast` node - the only one MIR sees structurally).
- MIR `mir_type_of` (lower.rs ~1148) is the A3 site: reads
  `typeck.expr_types`, defaults int32 when absent. comment: fallback never
  fires on corpus BUT "several Expr (notably Loop) never set expr_type" so
  int32 is currently load-bearing -> A3->ICE has real prerequisite gaps.
  C1 DONE 2026-06-13 (net up): `len` HIR node. `Expr::Len(ExprId)` added
  (body.rs variant + for_each_child + VisitExpr default). lowering emits
  `Expr::Len(operand)`, keeps ALL structural+array checks (arity/place/
  LenNotArray - still has types pre-cutover) + stamps usize. walker types
  Len=usize and `const_uint_index` peels Len for the `a[len(a)]` OOB check.
  MIR `lower_rvalue` folds Len -> `(usize)N` (reproduces the old cast-const
  output exactly, codegen byte-identical). hir-dump variant_name +
  `array_len_lowers_to_len_node` test updated. lsp rides VisitExpr default
  (no change). 350/0 green, shadow oracle intact (lowering still stamps Len
  usize = walker). NOTE: the old len fold's synthesized-cast-skip in
  `check_int_literal_ranges` is now dead (no len literal) - harmless, remove
  at C5. ALSO fixed 2 pre-existing clippy warns in the concurrent optimize.rs
  (collapsible-if + needless-ref in `inline_in_block`).
  C2 (net up): match bare-ident classification -> results pat-resolution table
  (typeck classifies binding-vs-variant; MIR reads). shadow stays.
  C3 DONE 2026-06-13 (net up, HARDENING): flipped MIR `mir_type_of`'s A3
  fallback int32 -> error_type and the whole corpus stayed green - the
  walker already types every expr MIR consumes, so the fallback never fires
  (the "Loop never typed" worry was over-cautious). A gap now surfaces as
  `void* /* ERROR TY */` in C, not a silent int32 miscompile. Regression
  test `corpus_generates_no_error_type` (e2e): every accepted showcase
  program generates C free of the error marker (rejected WIP programs -
  lang/linkedlist/mandlebrot/physics - skipped). C5 hardens error_type ->
  ICE once lowering stops stamping. shadow stays.
  C4 (THE FLIP, coordinated): walker records `Adjustment::Decay`; MIR applies
  decay from the adjustment; lowering's `maybe_decay` stops injecting the
  Cast. (Decay-only flip is shadow-compatible: non-decay expr types
  unchanged.) verify decay programs (strings/caesar) byte-identical.
  C5 (IRREVERSIBLE): delete lowering's `coerce` + all stamping
  (`expr_types` writes) + `record_match_result_override`; flip A3 int32 ->
  error_type/ICE; delete shadow.rs + tests/shadow.rs. after this the net is
  gone - C3's completeness test is the remaining guard.
  then D: per-fn `typeck_fn` salsa query + LSP hover.
  recommendation: execute C1..C5 as their own focused turns (each verified
  green), net deleted only at C5.

S4 EFFECTS (the "dual") - FOUNDATIONAL SLICE BUILT 2026-06-14. Brought forward
(out of strict S-order) because C3 proved the substantive precondition EFFECT.md
names ("types stable" = the walker types every expr MIR consumes), the
`InferObserver` seam was ready since S1, and it is additive (new crate, no
teardown). BUILT: `crates/effect` (eye-effect) - `EffectSet` bitset lattice
(io/ffi/state live + alloc/panic/diverge reserved), `EffectJudge` impl
`InferObserver` collecting per-body atoms (io=println, ffi=extern call + `*p`
on a Ptr/RawPtr, state=mut-global access) + call edges, `infer_body_effects`
driver, 5 tests (io/ffi/state/pure/call-edge). Seam extended: typeck now exposes
`ObserverCx { scope, body, types, expr_types }` and `InferObserver::visit(id,
expr, ty: Option<TypeRef>, cx)` (3 call sites in infer.rs, `()` no-op updated).
shadow oracle intact.
WHOLE-PROGRAM FIXPOINT BUILT 2026-06-14 (EFFECT.md "Path forward" step 1):
`infer_effects(&mut HIR) -> EffectMap`. Per-body `EffectResult` gained an
`indirect: bool` (set by the judge's Call `_` arm = a call through a fn-pointer
value, since unresolved non-intrinsic names are rejected upstream). The driver
collects every fn's `(own atoms, callees, indirect)` in arena order (a bodyless
fn synthesizes its extern verdict: `ffi` if extern else pure), builds the call
graph, runs a `tarjan_scc` mirroring `hir typegraph.rs`, and unions effects up
the condensation - SCC ids come out callee-first (Tarjan reverse-topo), so
seeding each SCC with its members' own atoms (+ full live set `EffectSet::live`
= io|ffi|state when any member is `indirect`) then processing SCCs in increasing
id is the fixpoint in one pass (O(V+E); recursion = a shared SCC verdict, no
iteration). 5 fixpoint tests (transitive io, mutual-recursion union, extern
propagation, fn-pointer conservatism = full live, pure-stays-pure); effect crate
10 tests, workspace 361 green, clippy clean.
DATABASE WIRING BUILT 2026-06-14 (EFFECT.md "Path forward" step 2):
`effect::infer_file(&mut hir) -> (typeck_map, EffectMap)` is the fused
dual-inference driver - `collect_results` runs ONE walk per body producing both
the `TypeckResults` and the per-body `EffectResult` (judge fused as the
observer), then `run_fixpoint` condenses. The database's `lowered_file` calls it
and stores the map in a new `CheckedFile.effects` field beside `typeck`, so types

- effects share a single traversal and memoize together (no second walk). Added
  `effect` as a database dep. `typeck::check_file` stays the type-only entry for
  the non-salsa paths (src/lib.rs compile helper, benches, judgment tests);
  `infer_effects` stays the effect-only standalone (tests). No backend consumer
  reads `effects` yet - it feeds the annotation contract check + prime gate. DB
  test `lowered_file_carries_the_effect_map`; database 6 tests, effect 10.
  S4 COMPLETE 2026-06-14: annotations + `EffectError` class + exact-match contract
- witnesses all built.
  - ANNOTATIONS: parser nests contextual effect idents before the fn name in a
    hand-written `EffectList` `SyntaxKind` node (`io render(...)`; name = the
    ident before `(`, so `FnDef::name()` unchanged). No ungram/xtask change and
    corpus stays annotation-free, so the tree-sitter parity gate is untouched.
    Collection stores `Function.declared_effects: Vec<(Text, Span)>` (raw names +
    spans; effect crate validates).
  - EFFECTERROR CLASS: `Class::Effect`/`E` added to diagnostics (8->9 classes);
    `EffectError { UnknownEffect (E001), EffectMismatch (E002) }` in
    `hir/errors.rs` (data only - effect crate is sole producer, keeps graph
    acyclic). `HirError::Effect` arm + `notes()` carries the witness trail.
  - CONTRACT: `effect::check_contracts` validates each annotated fn's declared
    set (parse names -> atoms, `pure`=empty, unknown=E001) and exact-matches it
    against the inferred set from the fixpoint (E002 either direction). Runs in
    `infer_file`, returns `Sink<HirError>` stored in
    `CheckedFile.effect_diagnostics`, merged into `hir_diagnostics`, gates `c_code`.
  - WITNESSES: `EffectResult.local_witness[3]` records per live atom the
    producing primitive (`Println`/`Extern`/`RawDeref`/`MutGlobal`/`Indirect`);
    `witness_trail` DFS-walks the call graph (error path only) to the leaf,
    naming the via-chain ("the `io` effect comes from a call to `println` (via
    `reporter`)").
    Tests: effect 16 (6 contract + witness), database 7 (effect map + E-class
    gating C); parser 64 (2 annotation), hir_raw_dump snapshot accepted
    (`declared_effects: []`). Workspace green, clippy clean.

PATH FORWARD (consolidated, 2026-06-14):

- S2C cutover: C1 done, C3 done; REMAINING C2 (match bare-ident
  classification -> typeck results table), C4 (Adjustment::Decay flip), C5
  (irreversible: delete coerce/stamping/shadow, A3 error_type->ICE), then D
  (typeck_fn salsa query + LSP hover). C4/C5 must account for decay in the
  type checks (return/let/arg) since the cast node disappears - see C4.
- S3 new judgments (M2 strict same-type operands, cast lattice, assign
  non-value) - ledger rulings below; ride the cutover.
- S4 effects: COMPLETE 2026-06-14 (fixpoint + DB wiring + annotations +
  `EffectError` E class + exact-match contract + witnesses). The whole effect
  inference layer is done; the two remaining inference bodies of work are the
  type-side cutover (C2/C4/C5+D) and S6 parallelism.
- S5 firewall (structural signature backdating).
- S5 firewall (structural signature backdating).
- S6 parallel wave: rayon per-body walks + lock-free interner (boxcar +
  papaya, &self intern) + determinism gate (corpus-diff-twice). THIS is the
  "parallelised" half; unstarted. Per-body sealed-body invariant already
  makes the walks embarrassingly parallel; the fixpoint is the one join.
- S7 row-polymorphic effects: effect variables on fn types for precise
  higher-order effect tracking. Monomorphic bitset -> row-polymorphic with
  body-local effect variables (obeying the sealed-body invariant).
  Requires S6 lock-free infrastructure (effect variable handles must be
  Send+Sync). Not started.
  ordering note: effects (S4) were brought forward; the cutover (C2/C4/C5) and
  S6 parallelism remain the two large bodies of work to "parallelised dual
  inference" complete. S7 is the third addition, sequenced after S6.

Rulings, all ratified 2026-06-12:

- **Strategy: sealed-body inference.** Invariant: no inference fact crosses
  a fn boundary (signatures = the only inter-fn channel) - per-body checking
  embarrassingly parallel, permanently. Tier 1 = bidirectional expectation
  spine, no variables. Tier 2 = provenance-carrying expectations (`Cause`
  chains; two-span diagnostics now, macro-expansion origin frames at
  Horizon 2 - the native-errors-for-injected-features foundation), built day
  one ("no half measures"). Tier 3 = dormant body-local unification seam
  (replaces PARALLEL.md's Hindley-Milner line).
- **M2 operand rule: strict same-type** (mismatched widths/signedness =
  error, cast explicitly; no promotion).
- **Assignment is non-value** (value-position `x = y` = type error).
- **Cast lattice**: int<->int, int<->float, char/bool/enum->int, ptr<->ptr,
  int<->ptr; everything else rejected (relaxable).
- **Effects: exact-match annotation contract** (declared set must equal
  inferred; upper-bound is the later relaxation). **Fused dual inference**
  (clarified same day): types and effects are inferred simultaneously in
  one per-body walk - the effect judge runs per visit with the
  just-computed type; the whole-program fixpoint is the only inherent wait.
  **Separate crates** (same day): `crates/effect` implements `typeck`'s
  `InferObserver` seam (trait + no-op impl land in S1); fusion crosses the
  crate boundary at zero cost (monomorphized). **Inference is total** -
  annotations never required, optional contracts only; effect names are
  _contextual_ keywords (effect position only; `state`/`io` stay legal
  identifiers; unknown effect = E-class at collect).
- **Effect diagnostics: new E class** (`EffectError`, E001+; taxonomy 8 -> 9).
- **Lock-free global type interner** for the parallel wave (`boxcar` arena +
  `papaya` dedup map, `&self` interning; revised same day from a
  sharded-lock design). Kills the per-fn interner clone and the two-path
  handle-comparability split.
- **Firewall in-build**: structural signature backdating = segment S5, not
  backlog.
- TypeckResults side table (expr_types complete-or-Error + Adjustment::Decay
  replaces coerce's HIR mutation); MIR A3 fallback becomes an ICE; shadow
  mode (S1) validates the new pass against the full suite before cutover.

### 2026-06-12 (later): design rulings - seven of nine design questions closed

Every ruling took the conservative or precedent-backed option; the freeze
gate's design-question requirement is now satisfied (remaining two rows are
post-freeze features).

- **`{{`/`}}` brace escape BUILT** (Rust-style): `{{` prints `{`, `}}`
  prints `}`, `{}` stays the placeholder, a lone brace still prints
  literally. The HIR arity scan (`check_println_args`) and the codegen
  renderer (`gen_println`) use the same scan rule. PRINT.md updated;
  HIR + e2e regression tests.
- **Void-fn tail call without `;` RATIFIED** (no code change): consistent
  with expression-block semantics (a void tail is discarded), and the
  same shape is legal Rust (`()`-typed tail in a `()` block).
- **Same-scope redeclaration REJECTED, BUILT** (`DuplicateLocal`, R015):
  `let x = 1; let x = 2;` in one scope is an error; shadowing in a nested
  block scope stays legal. Conservative by freeze asymmetry: reject now
  can be relaxed to a Rust-style shadowing rule later, the reverse breaks
  programs. Covers `let`/`mut`, local `const`, and destructure bindings
  (including a rename collision `{ x: a, y: a }` the duplicate-field
  check cannot see). The old e2e pin `mir_path_allows_same_block_shadowing`
  reversed into `same_scope_redeclaration_is_rejected` +
  `nested_block_shadowing_is_legal`; lang.eye's redeclaration corrected to
  a plain reassignment (its FIXME removed from the source register).
- **Enums OPAQUE, BUILT** (`ArithmeticOnEnum`, T035): arithmetic/bitwise
  binary operators and unary `-`/`~` on an enum-typed operand are
  rejected (no-footguns: C int arithmetic on tags). Comparisons stay
  allowed; `as` to an integer type stays as the explicit escape.
- **`&` of non-place REJECTED, BUILT** (`RefOfNonPlace`, T036): `&(a + b)`
  spilled to a MIR temp and took the temp's address with no visible
  lifetime. `&` now requires a place: local, global, field, index, or
  deref. Same freeze asymmetry as redeclaration.
- **Readable-C mode: spilled C RATIFIED as default**; codegen quality, not
  semantics, so it does not block the freeze. Moved to the architecture
  backlog.
- **`--check` EXTENDED through HIR lowering**: exits 0 only when lexer,
  parser, and lowering are diagnostic-free; writes no `.c` or binary. The
  parse-stage oracle the grammar parity gate needs is the new
  `--parse-only` (check-grammars.sh updated) - the gate must see exactly
  what tree-sitter sees, which an extended `--check` no longer is.

Verified: workspace tests green (new: brace-escape, redeclaration, enum
arithmetic, ref-of-non-place), corpus 43/43 (2 expected XFAIL), strict
gate 41/41, grammar parity gate green under `--parse-only`, clippy clean.

### 2026-06-12: loose ends - every non-typeck open bug closed

- **Guarded-switch uninitialized-temp corner FIXED**: when a guarded switch
  has no `default` (HIR proved exhaustiveness via the unguarded arms), the
  last unguarded arm is now emitted gated on the flag alone (`if (!_gN)`,
  no scrutinee test - tautological once every earlier unguarded arm has
  failed). The guarded chain's analogue of the unguarded chain's
  last-arm-as-`else` (M3); a rogue scrutinee can no longer read the
  value-match hoist temp uninitialized. E2E regression
  `guarded_exhaustive_switch_has_unconditional_tail`; MATCH.md updated.
- **A5 CLOSED**: the fix was already built in the LSP overhaul (commit
  8950b44) - `classify_pat` resolves a `BareIdentPat` against
  `hir.items.variants`, VARIABLE when not a variant. Regression test added
  (`bare_ident_pat_uses_hir_variant_resolution`).
- **statistics.eye highlight FIXED**: root cause was byte-based LSP
  positions - semantic-token columns/lengths and diagnostic ranges must be
  UTF-16 code units (the LSP default encoding), and the file's box-drawing
  and non-breaking-hyphen comment characters byte-inflated every length.
  New `SourceText::line_col_utf16`; both LSP paths (semantic tokens,
  diagnostics) converted; token lengths counted in UTF-16 units. Source
  FIXME and the highlight-debug scaffolding functions removed.
- **tree-sitter highlights audited** (eye-tools): added `extern_type` name
  -> @type and `struct_pattern`/`struct_pattern_field` captures; guard
  `if`, `const` statement, and `variadic_parameter` were already covered.
  Verified with `tree-sitter query` against a probe exercising every new
  node.
- **Latent mangling edge FIXED**: a parameter literally named like a
  generated local (`x_1` vs local `x` id 1, `_t2` vs a temp) collided in
  the same C scope (redefinition, or a silent shadow miscompile from a
  nested block). `local_names` now repairs a colliding generated name with
  trailing `_` (generated names never end in `_`, so the scheme stays
  injective). Found with it: **duplicate parameter names** leaked to clang
  as a C redefinition error - now rejected (`DuplicateParam`, R013; extern
  prototypes are types-only and exempt).
- **Latent printf clash FIXED**: a non-extern definition named `printf`
  suppressed the emitted prototype and shadowed the libc symbol the
  `println` intrinsic calls; `__eye`-prefixed names collided with the
  backend's own symbols (`__eye_main` shim, string statics, array
  wrappers). Both rejected (`NameIsReserved`, R014): `__eye` prefix at
  every name-check site, `printf` for file-scope ordinary-namespace names
  (function/global/struct/union/enum/variant). `extern printf` stays legal
  (same libc symbol; verified alongside `println`).
- **Latent non-ASCII char literal FIXED**: `'é'` emitted a multibyte C char
  constant (implementation-defined value, clang error under the default
  build). Rejected at lowering (`CharLiteralNotAscii`, T034) in both
  expression and match-pattern position; ASCII and escapes unaffected.
- Verified: 318 workspace tests green, corpus 43/43 (2 expected XFAIL),
  strict gate 41/41, clippy clean.

### 2026-06-12: trivial-tier fixes (U5, P2) + artifact hygiene

- **U5 fixed**: `check_println_args` (HIR) counts `{}` placeholders in a
  literal format string with the exact scan codegen uses and rejects a
  mismatch (`PrintlnArityMismatch`, T032). Sibling found with it:
  `println()` with no arguments emitted a bare `printf()` - not legal C -
  now rejected (`PrintlnMissingFormat`, T033). A non-literal format string
  stays uncounted (operands are forwarded unchanged). Regression test
  `println_placeholder_arity_is_checked`; one MIR test fixture was itself
  the bug class (`println("pos", 1)`) and was corrected.
- **P2 fixed**: `collect_strings` now walks the MIR (function arena order
  for deterministic static ids) and collects only string literals the
  emitter will reference - a literal `println` inlines (format or value,
  when the format is a string constant) gets no `__eye_str` static. The
  strict gate's `-Wno-unused-const-variable` suppression is removed, so
  the gate now enforces it. Snapshot diff confirmed: the dead `"{}"`
  static disappeared and nothing else changed.
- Four rows closed as already done or unconstructible (each verified):
  parser error codes already use explicit discriminants with a
  never-renumber policy (`crates/parser/src/errors.rs`); `Interner::lookup`
  already has the debug-only bounds check with a friendly message
  (`crates/lexer/src/lib.rs`); the unresolved-shorthand-field regression
  test already exists (`non_value_name_uses_are_rejected`, "undeclared
  shorthand field must be rejected" - the stale source FIXME was removed);
  the U1 regression test cannot be written yet - an aggregate-typed
  const/global is rejected by the scalar-only floor (verified: C008
  `NotAConstExpr`), so no legal program can make a const/global the sole
  reference to an array type. The U1 typegraph walk stays as defensive
  hardening; the test lands with aggregate const values (roadmap row).
- Artifact hygiene: `.gitignore` whitelists `eyesrc` (only `.eye` and
  `.sh` are stageable; generated `.c`/`.o`/binaries can never appear) and
  ignores `*.c`/`*.o` repo-wide (no tracked C exists; the backend's only
  C output is generated).
- All workspace tests green, corpus 43/43 (2 expected XFAIL), strict gate
  41/41 under the tightened flags, clippy clean.

### 2026-06-12: ledger unification + salsa-era closures

`docs/todo.md` deleted; every open item absorbed above. NOTES.md checkbox
markers neutralized (historical doc). CLEAK.md header updated: frozen audit
record, open rows tracked here. Sequencing decided: loose ends -> kernel
freeze -> typeck (freeze before typeck, within reason).

Rows closed as stale during the absorption (each verified against the
tree):

- **A1 - MIR double-lowering under dump flags**: closed by the salsa
  migration; `--dump-mir`, `--dump-mir-raw`, and `c_code` all consume the
  one memoized `mir_map(file)` query (`src/main.rs`). The `MirCache` is
  deleted.
- **A8 - `process::exit(1)` in the driver**: no `process::exit` remains in
  `src/main.rs`; errors propagate as `Result`.
- **Fuzz testing**: built - `fuzz/fuzz_targets/{fuzz_lexer,fuzz_parser,
fuzz_full}.rs`.
- **L1/L2/L5/L6/M1/M1b/P1**: were still listed open in the old ledger and
  todo.md; all fixed 2026-06-11 (see the coercion-point entry below).
- Stale source FIXMEs corrected: `example.eye` exhaustive struct-init
  (StructLitMissingFields catches it), `lang.eye` C-keyword field name
  (R010 rejects it; reject was chosen over mangling), `lang.eye` `off off`
  note (R012 catches undeclared field types now).

### 2026-06-12: salsa query database

`crates/database`: tracked queries for every phase (`lex`, `parse`,
`item_scope`, `lower_fn`, `lowered_file`, `mir_map`, `c_code`), wired into
the CLI driver and the LSP; stop-at-first-errored-phase contract preserved.
`RefCell` evicted from `HIR.types` (plain `TypeInterner`; `LoweringCtx`
owns a working interner). Per-fn and whole-file lowering paths split
(diagnostics vs codegen). 309 tests + 43-file corpus green. Divergences
from the plan recorded in `docs/design/SALSA.md`.

### 2026-06-11 (later): coercion-point unification + companion rejects

CLEAK fix-order step 3. `LoweringCtx::coerce` (`crates/hir/src/core/lower/coerce.rs`)
is now the single coercion point - array-literal re-typing (recursive),
integer-literal typing, and `&[T; N]` decay - applied at all six sites with a
locally-known expected type: `let` init, call argument, explicit `return`,
function tail, struct-literal field, array-literal element. The four
scattered `maybe_decay` calls and the per-site array re-typing blocks are
deleted.

Closed with it:

- **L1 + L2**: string decay at struct-literal fields and array-literal
  elements (the lang.eye compile blocker). lang.eye compiled and ran.
- **M1 + M1b (T030)**: every integer literal is range-checked against the
  type it ends up with (coerced or the `int32` default) by one post-lowering
  sweep; `let int32 x = 5000000000;` is now an error instead of 705032704.
- **M4, new find (T031)**: positional struct literals (`Point { 1, 2 }`)
  silently dropped their values and zero-initialized the struct - a verified
  miscompile. Rejected; struct literals are named-only.
- **L3 (T026)**: call arity checked - exact for defined fns, minimum for
  variadic externs. Indirect calls stay unchecked (no variadic flag on fn
  types yet; typeck-scope row above).
- **L5 (R011)**: a struct literal must name a declared struct or union.
- **L6 (R012)**: every Path name in a declared type must be a declared type -
  item signatures validated post-collect (forward refs resolve), body
  annotations and casts eagerly. `sizeof` stays lean-on-C (SIZEOF.md).
- **L7 (T027) + deref sibling (T028)**: indexing and dereferencing `ptr`
  rejected (no element/pointee type).
- **P1 decided + fixed (T029)**: arithmetic/bitwise on `ptr` rejected;
  comparisons stay; `T*` keeps C semantics.

Also: lang.eye's extern `exit(int code)` corrected to `int32` (C's `int` was
leaking through verbatim - exactly the class R012 now rejects); the salsa
`compile_file` query stops at the first errored phase like the pre-salsa
driver (MIR assumes a diagnostic-free HIR); snapshot harness terminators
moved to the stable `c source written` marker. 305 tests green, clippy clean
(one accepted `Arc<CompileResult>` Send/Sync note in eye-database), corpus
43/43 with 2 documented XFAIL, strict-C gate 41/41.

### 2026-06-11: C-leak audit + strict-C gate + mechanical fixes

Full audit of implicit type decisions across HIR lowering, MIR lowering, and
codegen, written to `docs/design/CLEAK.md` (classification: M miscompile /
L C-leak / P pedantic / T typeck-required; every M and L row has a verified
reproducer). Detection infrastructure: `scripts/check-c-strict.sh` compiles
the corpus's generated C under `-std=c11 -pedantic-errors -Wall -Wextra
-Werror`; CI gained a `corpus` job running it plus `check_all.sh`, which
gained a stale-checked XFAIL list.

Fixed the same day (the mechanical tier):

- **Exhaustive value-match UB (M3)**: a switch HIR proved exhaustive (no
  default) emitted a tested last arm, leaving the hoist temp uninitialized
  on the rogue-value path (`-Wsometimes-uninitialized`). The last arm is now
  the chain's `else`.
- **C-keyword names (L8)**: R010 `NameIsCKeyword` rejects at collect every
  name the backend emits verbatim (item, field, parameter, enum variant,
  global, opaque type). Extern parameter names exempt (prototypes are
  types-only).
- **Unprototyped zero-param functions (L9)**: `T f()` is now `T f(void)`.
- **Empty-string zero-length array (L10)**: `data[0]` storage pads to
  `data[1]`; type-level length stays 0.
- **`%p` varargs UB (L11)**: `ptr` formats as `%p` (was `%d`); ref/ptr/fn-ptr
  `println` arguments are cast to `(void*)`.

297 tests green, clippy clean, corpus 41/41 + 2 XFAIL, strict gate 41/41.

### 2026-06-11: Harden-before-freeze pass + performance analysis

**Code audit fixes:**

- U3 (non-finite float const-eval) - **FIXED** in `const_eval.rs`
- A4 (`string_id` unwrap_or(0) -> expect) - **FIXED** in `mir_emit.rs`
- A2 (`place_type` recursion) - memoized via `FxHashMap<Place, Type>` cache
  on `MirGen`, cleared per-function
- U1 (walk consts/globals/SizeOf in typegraph) - **EXPERIMENTAL** in
  `typegraph.rs` (regression-test row open above)
- A3 (mir_type_of int32 fallback) - attempted, reverted (false positives);
  typeck-scope row above
- L6 first attempt (inline in collect) - reverted (forward-ref false
  positives); solved by the post-collect `validate_type_names` pass (R012)

**Performance analysis:** `docs/performance.md`. Key findings: full
pipeline ~57 µs for a 58-line program (HIR 41%, parse 35%, codegen 17%);
rowan green tree ~60% of parse time (by-design); no O(N²) hot paths.

### 2026-06-10: match-guard S3 bugs (review of `9bfcf49..HEAD`)

All masked by `guard_example.eye` testing only the bare-local guard shape.
Fixed via the `Guard { stmts, cond }` MIR node + a flag-gated codegen chain
(`gen_guarded_switch`); see `docs/features/MATCH.md`. Closed: ordinary/
comparison guards miscompiling (no fallthrough on false guard); guard on
`_` silently dropped (guarded catch-all is now an ordered `ArmTest::Always`
arm); value-position void diagnosed (`VoidValueInValuePosition`), guarded
match no longer discharges coverage; multiple irrefutable arms rejected
(`UnreachableAfterWildcard`).

### 2026-06-10: MIR panic on value-position `loop`

`lower_rvalue` now handles `Expr::Loop` in the same divergent-control-flow
group as `Return`/`Break`/`Continue`: it lowers the loop as a statement and
returns poison `0`. **Residual (Fork D):** a value-position loop cannot
yield a real value until break-with-value lands; `break v;` still drops `v`.

### 2026-06-10: Repeat array literal `[value; N]`

The array-fill primitive: `value` evaluated once, copied `N` times (value
semantics); `N` a const length. New `Expr::ArrayRepeat` / `RValue::ArrayRepeat`
(kept first-class, not desugared, so a native backend can `memset`/loop).
`eyesrc/lang/array_fill.eye` is the showcase; `sieve.eye` restored.
**Latent drift fixed:** the `MatchArm` guard was missing from `eye.ungram`;
the ungram now carries it so xtask codegen keeps the `guard()` accessor.

### 2026-06-10: Non-int32 2D arrays fix

Nested array-literal coercion now recurses every level (was outer-only);
a false-positive codegen `panic!` guarding the bug was removed (it also
fired on valid `let int8 x = 5;`), replaced by HIR-level correctness + a
regression test.

### 2026-06-08: LSP pipes all diagnostics

`compute_diagnostics` runs all three phases (lexer, parser, HIR); every
8-class variant reaches the editor with phase label, code, notes, help,
secondary labels, severity. `docs/features/LSP.md` updated.

### 2026-06-08: Void value / missing return diagnostics

`TypeError::VoidValueInValuePosition` (T024) for void calls in typed
contexts; `ReturnMissingValue` (T005) when a non-void function has neither
tail expression nor `return val;`.

### Pre-2026-06-08 history (absorbed from todo.md)

- Block-scope `const` (2026-06-11): `const` is a statement, folded at the
  declaration site, `Body::local_consts`, lexical scoping + shadowing
  (CONST.md).
- Type interning (2026-06-08): `TypeRef(u32)` handles via `TypeInterner`,
  pre-injected primitives, `Rc<TypeRef>` eliminated.
- Tarjan's SCC for typegraph cycle detection (replaced O(V²) DFS).
- Cached pre-built Fn `TypeRef` on `Function`.
- ryu + itoa formatting; unique diagnostic codes; `local_names` on SmolStr.
- FxHashMap sweep (2026-06-10): direct `rustc_hash::FxHashMap` everywhere.
- `trimmed_text_range` returns `text_size::TextRange` (one spelling
  everywhere).
- `RefCell<TypeInterner>` ratification (since superseded: the salsa
  migration removed the `RefCell` entirely).
- `block()` emit helper in `mir_emit.rs` (9 repeat sites, byte-identical C).
- Early return + `main` de-magicked (2026-06-04/05): `return` through both
  MIR paths, three return-arity diagnostics; user `main` emits as
  `__eye_main` behind a C shim, integer return forwards as exit code, only
  rejection is `MainHasParams`. Restored `floodfill.eye`.
- MIR cutover bundled fixes: value-position `match` in value-position `if`
  lowers in place (`wierd.eye` acid test; `TernaryMatch` ban deleted);
  index typing peels one ref/ptr to an array (`floodfill.eye`,
  `bubblesort.eye`).
