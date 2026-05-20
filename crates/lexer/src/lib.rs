//! The lexer - raw source bytes to a flat [`Token`] stream.
//!
//! [`Lexer::tokenize`] drives `logos` (the lex rules live on [`TokenKind`] in
//! the `token` crate) and yields a [`Lexed`]: the token vector, the populated
//! [`Interner`], and any [`Diagnostic`]s. [`SourceText`] owns the input (heap
//! string or `mmap`) and answers byte-offset/line-column queries.

use logos::Logos;
use memchr::memchr_iter;
use memmap2::Mmap;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use text_size::{TextRange, TextSize};

use token::{Token, TokenKind};

pub use token::Diagnostic;

/// A interned string handle - an index into an [`Interner`]'s table. `Copy`
/// and pointer-free, so name comparison downstream is a `u32` equality check
/// instead of a `str` compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(pub u32);

/// The canonical string table. Every identifier and string literal the lexer
/// sees is interned here, so the same text always maps to the same [`Symbol`].
///
/// Strings are stored as [`SmolStr`]: identifiers - almost always short -
/// stay inline with no heap allocation, and a cache-hit clone is `O(1)`.
///
/// The lexer pre-populates this during tokenizing; later stages (HIR name
/// resolution) re-intern identifier text against the *same* table - a cache
/// hit yields the original `Symbol`. The table outlives the lexer: it is
/// handed off in [`Lexed`].
#[derive(Debug)]
pub struct Interner {
    map: FxHashMap<SmolStr, Symbol>,
    vec: Vec<SmolStr>,
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
        let owned = SmolStr::new(s);
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

/// line (one based), col (one based, byte offset from line start)
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

    /// The source text a [`TextRange`] covers, or `None` if it is out of
    /// bounds or not on `char` boundaries.
    pub fn slice(&self, range: TextRange) -> Option<&str> {
        self.as_str()
            .get(usize::from(range.start())..usize::from(range.end()))
    }
}

/// The complete result of tokenizing: the token stream, the populated string
/// table, and every diagnostic. Produced by [`Lexer::tokenize`].
#[derive(Debug)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub interner: Interner,
    pub diags: Vec<Diagnostic>,
}

/// A thin driver over `logos`. Holds only the borrowed source; the real work
/// is in [`Lexer::tokenize`].
pub struct Lexer<'a> {
    pub source: &'a SourceText,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a SourceText) -> Self {
        Lexer { source }
    }

    /// Drives `logos` to completion, yielding the token stream (up to and
    /// including a synthesized [`TokenKind::Eof`]), the populated string
    /// table, and every diagnostic - see [`Lexed`].
    pub fn tokenize(self) -> Lexed {
        let src = self.source.as_str();
        let mut lex = TokenKind::lexer(src);
        let mut tokens = Vec::with_capacity(src.len() / 4 + 1);
        let mut interner = Interner::new();
        // diagnostics from lex errors; the callback diagnostics for unclosed
        // literals/comments accumulate separately in `lex.extras`
        let mut err_diags: Vec<Diagnostic> = Vec::new();

        while let Some(result) = lex.next() {
            let span = lex.span();
            let range = TextRange::new(
                TextSize::from(span.start as u32),
                TextSize::from(span.end as u32),
            );
            let kind = match result {
                Ok(kind) => kind,
                Err(()) => {
                    err_diags.push(Diagnostic {
                        msg: format!("unexpected character: '{}'", &src[span.clone()]).into(),
                        range,
                    });
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

        // merge the two diagnostic streams into one source-ordered list
        let mut diags = err_diags;
        diags.extend(lex.extras.0);
        diags.sort_by_key(|d| d.range.start());

        Lexed {
            tokens,
            interner,
            diags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokenize `src` into its full [`Lexed`] result.
    fn lex(src: &str) -> Lexed {
        let source = SourceText::new(src.to_string());
        Lexer::new(&source).tokenize()
    }

    /// Non-trivia token kinds - the shape the parser actually consumes.
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
            [
                Const, Var, Structure, Enum, If, Else, Loop, Break, Continue, Ident, Eof
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
        // Windows `\r\n`: `memchr` finds the `\n`, so the line start lands one
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
    fn token_stream_snapshot() {
        let lexed = lex("add(int32 a) -> int32 { a + 1 }");
        insta::assert_debug_snapshot!(lexed.tokens);
    }
}
