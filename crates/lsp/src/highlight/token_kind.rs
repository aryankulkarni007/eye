//! Lexer-only token classification (keywords, literals, operators).

use syntax::SyntaxKind;

use crate::legend;

pub fn token_type_for_syntax_kind(kind: SyntaxKind) -> Option<u32> {
    match kind {
        SyntaxKind::Let
        | SyntaxKind::Mut
        | SyntaxKind::Const
        | SyntaxKind::Structure
        | SyntaxKind::Enum
        | SyntaxKind::Union
        | SyntaxKind::Extern
        | SyntaxKind::Type
        | SyntaxKind::If
        | SyntaxKind::Else
        | SyntaxKind::Loop
        | SyntaxKind::Break
        | SyntaxKind::Continue
        | SyntaxKind::Return
        | SyntaxKind::Match
        | SyntaxKind::As
        | SyntaxKind::True
        | SyntaxKind::False
        | SyntaxKind::Underscore => Some(legend::KEYWORD),

        SyntaxKind::Int | SyntaxKind::Float => Some(legend::NUMBER),

        SyntaxKind::String | SyntaxKind::Char => Some(legend::STRING),

        SyntaxKind::Lcomment | SyntaxKind::Bcomment | SyntaxKind::Dcomment => Some(legend::COMMENT),

        SyntaxKind::Assign
        | SyntaxKind::PlusEq
        | SyntaxKind::MinusEq
        | SyntaxKind::StarEq
        | SyntaxKind::SlashEq
        | SyntaxKind::PercentEq
        | SyntaxKind::AmpEq
        | SyntaxKind::PipeEq
        | SyntaxKind::CaretEq
        | SyntaxKind::ShlEq
        | SyntaxKind::ShrEq
        | SyntaxKind::Plus
        | SyntaxKind::Minus
        | SyntaxKind::Star
        | SyntaxKind::Slash
        | SyntaxKind::Percent
        | SyntaxKind::And
        | SyntaxKind::Or
        | SyntaxKind::Bang
        | SyntaxKind::Eq
        | SyntaxKind::Neq
        | SyntaxKind::Lt
        | SyntaxKind::Gt
        | SyntaxKind::Leq
        | SyntaxKind::Geq
        | SyntaxKind::Tilde
        | SyntaxKind::Caret
        | SyntaxKind::Shl
        | SyntaxKind::Shr
        | SyntaxKind::Arrow
        | SyntaxKind::Farrow
        | SyntaxKind::Dot
        | SyntaxKind::Ellipsis
        | SyntaxKind::Amp
        | SyntaxKind::Pipe => Some(legend::OPERATOR),

        SyntaxKind::Ident => None,

        _ => None,
    }
}
