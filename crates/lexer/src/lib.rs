//! the lexer - raw source bytes to a flat [`Token`] stream.
//!
//! [`Lexer::tokenize`] drives `logos` (the lex rules live on [`TokenKind`] in
//! the `token` crate) and yields a [`Lexed`]: the token vector, the populated
//! [`Interner`], and any [`LexError`]s. [`SourceText`] owns the input (heap
//! string or `mmap`) and answers byte-offset/line-column queries.

use logos::Logos;
use memchr::memchr_iter;
use memmap2::Mmap;
use rustc_hash::{FxBuildHasher, FxHashMap};
use smol_str::SmolStr;
use text_size::{TextRange, TextSize};

use diagnostics::{Class, Code, Sink};
use token::{LexErrorTag, Token, TokenKind};

/// a lexer diagnostic (class `L`). the single source of truth for a malformed
/// lexeme; the prose message comes from [`std::fmt::Display`] via `thiserror`.
/// most variants map one-to-one from a [`LexErrorTag`] recorded by the `token`
/// crate's logos callbacks; [`LexError::UnexpectedChar`] is raised here, for a
/// byte `logos` could not start any token from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LexError {
    #[error("unclosed string literal")]
    UnclosedString,
    #[error("empty character literal")]
    EmptyChar,
    #[error("unterminated escape")]
    UnterminatedEscape,
    #[error("unclosed char literal")]
    UnclosedChar,
    #[error("invalid char in literal")]
    InvalidChar,
    #[error("missing closing quote")]
    MissingQuote,
    #[error("unclosed block comment")]
    UnclosedBlockComment,
    #[error("unexpected character: '{0}'")]
    UnexpectedChar(char),
}

impl LexError {
    /// lift a callback-recorded [`LexErrorTag`] into the typed kind.
    fn from_tag(tag: LexErrorTag) -> Self {
        match tag {
            LexErrorTag::UnclosedString => LexError::UnclosedString,
            LexErrorTag::EmptyChar => LexError::EmptyChar,
            LexErrorTag::UnterminatedEscape => LexError::UnterminatedEscape,
            LexErrorTag::UnclosedChar => LexError::UnclosedChar,
            LexErrorTag::InvalidChar => LexError::InvalidChar,
            LexErrorTag::MissingQuote => LexError::MissingQuote,
            LexErrorTag::UnclosedBlockComment => LexError::UnclosedBlockComment,
        }
    }
}

impl diagnostics::Diagnostic for LexError {
    fn code(&self) -> Code {
        let number = match self {
            LexError::UnclosedString => 1,
            LexError::EmptyChar => 2,
            LexError::UnterminatedEscape => 3,
            LexError::UnclosedChar => 4,
            LexError::InvalidChar => 5,
            LexError::MissingQuote => 6,
            LexError::UnclosedBlockComment => 7,
            LexError::UnexpectedChar(_) => 8,
        };
        Code::new(Class::Lex, number)
    }
}

/// a interned string handle - an index into an [`Interner`]'s table. `Copy`
/// and pointer-free, so name comparison downstream is a `u32` equality check
/// instead of a `str` compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(pub u32);

/// the canonical string table. every identifier and string literal the lexer
/// sees is interned here, so the same text always maps to the same [`Symbol`].
///
/// strings are stored as [`SmolStr`]: identifiers - almost always short -
/// stay inline with no heap allocation, and a cache-hit clone is `O(1)`.
///
/// the lexer pre-populates this during tokenizing; later stages (HIR name
/// resolution) re-intern identifier text against the *same* table - a cache
/// hit yields the original `Symbol`. the table outlives the lexer: it is
/// handed off in [`Lexed`].
#[derive(Debug)]
/// a string interner backed by a hash map. every distinct string is stored once
/// and identified by a lightweight [`Symbol`] handle.
///
/// ```
/// # use lexer::Interner;
/// let mut interner = Interner::new();
/// let a = interner.intern("hello");
/// let b = interner.intern("world");
/// let c = interner.intern("hello"); // same as `a`
///
/// assert_eq!(interner.lookup(a), "hello");
/// assert_eq!(interner.lookup(b), "world");
/// assert_eq!(a, c);
/// assert_eq!(interner.len(), 2);
/// ```
pub struct Interner {
    map: FxHashMap<SmolStr, Symbol>,
    vec: Vec<SmolStr>,
}

impl Interner {
    pub fn new() -> Self {
        Interner {
            map: FxHashMap::with_capacity_and_hasher(256, FxBuildHasher),
            vec: Vec::new(),
        }
    }

    /// intern `s`, returning its handle. idempotent: equal strings always map
    /// to the same [`Symbol`].
    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = Symbol(self.vec.len() as u32);
        let owned = SmolStr::new(s);
        self.map.insert(owned.clone(), id);
        self.vec.push(owned);
        id
    }

    /// the text behind a [`Symbol`]. panics if `id` came from another table.
    pub fn lookup(&self, id: Symbol) -> &str {
        debug_assert!(
            (id.0 as usize) < self.vec.len(),
            "Symbol({}) out of range for this Interner (len {}); it likely came from a different table",
            id.0,
            self.vec.len()
        );
        &self.vec[id.0 as usize]
    }

    /// number of distinct strings interned.
    /// retrieve the canonical [`SmolStr`] for `s` if it was already interned.
    /// returns `None` if `s` is not in the table. the clone is o(1) - short
    /// strings (≤22 bytes) are inline; long strings bump an `Arc` refcount.
    pub fn get(&self, s: &str) -> Option<SmolStr> {
        self.map.get(s).map(|&sym| self.vec[sym.0 as usize].clone())
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

/// a one-based source position: `line` and `col` (a byte offset from the line
/// start, not a character count). both are `u32` rather than `usize` to halve
/// the struct's size; this caps a source file at ~4 billion lines and ~4 billion
/// bytes per line, far beyond any real input.
#[derive(Debug)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug)]
pub enum SourceHolder {
    Owned(String),
    Mmap(Mmap),
}

impl SourceHolder {
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            SourceHolder::Owned(s) => s.as_bytes(),
            SourceHolder::Mmap(m) => m,
        }
    }
}

impl std::ops::Deref for SourceHolder {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            SourceHolder::Owned(s) => s.as_bytes(),
            SourceHolder::Mmap(m) => m.as_ref(),
        }
    }
}

#[derive(Debug)]
pub struct SourceText {
    pub source: SourceHolder,
    pub lstart: Vec<usize>,
}

/// calculates lstarts using memchr_iter (SIMD)
fn lstarts(bytes: &[u8]) -> Vec<usize> {
    let mut lstart = Vec::with_capacity(bytes.len() / 40);
    lstart.push(0);

    for n_pos in memchr_iter(b'\n', bytes) {
        lstart.push(n_pos + 1);
    }
    lstart
}

impl SourceText {
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        self.source.as_bytes()
    }

    /// create from an mmap. returns an error when the file is not valid
    /// UTF-8 - the user sees a graceful diagnostic rather than a panic.
    /// both constructors validate UTF-8 so that [`SourceText::as_str`] is
    /// safe (the `unsafe` call is justified by construction).
    /// EXPERIMENTAL
    pub fn from_mmap(mmap: Mmap) -> Result<Self, std::str::Utf8Error> {
        std::str::from_utf8(&mmap)?;
        let lstart = lstarts(&mmap);
        Ok(SourceText {
            source: SourceHolder::Mmap(mmap),
            lstart,
        })
    }

    /// create from a string (tests/internal)
    pub fn new(content: String) -> Self {
        // we scan the string bytes directly
        let lstart = lstarts(content.as_bytes());
        SourceText {
            source: SourceHolder::Owned(content),
            lstart,
        }
    }

    #[inline(always)]
    pub fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.source) }
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source.len() == 0
    }

    /// converts a byte offset to one-based line and col
    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let offset = usize::from(offset);
        assert!(offset <= self.source.len(), "offset out of bounds");
        let line = self.lstart.partition_point(|&start| start <= offset) - 1;
        let col = offset - self.lstart[line];
        LineCol {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
        }
    }

    /// converts a byte offset to a one-based line and a one-based column
    /// counted in UTF-16 code units - the LSP default position encoding.
    /// [`Self::line_col`] reports byte columns; an LSP payload built from
    /// those mis-places every position after a multibyte character on the
    /// same line. `offset` must lie on a `char` boundary (token and node
    /// ranges always do).
    pub fn line_col_utf16(&self, offset: TextSize) -> LineCol {
        let offset = usize::from(offset);
        assert!(offset <= self.source.len(), "offset out of bounds");
        let line = self.lstart.partition_point(|&start| start <= offset) - 1;
        let prefix = &self.as_str()[self.lstart[line]..offset];
        let col: usize = prefix.chars().map(char::len_utf16).sum();
        LineCol {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
        }
    }

    /// the source text a [`TextRange`] covers, or `None` if it is out of
    /// bounds or not on `char` boundaries.
    pub fn slice(&self, range: TextRange) -> Option<&str> {
        self.as_str()
            .get(usize::from(range.start())..usize::from(range.end()))
    }
}

/// the complete result of tokenizing: the token stream, the populated string
/// table, and every diagnostic. produced by [`Lexer::tokenize`].
#[derive(Debug)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub interner: Interner,
    pub diags: Sink<LexError>,
}

/// a thin driver over `logos`.
/// holds only the borrowed source;
/// the real work is in [`Lexer::tokenize`].
pub struct Lexer<'a> {
    pub source: &'a SourceText,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a SourceText) -> Self {
        Lexer { source }
    }

    /// drives `logos` to completion, yielding the token stream (up to and
    /// including a synthesized [`TokenKind::Eof`]), the populated string
    /// table, and every diagnostic - see [`Lexed`].
    pub fn tokenize(self) -> Lexed {
        let src = self.source.as_str();
        let mut lex = TokenKind::lexer(src);
        let mut tokens = Vec::with_capacity(src.len() / 4 + 1);
        let mut interner = Interner::new();
        // diagnostics from lex errors; the callback diagnostics for unclosed
        // literals/comments accumulate separately in `lex.extras`
        let mut err_diags: Vec<(TextRange, LexError)> = Vec::new();

        while let Some(result) = lex.next() {
            let span = lex.span();
            let range = TextRange::new(
                TextSize::from(span.start as u32),
                TextSize::from(span.end as u32),
            );
            let kind = match result {
                Ok(kind) => kind,
                Err(()) => {
                    let ch = src[span.clone()].chars().next().unwrap_or('\u{fffd}');
                    err_diags.push((range, LexError::UnexpectedChar(ch)));
                    TokenKind::Illegal
                }
            };

            // identifiers and string literals are interned so later stages
            // share one canonical handle per string; a string interns its
            // contents without the surrounding quotes
            if matches!(kind, TokenKind::Ident | TokenKind::String) {
                let lexeme = &src[span];
                let text = if kind == TokenKind::String {
                    let inner = lexeme.strip_prefix('"').unwrap_or(lexeme);
                    inner.strip_suffix('"').unwrap_or(inner)
                } else {
                    lexeme
                };
                interner.intern(text);
            }

            tokens.push(Token { kind, range });
        }

        // a final zero-width `Eof` so the parser always has a terminator
        let end = TextSize::from(src.len() as u32);
        tokens.push(Token {
            kind: TokenKind::Eof,
            range: TextRange::empty(end),
        });

        // merge the two diagnostic streams into one source-ordered list, then
        // build the typed sink (the kind enum carries no span; the sink pairs
        // each kind with its range)
        let mut all = err_diags;
        all.extend(
            lex.extras
                .0
                .into_iter()
                .map(|(tag, range)| (range, LexError::from_tag(tag))),
        );
        all.sort_by_key(|(range, _)| range.start());

        let mut diags = Sink::new();
        for (range, kind) in all {
            diags.emit(range, kind);
        }

        Lexed {
            tokens,
            interner,
            diags,
        }
    }
}

// ---------------------------------------------------------------------------
// EXPERIMENTAL: `StringTable` impl + per-file context (QUERY.md)
// ---------------------------------------------------------------------------

use syntax::StringTable;

impl StringTable for Interner {
    fn get(&self, s: &str) -> Option<SmolStr> {
        self.map.get(s).map(|&sym| self.vec[sym.0 as usize].clone())
    }
}

/// EXPERIMENTAL: per-source-file context bundling the source text and the
/// lexer's string table. this is the single-file precursor to a multi-file
/// `SourceCache` (QUERY.md) that would map `FileId → SourceFile`.
///
/// the `StringTable` impl delegates to the inner [`Interner`], so HIR lowering
/// can request canonical strings without knowing which concrete type owns the
/// table -- crucial when the source comes from a project database rather than a
/// single invocation.
#[derive(Debug)]
pub struct SourceFile {
    pub text: SourceText,
    pub interner: Interner,
}

impl SourceFile {
    pub fn new(text: SourceText, interner: Interner) -> Self {
        Self { text, interner }
    }
}

impl StringTable for SourceFile {
    fn get(&self, s: &str) -> Option<SmolStr> {
        self.interner.get(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tokenize `src` into its full [`Lexed`] result.
    fn lex(src: &str) -> Lexed {
        let source = SourceText::new(src.to_string());
        Lexer::new(&source).tokenize()
    }

    /// non-trivia token kinds - the shape the parser actually consumes.
    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src)
            .tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| {
                !matches!(
                    k,
                    TokenKind::Wspace
                        | TokenKind::Newline
                        | TokenKind::Lcomment
                        | TokenKind::Dcomment
                        | TokenKind::Bcomment
                )
            })
            .collect()
    }

    #[test]
    fn keywords_vs_idents() {
        use TokenKind::*;
        assert_eq!(
            kinds("let mut structure enum if else loop break continue foo"),
            [
                Let, Mut, Structure, Enum, If, Else, Loop, Break, Continue, Ident, Eof
            ]
        );
    }

    #[test]
    fn operators_and_delimiters() {
        use TokenKind::*;
        assert_eq!(
            kinds("+ - * / && || == != < > <= >= = -> => . ( ) { } [ ] , ; :"),
            [
                Plus, Minus, Star, Slash, And, Or, Eq, Neq, Lt, Gt, Leq, Geq, Assign, Arrow,
                Farrow, Dot, Oparen, Cparen, Obrace, Cbrace, Obrack, Cbrack, Comma, Semicolon,
                Colon, Eof
            ]
        );
    }

    #[test]
    fn numbers() {
        use TokenKind::*;
        assert_eq!(kinds("0 42 3.14"), [Int, Int, Float, Eof]);
        // a trailing dot is not consumed into the number
        assert_eq!(kinds("12."), [Int, Dot, Eof]);
    }

    #[test]
    fn literals() {
        use TokenKind::*;
        assert_eq!(
            kinds("\"hello\" 'a' true false"),
            [String, Char, True, False, Eof]
        );
    }

    #[test]
    fn comments_are_trivia() {
        // line, doc and block comments carry no significant tokens
        assert_eq!(kinds("-- line\n--- doc\n--* block --*"), [TokenKind::Eof]);
    }

    #[test]
    fn minus_disambiguation() {
        use TokenKind::*;
        assert_eq!(kinds("a - b"), [Ident, Minus, Ident, Eof]);
        assert_eq!(kinds("->"), [Arrow, Eof]);
        assert_eq!(kinds("-- c"), [Eof]); // line comment
    }

    /// regression: `-` immediately before a multi-byte char used to index
    /// into the middle of that char and panic. it must lex cleanly now.
    #[test]
    fn minus_before_multibyte_char_no_panic() {
        use TokenKind::*;
        // `é` is a valid identifier start, so this is `Minus Ident`
        assert_eq!(kinds("-é"), [Minus, Ident, Eof]);
        assert_eq!(kinds("-"), [Minus, Eof]);
        assert_eq!(kinds("- 世界"), [Minus, Ident, Eof]);
    }

    #[test]
    fn string_interning_dedups() {
        let lexed = lex("\"hi\" \"hi\" name name");
        // `hi` and `name` are each interned exactly once
        assert_eq!(lexed.interner.len(), 2);
    }

    #[test]
    fn unclosed_string_diagnoses() {
        let lexed = lex("\"oops");
        assert_eq!(lexed.diags.len(), 1);
        assert!(matches!(
            lexed.diags.entries()[0].1,
            LexError::UnclosedString
        ));
    }

    #[test]
    fn unclosed_block_comment_diagnoses() {
        let lexed = lex("--* never closed");
        assert_eq!(lexed.diags.len(), 1);
        assert!(matches!(
            lexed.diags.entries()[0].1,
            LexError::UnclosedBlockComment
        ));
    }

    #[test]
    fn empty_char_diagnoses() {
        let lexed = lex("''");
        assert_eq!(lexed.diags.len(), 1);
        assert!(matches!(lexed.diags.entries()[0].1, LexError::EmptyChar));
    }

    #[test]
    fn spans_tile_the_source_with_no_gaps() {
        let src = "let x = 0;";
        let lexed = lex(src);
        let mut cursor = TextSize::from(0);
        for tok in &lexed.tokens {
            assert_eq!(tok.range.start(), cursor, "gap or overlap at {cursor:?}");
            cursor = tok.range.end();
        }
        assert_eq!(
            usize::from(cursor),
            src.len(),
            "tokens do not cover the whole source"
        );
    }

    #[test]
    fn crlf_line_tracking() {
        // windows `\r\n`: `memchr` finds the `\n`, so the line start lands one
        // byte past it - the `\r` is the trailing byte of the previous line.
        let src = "a\r\nbc\r\nd";
        let st = SourceText::new(src.to_string());
        // line starts: 0, after first `\n` (=3), after second `\n` (=7)
        assert_eq!(st.lstart, [0, 3, 7]);

        // offset 0 (`a`): line 1, col 1
        let lc = st.line_col(TextSize::from(0));
        assert_eq!((lc.line, lc.col), (1, 1));
        // offset 3 (`b`, first byte of line 2): line 2, col 1
        let lc = st.line_col(TextSize::from(3));
        assert_eq!((lc.line, lc.col), (2, 1));
        // offset 7 (`d`, first byte of line 3): line 3, col 1
        let lc = st.line_col(TextSize::from(7));
        assert_eq!((lc.line, lc.col), (3, 1));

        // `\r` lexes as whitespace, never breaks a token
        assert_eq!(
            kinds(src),
            [
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Ident,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn line_col_utf16_counts_code_units() {
        // `é` is 2 bytes / 1 UTF-16 unit; `𝄞` is 4 bytes / 2 UTF-16 units.
        let src = "-- é𝄞x\nyz";
        let st = SourceText::new(src.to_string());
        // offset of `x`: 3 ("-- ") + 2 (é) + 4 (𝄞) = 9 bytes; UTF-16 col is
        // 3 + 1 + 2 = 6 zero-based, 7 one-based (byte col would be 10).
        let lc = st.line_col_utf16(TextSize::from(9));
        assert_eq!((lc.line, lc.col), (1, 7));
        assert_eq!(st.line_col(TextSize::from(9)).col, 10);
        // multibyte on an earlier line does not affect a later line
        let y_offset = src.find('y').unwrap() as u32;
        let lc = st.line_col_utf16(TextSize::from(y_offset));
        assert_eq!((lc.line, lc.col), (2, 1));
    }

    #[test]
    fn token_stream_snapshot() {
        let lexed = lex("add(int32 a) -> int32 { a + 1 }");
        insta::assert_debug_snapshot!(lexed.tokens);
    }

    /// v0.2 surface forms that are new since v0.1: the `&` ref token in a
    /// type and as a prefix, `.` member access on the LHS of an assignment,
    /// and `=>` farrow.
    #[test]
    fn v02_punctuation_and_prefixes() {
        use TokenKind::*;
        // `mut &Point pt_ref = &pt;` - `&` appears both in a type position
        // and as a prefix operator, lexed as the same `Amp` token either way
        assert_eq!(
            kinds("mut &Point pt_ref = &pt;"),
            [Mut, Amp, Ident, Ident, Assign, Amp, Ident, Semicolon, Eof]
        );

        // `pt.x = 15;` - member access then assignment
        assert_eq!(
            kinds("pt.x = 15;"),
            [Ident, Dot, Ident, Assign, Int, Semicolon, Eof]
        );

        // `=>` farrow is its own token, not `Assign` + `Gt`
        assert_eq!(kinds("=> ="), [Farrow, Assign, Eof]);
    }

    /// the v0.2 waterfall enum syntax: `enum X = | A | B ;` - each `|` is a
    /// `Pipe` token, distinct from the boolean `||`.
    #[test]
    fn waterfall_enum_pipes() {
        use TokenKind::*;
        assert_eq!(
            kinds("enum Shape = | Square | Circle ;"),
            [
                Enum, Ident, Assign, Pipe, Ident, Pipe, Ident, Semicolon, Eof
            ]
        );
        // `||` still lexes as a single `Or`, not two `Pipe`s
        assert_eq!(kinds("|| |"), [Or, Pipe, Eof]);
    }

    /// `if`/`else`/`loop`/`break`/`continue` keywords are reserved - the
    /// trailing `breakage` is one identifier, not `break` + `age`.
    #[test]
    fn control_flow_keywords_dont_split_idents() {
        use TokenKind::*;
        assert_eq!(
            kinds("if else loop break continue"),
            [If, Else, Loop, Break, Continue, Eof]
        );
        assert_eq!(
            kinds("breakage continuee elsewhere"),
            [Ident, Ident, Ident, Eof]
        );
    }

    /// `match` is a reserved keyword (not an ident); bare `_` is its own
    /// `Underscore` token, while `_foo` still lexes as a single `Ident`.
    #[test]
    fn match_keyword_and_underscore_tokens() {
        use TokenKind::*;
        assert_eq!(
            kinds("match x { _ -> 0 }"),
            [Match, Ident, Obrace, Underscore, Arrow, Int, Cbrace, Eof]
        );
        // `matches` is just an identifier - keyword match is exact
        assert_eq!(kinds("matches"), [Ident, Eof]);
        // `_foo`/`foo_` are idents; bare `_` is the underscore token
        assert_eq!(kinds("_ _foo foo_"), [Underscore, Ident, Ident, Eof]);
    }

    /// `->` arrow as a return-type marker stays one token even when wedged
    /// between identifiers.
    #[test]
    fn arrow_return_type() {
        use TokenKind::*;
        assert_eq!(
            kinds("add(int32 a, int32 b) -> int32"),
            [
                Ident, Oparen, Ident, Ident, Comma, Ident, Ident, Cparen, Arrow, Ident, Eof
            ]
        );
    }

    /// a multi-line block comment with `--*` as both open and close delimiter
    /// - the form used in `eyesrc/design.eye`.
    #[test]
    fn block_comment_multiline() {
        let lexed = lex("--*\n  * note\n--*\nlet x = 0;");
        assert!(lexed.diags.is_empty(), "diags: {:?}", lexed.diags);
        use TokenKind::*;
        let stream: Vec<_> = lexed.tokens.iter().map(|t| t.kind).collect();
        // block comment is one token; whitespace/newlines surround it
        assert!(stream.contains(&Bcomment));
        // significant tokens after the comment match `let x = 0;`
        let nonws: Vec<_> = stream
            .into_iter()
            .filter(|k| !matches!(k, Wspace | Newline | Lcomment | Dcomment | Bcomment))
            .collect();
        assert_eq!(nonws, [Let, Ident, Assign, Int, Semicolon, Eof]);
    }
}
