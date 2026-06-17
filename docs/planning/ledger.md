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

Acceptance corpus for the pass: lang.eye plus the CLEAK reproducers.

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
- [ ] **Statement-boundary footgun: `if {} * p` parses as multiplication.**
      A block-like expression (`if`/`loop`/`match`/`{}`) in statement position is
      not terminated at the closing brace - a following prefix-`*` / `-` / `&`
      binds as an infix operator on the block's value (`(if c {a} else {b}) * p`)
      instead of starting a new statement. Surfaced by the unit-type work (a
      value-less `if` followed by `*p` deref). Rust's rule: a block-like expr in
      statement position is a complete statement; the next token starts a new one.
      Decision pending (adopt the Rust statement-expression boundary in
      `grammar.rs`); no-footgun directive applies.
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
- [ ] **Ref (`&T`) / ptr (`T*`) auto-conversion** — decided: `&T` and `T*`
      should convert automatically in coercion-site positions (argument
      passing, field init, return). Not a general implicit coercion — a
      `site_assignable` rule. Not yet implemented. `*lang` → `&lang` hacks in
      lang.eye reverted; the program should use `&Language` wherever a ref is
      expected.

---

## Performance backlog

- [ ] Rowan `NodeCache` allocation is the main memory-pressure point
      (flame graph): vendor rowan or pre-reserve, or drop the cache.
- [ ] Typed arenas per object type (low priority since type interning
      removed the main hashing bottleneck).
- [ ] Dense-integer-keyed maps -> `Vec`/arena indexing: `local_map`
      (mir/lower.rs), `string_index` (codegen), `fn_names` (dump) are hash
      maps keyed by dense newtype ids; direct indexing is O(1) with no
      hashing and better locality. Pairs with typed arenas.
- [x] PARALLEL.md sharing: the type interner needs a concurrent structure -
      DONE 2026-06-16 (S6). `TypeInterner` is now lock-free (`boxcar::Vec` +
      `papaya::HashMap`, `&self` intern), so the whole-file per-body walk fans
      out across rayon with no clone. The global _symbol_ table (cross-file)
      still wants the same treatment at the multifile milestone (`lasso`).
- [ ] `TypeKind::Fn { params, ret }` stores full `Vec<TypeRef>` copies
      while `Function::fn_type` already holds the interned handle;
      consider storing only the handle.

---

## Tests / tooling

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
