# TODO

- [x] const decl should be allowed inside functions i.e. not necessarily global
      scope. Done 2026-06-11: `const` is a statement (same `ConstDef` node),
      folded at the declaration site against top-level + enclosing local consts,
      value in `Body::local_consts`, inlined like a top-level const (`&`/assign
      rejected, array lengths work, lexical scoping + shadowing). See
      docs/features/CONST.md "Block-scope const".

## Bugs - match guards (S3, found 2026-06-10 in review of `9bfcf49..HEAD`) - RESOLVED 2026-06-10

All masked by `eyesrc/lang/guard_example.eye` testing only the `A if flag`
(bare-local) guard shape. Fixed via the `Guard { stmts, cond }` MIR node + a
flag-gated codegen chain (`gen_guarded_switch`); see `docs/features/MATCH.md`.

- [x] 🔴 Ordinary/comparison guards miscompile - no fallthrough on false guard.
      Any switch with a guard is now a flag-gated chain: a false guard leaves the
      flag unset and the next arm's test is re-checked. e2e:
      `complex_match_guard_falls_through`.
- [x] 🔴 Guard on `_` silently dropped. A guarded catch-all is now an ordered
      `ArmTest::Always` arm, not the `default` slot, so its guard runs and falls
      through. Guarded `_`/binding catch-alls are fully supported (`_ if c`,
      `x if c`); e2e `guarded_wildcard_catchall_falls_through`,
      `guarded_binding_catchall_falls_through`.
- [x] 🟡 Value-position block / else-less value `if` reads uninitialized temp.
      Diagnosed in HIR (`VoidValueInValuePosition`); a guarded match also no
      longer discharges coverage, so a guarded full-coverage match needs an
      unconditional catch-all (keeps the hoist temp initialized).
- [x] ❓ Multiple irrefutable arms - last `default` silently wins. HIR rejects the
      unreachable trailing arm (`UnreachableAfterWildcard`); test
      `match_multiple_irrefutable_arms_rejected`.
- [x] 🔵 Stale doc: `crates/mir/src/core.rs` `RValue::Print` -> `Println`.

Deferred (clean reject, not a miscompile): struct patterns in match arms
(`GrammarError::StructPatInMatchArm`), or-patterns and ranges (parser
`ExpectedMatchArm`). These are S3/S4 features, not bugs.

## Bugs - lang.eye port audit (2026-06-11)

`eyesrc/programs/lang.eye` (C language-simulator port) exposed these. Every
item below was reproduced against the tree on 2026-06-11. SUPERSEDED the same
day by the full C-leak audit: `docs/design/CLEAK.md` is now the authoritative
ledger (this section's items appear there as L1/L2/L8/L6/M2/L10, plus new
findings L3/L4/L5/L7/M1/P1/P2), and `docs/planning/ledger.md` carries the
open rows. Status below updated where the 2026-06-11 mechanical pass fixed
items; the file still fails to compile on item 1.

### C-leak bugs (no-footgun violations: Eye accepts, clang errors or emits wrong C)

- [ ] 🔴 String decay missing at struct-literal field init. `Syllable { str: "cvc" }`
      (field `string str`) emits `.str = (__eye_arr_3_5uint8*)__eye_str3` into a
      `const char*` field - clang `incompatible-pointer-types` error. Decay was
      built at 4 sites (let-init / arg / return / cast); struct-lit field init is
      a missing 5th site. This is the lang.eye compile blocker.
- [ ] 🔴 Same decay gap at array-literal elements. `let [char*; 3] xs = ["a","b","c"]`
      puts wrapper-pointer casts into `char*` elements - clang error. The
      `[ptr; 10] = [""; 10]` workaround in lang.eye compiles only by accident
      (C implicitly converts any `T*` to `void*`).
- [x] 🔴 C-keyword field names emit illegal C. Fixed 2026-06-11: R010
      `NameIsCKeyword` rejects at collect, for fields and every other name the
      backend emits verbatim (items, parameters, enum variants, globals,
      opaque types). Reject chosen over mangling so the emitted C and any
      debugger keep the source name. CLEAK L8.
- [ ] 🟡 Undeclared field type leaks a raw clang error. `structure Arena { off off, }`
      emits `off off;` - "unknown type name 'off'". Root: no type-name resolution
      pass exists (same family as opaque value-position misuse); lands with the
      typeck split, listed because lang.eye hit it in practice.
- [ ] 🟡 Mixed-width integer arithmetic silently narrows. In `align_alloc`,
      `(7 - (current_addr & mask))` with `usize` operands emits
      `int32_t _t6 = (7 - _t5);` - the int-literal side types the temp `int32`
      while the C expression is `size_t`, so C silently truncates 64 -> 32 bits.
      Harmless for this padding math (value <= 7), a miscompile class in general.
- [x] 🟡 Empty string emits nonstandard C. Fixed 2026-06-11: the wrapper
      storage pads to `data[1]` (the type-level length stays 0; only `""` can
      produce a zero-length wrapper, `[T; 0]` is rejected upstream). Standard
      C, runs, passes the strict gate. CLEAK L10.

### Typeck-absence cluster (all land with the Horizon 1 split; recorded for coverage)

lang.eye hit each of these in practice:

- [ ] struct-literal field *value* type unchecked: `P { x: "hello" }` with
      `int32 x` reaches clang. (Missing *fields* ARE caught -
      `StructLitMissingFields` fired correctly, verified.)
- [ ] call arguments unchecked (arity, types, order) for defined and extern fns -
      `generate_lang(&lang, &arena, syllable, 10)` would accept swapped args.
- [ ] `as` casts unrestricted, any-to-any (`arena.off as ptr` = int-to-pointer).
      The cast lattice (what converts, what needs explicit blessing) is a typeck
      design item.
- [ ] `const` declared type vs folded value unchecked (existing DEFER row;
      lang.eye re-hit it: `const ptr NULL = 0 as ptr` folds to a bare `0`, the
      cast is stripped by the fold - works, but nothing checked `ptr` against it).

### Shortcomings / design questions (not bugs; decide, do not just fix)

- [ ] void-fn tail call without `;` is legal: `reset(...) { free(arena.buffer) }`
      compiles - the call is the block tail expression and its value (void) is
      discarded. Consistent with expression-block semantics, surprising to C
      eyes. Ratify or require `;` for statement-position calls.
- [ ] no CLI arguments: `main` takes no parameters, argc/argv unreachable.
      Post-freeze feature (needs a slice/string story, or a raw `ptr` form).
- [ ] self-referential structs still impossible (no null, no two-phase init), so
      no linked list - lang.eye ports the arena instead. Known
      (`RecursiveValueType` by-value; `&Node` fields work but cannot be
      initialized self-referentially). Needs the runtime-safety/null theme.
- [ ] generated C is hard to debug: every subexpression spills to a `_tN` temp
      (MIR operand spilling). Consider a readable-C mode - keep nested
      expressions where legal, gate full spilling behind a flag (lang.eye's own
      FIXME asks for this).
- [ ] tree-sitter *highlighting* still wrong despite the 2026-06-11 grammar.js
      re-sync - the queries (highlights.scm) were not audited for the new nodes
      (const statement, guards, struct patterns, `extern type`, variadic).
      eye-tools repo, not this one.

## Continuity - next session pickup (updated 2026-06-11, harden-first pass)

ROADMAP REORDERED 2026-06-11: trust in the pipeline dropped after the lang.eye
audit; the kernel freeze and typeck split now WAIT until pipeline correctness
is established. New order (docs/design/CLEAK.md "Fix order"):

1. DONE: C-leak audit (`docs/design/CLEAK.md`) - every implicit type decision
   in HIR lowering / MIR lowering / codegen, classified M/L/P/T, M+L rows all
   reproduced. `docs/planning/ledger.md` carries the open rows.
2. DONE: detection infrastructure - `scripts/check-c-strict.sh` (pedantic
   clang over generated corpus C), CI `corpus` job, XFAIL list in
   check_all.sh (linkedlist intentional, lang.eye known bug, stale-checked).
3. DONE: mechanical fixes - M3 exhaustive-match uninit-temp UB (last arm now
   `else`), L8 C-keyword names (R010 at collect), L9 `f(void)` prototypes,
   L10 empty-string `data[1]`, L11 `%p`/`ptr` printf specs + `(void*)` casts.
4. NEXT ACTION: coercion-point unification - one `coerce(expr, expected)`
   replacing the 4 scattered `maybe_decay` sites and covering struct-lit
   fields + array-lit elements (closes L1/L2/most-of-L4, un-breaks lang.eye;
   remove its XFAIL entry when green). Mechanical companions in the same
   pass: L3 call arity, L5 struct-name existence, L6 field-type names, L7
   `ptr` indexing reject, M1 literal range check at annotated sites.
5. THEN: typeck split (Horizon 1), scoped by CLEAK's T section; then match
   S4/S5 on the typed pipeline; freeze LAST (acceptance test: lang.eye
   compiles and runs clean, strict gate fully green with no XFAIL).

State: 297 tests green, clippy 0, corpus 41/41 + 2 XFAIL, strict gate 41/41,
all uncommitted. `println` reclassification (prime-era stdlib eviction) and
match S4/S5 scope notes from the earlier 2026-06-11 session still stand;
match work is now sequenced after typeck.

## Compiler architecture

- [ ] No separate type-checking pass. HIR has no dedicated type
      inference/resolution pass; type refs stay as `Path(name)` until codegen,
      where lookup happens against `ItemScope`. Consequences: - [ ] No type inference for `let x = expr;` (no annotation) is supported. - [ ] Type errors like `StructLitMissingFields` are reported in lowering
      (pass 3), which is late. - [ ] A dedicated type-check phase would improve errors and enable
      inference.

- [ ] fs caching -> a source manager or VFS that loads source text and does what
      it needs when the requested source is not loaded.

- [ ] `c_fn_name` and main shim fragility (`codegen/src/core/mir_emit.rs:94-100`).
      User `main` is renamed to `__eye_main` and a C `main` shim is generated.
      Works, but the symbolic debugger sees `__eye_main`, not `main`. Consider
      `__attribute__((weak))` or a linker alias for debug UX.

- [ ] `println` intrinsic in MIR (`crates/mir/src/core.rs:174-176`). Carried as
      a dedicated `RValue::Println` sniffed during lowering by unresolved callee
      name - a thin pass-through. To remove later (compose `printf` in stdlib)
      you'll need a pre-lowering pass to detect `println` calls and rewrite them.

## Performance

- [ ] vendor rowan -> flame graph shows we could allocate `NodeCache` with an
      initial reserve (main memory-pressure point), or maybe don't cache.

- [ ] typed arenas for each object type (e.g. typed-arena or generational-arena
      for items; low priority now that type interning removes the main hashing
      bottleneck).

- [ ] dense-integer-keyed maps -> `Vec`/arena indexing (non-trivial; approach in
      natural order). The maps keyed by dense newtype ids - `local_map`
      (`HirLocalId -> LocalId`, mir/lower.rs), `string_index` (codegen),
      `fn_names` (dump) - should not be hash maps at all: direct `Vec`/arena
      indexing is O(1) with no hashing, no collisions, better cache locality.
      Flagged during the 2026-06-10 FxHashMap sweep; the real win for those keys
      is structural, not the hasher. Pairs with the typed-arenas item above.

- [ ] FxHashMap vs parallelism (PARALLEL.md). FxHashMap is now the workspace
      convention (single-threaded, rustc's choice) - good for the current batch
      pipeline. But PARALLEL.md plans parallel bidirectional type + effect
      inference over a shared global symbol table. A plain `FxHashMap` shared
      across worker threads will not do: those specific shared tables (the type
      interner, the global symbol table) will need a concurrent structure
      (sharded map / `dashmap` / `RwLock`, or per-thread-collect-then-merge) when
      that lands. The Fx hasher itself stays usable _inside_ whatever concurrent
      map is chosen; this is a map-structure decision, not a hasher swap.

- [ ] `fn_type` and `TypeKind::Fn` duplication. `Function::fn_type:
Option<TypeRef>` is computed once, but `TypeKind::Fn { params, ret }`
      stores full `Vec<TypeRef>` copies. For many-param functions this
      duplicates interned handles. Consider storing just the `TypeRef` handle to
      the fn type (already in `fn_type`), not the structural `TypeKind::Fn`.

## Parser / lexer / diagnostics

- [ ] audit intelligent error spans. Reduce calculations when lexing and trim
      spans at emit time intelligently; only scan for intelligent spans on the
      error path.

- [ ] re-evaluate parser sync mechanism. Resilient, but recover to next valid
      code without producing rubbish diagnostics. The parser needs to understand
      the code.

- [ ] Error code numbering as `u16 + 1` (`crates/parser/src/errors.rs:127`).
      `*self as u16 + 1` relies on variant ordering; adding a variant in the
      middle shifts subsequent codes. Use an explicit numbering scheme (as the
      HIR errors have) for API stability.

- [ ] `Interner::lookup` panics on unknown symbol
      (`crates/lexer/src/lib.rs:135-137`). `self.vec[id.0 as usize]` panics if
      `id` comes from a different `Interner`. Safe today (interner handed off
      with `Lexed`), but a debug-only bound check with a friendly message would
      help catch bugs.

## CLI / tests

- [ ] CLI `--check` flag only checks parsing (`src/main.rs:61-63`). Documents
      correctly that it stops before HIR. Consider renaming to `--parse-only`.

- [ ] implement fuzz testing (we have the formal grammar; this should be
      possible).

- [ ] No `--help` / error-message tests. e2e tests only check successful
      execution. Add tests asserting specific error messages/diagnostics for
      malformed programs.

## Tooling

- [ ] add graphviz and a relevant flag so we can visualise internal logic. Make
      it aesthetic too (low priority).

## Minor nits / consistency

All closed 2026-06-11.

- [x] `FxHashMap` vs `HashMap` throughout. Resolved by the 2026-06-10 FxHashMap
      sweep: every file imports `rustc_hash::FxHashMap` directly, zero
      `std::collections::HashMap` remains. The convention is direct `FxHashMap`,
      not the once-suggested `use FxHashMap as HashMap` alias.

- [x] text-size vs `rowan::TextRange`. `trimmed_text_range` now returns
      `text_size::TextRange` (syntax gained the workspace `text-size` dep),
      matching `diagnostics::Span` and every other crate's spelling. Same type,
      one path.

- [x] `no_struct_lit` `Cell<bool>` - already a plain `bool`
      (`crates/parser/src/lib.rs:89`); fixed in an earlier pass, entry was stale.

- [x] `pub types: RefCell<TypeInterner>` - ratified as deliberate, documented at
      the field (`crates/hir/src/core.rs`): single-threaded pipeline, passes run
      to completion, `&mut HIR` plumbing through 100+ sites buys nothing. Borrow
      discipline stated: never hold a `borrow()` guard across a call that may
      intern. Revisit only when passes become re-entrant or parallel
      (PARALLEL.md).

- [x] `LineCol` "u32 to save memory" comment - already rewritten accurately
      (halves struct size, ~4B line/col cap stated); entry was stale.

- [x] block-emit FIXME in `mir_emit.rs` - replaced with a
      `block(close, body: impl FnOnce(&mut Self))` helper (bump indent, run
      body, restore, emit indented close). Applied at all 9 repeat sites: fn
      body, if/else, loop, switch arms + default, guarded-switch arm/inner/
      default, enum body, record def. Caller still writes the opening line
      (varies too much to fold in). Emitted C is byte-identical; e2e green.

## Done

- [x] Match-guard codegen (S3) - `ArmTest::Const`/`Variant`, simple-guard `&&`
      chain. NOTE: see Bugs section above - complex/wildcard guards still broken.
- [x] type interning done 2026-06-08. `TypeRef(u32)` handle-based interning via
      `TypeInterner` (arena: `Vec<TypeKind>`, map: `FxHashMap<TypeKind, TypeRef>`).
      Pre-injected primitives (int8..64, uint8..64, float32/64, bool, char,
      string, usize, isize, ptr, void, Error). `TypeRef` is `Copy` - O(1)
      clone/hash/eq. `RefCell<TypeInterner>` on `HIR` for interior mutability
      during lowering. Patterns and constructions updated through all 10+
      crates/modules. `Rc<TypeRef>` eliminated; `TypeNode::Fn` changed from
      `Option<Rc<TypeRef>>` to `Option<TypeRef>`.
- [x] Tarjan's SCC for cycle detection (replaced O(V²) `reaches_from` DFS).
- [x] Cache pre-built Fn `TypeRef` on `Function` (avoids rebuilding per param
      reference).
- [x] Stabilized EXPERIMENTAL labels (guards, short-circuit, value-position
      blocks).
- [x] ryu + itoa for faster float/int formatting.
- [x] Unique diagnostic codes assigned per variant.
- [x] `local_names` switched to SmolStr (compact inline storage for short names).
