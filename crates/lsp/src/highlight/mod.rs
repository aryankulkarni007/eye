//! Semantic token computation: CST classification merged with lexer tokens.

mod cst;
mod token_kind;

use lexer::{Lexer, SourceText};
use lsp_types::{SemanticToken, SemanticTokens};
use syntax::SyntaxKind;
use text_size::TextRange;

use parser::parse;

use crate::legend;

pub fn compute_semantic_tokens(text: &str) -> anyhow::Result<SemanticTokens> {
    let source = SourceText::new(text.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parse(&lexed.tokens, &source);

    let classified = cst::classify_spans(&parse.green);

    let mut data = Vec::new();
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for token in lexed.tokens {
        if token.kind == token::TokenKind::Eof {
            continue;
        }

        let kind = SyntaxKind::from(token.kind);
        let token_type = if kind == SyntaxKind::Ident {
            cst::lookup_ident(token.range, &classified).unwrap_or(legend::VARIABLE)
        } else {
            match token_kind::token_type_for_syntax_kind(kind) {
                Some(t) => t,
                None => continue,
            }
        };

        push_token(
            &source,
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
    use crate::legend;

    #[test]
    fn let_keyword_is_highlighted() {
        let tokens = compute_semantic_tokens("let x = 1;").unwrap();
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
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parse(&lexed.tokens, &source);
        let classified = cst::classify_spans(&parse.green);

        let types_for = |name: &str| -> Vec<u32> {
            lexed
                .tokens
                .iter()
                .filter(|t| t.kind == token::TokenKind::Ident)
                .filter(|t| &src[t.range] == name)
                .filter_map(|t| cst::lookup_ident(t.range, &classified))
                .collect()
        };

        assert!(types_for("Point").contains(&legend::STRUCT));
        assert!(types_for("add").contains(&legend::FUNCTION));
        assert!(types_for("a").contains(&legend::PARAMETER));
        assert!(types_for("c").contains(&legend::VARIABLE));
        assert!(types_for("x").contains(&legend::PROPERTY));
    }

    #[test]
    fn union_extern_as_keywords() {
        let tokens = compute_semantic_tokens("union U { int32 a, }; extern { f() -> int32; } as x = 0;").unwrap();
        let keyword_count = tokens
            .data
            .iter()
            .filter(|t| t.token_type == legend::KEYWORD)
            .count();
        assert!(keyword_count >= 3, "expected union, extern, as as keywords");
    }
}
