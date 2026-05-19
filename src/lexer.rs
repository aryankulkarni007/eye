use std::borrow::Cow;

#[macro_export]
macro_rules! span {
    ($start:expr, $end:expr) => {
        $crate::token::Span::new($start, $end)
    };
}

#[macro_export]
macro_rules! tok {
    ($kind:ident, $start:expr, $end:expr) => {
        Token {
            kind: $kind,
            span: span![$start, $end],
        }
    };
    ($kind:ident, $span:expr) => {
        Token {
            kind: $kind,
            span: $span,
        }
    };
}

use memchr::memchr_iter;
use memmap2::Mmap;
use rustc_hash::FxHashMap;

use crate::token::{Span, Token, TokenKind};

/// A interned string handle — an index into an [`Interner`]'s table. `Copy`
/// and pointer-free, so name comparison downstream is a `u32` equality check
/// instead of a `str` compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(pub u32);

/// The canonical string table. Every identifier and string literal the lexer
/// sees is interned here, so the same text always maps to the same [`Symbol`].
///
/// The lexer pre-populates this during tokenizing; later stages (HIR name
/// resolution) re-intern identifier text against the *same* table — a cache
/// hit yields the original `Symbol`. The table outlives the lexer: it is
/// handed off in [`Lexed`].
#[derive(Debug)]
pub struct Interner {
    map: FxHashMap<String, Symbol>,
    vec: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        Interner {
            map: FxHashMap::default(),
            vec: Vec::new(),
        }
    }

    /// Intern `s`, returning its handle. Idempotent: equal strings always map
    /// to the same [`Symbol`].
    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = Symbol(self.vec.len() as u32);
        let owned = s.to_string();
        self.map.insert(owned.clone(), id);
        self.vec.push(owned);
        id
    }

    /// The text behind a [`Symbol`]. Panics if `id` came from another table.
    pub fn lookup(&self, id: Symbol) -> &str {
        &self.vec[id.0 as usize]
    }

    /// Number of distinct strings interned.
    pub fn len(&self) -> usize {
        self.vec.len()
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub msg: Cow<'static, str>,
    pub span: Span,
}

/// line (zero based), col (byte offset from line start)
/// WARNING: u32 to save memory; used to be usize
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

pub struct SourceText {
    pub source: SourceHolder,
    pub lstart: Vec<usize>,
}

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

    /// create from an mmap (production)
    pub fn from_mmap(mmap: Mmap) -> Self {
        std::str::from_utf8(&mmap).expect("invalid utf-8");
        let lstart = lstarts(&mmap);
        SourceText {
            source: SourceHolder::Mmap(mmap),
            lstart,
        }
    }

    /// create from a string (tests/internal)
    pub fn new(content: String) -> Self {
        // We scan the string bytes directly
        let lstart = lstarts(content.as_bytes());
        SourceText {
            source: SourceHolder::Owned(content),
            lstart,
        }
    }

    #[inline(always)]
    pub(crate) fn as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.source) }
    }

    pub(crate) fn len(&self) -> usize {
        self.source.len()
    }

    /// converts a byte offset to zero-based line and col
    pub fn line_col(&self, offset: usize) -> LineCol {
        assert!(offset <= self.source.len(), "offset out of bounds");
        let line = self.lstart.partition_point(|&start| start <= offset) - 1;
        let col = offset - self.lstart[line];
        LineCol {
            line: (line + 1) as u32,
            col: (col + 1) as u32,
        }
    }

    pub fn slice(&self, span: Span) -> Option<&str> {
        self.as_str().get(span.start..span.end)
    }
}

/// The complete result of tokenizing: the token stream, the populated string
/// table, and every diagnostic. Produced by [`Lexer::tokenize`], which
/// consumes the lexer so the owned [`Interner`] can be handed off intact.
#[derive(Debug)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub interner: Interner,
    pub diags: Vec<Diagnostic>,
}

pub struct Lexer<'a> {
    pub source: &'a SourceText,
    /// The string table, owned by the lexer and surfaced in [`Lexed`].
    pub interner: Interner,
    pub cursor: usize,
    pub tstart: usize,
    pub diags: Vec<Diagnostic>,
    /// set once `Eof` is yielded so the `Iterator` impl terminates
    eof_emitted: bool,
}

fn keyword(ident: &str) -> TokenKind {
    match ident {
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "const" => TokenKind::Const,
        "var" => TokenKind::Var,
        "structure" => TokenKind::Structure,
        "enum" => TokenKind::Enum,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "loop" => TokenKind::Loop,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        _ => TokenKind::Ident,
    }
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a SourceText) -> Self {
        Lexer {
            source,
            interner: Interner::new(),
            cursor: 0,
            tstart: 0,
            diags: Vec::new(),
            eof_emitted: false,
        }
    }

    /// returns the char at (cursor + offset characters) and its byte length
    #[inline(always)]
    fn peek(&self, offset: usize) -> Option<char> {
        // Indexing bytes is O(1).
        let pos = self.cursor + offset;
        if pos >= self.source.len() {
            return None;
        }

        let b = self.source.as_bytes()[pos];

        // if it's ascii (0-127), we can just cast it.
        // this covers 99% of tokenkind
        if b < 128 {
            return Some(b as char);
        }

        // fallback for multi-byte utf-8 (only if necessary)
        self.source.as_str()[pos..].chars().next()
    }

    #[inline(always)]
    fn at_eof(&self) -> bool {
        self.cursor >= self.source.as_bytes().len()
    }

    /// next char
    #[inline(always)]
    fn advance(&mut self) {
        let b = self.source.as_bytes()[self.cursor];
        if b < 128 {
            self.cursor += 1;
        } else {
            // only do the heavy decoding for actual utf-8
            let c = self.source.as_str()[self.cursor..].chars().next().unwrap();
            self.cursor += c.len_utf8();
        }
    }

    // use to advance a known number of bytes
    #[inline(always)]
    fn advance_by(&mut self, bytes: usize) {
        // so that you cannot call in the middle of a char (utf-8 bs)
        debug_assert!(self.source.as_str().is_char_boundary(self.cursor + bytes));
        self.cursor += bytes
    }

    // advances while predicate is true
    fn consume_while<F>(&mut self, pred: F)
    where
        F: Fn(u8) -> bool,
    {
        let bytes = self.source.as_bytes();
        while self.cursor < bytes.len() && pred(bytes[self.cursor]) {
            self.cursor += 1;
        }
    }

    #[inline(always)]
    fn lexeme(&self, start: usize) -> &'a str {
        // SAFETY: The lexer cursor only moves over valid UTF-8 boundaries.
        // by using get_unchecked, we remove the bounds check and the option handling.
        unsafe { self.source.as_str().get_unchecked(start..self.cursor) }
    }

    #[inline(always)]
    fn start_token(&mut self) {
        self.tstart = self.cursor
    }

    /// Finalizes the current token. Identifiers and string literals are
    /// interned so later stages share one canonical handle per string; the
    /// returned [`Symbol`] is discarded here because the text now lives in
    /// the table, keyed by the same text the next stage will re-intern.
    fn end_token(&mut self, kind: TokenKind) -> Option<Token> {
        let span = span![self.tstart, self.cursor];
        if matches!(kind, TokenKind::Ident | TokenKind::String) {
            let lexeme = &self.source.as_str()[span.range()];
            // a string literal interns its contents, without the surrounding
            // quotes; strip exactly one quote each side (a missing closing
            // quote on an unclosed literal simply leaves that side as-is)
            let text = if kind == TokenKind::String {
                let inner = lexeme.strip_prefix('"').unwrap_or(lexeme);
                inner.strip_suffix('"').unwrap_or(inner)
            } else {
                lexeme
            };
            self.interner.intern(text);
        }
        Some(tok![kind, span])
    }

    fn end_and_advance(&mut self, kind: TokenKind) -> Option<Token> {
        self.advance();
        self.end_token(kind)
    }

    fn lex_ident(&mut self) -> Option<Token> {
        self.consume_while(|b| b.is_ascii_alphanumeric() || b == b'_');

        if let Some(ch) = self.peek(0)
            && ch >= '\u{80}'
            && ch.is_alphanumeric()
        {
            while let Some(c) = self.peek(0) {
                if c.is_alphanumeric() || c == '_' {
                    self.advance_by(c.len_utf8());
                } else {
                    break;
                }
            }
        }

        let lexeme = self.lexeme(self.tstart);
        self.end_token(keyword(lexeme))
    }

    fn lex_number_kind(num: &str) -> TokenKind {
        // `lex_number` only consumes a `.` when a digit follows, so a numeric
        // lexeme has at most one decimal point — a dot means it is a float
        if num.contains('.') {
            TokenKind::Float
        } else {
            TokenKind::Int
        }
    }

    fn lex_string(&mut self) -> Option<Token> {
        self.advance();
        while let Some(ch) = self.peek(0) {
            if ch == '"' {
                self.advance();
                return self.end_token(TokenKind::String);
            }
            // `ch` came from `peek(0)`, so EOF is already excluded here; a
            // newline is what cuts a string literal short
            if ch == '\n' {
                self.diags.push(Diagnostic {
                    msg: "unclosed string literal".into(),
                    span: span![self.tstart, self.cursor],
                });
                return self.end_token(TokenKind::String);
            }
            if ch == '\\' {
                self.advance();
                if self.peek(0).is_some() {
                    self.advance();
                }
                continue;
            }
            self.advance();
        }
        self.diags.push(Diagnostic {
            msg: "unclosed string literal".into(),
            span: span![self.tstart, self.cursor],
        });
        self.end_token(TokenKind::String)
    }

    #[cold]
    fn error(&mut self, msg: impl Into<String>, span: Span) {
        self.diags.push(Diagnostic {
            msg: msg.into().into(),
            span,
        });
    }

    fn lex_char(&mut self) -> Option<Token> {
        let start = self.cursor;
        self.advance(); // consume opening '

        // empty char literal: ''
        if self.peek(0) == Some('\'') {
            self.advance();
            self.error("empty character literal", span![start, self.cursor]);
            return self.end_token(TokenKind::Char);
        }

        match self.peek(0) {
            Some('\\') => {
                self.advance(); // consume '\'
                if let Some(escaped) = self.peek(0) {
                    let len = escaped.len_utf8();
                    self.advance_by(len);
                } else {
                    self.error("unterminated escape", span![start, self.cursor]);
                    return self.end_token(TokenKind::Char);
                }
            }
            Some(ch) if ch != '\n' && ch != '\'' => {
                self.advance_by(ch.len_utf8());
            }
            _ => {
                if self.at_eof() {
                    self.error("unclosed char literal", span![start, self.cursor]);
                } else {
                    self.error("invalid char in literal", span![start, self.cursor]);
                }
                return self.end_token(TokenKind::Char);
            }
        }

        // closing '''
        if self.peek(0) == Some('\'') {
            self.advance();
        } else {
            self.error("missing closing quote", span![start, self.cursor]);
        }

        self.end_token(TokenKind::Char)
    }

    fn lex_number(&mut self) -> Option<Token> {
        self.consume_while(|b| b.is_ascii_digit());
        while self.peek(0) == Some('.') && self.peek(1).is_some_and(|b| b.is_ascii_digit()) {
            self.advance(); // consume '.'
            self.consume_while(|b| b.is_ascii_digit());
        }

        let lexeme = self.lexeme(self.tstart);
        let kind = Self::lex_number_kind(lexeme);
        self.end_token(kind)
    }

    fn lex_line_comment(&mut self) -> Option<Token> {
        self.advance(); // '-'

        let is_doc = self.peek(0).is_some_and(|c| c == '-');
        if is_doc {
            self.advance(); // '-'
        }
        self.consume_while(|b| b != b'\n');

        let kind = if is_doc {
            TokenKind::Dcomment
        } else {
            TokenKind::Lcomment
        };
        self.end_token(kind)
    }

    fn lex_block_comment(&mut self) -> Option<Token> {
        self.advance(); // '*' — opening `--*` now fully consumed

        // symmetric delimiter: a block comment closes on the next `--*`
        while let Some(c) = self.peek(0) {
            if c == '-'
                && self.peek(1).is_some_and(|c2| c2 == '-')
                && self.peek(2).is_some_and(|c3| c3 == '*')
            {
                self.advance(); // '-'
                self.advance(); // '-'
                self.advance(); // '*'
                return self.end_token(TokenKind::Bcomment);
            }
            self.advance();
        }

        // EOF reached without seeing a closing `--*`
        self.diags.push(Diagnostic {
            msg: "unclosed block comment".into(),
            span: span![self.tstart, self.cursor],
        });
        self.end_token(TokenKind::Bcomment)
    }

    #[cold]
    fn lex_utf8_fallback(&mut self) -> Option<Token> {
        let start = self.cursor;

        // peek the actual unicode character
        // we use the existing peek(0) logic that handles utf-8 decoding
        if let Some(ch) = self.peek(0) {
            if unicode_ident::is_xid_start(ch) {
                // If it's a valid Unicode start, let lex_ident handle it
                // lex_ident should be updated to use ch.len_utf8()
                return self.lex_ident();
            } else {
                // it's a valid utf-8 char, but not a valid identifier start (e.g., an emoji)
                self.advance_by(ch.len_utf8());
                self.error(
                    format!("unexpected character: '{}'", ch),
                    span![start, self.cursor],
                );
                return self.end_token(TokenKind::Illegal);
            }
        }

        // actually invalid utf-8 bytes
        self.advance_by(1);
        self.error("invalid utf-8 sequence", span![start, self.cursor]);
        self.end_token(TokenKind::Illegal)
    }

    /// disambiguates a leading `-`: `->`, `--`/`---` comments, `--*` block, or `Minus`
    fn lex_minus_or_comment(&mut self) -> Option<Token> {
        match self.peek(1) {
            // `--` opens a comment. The second `-` is consumed *before* the
            // next peek so the cursor sits on a known ascii byte — a single
            // eager `peek(2)` could index into the middle of a multi-byte
            // char and panic (e.g. input `-é`).
            Some('-') => {
                self.advance(); // first '-'
                if self.peek(1) == Some('*') {
                    self.advance(); // second '-'
                    self.lex_block_comment() // consumes the `*`
                } else {
                    self.lex_line_comment() // consumes the second '-'
                }
            }
            Some('>') => {
                self.advance(); // '-'
                self.end_and_advance(TokenKind::Arrow)
            }
            _ => self.end_and_advance(TokenKind::Minus),
        }
    }

    /// consumes one byte known not to start any defined token
    #[cold]
    fn lex_illegal_byte(&mut self, b: u8) -> Option<Token> {
        self.advance();
        self.error(
            format!("unexpected character: '{}'", b as char),
            span![self.tstart, self.cursor],
        );
        self.end_token(TokenKind::Illegal)
    }

    /// produces exactly one token, including trivia (whitespace/comments/newlines)
    /// and a final [`TokenKind::Eof`]; never returns `None`
    fn next_token(&mut self) -> Token {
        self.start_token();

        if self.at_eof() {
            return self.end_token(TokenKind::Eof).unwrap();
        }

        // a non-ascii lead byte is the only path that decodes utf-8 here;
        // every defined token starts with an ascii byte
        let b = self.source.as_bytes()[self.cursor];
        let token = match b {
            b' ' | b'\t' | b'\r' => {
                self.consume_while(|b| matches!(b, b' ' | b'\t' | b'\r'));
                self.end_token(TokenKind::Wspace)
            }
            b'\n' => self.end_and_advance(TokenKind::Newline),

            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(),
            b'0'..=b'9' => self.lex_number(),
            b'"' => self.lex_string(),
            b'\'' => self.lex_char(),

            // `(` and `)` always lex separately; `()` as a unit is inferred
            // by the parser, never collapsed here
            b'(' => self.end_and_advance(TokenKind::Oparen),
            b')' => self.end_and_advance(TokenKind::Cparen),
            b'{' => self.end_and_advance(TokenKind::Obrace),
            b'}' => self.end_and_advance(TokenKind::Cbrace),
            b'[' => self.end_and_advance(TokenKind::Obrack),
            b']' => self.end_and_advance(TokenKind::Cbrack),
            b',' => self.end_and_advance(TokenKind::Comma),
            b';' => self.end_and_advance(TokenKind::Semicolon),
            b':' => self.end_and_advance(TokenKind::Colon),

            b'+' => self.end_and_advance(TokenKind::Plus),
            b'*' => self.end_and_advance(TokenKind::Star),
            b'/' => self.end_and_advance(TokenKind::Slash),
            b'.' => self.end_and_advance(TokenKind::Dot),

            b'-' => self.lex_minus_or_comment(),

            b'=' => match self.peek(1) {
                Some('=') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Eq)
                }
                Some('>') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Farrow)
                }
                _ => self.end_and_advance(TokenKind::Assign),
            },
            b'<' => match self.peek(1) {
                Some('=') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Leq)
                }
                _ => self.end_and_advance(TokenKind::Lt),
            },
            b'>' => match self.peek(1) {
                Some('=') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Geq)
                }
                _ => self.end_and_advance(TokenKind::Gt),
            },
            b'&' => match self.peek(1) {
                Some('&') => {
                    self.advance();
                    self.end_and_advance(TokenKind::And)
                }
                _ => self.lex_illegal_byte(b),
            },
            b'|' => match self.peek(1) {
                Some('|') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Or)
                }
                _ => self.lex_illegal_byte(b),
            },
            b'!' => match self.peek(1) {
                Some('=') => {
                    self.advance();
                    self.end_and_advance(TokenKind::Neq)
                }
                _ => self.lex_illegal_byte(b),
            },

            // any other ascii byte starts no defined token
            0..=127 => self.lex_illegal_byte(b),
            // non-ascii: identifier continuation or a diagnosed stray char
            _ => self.lex_utf8_fallback(),
        };

        // every helper above yields `Some`; the `Option` is purely uniform plumbing
        token.unwrap()
    }

    /// Drives the lexer to completion and consumes it, yielding the token
    /// stream (up to and including [`TokenKind::Eof`]), the populated string
    /// table, and every diagnostic — see [`Lexed`].
    pub fn tokenize(mut self) -> Lexed {
        let mut tokens = Vec::with_capacity(self.source.len() / 4 + 1);
        for token in self.by_ref() {
            tokens.push(token);
        }
        Lexed {
            tokens,
            interner: self.interner,
            diags: self.diags,
        }
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    #[inline]
    fn next(&mut self) -> Option<Token> {
        if self.eof_emitted {
            return None;
        }
        let token = self.next_token();
        self.eof_emitted = token.kind == TokenKind::Eof;
        Some(token)
    }
}

// after `Eof` the iterator yields `None` forever
impl std::iter::FusedIterator for Lexer<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokenize `src` into its full [`Lexed`] result.
    fn lex(src: &str) -> Lexed {
        let source = SourceText::new(src.to_string());
        Lexer::new(&source).tokenize()
    }

    /// Non-trivia token kinds — the shape the parser actually consumes.
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
            kinds("const var structure enum if else loop break continue foo"),
            [Const, Var, Structure, Enum, If, Else, Loop, Break, Continue, Ident, Eof]
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

    /// Regression: `-` immediately before a multi-byte char used to index
    /// into the middle of that char and panic. It must lex cleanly now.
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
        assert!(lexed.diags[0].msg.contains("unclosed string"));
    }

    #[test]
    fn unclosed_block_comment_diagnoses() {
        let lexed = lex("--* never closed");
        assert_eq!(lexed.diags.len(), 1);
        assert!(lexed.diags[0].msg.contains("unclosed block comment"));
    }

    #[test]
    fn empty_char_diagnoses() {
        let lexed = lex("''");
        assert_eq!(lexed.diags.len(), 1);
        assert!(lexed.diags[0].msg.contains("empty character"));
    }

    #[test]
    fn spans_tile_the_source_with_no_gaps() {
        let src = "const x = 0;";
        let lexed = lex(src);
        let mut cursor = 0;
        for tok in &lexed.tokens {
            assert_eq!(tok.span.start, cursor, "gap or overlap at byte {cursor}");
            cursor = tok.span.end;
        }
        assert_eq!(cursor, src.len(), "tokens do not cover the whole source");
    }

    #[test]
    fn token_stream_snapshot() {
        let lexed = lex("add(int32 a) -> int32 { a + 1 }");
        insta::assert_debug_snapshot!(lexed.tokens);
    }
}
