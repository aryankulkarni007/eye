# Architecture audit (snapshot)

> **Status: point-in-time external audit**, taken around the MIR cutover. It is a
> layer-by-layer professional-grade assessment, kept as a snapshot. Some findings
> have since moved: the MIR layer it praises is fully cut over (the only codegen
> path), and the typed-diagnostics direction is built ([DIAGNOSTICS.md](features/DIAGNOSTICS.md)).
> Its standing weaknesses - no standalone typecheck pass (Track 3, see
> [TYPECK.md](features/TYPECK.md)), `TypeRef` still name-based, C-backend leakage, partial
> LSP diagnostics - remain accurate. For current state see
> [CAPABILITIES.md](dev/CAPABILITIES.md) and the doc index ([README.md](dev/README.md)).

---

# Pipeline

Current compiler shape:

.eye source -> lexer -> rowan CST -> typed AST -> HIR -> MIR -> C -> clang -> native binary

The CLI driver wires this directly in src/main.rs:31: lex, parse, cast AST, lower to HIR, reject
diagnostics, then emit/compile. That is a clean, conventional pipeline.

Layer Audit

Layer Token/lexer
Architecture logos token rules live in token; lexer drives tokenization, tracks
line starts, interns identifiers/strings, emits typed lex
diagnostics.
Key decisions Token kinds are a shared leaf crate; lexer diagnostics are typed;
source can be mmap-backed.
Professional-language audit Strong. This is representative of serious compilers: isolated token
vocabulary, fast scanning, stable ranges, interning.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Syntax/CST
Architecture syntax defines one SyntaxKind for tokens + nodes and binds it to
rowan. Parser output is a lossless CST.
Key decisions Trivia is preserved in the tree but skipped for parser lookahead.
Professional-language audit Very professional for tooling. Lossless CST is exactly what editor-
aware compilers use.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Parser
Architecture Event-stream parser: flat Vec<Event>, marker API, recovery nodes,
diagnostics out-of-band. See crates/parser/src/lib.rs:1.
Key decisions rust-analyzer-style events, DropBomb markers, parser fuel guard,
Pratt expressions, recovery instead of early bailout.
Professional-language audit Strong. This is more professional than a simple recursive parser
that only builds AST and panics on malformed input.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Typed AST
Architecture Generated wrappers over CST from eye.ungram; lazy accessors, no
copying.
Key decisions AST is a typed view, not an owned semantic tree.
Professional-language audit Strong for language tooling. It keeps syntax and semantics separated
and makes grammar evolution manageable.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Diagnostics
Architecture Shared diagnostics crate with Diagnostic, Span, Severity, Code,
Sink; producing layers own typed error enums.
Key decisions Messages derive from structured error kinds; renderer isolated at
CLI edge.
Professional-language audit Strong direction. The weakness is error code numbering: HIR
currently maps each class to placeholder 001, so the code system is
not mature yet.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer HIR
Architecture Arena-backed semantic IR: item arenas, body arenas, typed IDs,
source maps, local scopes. See crates/hir/src/core.rs:1.
Key decisions Items collected before bodies, so forward refs work. Expressions
carry Resolution. Type refs remain mostly name-based.
Professional-language audit Mixed but good. Arena HIR + source maps are professional. The main
weakness is that lowering, name resolution, type stamping, and
semantic checks are still fused.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Type checking
Architecture No standalone pass yet. Types are stored in Body::expr_types,
populated opportunistically during HIR lowering.
Key decisions Current checks are pragmatic and local: let mismatch, return
mismatch, match arm checks, array length checks, print checks.
Professional-language audit Not yet professional-grade. This is the biggest gap. A serious
language needs a distinct typeck/inference result over finished HIR,
with complete type coverage before MIR/codegen.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer MIR
Architecture Structured, target-neutral-ish IR with locals, operands, places,
rvalues, explicit temps, and structured control flow. See crates/
mir/src/core.rs:1.
Key decisions Three-address value model; operands are trivial; value if/match
lowers into temps; short-circuiting is control flow, not eager
binary ops.
Professional-language audit Strong. This is the best architectural move in the repo. It moves
semantic lowering out of codegen and makes future backend work
plausible.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Codegen
Architecture MIR-to-C “direct printer”; emits declarations, functions, control
flow, arrays, structs, unions, enums. See crates/codegen/src/core/
mir_emit.rs:1.
Key decisions C backend is mechanical; clang does final compilation; main is
special-cased to return int.
Professional-language audit Good for a bootstrap compiler. Not yet production-grade because C is
still the semantic escape hatch, but the MIR boundary keeps this
from poisoning the whole architecture.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Backend driver
Architecture Writes generated C, optionally formats with clang-format, invokes
clang -O2.
Key decisions Simple transpiler backend rather than native object/codegen backend.
Professional-language audit Fine for early language work. Professional as a prototype, not as a
final compiler backend.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer LSP
Architecture Separate crate, currently semantic highlighting + parser
diagnostics.
Key decisions Reuses lexer/parser/CST rather than inventing a parallel parser.
Professional-language audit Good foundation, incomplete capability. It currently does not
surface full HIR/type diagnostics.
──────────────────────────────────────────────────────────────────────────────────────────────────
Layer Tests
Architecture Unit/snapshot tests plus e2e tests that compile Eye programs and run
native binaries.
Key decisions Tests assert observable output, not just generated C text.
Professional-language audit Good. Needs more negative tests and pass-specific invariant tests as
the language grows.

Major Design Wins
The strongest design decision is the rust-analyzer-inspired front end: lossless CST, generated
typed AST, arena-backed HIR, and source maps. That is a professional tooling architecture, not a
toy compiler architecture.

The second strong decision is MIR. The repo explicitly moved control-flow/value lowering out of
codegen. The MIR invariant that RValue arguments are trivial operands is the right kind of
constraint: it makes codegen boring and makes evaluation order explicit.

Typed diagnostics are also a serious step. Layer-owned error enums plus a shared carrier avoids
stringly-typed compiler errors and keeps tests from depending on prose.

Main Weaknesses
The type system architecture is not there yet. HIR lowering currently does too much: construction,
name resolution, type stamping, and semantic validation. That is workable for a small language,
but it will become brittle as features like generics, richer inference, effects, traits/
interfaces, modules, or macros arrive.

TypeRef is still mostly unresolved names reused into MIR and C. That means the compiler lacks a
canonical semantic type layer. Professional compilers usually have parsed type syntax, resolved
type IDs, and type-check results as distinct concepts.

The C backend still leaks into design decisions: print is an intrinsic, arrays use C wrapper
structs, main is special-cased, and some type rendering maps directly to C. This is acceptable for
bootstrapping, but not representative of a finished professional language backend.

The aspirational docs overstate the current language. The “forensic/meta-platform/effect-tracking/
bridge” vision is not implemented. The real compiler is a conventional statically typed C-
transpiled language with a good front-end architecture and an emerging MIR.

Verdict
As a compiler project, this is significantly more professional than a typical hobby language: the
CST/AST/HIR/MIR layering, diagnostics model, source maps, parser recovery, and e2e tests are all
serious choices.

As a professional language implementation, it is mid-stage. The architecture is pointing in the
right direction, but the missing standalone type-check pass, unresolved semantic type model,
limited LSP diagnostics, and C-backend dependence are the gaps that separate it from production-
grade language infrastructure.
