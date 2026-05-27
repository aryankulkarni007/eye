//! The unified syntax-kind enum and the `rowan` language binding.
//!
//! `rowan` keeps one kind enum for every node in the tree - leaves (tokens)
//! and internal nodes alike - so [`SyntaxKind`] is the superset of the lexer's
//! [`TokenKind`] plus the grammar's node kinds.

use token::TokenKind;

/// Defines [`SyntaxKind`] from a single variant list and derives the
/// `u16` -> variant lookup from it, so the `repr` discriminants and the
/// reverse mapping can never drift apart.
macro_rules! syntax_kinds {
    ($($variant:ident),* $(,)?) => {
        /// Every kind of node that can appear in the concrete syntax tree.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        pub enum SyntaxKind {
            $($variant),*
        }

        impl SyntaxKind {
            /// Inverse of `self as u16`. `raw` must come from a prior
            /// `kind as u16` on this same enum (rowan upholds this).
            #[inline]
            fn from_u16(raw: u16) -> SyntaxKind {
                // discriminants are the contiguous range `0..VARIANTS.len()`
                const VARIANTS: &[SyntaxKind] = &[$(SyntaxKind::$variant),*];
                VARIANTS[raw as usize]
            }
        }
    };
}

syntax_kinds! {
    // ---- token kinds (leaves) - mirror of `TokenKind` ----
    Eof, Illegal,
    Ident,
    Int, Float, String, True, False, Char,
    Const, Var, Structure, Enum,
    If, Else, Loop, Break, Continue,
    Oparen, Cparen, Obrace, Cbrace, Obrack, Cbrack, Comma, Semicolon, Colon,
    Assign,
    Plus, Minus, Star, Slash, And, Or, Eq, Neq, Lt, Gt, Leq, Geq,
    Arrow, Farrow, Dot, Amp, Pipe,
    Wspace, Lcomment, Bcomment, Dcomment, Newline,

    // ---- node kinds (internal) ----
    SourceFile,
    StructDef, FieldList, Field,
    EnumDef, Variant,
    FnDef, ParamList, Param, Block,
    IdentType, RefType, PtrType,
    LetStmt, ExprStmt,
    Literal, NameRef, CallExpr, ArgList,
    BinExpr, PrefixExpr, FieldExpr,
    AssignExpr, IfExpr, LoopExpr, BreakExpr, ContinueExpr,
    RefExpr, DerefExpr,
    StructLit, StructLitFieldList, StructLitField,
    ErrorNode,
}

impl SyntaxKind {
    /// Trivia is syntactically inert: whitespace, newlines and comments.
    /// The parser skips it for lookahead but the tree still stores it.
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::Wspace
                | SyntaxKind::Newline
                | SyntaxKind::Lcomment
                | SyntaxKind::Dcomment
                | SyntaxKind::Bcomment
        )
    }
}

/// Lifts a lexer token kind into the unified kind. The exhaustive `match`
/// is deliberate: a new [`TokenKind`] variant fails to compile here until
/// it is mapped.
impl From<TokenKind> for SyntaxKind {
    fn from(t: TokenKind) -> SyntaxKind {
        use SyntaxKind as S;
        use TokenKind as T;
        match t {
            T::Eof => S::Eof,
            T::Illegal => S::Illegal,
            T::Ident => S::Ident,
            T::Int => S::Int,
            T::Float => S::Float,
            T::String => S::String,
            T::True => S::True,
            T::False => S::False,
            T::Char => S::Char,
            T::Const => S::Const,
            T::Var => S::Var,
            T::Structure => S::Structure,
            T::Enum => S::Enum,
            T::If => S::If,
            T::Else => S::Else,
            T::Loop => S::Loop,
            T::Break => S::Break,
            T::Continue => S::Continue,
            T::Oparen => S::Oparen,
            T::Cparen => S::Cparen,
            T::Obrace => S::Obrace,
            T::Cbrace => S::Cbrace,
            T::Obrack => S::Obrack,
            T::Cbrack => S::Cbrack,
            T::Comma => S::Comma,
            T::Semicolon => S::Semicolon,
            T::Colon => S::Colon,
            T::Assign => S::Assign,
            T::Plus => S::Plus,
            T::Minus => S::Minus,
            T::Star => S::Star,
            T::Slash => S::Slash,
            T::And => S::And,
            T::Or => S::Or,
            T::Eq => S::Eq,
            T::Neq => S::Neq,
            T::Lt => S::Lt,
            T::Gt => S::Gt,
            T::Leq => S::Leq,
            T::Geq => S::Geq,
            T::Arrow => S::Arrow,
            T::Farrow => S::Farrow,
            T::Dot => S::Dot,
            T::Amp => S::Amp,
            T::Pipe => S::Pipe,
            T::Wspace => S::Wspace,
            T::Lcomment => S::Lcomment,
            T::Bcomment => S::Bcomment,
            T::Dcomment => S::Dcomment,
            T::Newline => S::Newline,
        }
    }
}

/// The `rowan` language marker for eye. Binds [`SyntaxKind`] as the tree's
/// kind type; conversion to/from rowan's raw `u16` kind is allocation- and
/// transmute-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EyeLang {}

impl rowan::Language for EyeLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_u16(raw.0)
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<EyeLang>;
pub type SyntaxToken = rowan::SyntaxToken<EyeLang>;
pub type SyntaxNodeChildren = rowan::SyntaxNodeChildren<EyeLang>;
pub type SyntaxNodePtr = rowan::ast::SyntaxNodePtr<EyeLang>;

/// Maps eye surface syntax - punctuation and keywords - to [`SyntaxKind`],
/// so grammar code reads as `p.at(T![;])` instead of naming enum variants.
/// Every punctuation/keyword token in [`TokenKind`] has an arm here; expands
/// to a fully-qualified path so the macro is usable in both expression and
/// pattern position.
#[macro_export]
macro_rules! T {
    // ---- punctuation ----
    [;]     => { $crate::SyntaxKind::Semicolon };
    [,]     => { $crate::SyntaxKind::Comma };
    [:]     => { $crate::SyntaxKind::Colon };
    [=]     => { $crate::SyntaxKind::Assign };
    [.]     => { $crate::SyntaxKind::Dot };
    [&]     => { $crate::SyntaxKind::Amp };
    [|]     => { $crate::SyntaxKind::Pipe };
    ['(']   => { $crate::SyntaxKind::Oparen };
    [')']   => { $crate::SyntaxKind::Cparen };
    ['{']   => { $crate::SyntaxKind::Obrace };
    ['}']   => { $crate::SyntaxKind::Cbrace };
    ['[']   => { $crate::SyntaxKind::Obrack };
    [']']   => { $crate::SyntaxKind::Cbrack };
    [->]    => { $crate::SyntaxKind::Arrow };
    [=>]    => { $crate::SyntaxKind::Farrow };

    // ---- arithmetic / logical operators ----
    [+]     => { $crate::SyntaxKind::Plus };
    [-]     => { $crate::SyntaxKind::Minus };
    [*]     => { $crate::SyntaxKind::Star };
    [/]     => { $crate::SyntaxKind::Slash };
    [&&]    => { $crate::SyntaxKind::And };
    [||]    => { $crate::SyntaxKind::Or };

    // ---- comparison ----
    [==]    => { $crate::SyntaxKind::Eq };
    [!=]    => { $crate::SyntaxKind::Neq };
    [<]     => { $crate::SyntaxKind::Lt };
    [>]     => { $crate::SyntaxKind::Gt };
    [<=]    => { $crate::SyntaxKind::Leq };
    [>=]    => { $crate::SyntaxKind::Geq };

    // ---- keywords ----
    [const]     => { $crate::SyntaxKind::Const };
    [var]       => { $crate::SyntaxKind::Var };
    [structure] => { $crate::SyntaxKind::Structure };
    [enum]      => { $crate::SyntaxKind::Enum };
    [if]        => { $crate::SyntaxKind::If };
    [else]      => { $crate::SyntaxKind::Else };
    [loop]      => { $crate::SyntaxKind::Loop };
    [break]     => { $crate::SyntaxKind::Break };
    [continue]  => { $crate::SyntaxKind::Continue };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `T!` arm expands to the corresponding `SyntaxKind` variant. A new
    /// arm that resolves to the wrong variant - or a renamed variant that
    /// breaks an arm - fails compilation here, not at the call site.
    #[test]
    fn t_macro_punctuation() {
        assert_eq!(T![;], SyntaxKind::Semicolon);
        assert_eq!(T![,], SyntaxKind::Comma);
        assert_eq!(T![:], SyntaxKind::Colon);
        assert_eq!(T![=], SyntaxKind::Assign);
        assert_eq!(T![.], SyntaxKind::Dot);
        assert_eq!(T![&], SyntaxKind::Amp);
        assert_eq!(T![|], SyntaxKind::Pipe);
        assert_eq!(T!['('], SyntaxKind::Oparen);
        assert_eq!(T![')'], SyntaxKind::Cparen);
        assert_eq!(T!['{'], SyntaxKind::Obrace);
        assert_eq!(T!['}'], SyntaxKind::Cbrace);
        assert_eq!(T!['['], SyntaxKind::Obrack);
        assert_eq!(T![']'], SyntaxKind::Cbrack);
        assert_eq!(T![->], SyntaxKind::Arrow);
        assert_eq!(T![=>], SyntaxKind::Farrow);
    }

    #[test]
    fn t_macro_operators() {
        assert_eq!(T![+], SyntaxKind::Plus);
        assert_eq!(T![-], SyntaxKind::Minus);
        assert_eq!(T![*], SyntaxKind::Star);
        assert_eq!(T![/], SyntaxKind::Slash);
        assert_eq!(T![&&], SyntaxKind::And);
        assert_eq!(T![||], SyntaxKind::Or);
        assert_eq!(T![==], SyntaxKind::Eq);
        assert_eq!(T![!=], SyntaxKind::Neq);
        assert_eq!(T![<], SyntaxKind::Lt);
        assert_eq!(T![>], SyntaxKind::Gt);
        assert_eq!(T![<=], SyntaxKind::Leq);
        assert_eq!(T![>=], SyntaxKind::Geq);
    }

    #[test]
    fn t_macro_keywords() {
        assert_eq!(T![const], SyntaxKind::Const);
        assert_eq!(T![var], SyntaxKind::Var);
        assert_eq!(T![structure], SyntaxKind::Structure);
        assert_eq!(T![enum], SyntaxKind::Enum);
        assert_eq!(T![if], SyntaxKind::If);
        assert_eq!(T![else], SyntaxKind::Else);
        assert_eq!(T![loop], SyntaxKind::Loop);
        assert_eq!(T![break], SyntaxKind::Break);
        assert_eq!(T![continue], SyntaxKind::Continue);
    }

    /// `T!` resolves in pattern position too - grammar code matches on it.
    #[test]
    fn t_macro_pattern_position() {
        let k = SyntaxKind::Amp;
        assert!(matches!(k, T![&]));
        let k = SyntaxKind::Pipe;
        assert!(matches!(k, T![|]));
        let k = SyntaxKind::Arrow;
        assert!(matches!(k, T![->]));
        let k = SyntaxKind::If;
        assert!(matches!(k, T![if]));
    }

    /// New v0.2 token kinds map through `From<TokenKind>`. Every other variant
    /// is covered by the exhaustive `match` in the impl itself.
    #[test]
    fn from_tokenkind_v02_tokens() {
        assert_eq!(SyntaxKind::from(TokenKind::Amp), SyntaxKind::Amp);
        assert_eq!(SyntaxKind::from(TokenKind::Pipe), SyntaxKind::Pipe);
        assert_eq!(SyntaxKind::from(TokenKind::If), SyntaxKind::If);
        assert_eq!(SyntaxKind::from(TokenKind::Else), SyntaxKind::Else);
        assert_eq!(SyntaxKind::from(TokenKind::Loop), SyntaxKind::Loop);
        assert_eq!(SyntaxKind::from(TokenKind::Break), SyntaxKind::Break);
        assert_eq!(SyntaxKind::from(TokenKind::Continue), SyntaxKind::Continue);
        assert_eq!(SyntaxKind::from(TokenKind::Enum), SyntaxKind::Enum);
        assert_eq!(SyntaxKind::from(TokenKind::Arrow), SyntaxKind::Arrow);
        assert_eq!(SyntaxKind::from(TokenKind::Farrow), SyntaxKind::Farrow);
    }

    /// `u16` round-trip through the rowan language binding. Picks new v0.2
    /// node kinds plus a few originals to prove `from_u16` returns the right
    /// variant for the full enum range.
    #[test]
    fn syntax_kind_u16_roundtrip() {
        let kinds = [
            SyntaxKind::Eof,
            SyntaxKind::Amp,
            SyntaxKind::Pipe,
            SyntaxKind::EnumDef,
            SyntaxKind::Variant,
            SyntaxKind::Param,
            SyntaxKind::IdentType,
            SyntaxKind::RefType,
            SyntaxKind::PtrType,
            SyntaxKind::AssignExpr,
            SyntaxKind::IfExpr,
            SyntaxKind::LoopExpr,
            SyntaxKind::BreakExpr,
            SyntaxKind::ContinueExpr,
            SyntaxKind::RefExpr,
            SyntaxKind::DerefExpr,
            SyntaxKind::ErrorNode,
        ];
        for k in kinds {
            let raw = k as u16;
            assert_eq!(SyntaxKind::from_u16(raw), k);
        }
    }

    /// Trivia detection still recognises every trivia variant. Guards against
    /// a new variant slipping in without being classified.
    #[test]
    fn trivia_classification() {
        assert!(SyntaxKind::Wspace.is_trivia());
        assert!(SyntaxKind::Newline.is_trivia());
        assert!(SyntaxKind::Lcomment.is_trivia());
        assert!(SyntaxKind::Dcomment.is_trivia());
        assert!(SyntaxKind::Bcomment.is_trivia());
        // non-trivia spot checks
        assert!(!SyntaxKind::Ident.is_trivia());
        assert!(!SyntaxKind::Amp.is_trivia());
        assert!(!SyntaxKind::If.is_trivia());
    }
}
