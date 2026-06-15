//! semantic token computation against cached salsa query results.
//!
//! unlike the pre-database LSP (which ran its own lexer + parser), this module
//! receives the already-compiled result from `database::lowered_file` and
//! enriches CST-only classification with HIR name resolution - specifically to
//! fix the A5 pattern-variable mis-classification (`BareIdentPat -> VARIABLE`
//! when the name is not a known enum variant).

mod cst;
mod token_kind;

use database::ParseResult;
use hir::core::HIR;
use lexer::{Lexed, SourceText};
use lsp_types::{SemanticToken, SemanticTokens};
use syntax::SyntaxKind;
use text_size::TextRange;

use crate::legend;

pub fn compute_semantic_tokens(
    source: &SourceText,
    lexed: &Lexed,
    parse: &ParseResult,
    hir: &HIR,
) -> anyhow::Result<SemanticTokens> {
    let green = parse.syntax();
    let classified = cst::classify_spans(&green, Some(hir));

    let mut data = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for token in &lexed.tokens {
        let kind = SyntaxKind::from(token.kind);
        if kind == SyntaxKind::Eof {
            continue;
        }

        let token_type = if kind == SyntaxKind::Ident {
            cst::lookup_ident(token.range, &classified).unwrap_or(legend::VARIABLE)
        } else {
            match token_kind::token_type_for_syntax_kind(kind) {
                Some(t) => t,
                None => continue,
            }
        };

        push_token(
            source,
            token.range,
            token_type,
            &mut data,
            &mut prev_line,
            &mut prev_start,
        );
    }

    Ok(SemanticTokens {
        result_id: None,
        data,
    })
}

fn push_token(
    source: &SourceText,
    range: TextRange,
    token_type: u32,
    data: &mut Vec<SemanticToken>,
    prev_line: &mut u32,
    prev_start: &mut u32,
) {
    // semantic-token columns and lengths are UTF-16 code units (the LSP
    // default encoding), not bytes. byte-based values mis-place and
    // over-extend every token at or after a multibyte character on its line,
    // and a strict client garbles the rest of the file from there.
    let lc = source.line_col_utf16(range.start());
    let line = lc.line.saturating_sub(1);
    let start_char = lc.col.saturating_sub(1);
    let length = source
        .slice(range)
        .map(|s| s.encode_utf16().count() as u32)
        .unwrap_or_else(|| u32::from(range.len()));

    let delta_line = line.saturating_sub(*prev_line);
    let delta_start = if delta_line == 0 {
        start_char.saturating_sub(*prev_start)
    } else {
        start_char
    };

    data.push(SemanticToken {
        delta_line,
        delta_start,
        length,
        token_type,
        token_modifiers_bitset: 0,
    });

    *prev_line = line;
    *prev_start = start_char;
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::Database;

    #[test]
    fn let_keyword_is_highlighted() {
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), "let x = 1;".into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let checked = database::lowered_file(&db, input);
        let hir = &checked.hir;
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, hir).unwrap();
        assert!(!tokens.data.is_empty());
        assert!(tokens.data.iter().any(|t| t.token_type == legend::KEYWORD));
    }

    #[test]
    fn cst_classifies_struct_fn_param_and_local() {
        let src = "\
structure Point { int32 x, };
add(int32 a, int32 b) -> int32 {
    let c = a;
    b;
}
";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let checked = database::lowered_file(&db, input);
        let hir = &checked.hir;
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, hir).unwrap();
        assert!(!tokens.data.is_empty());
    }

    /// semantic-token positions and lengths are UTF-16 code units, not bytes
    /// (the statistics.eye highlight failure: box-drawing and non-breaking
    /// hyphen characters in comments made every token length byte-inflated).
    #[test]
    fn semantic_tokens_use_utf16_units() {
        // the comment is 10 UTF-16 units (`-- ` + 6 box chars + `x`) but
        // 3 + 6 * 3 + 1 = 22 bytes. `let` follows on the same line.
        let src = "-- ──────x\nlet y = 1;";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let checked = database::lowered_file(&db, input);
        let hir = &checked.hir;
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, hir).unwrap();
        let comment = &tokens.data[0];
        assert_eq!(
            comment.length, 10,
            "comment length must be UTF-16 units, not bytes"
        );
        // `let` opens line 2: delta_line 1, column 0.
        let let_tok = &tokens.data[1];
        assert_eq!((let_tok.delta_line, let_tok.delta_start), (1, 0));
    }

    /// A5 regression: a bare-ident match pattern is a VARIABLE when the name
    /// is not a declared enum variant (a binding over a primitive scrutinee),
    /// and enum_member when it is.
    #[test]
    fn bare_ident_pat_uses_hir_variant_resolution() {
        let src = "\
enum E = Circle | Square;
f(E e, int32 x) -> int32 {
    match e { Circle -> 1, _ -> 2, };
    match x { y -> y, }
}
";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let checked = database::lowered_file(&db, input);
        let hir = &checked.hir;
        let classified = cst::classify_spans(&parse.syntax(), Some(hir));

        // `lookup_ident` keys on the token's exact range: (offset of the
        // pattern occurrence, ident length).
        let type_at = |needle: &str, len: u32| {
            let offset = src.find(needle).expect("needle in source") as u32;
            cst::lookup_ident(TextRange::at(offset.into(), len.into()), &classified)
        };
        // the pattern `Circle ->` (not the declaration) resolves as a variant.
        assert_eq!(
            type_at("Circle ->", 6),
            Some(legend::ENUM_MEMBER),
            "variant pattern must classify as ENUM_MEMBER"
        );
        // the binding `y ->` over an int scrutinee is a variable, not a variant.
        assert_eq!(
            type_at("y ->", 1),
            Some(legend::VARIABLE),
            "bare-ident binding must classify as VARIABLE (A5)"
        );
        let _ = lexed;
    }

    #[test]
    fn union_extern_as_keywords() {
        let src = "union U { int32 a, }; extern { f() -> int32; } as x = 0;";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let checked = database::lowered_file(&db, input);
        let hir = &checked.hir;
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, hir).unwrap();
        let keyword_count = tokens
            .data
            .iter()
            .filter(|t| t.token_type == legend::KEYWORD)
            .count();
        assert!(keyword_count >= 3, "expected union, extern, as as keywords");
    }
}
