macro_rules! define_tokens {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $display:expr
    ),* $(,)?) => {
        #[repr(u8)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// start, end
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Span { start, end }
    }

    pub const fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

define_tokens! {
    Eof = "EOF",
    Illegal = "ILLEGAL",

    Ident = "IDENT",

    // literals
    Int = "INT",
    Float = "FLOAT",
    String = "STRING",
    True = "TRUE",
    False = "FALSE",
    Char = "CHAR",

    // keywords
    Const = "CONST",
    Var = "VAR",
    Structure = "STRUCTURE",
    Enum = "ENUM",

    // control flow
    If = "IF",
    Else = "ELSE",
    Loop = "LOOP",
    Break = "BREAK",
    Continue = "CONTINUE",


    // delimiters
    Oparen = "(",
    Cparen = ")",
    Obrace = "{",
    Cbrace = "}",
    Obrack = "[",
    Cbrack = "]",
    Comma = ",",
    Semicolon = ";",
    Colon = ":",

    Assign = "=",

    // operators
    Plus = "+",      // +
    Minus = "-",     // -
    Star = "*",      // *
    Slash = "/",     // /
    And = "&&",      // &&
    Or = "||",       // ||
    Eq = "==",       // ==
    Neq = "!=",      // !=
    Lt = "<",        // <
    Gt = ">",        // >
    Leq = "<=",      // <=
    Geq = ">=",      // >=

    Arrow = "->",    // ->
    Farrow = "=>",   // =>
    Dot = ".",       // .
                     //
    Wspace = "WHITESPACE",    // ' '
    Lcomment = "LINE COMMENT", // --
    Bcomment = "BLOCK COMMENT", // --* --*
    Dcomment = "DOC COMMENT",  // ---
    Newline = "NEWLINE",      // '\n'
}
