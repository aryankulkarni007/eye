//! The unified syntax-kind enum and the `rowan` language binding.
//!
//! `rowan` keeps one kind enum for every node in the tree — leaves (tokens)
//! and internal nodes alike — so [`SyntaxKind`] is the superset of the lexer's
//! [`TokenKind`] plus the grammar's node kinds.

use crate::token::TokenKind;

/// Defines [`SyntaxKind`] from a single variant list and derives the
/// `u16` → variant lookup from it, so the `repr` discriminants and the
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
    // ---- token kinds (leaves) — mirror of `TokenKind` ----
    Eof, Illegal,
    Ident,
    Int, Float, String, True, False, Char,
    Const, Var, Structure, Enum,
    If, Else, Loop, Break, Continue,
    Oparen, Cparen, Obrace, Cbrace, Obrack, Cbrack, Comma, Semicolon, Colon,
    Assign,
    Plus, Minus, Star, Slash, And, Or, Eq, Neq, Lt, Gt, Leq, Geq,
    Arrow, Farrow, Dot,
    Wspace, Lcomment, Bcomment, Dcomment, Newline,

    // ---- node kinds (internal) ----
    SourceFile,
    StructDef, FieldList, Field, TypeRef,
    FnDef, ParamList, Block,
    LetStmt, ExprStmt,
    Literal, NameRef, CallExpr, ArgList,
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

/// Maps eye surface syntax — punctuation and keywords — to [`SyntaxKind`],
/// so grammar code reads as `p.at(T![;])` instead of naming enum variants.
#[macro_export]
macro_rules! T {
    [;]     => { $crate::syntax::SyntaxKind::Semicolon };
    [,]     => { $crate::syntax::SyntaxKind::Comma };
    [:]     => { $crate::syntax::SyntaxKind::Colon };
    [=]     => { $crate::syntax::SyntaxKind::Assign };
    ['(']   => { $crate::syntax::SyntaxKind::Oparen };
    [')']   => { $crate::syntax::SyntaxKind::Cparen };
    ['{']   => { $crate::syntax::SyntaxKind::Obrace };
    ['}']   => { $crate::syntax::SyntaxKind::Cbrace };
    ['[']   => { $crate::syntax::SyntaxKind::Obrack };
    [']']   => { $crate::syntax::SyntaxKind::Cbrack };
    [->]    => { $crate::syntax::SyntaxKind::Arrow };
    [const] => { $crate::syntax::SyntaxKind::Const };
    [var]   => { $crate::syntax::SyntaxKind::Var };
    [structure] => { $crate::syntax::SyntaxKind::Structure };
    [enum]  => { $crate::syntax::SyntaxKind::Enum };
}
