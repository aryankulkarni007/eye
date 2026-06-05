//! Lexical tokens - the shared vocabulary of the compiler front-end.
//!
//! [`Token`] and [`TokenKind`] are produced by the lexer and consumed by the
//! syntax and parser crates, so they live in this leaf crate that everything
//! downstream depends on.
//!
//! [`TokenKind`] carries the `logos` lexer rules directly: every variant is
//! annotated with the `#[token]`/`#[regex]` that matches it, so the lexer
//! crate is a thin driver over `TokenKind::lexer`. Ranges are `text-size`'s
//! [`TextRange`] - the same range type `rowan` uses for the CST.

use logos::Logos;
use text_size::{TextRange, TextSize};
use thin_vec::ThinVec;

/// Builds a [`TextRange`] from a `logos` byte span.
fn to_range(span: std::ops::Range<usize>) -> TextRange {
    TextRange::new(
        TextSize::from(span.start as u32),
        TextSize::from(span.end as u32),
    )
}

/// A lexeme-level error tag recorded by the logos callbacks. Payload-free: the
/// `lexer` crate maps each tag to a typed diagnostic kind. Kept in this leaf
/// crate so the callbacks carry no `diagnostics` dependency (which would form a
/// `token -> diagnostics -> syntax -> token` cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorTag {
    UnclosedString,
    EmptyChar,
    UnterminatedEscape,
    UnclosedChar,
    InvalidChar,
    MissingQuote,
    UnclosedBlockComment,
}

/// `logos` lexer state - tagged lexeme errors for unclosed or malformed
/// lexemes, each paired with the span it covers.
#[derive(Debug, Default)]
pub struct LexExtras(pub ThinVec<(LexErrorTag, TextRange)>);

/// A token -> kind: `TokenKind`, range: `TextRange`
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: TextRange,
}

/// Every lexical token kind. The `logos` rules live in the
/// per-variant attributes; [`TokenKind::lexer`] drives them.
macro_rules! define_tokens {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $display:expr
    ),* $(,)?) => {
        #[repr(u8)]
        #[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
        #[logos(extras = LexExtras)]
        pub enum TokenKind {
            $(
                $(#[$attr])*
                $variant
            ),*
        }

        impl std::fmt::Display for TokenKind {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(TokenKind::$variant => write!(f, "{}", $display)),*
                }
            }
        }
    };
}

define_tokens! {
    // `Eof` and `Illegal` are never produced by `logos` - the lexer driver
    // synthesizes `Eof` at the end of input and `Illegal` from a lex error.
    Eof = "EOF",
    Illegal = "ILLEGAL",

    #[regex(r"[\p{XID_Start}_]\p{XID_Continue}*")]
    Ident = "IDENT",

    // literals.
    // Integers carry an optional base prefix: `0x`/`0X` hex, `0b`/`0B` binary,
    // `0o`/`0O` octal, else decimal. Logos picks the longest match, so `0x1F`
    // takes the hex rule over the decimal rule (which would match only `0`).
    // The literal's value is parsed in HIR (`parse_int_literal`); codegen emits
    // it in decimal, so C never sees a `0b`/`0o` prefix it cannot read.
    #[regex(r"0[xX][0-9a-fA-F]+")]
    #[regex(r"0[bB][01]+")]
    #[regex(r"0[oO][0-7]+")]
    #[regex(r"[0-9]+")]
    Int = "INT",
    #[regex(r"[0-9]+(\.[0-9]+)+")]
    Float = "FLOAT",
    #[token("\"", lex_string)]
    String = "STRING",
    #[token("true")]
    True = "TRUE",
    #[token("false")]
    False = "FALSE",
    #[token("'", lex_char)]
    Char = "CHAR",

    // keywords
    #[token("let")]
    Let = "LET",
    #[token("mut")]
    Mut = "MUT",
    #[token("const")]
    Const = "CONST",
    #[token("structure")]
    Structure = "STRUCTURE",
    #[token("enum")]
    Enum = "ENUM",
    #[token("union")]
    Union = "UNION",
    #[token("extern")]
    Extern = "EXTERN",
    // `type Name;` inside an extern block declares an opaque FFI type.
    #[token("type")]
    Type = "TYPE",

    // control flow
    #[token("if")]
    If = "IF",
    #[token("else")]
    Else = "ELSE",
    #[token("loop")]
    Loop = "LOOP",
    #[token("break")]
    Break = "BREAK",
    #[token("continue")]
    Continue = "CONTINUE",
    #[token("return")]
    Return = "RETURN",
    #[token("match")]
    Match = "MATCH",
    #[token("as")]
    As = "AS",

    // a lone `_`. The ident regex would also match it - `priority = 3`
    // breaks the tie in favour of `Underscore`. `_foo` still lexes as
    // `Ident` because the regex match is strictly longer.
    #[token("_", priority = 3)]
    Underscore = "_",

    // delimiters
    #[token("(")]
    Oparen = "(",
    #[token(")")]
    Cparen = ")",
    #[token("{")]
    Obrace = "{",
    #[token("}")]
    Cbrace = "}",
    #[token("[")]
    Obrack = "[",
    #[token("]")]
    Cbrack = "]",
    #[token(",")]
    Comma = ",",
    #[token(";")]
    Semicolon = ";",
    #[token(":")]
    Colon = ":",

    #[token("=")]
    Assign = "=",
    // compound assignment. `-=` never collides with the `--` line-comment
    // rule (that needs two dashes); maximal munch picks `+=`/`-=` over `+`/`-`.
    #[token("+=")]
    PlusEq = "+=",
    #[token("-=")]
    MinusEq = "-=",
    // The remaining compound forms. Maximal munch keeps each whole: `<<=`
    // outranks `<<`/`<=`/`<`, `>>=` outranks `>>`, and `/=` never collides with
    // a comment (Eye line comments are `--`, blocks `--*`).
    #[token("*=")]
    StarEq = "*=",
    #[token("/=")]
    SlashEq = "/=",
    #[token("%=")]
    PercentEq = "%=",
    #[token("&=")]
    AmpEq = "&=",
    #[token("|=")]
    PipeEq = "|=",
    #[token("^=")]
    CaretEq = "^=",
    #[token("<<=")]
    ShlEq = "<<=",
    #[token(">>=")]
    ShrEq = ">>=",

    // operators
    #[token("+")]
    Plus = "+",
    #[token("-")]
    Minus = "-",
    #[token("*")]
    Star = "*",
    #[token("/")]
    Slash = "/",
    #[token("%")]
    Percent = "%",
    // bitwise: ~ prefix complement, ^ xor, <</>> shifts. Infix &/|
    // (bitand/bitor) reuse the Amp/Pipe tokens, disambiguated by parser
    // position from prefix-ref / enum-separator.
    #[token("~")]
    Tilde = "~",
    #[token("^")]
    Caret = "^",
    #[token("<<")]
    Shl = "<<",
    #[token(">>")]
    Shr = ">>",
    // ! logical-not. != wins maximal munch, so ! only matches alone.
    #[token("!")]
    Bang = "!",
    #[token("&&")]
    And = "&&",
    #[token("||")]
    Or = "||",
    #[token("==")]
    Eq = "==",
    #[token("!=")]
    Neq = "!=",
    #[token("<")]
    Lt = "<",
    #[token(">")]
    Gt = ">",
    #[token("<=")]
    Leq = "<=",
    #[token(">=")]
    Geq = ">=",

    #[token("->")]
    Arrow = "->",
    #[token("=>")]
    Farrow = "=>",
    #[token(".")]
    Dot = ".",
    // variadic marker in an extern signature. Maximal munch keeps `...`
    // whole over three `.` tokens.
    #[token("...")]
    Ellipsis = "...",
    #[token("&")]
    Amp = "&",
    #[token("|")]
    Pipe = "|",

    // trivia
    #[regex(r"[ \t\r]+")]
    Wspace = "WHITESPACE",
    // a line comment is -- not opening a block comment: the [^*\n]
    // after -- keeps this rule from swallowing a --* block open.
    // allow_greedy: a comment is meant to run to end of line.
    #[regex(r"--([^*\n][^\n]*)?", allow_greedy = true)]
    Lcomment = "LINE COMMENT",
    // ---…  outranks the line-comment rule on the equal-length --- tie
    #[regex(r"---[^\n]*", priority = 5, allow_greedy = true)]
    Dcomment = "DOC COMMENT",
    #[token("--*", lex_block_comment)]
    Bcomment = "BLOCK COMMENT",
    #[token("\n")]
    Newline = "NEWLINE",
}

// ---- literal / comment callbacks ----
//
// logos matches only the opening byte(s) of these; the callback scans the
// remainder, bumps the token to its true end, and records a diagnostic for
// an unclosed or malformed lexeme - so an unclosed literal is still a real
// String/Char/Bcomment token, never a lex error.

/// Records a tagged error spanning the just-lexed (bumped) token.
fn diag(lex: &mut logos::Lexer<TokenKind>, tag: LexErrorTag) {
    let range = to_range(lex.span());
    lex.extras.0.push((tag, range));
}

/// " opened a string literal. Consumes through the closing quote; a newline
/// or end of input cuts it short with an "unclosed string literal" diagnostic.
fn lex_string(lex: &mut logos::Lexer<TokenKind>) {
    let rem = lex.remainder();
    let bytes = rem.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                lex.bump(i + 1);
                return;
            }
            b'\n' => {
                lex.bump(i);
                diag(lex, LexErrorTag::UnclosedString);
                return;
            }
            b'\\' => {
                i += 1; // the backslash
                if i < bytes.len() {
                    // skip one whole escaped char (it may be multi-byte)
                    let c = rem[i..].chars().next().unwrap();
                    i += c.len_utf8();
                }
            }
            _ => i += 1,
        }
    }
    lex.bump(i);
    diag(lex, LexErrorTag::UnclosedString);
}

/// ' opened a char literal. Mirrors the per-case diagnostics of the old
/// hand-written lex_char: empty literal, unterminated escape, invalid char,
/// missing closing quote.
fn lex_char(lex: &mut logos::Lexer<TokenKind>) {
    let rem = lex.remainder();
    let mut i = 0usize;
    match rem.chars().next() {
        // empty literal: `''`
        Some('\'') => {
            lex.bump(1);
            diag(lex, LexErrorTag::EmptyChar);
            return;
        }
        Some('\\') => {
            i += 1; // the backslash
            match rem[i..].chars().next() {
                Some(c) => i += c.len_utf8(),
                None => {
                    lex.bump(i);
                    diag(lex, LexErrorTag::UnterminatedEscape);
                    return;
                }
            }
        }
        Some(c) if c != '\n' => i += c.len_utf8(),
        // `\n` or end of input - no char to put in the literal
        other => {
            lex.bump(i);
            diag(
                lex,
                if other.is_none() {
                    LexErrorTag::UnclosedChar
                } else {
                    LexErrorTag::InvalidChar
                },
            );
            return;
        }
    }
    if rem[i..].starts_with('\'') {
        lex.bump(i + 1);
    } else {
        lex.bump(i);
        diag(lex, LexErrorTag::MissingQuote);
    }
}

/// `--*` opened a block comment. Block comments use the symmetric `--*`
/// delimiter, so this consumes through the next `--*`.
fn lex_block_comment(lex: &mut logos::Lexer<TokenKind>) {
    let rem = lex.remainder();
    match rem.find("--*") {
        Some(pos) => lex.bump(pos + 3),
        None => {
            lex.bump(rem.len());
            diag(lex, LexErrorTag::UnclosedBlockComment);
        }
    }
}
