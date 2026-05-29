//! Lexer-only token classification (keywords, literals, operators).

use syntax::SyntaxKind;

use crate::legend;

pub fn token_type_for_syntax_kind(kind: SyntaxKind) -> Option<u32> {
    match kind {
        SyntaxKind::Let
        | SyntaxKind::Mut
        | SyntaxKind::Structure
        | SyntaxKind::Enum
        | SyntaxKind::Union
        | SyntaxKind::Extern
        | SyntaxKind::If
        | SyntaxKind::Else
        | SyntaxKind::Loop
        | SyntaxKind::Break
        | SyntaxKind::Continue
        | SyntaxKind::Match
        | SyntaxKind::As
        | SyntaxKind::True
        | SyntaxKind::False
        | SyntaxKind::Underscore => Some(legend::KEYWORD),

        SyntaxKind::Int | SyntaxKind::Float => Some(legend::NUMBER),

        SyntaxKind::String | SyntaxKind::Char => Some(legend::STRING),

        SyntaxKind::Lcomment | SyntaxKind::Bcomment | SyntaxKind::Dcomment => Some(legend::COMMENT),

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
        | SyntaxKind::Pipe => Some(legend::OPERATOR),

        SyntaxKind::Ident => None,

        _ => None,
    }
}
