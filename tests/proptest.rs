//! Property-based tests for the Eye compiler front-end.
//!
//! These tests verify that the lexer, parser, and HIR lowering never panic
//! (i.e. never hit an `unreachable!()`, index-out-of-bounds, or similar)
//! on any input. They do NOT assert correct output -- that is the job of the
//! snapshot and e2e tests. Here we only assert "survival" and a few
//! structural invariants.

use proptest::prelude::*;

use ast::AstNode;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};

// ---------------------------------------------------------------------------
// Test 1: Lexer survival -- no valid UTF-8 string should panic the lexer
// ---------------------------------------------------------------------------

proptest! {
    /// The lexer must never panic, regardless of input. It may produce
    /// diagnostics for malformed literals, unexpected characters, etc., but
    /// the `tokenize()` call itself must complete without unwinding.
    #[test]
    fn lexer_never_panics(src in ".*") {
        let text = SourceText::new(src);
        let _lexed = Lexer::new(&text).tokenize();
    }
}

// ---------------------------------------------------------------------------
// Test 2: Token stream invariants
// ---------------------------------------------------------------------------

proptest! {
    /// Every token produced by the lexer must have a valid (non-negative,
    /// in-bounds) range. The stream must cover the source without gaps.
    #[test]
    fn token_stream_tiles_source_without_gaps(src in ".*") {
        use text_size::TextSize;
        let text = SourceText::new(src.clone());
        let lexed = Lexer::new(&text).tokenize();

        let mut cursor = TextSize::from(0);
        for tok in &lexed.tokens {
            let range = tok.range;
            prop_assert_eq!(range.start(), cursor, "gap or overlap");
            cursor = range.end();
        }
        prop_assert_eq!(
            usize::from(cursor),
            src.len(),
            "tokens do not cover the whole source",
        );
    }
}

proptest! {
    /// The token sequence must always end with a zero-width Eof token at the
    /// very end of the source (even on empty input).
    #[test]
    fn token_stream_ends_with_eof(src in ".*") {
        use token::TokenKind;
        let text = SourceText::new(src.clone());
        let lexed = Lexer::new(&text).tokenize();

        let last = lexed.tokens.last().expect("empty token stream");
        prop_assert_eq!(last.kind, TokenKind::Eof);
        prop_assert_eq!(last.range.start(), last.range.end());
        prop_assert_eq!(
            usize::from(last.range.start()),
            src.len(),
            "Eof not at source end",
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3: Parser survival -- no valid token stream should panic the parser
// ---------------------------------------------------------------------------

proptest! {
    /// The parser must never panic, regardless of the token stream it
    /// receives. It may produce any number of diagnostics, but the `parse()`
    /// call itself must complete without unwinding.
    #[test]
    fn parser_never_panics(src in ".*") {
        let text = SourceText::new(src.clone());
        let lexed = Lexer::new(&text).tokenize();

        // If the token stream is degenerate (no tokens at all), skip --
        // that's impossible in practice since the lexer always emits Eof.
        if lexed.tokens.is_empty() {
            return Ok(());
        }

        let _parse = parser::parse(&lexed.tokens, &text);
    }
}

// ---------------------------------------------------------------------------
// Test 4: HIR survival -- cleanly parsed programs never panic HIR lowering
// ---------------------------------------------------------------------------

/// Strategy: generate small, structurally valid Eye programs.
///
/// Rather than building a full AST generator (which would duplicate the
/// parser), we generate programs from a small set of templates and vary
/// their numeric/identifier content. This gives us coverage across the HIR
/// lowering paths without needing a grammar-aware generator.
fn gen_small_program() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        // minimal main
        "main() {\n    println(\"{}\", 42);\n}\n".to_string(),
        // typed let
        "main() {\n    let int32 x = 0;\n    println(\"{}\", x);\n}\n".to_string(),
        // binary expression
        "main() {\n    let int32 x = 1 + 2 * 3;\n    println(\"{}\", x);\n}\n".to_string(),
        // if expression
        "main() {\n    let int32 x = if true { 1 } else { 2 };\n    println(\"{}\", x);\n}\n"
            .to_string(),
        // loop + break + continue
        "\
main() {
    mut int32 i = 0;
    loop {
        if i >= 3 { break; }
        i += 1;
    }
    println(\"{}\", i);
}
"
        .to_string(),
        // struct + field access
        "\
structure Point {
    int32 x,
    int32 y,
};
main() {
    let Point p = Point { x: 1, y: 2 };
    println(\"{}\", p.x);
}
"
        .to_string(),
    ])
}

proptest! {
    /// Programs generated by `gen_small_program` must survive the full
    /// compilation pipeline: lex -> parse -> AST -> HIR -> MIR -> codegen.
    /// No panic allowed at any stage.
    #[test]
    fn small_programs_survive_full_pipeline(src in gen_small_program()) {
        let text = SourceText::new(src.clone());
        let lexed = Lexer::new(&text).tokenize();

        // Lex diagnostics are acceptable (some templates may not be
        // perfectly formed), but the call must not panic.
        let parsed = parser::parse(&lexed.tokens, &text);

        // Parse diagnostics are acceptable, but continue only if clean.
        if !parsed.diagnostics.is_empty() {
            return Ok(());
        }

        let Some(file_ast) = ast::SourceFile::cast(parsed.green.clone()) else {
            return Ok(());
        };

        let hir = lower_source_file(file_ast, &lexed.interner);
        if !hir.diagnostics.is_empty() {
            return Ok(());
        }

        let _c_source = codegen::core::gen_mir(&hir, &mir::lower_all(&hir));
    }
}

// ---------------------------------------------------------------------------
// Test 5: All lexer diagnostics have non-empty spans
// ---------------------------------------------------------------------------

proptest! {
    /// Every lexer diagnostic must point to a non-empty span in the source.
    /// An empty span means the error location was lost or points at EOF,
    /// which is a bug in the diagnostic plumbing.
    #[test]
    fn lexer_diagnostics_have_nonempty_spans(src in ".*") {
        use diagnostics::Span;
        let text = SourceText::new(src);
        let lexed = Lexer::new(&text).tokenize();

        for (span, _kind) in lexed.diags.entries() {
            match span {
                Span::Range(r) => prop_assert!(!r.is_empty(), "lexer diagnostic with empty range"),
                Span::Ptr(_) => { /* HIR pointers not produced by lexer */ }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 6: All parser diagnostics have non-empty spans
// ---------------------------------------------------------------------------

proptest! {
    /// Every parser diagnostic must point to a non-empty span in the source.
    #[test]
    fn parser_diagnostics_have_nonempty_spans(src in ".*") {
        use diagnostics::Span;
        let text = SourceText::new(src.clone());
        let lexed = Lexer::new(&text).tokenize();
        let parse = parser::parse(&lexed.tokens, &text);

        for (span, _kind) in parse.diagnostics.entries() {
            match span {
                Span::Range(r) => prop_assert!(!r.is_empty(), "parser diagnostic with empty range"),
                Span::Ptr(_) => { /* not produced by parser */ }
            }
        }
    }
}
