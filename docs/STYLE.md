# doc style

adopted 2026-06-12. applies to new docs and docs being substantially
edited; older docs migrate when touched, not in bulk.

two rules carried over unchanged: every claim matches the code on disk
(verify before writing), and no personality. this file adds the sigil
legend and the lowercase rule so status lives in one character and words
are spent on detail.

## sigils

| sigil | meaning |
|-------|---------|
| `+`   | built / true now / verified |
| `~`   | partial / in progress |
| `-`   | planned / not built |
| `!`   | limitation / risk / warning |
| `?`   | open question / undecided |
| `x`   | rejected / dead end (keep the reason) |
| `=`   | definition |
| `->`  | consequence / sequencing |

a status sigil leads the line it describes:

```
+ RawPtr kind: ptr is structural, Path("ptr") magic dead
~ typeck walker: types only; diagnostics still lowering's until S2
- effects fixpoint
! adjustments inert until coerce dies - shadow cannot validate them
? pointer-operand arithmetic judgment (specified at build, vs corpus)
x sharded-lock interner - replaced by lock-free boxcar+papaya same day
```

## lowercase

prose is lowercase, including sentence starts. casing is kept where it
is information: code identifiers (`TypeckResults`, `check_body`),
acronyms (HIR, MIR, LSP, C, FFI), file names (TYPECK.md), proper nouns
(Rust, Tarjan). doc titles are lowercase.

## unchanged

- no em-dashes; plain `-` only
- code blocks verbatim, never restyled
- open items live in docs/planning/ledger.md or registered FIXMEs, not
  in prose
