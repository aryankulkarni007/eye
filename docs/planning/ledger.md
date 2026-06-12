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

- [ ] **M2 - mixed-width arithmetic narrows** [typeck]. A binary expression
      takes the LHS type, so `(7 - (current_addr & mask))` with `usize`
      operands types `int32` from the literal `7` and the MIR temp truncates
      the C `size_t` result to 32 bits (lang.eye `align_alloc`). Also
      asymmetric: `x + 7` types as `x`, `7 + x` as `int32`. Miscompile
      class; needs an operand-unification rule - typeck's first real
      customer. (CLEAK M2, VERIFIED 2026-06-11.)

- [ ] **L4 residue - cross-element array-literal types unchecked** [typeck].
      `let [int32; 3] xs = [1, true, "x"]`: the string element errors in
      clang, the bool converts silently. The literal-typing/decay half was
      closed 2026-06-11 by `coerce`; the type *judgment* against the element
      type remains. (CLEAK L4 PARTIAL.)

- [ ] **U2 - const-eval `apply_int` wraps unchecked** [typeck]. `const int8
      X = 200` evaluates to 200, not an error (and not -56); no
      value-in-range check against the declared type.

- [ ] **U4 - const-eval `apply_cast` silently truncates** [typeck].
      int->float can hit inf, float->int truncates, char/bool->int ignores
      signedness.

- [ ] **A3 - `mir_type_of` falls back to `int32`** [typeck]
      (`crates/mir/src/lower.rs`). A missing `expr_types` entry silently
      types a temp `int32` - measured never to fire on the corpus, but it
      is the silent amplifier under every typing gap above. An inline fix
      was attempted 2026-06-11 and reverted (false positives); typeck flips
      it to a hard error.

All non-typeck open bugs were closed 2026-06-12 (see the loose-ends
completed entry); the rows above are the typeck pass's scope and are
accepted as documented limitations of the frozen kernel.

---

## Typeck split scope (Horizon 1, post-freeze)

The consolidated requirements list for the pass design. No fixes here until
the pass exists.

- Struct-literal field **value** types unchecked: `P { x: "hello" }` with
  `int32 x` reaches clang. (Missing *fields* ARE caught.)
- Call argument **types** unchecked (arity is checked, T026): swapped args
  accepted. Source marker: `eyesrc/programs/lang.eye` (FIXME at the
  `generate_lang` call).
- `as` casts unrestricted any-to-any; the cast lattice (what converts, what
  needs explicit blessing) is a design item.
- `const` declared type vs folded value unchecked (`const bool B = 5`;
  also a DEFER row). lang.eye re-hit: `const ptr NULL = 0 as ptr` folds to
  a bare `0`, nothing checks `ptr` against it.
- M2 operand unification (open-bug row above).
- Integer-literal typing: the `int32` default replaced by expected-type
  propagation (the T030 range sweep already guards the values).
- A3 `mir_type_of` fallback flipped to a hard error (open-bug row above).
- `types_compatible` integer-family leniency (any int matches any int)
  removed with the literal default.
- Assignment expressions type as their RHS, not the target.
- `ptr` duality: magic `Path("ptr")` vs `Ptr(inner)` appears at every type
  dispatch; give `ptr` a real representation.
- Fn-type variadic flag: `TypeKind::Fn` carries none, so indirect calls
  through a fn-pointer value skip the arity check (L3 residue).
- U2/U4 const-eval range and cast checks (open-bug rows above).
- L4 cross-element judgment (open-bug row above).
- Tail-expression type enforcement in value-position blocks: a tail whose
  type does not match the required type is accepted (`malloc(...)` tail in
  an `int32*`-typed `if` arm). Source marker: `eyesrc/programs/lang.eye`
  (FIXME at the `onset_head` init).

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

- [ ] **Salsa structural backdating** (SALSA.md divergence 5). `Memo<T>`
      equality is `Arc::ptr_eq`, so every re-executed query counts as
      changed and dependents re-run - conservative, never stale. Per-type
      structural equality opt-in (e.g. token-stream equality letting a
      comment edit stop at `lex`) is the unlock for real incrementality;
      matters first for LSP latency. Pairs with the two rows below.
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
- [ ] PARALLEL.md sharing: the type interner and global symbol table will
      need a concurrent structure (sharded map / per-thread-collect-merge)
      when parallel inference lands; map-structure decision, not a hasher
      swap.
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

| location | ledger row |
|----------|------------|
| `eyesrc/programs/lang.eye` top-of-file FIXME | readable-C mode (design questions) |
| `eyesrc/programs/lang.eye` `off off` FIXME | typeck split scope (value/arg types) |
| `eyesrc/programs/lang.eye` `onset_head` FIXME | typeck scope: tail-expression type enforcement |
| `eyesrc/programs/lang.eye` `generate_lang` FIXME | typeck scope: call argument types |

---

## Completed

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
