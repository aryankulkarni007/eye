//! EXPERIMENTAL: Semantic token computation against cached salsa query results.
//!
//! Unlike the pre-Database LSP (which ran its own lexer + parser), this module
//! receives the already-compiled result from `database::lowered_file` and
//! enriches CST-only classification with HIR name resolution — specifically to
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
    let lc = source.line_col(range.start());
    let line = lc.line.saturating_sub(1);
    let start_char = lc.col.saturating_sub(1);
    let length = u32::from(range.len());

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
        let mut db = Database::default();
        let input = database::SourceFileInput::new(&mut db, "test.eye".into(), "let x = 1;".into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let hir = database::lowered_file(&db, input);
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, &hir).unwrap();
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
        let mut db = Database::default();
        let input = database::SourceFileInput::new(&mut db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let hir = database::lowered_file(&db, input);
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, &hir).unwrap();
        assert!(!tokens.data.is_empty());
    }

    #[test]
    fn union_extern_as_keywords() {
        let src = "union U { int32 a, }; extern { f() -> int32; } as x = 0;";
        let mut db = Database::default();
        let input = database::SourceFileInput::new(&mut db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let lexed = database::lex(&db, input);
        let parse = database::parse(&db, input);
        let hir = database::lowered_file(&db, input);
        let tokens = compute_semantic_tokens(&source, &lexed, &parse, &hir).unwrap();
        let keyword_count = tokens
            .data
            .iter()
            .filter(|t| t.token_type == legend::KEYWORD)
            .count();
        assert!(keyword_count >= 3, "expected union, extern, as as keywords");
    }
}
