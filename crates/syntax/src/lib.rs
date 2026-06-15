//! the unified syntax-kind enum and the `rowan` language binding.
//!
//! `rowan` keeps one kind enum for every node in the tree - leaves (tokens)
//! and internal nodes alike - so [`SyntaxKind`] is the superset of the lexer's
//! [`TokenKind`] plus the grammar's node kinds.

use token::TokenKind;

/// defines [`SyntaxKind`] from a single variant list and derives the
/// `u16` -> variant lookup from it, so the `repr` discriminants and the
/// reverse mapping can never drift apart.
macro_rules! syntax_kinds {
    ($($variant:ident),* $(,)?) => {
        /// every kind of node that can appear in the concrete syntax tree.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        pub enum SyntaxKind {
            $($variant),*
        }

        impl SyntaxKind {
            /// inverse of `self as u16`. `raw` must come from a prior
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
    Let, Mut, Const, Structure, Enum, Union, Extern, Type,
    If, Else, Loop, Break, Continue, Return,
    Match, Underscore, As,
    Oparen, Cparen, Obrace, Cbrace, Obrack, Cbrack, Comma, Semicolon, Colon,
    Assign, PlusEq, MinusEq, StarEq, SlashEq, PercentEq, AmpEq, PipeEq, CaretEq, ShlEq, ShrEq,
    Plus, Minus, Star, Slash, Percent, And, Or, Eq, Neq, Lt, Gt, Leq, Geq,
    Tilde, Caret, Shl, Shr, Bang,
    Arrow, Farrow, Dot, Ellipsis, Amp, Pipe,
    Wspace, Lcomment, Bcomment, Dcomment, Newline,

    // ---- node kinds (internal) ----
    SourceFile,
    ConstDef,
    GlobalDef,
    StructDef, FieldList, Field,
    EnumDef, Variant,
    UnionDef,
    ExternBlock, ExternFn, ExternTypeDef,
    FnDef, EffectList, ParamList, Param, Variadic, Block,
    IdentType, RefType, PtrType, ArrayType, FnType, FnTypeParam,
    LetStmt, ExprStmt,
    Literal, NameRef, CallExpr, ArgList,
    ArrayLit, ArrayRepeat, IndexExpr,
    BinExpr, PrefixExpr, FieldExpr,
    AssignExpr, IfExpr, LoopExpr, BreakExpr, ContinueExpr, ReturnExpr,
    RefExpr, DerefExpr, CastExpr, ParenExpr,
    StructLit, StructLitFieldList, StructLitField,
    MatchExpr, MatchArmList, MatchArm, MatchGuard,
    PathPat, BareIdentPat, WildcardPat, LiteralPat,
    StructPat, StructPatFieldList, StructPatField,
    ErrorNode,
}

impl SyntaxKind {
    /// trivia is syntactically inert: whitespace, newlines and comments.
    /// the parser skips it for lookahead but the tree still stores it.
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

/// lifts a lexer token kind into the unified kind. the exhaustive `match`
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
            T::Let => S::Let,
            T::Mut => S::Mut,
            T::Const => S::Const,
            T::Structure => S::Structure,
            T::Enum => S::Enum,
            T::Union => S::Union,
            T::Extern => S::Extern,
            T::Type => S::Type,
            T::If => S::If,
            T::Else => S::Else,
            T::Loop => S::Loop,
            T::Break => S::Break,
            T::Continue => S::Continue,
            T::Return => S::Return,
            T::Match => S::Match,
            T::Underscore => S::Underscore,
            T::As => S::As,
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
            T::PlusEq => S::PlusEq,
            T::MinusEq => S::MinusEq,
            T::StarEq => S::StarEq,
            T::SlashEq => S::SlashEq,
            T::PercentEq => S::PercentEq,
            T::AmpEq => S::AmpEq,
            T::PipeEq => S::PipeEq,
            T::CaretEq => S::CaretEq,
            T::ShlEq => S::ShlEq,
            T::ShrEq => S::ShrEq,
            T::Plus => S::Plus,
            T::Minus => S::Minus,
            T::Star => S::Star,
            T::Slash => S::Slash,
            T::Percent => S::Percent,
            T::Tilde => S::Tilde,
            T::Caret => S::Caret,
            T::Shl => S::Shl,
            T::Shr => S::Shr,
            T::Bang => S::Bang,
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
            T::Ellipsis => S::Ellipsis,
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

/// the `rowan` language marker for eye. binds [`SyntaxKind`] as the tree's
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

/// a node's source range with leading and trailing trivia stripped, so a
/// diagnostic underlines exactly the meaningful tokens rather than the
/// surrounding whitespace, newlines, and comments the lossless tree attaches
/// to a node's edges. tokens are visited in source order; the span runs from
/// the first non-trivia token's start to the last non-trivia token's end. a
/// node with no non-trivia tokens falls back to its full range.
///
/// returns `text_size::TextRange` - the same type rowan re-exports as
/// `rowan::TextRange` - spelled by its `text_size` path to match the rest of
/// the workspace (`diagnostics::Span`, lexer, parser).
pub fn trimmed_text_range(node: &SyntaxNode) -> text_size::TextRange {
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia());
    let Some(first) = tokens.next() else {
        return node.text_range();
    };
    let end = tokens
        .last()
        .map(|t| t.text_range().end())
        .unwrap_or_else(|| first.text_range().end());
    text_size::TextRange::new(first.text_range().start(), end)
}

/// maps eye surface syntax - punctuation and keywords - to [`SyntaxKind`],
/// so grammar code reads as `p.at(T![;])` instead of naming enum variants.
/// every punctuation/keyword token in [`TokenKind`] has an arm here; expands
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
    [...]   => { $crate::SyntaxKind::Ellipsis };

    // ---- arithmetic / logical operators ----
    [+]     => { $crate::SyntaxKind::Plus };
    [-]     => { $crate::SyntaxKind::Minus };
    [*]     => { $crate::SyntaxKind::Star };
    [/]     => { $crate::SyntaxKind::Slash };
    [%]     => { $crate::SyntaxKind::Percent };
    [&&]    => { $crate::SyntaxKind::And };
    [||]    => { $crate::SyntaxKind::Or };

    // ---- bitwise / prefix-unary ----
    [~]     => { $crate::SyntaxKind::Tilde };
    [^]     => { $crate::SyntaxKind::Caret };
    [<<]    => { $crate::SyntaxKind::Shl };
    [>>]    => { $crate::SyntaxKind::Shr };
    [!]     => { $crate::SyntaxKind::Bang };

    // ---- compound assignment ----
    [+=]    => { $crate::SyntaxKind::PlusEq };
    [-=]    => { $crate::SyntaxKind::MinusEq };
    [*=]    => { $crate::SyntaxKind::StarEq };
    [/=]    => { $crate::SyntaxKind::SlashEq };
    [%=]    => { $crate::SyntaxKind::PercentEq };
    [&=]    => { $crate::SyntaxKind::AmpEq };
    [|=]    => { $crate::SyntaxKind::PipeEq };
    [^=]    => { $crate::SyntaxKind::CaretEq };
    [<<=]   => { $crate::SyntaxKind::ShlEq };
    [>>=]   => { $crate::SyntaxKind::ShrEq };

    // ---- comparison ----
    [==]    => { $crate::SyntaxKind::Eq };
    [!=]    => { $crate::SyntaxKind::Neq };
    [<]     => { $crate::SyntaxKind::Lt };
    [>]     => { $crate::SyntaxKind::Gt };
    [<=]    => { $crate::SyntaxKind::Leq };
    [>=]    => { $crate::SyntaxKind::Geq };

    // ---- keywords ----
    [let]       => { $crate::SyntaxKind::Let };
    [mut]       => { $crate::SyntaxKind::Mut };
    [const]     => { $crate::SyntaxKind::Const };
    [structure] => { $crate::SyntaxKind::Structure };
    [enum]      => { $crate::SyntaxKind::Enum };
    [union]     => { $crate::SyntaxKind::Union };
    [extern]    => { $crate::SyntaxKind::Extern };
    [type]      => { $crate::SyntaxKind::Type };
    [if]        => { $crate::SyntaxKind::If };
    [else]      => { $crate::SyntaxKind::Else };
    [loop]      => { $crate::SyntaxKind::Loop };
    [break]     => { $crate::SyntaxKind::Break };
    [continue]  => { $crate::SyntaxKind::Continue };
    [return]    => { $crate::SyntaxKind::Return };
    [match]     => { $crate::SyntaxKind::Match };
    [as]        => { $crate::SyntaxKind::As };
    [_]         => { $crate::SyntaxKind::Underscore };
}

// ---------------------------------------------------------------------------
// shared string table trait for the query-driven pipeline
// (QUERY.md). defined here so both `lexer` (the concrete `Interner`) and `hir`
// (which consumes it) share a single dependency rather than coupling directly.
// ---------------------------------------------------------------------------

pub use smol_str::SmolStr;

/// a read-only string table that maps `&str` to a canonical
/// [`SmolStr`] when the string has been pre-interned (e.g. by the lexer).
/// the returned clone is o(1) - short strings (<=22 bytes) are inline; long
/// strings bump an `Arc` refcount.
///
/// why a trait? the lexer's [`Interner`] is the only implementation today,
/// but in a multi-file QUERY architecture (QUERY.md) a `SourceFile` wrapper
/// would also implement this, letting HIR lowering request strings without
/// caring which concrete type owns the table.
///
/// # stability
/// provisional (2026-06-11): the trait may gain lookup-by-id methods when
/// `Symbol` handles are threaded downstream.
pub trait StringTable {
    fn get(&self, s: &str) -> Option<SmolStr>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// every `T!` arm expands to the corresponding `SyntaxKind` variant. a new
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
        assert_eq!(T![let], SyntaxKind::Let);
        assert_eq!(T![mut], SyntaxKind::Mut);
        assert_eq!(T![structure], SyntaxKind::Structure);
        assert_eq!(T![enum], SyntaxKind::Enum);
        assert_eq!(T![if], SyntaxKind::If);
        assert_eq!(T![else], SyntaxKind::Else);
        assert_eq!(T![loop], SyntaxKind::Loop);
        assert_eq!(T![break], SyntaxKind::Break);
        assert_eq!(T![continue], SyntaxKind::Continue);
        assert_eq!(T![match], SyntaxKind::Match);
        assert_eq!(T![_], SyntaxKind::Underscore);
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

    /// new v0.2 token kinds map through `From<TokenKind>`. every other variant
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
        assert_eq!(SyntaxKind::from(TokenKind::Match), SyntaxKind::Match);
        assert_eq!(
            SyntaxKind::from(TokenKind::Underscore),
            SyntaxKind::Underscore
        );
    }

    /// `u16` round-trip through the rowan language binding. picks new v0.2
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
            SyntaxKind::MatchExpr,
            SyntaxKind::MatchArmList,
            SyntaxKind::MatchArm,
            SyntaxKind::PathPat,
            SyntaxKind::BareIdentPat,
            SyntaxKind::WildcardPat,
            SyntaxKind::LiteralPat,
            SyntaxKind::StructPat,
            SyntaxKind::StructPatFieldList,
            SyntaxKind::StructPatField,
            SyntaxKind::ErrorNode,
        ];
        for k in kinds {
            let raw = k as u16;
            assert_eq!(SyntaxKind::from_u16(raw), k);
        }
    }

    /// trivia detection still recognises every trivia variant. guards against
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
