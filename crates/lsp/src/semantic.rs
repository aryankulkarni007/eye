use lexer::{Lexer, SourceText};
use lsp_types::{SemanticToken, SemanticTokens};
use syntax::SyntaxKind;

pub fn compute_semantic_tokens(text: &str) -> anyhow::Result<SemanticTokens> {
    let mut data = Vec::new();

    let source = SourceText::new(text.to_string());
    let lexed = Lexer::new(&source).tokenize();

    let mut prev_line = 0;
    let mut prev_start = 0;

    for token in lexed.tokens {
        let kind = SyntaxKind::from(token.kind);

        let token_type = match kind {
            SyntaxKind::Let
            | SyntaxKind::Mut
            | SyntaxKind::Structure
            | SyntaxKind::Enum
            | SyntaxKind::If
            | SyntaxKind::Else
            | SyntaxKind::Loop
            | SyntaxKind::Break
            | SyntaxKind::Continue
            | SyntaxKind::Match
            | SyntaxKind::True
            | SyntaxKind::False => 9,

            SyntaxKind::Ident => 4,

            SyntaxKind::Int | SyntaxKind::Float => 12,

            SyntaxKind::String | SyntaxKind::Char => 11,

            SyntaxKind::Lcomment | SyntaxKind::Bcomment | SyntaxKind::Dcomment => 10,

            SyntaxKind::Assign
            | SyntaxKind::Plus
            | SyntaxKind::Minus
            | SyntaxKind::Star
            | SyntaxKind::Slash
            | SyntaxKind::And
            | SyntaxKind::Or
            | SyntaxKind::Eq
            | SyntaxKind::Neq
            | SyntaxKind::Lt
            | SyntaxKind::Gt
            | SyntaxKind::Leq
            | SyntaxKind::Geq
            | SyntaxKind::Arrow
            | SyntaxKind::Farrow
            | SyntaxKind::Dot
            | SyntaxKind::Amp
            | SyntaxKind::Pipe => 13,

            _ => continue,
        };

        let lc = source.line_col(token.range.start());

        let line = lc.line - 1;
        let start_char = lc.col - 1;
        let length = u32::from(token.range.len());

        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 {
            start_char - prev_start
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

        prev_line = line;
        prev_start = start_char;
    }

    Ok(SemanticTokens {
        result_id: None,
        data,
    })
}
