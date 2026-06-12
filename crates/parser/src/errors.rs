//! Typed parser diagnostics. Two classes:
//! - [`SyntaxError`] (`S`): a token or node the grammar required but did not
//!   find.
//! - [`GrammarError`] (`G`): a deliberate rejection of input the grammar *could*
//!   parse but the language bans (footguns).
//!
//! Both are carried by [`ParseError`]; the prose message is the
//! [`Display`](std::fmt::Display) rendering via `thiserror`, never stored as the
//! source of truth.
//!
//! Each variant carries an **explicit** diagnostic number as its discriminant.
//! The number is part of the stable surface, so it must never change once
//! assigned: append a new variant with the next free number, never renumber or
//! reorder. `code()` reads the discriminant directly (`*self as u16`), so a
//! reorder cannot silently shift any other code.

use diagnostics::{Class, Code, Diagnostic};

/// A missing-token / missing-node syntax error (class `S`). One variant per
/// distinct grammar message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[repr(u16)]
pub enum SyntaxError {
    #[error("expected an item")]
    ExpectedItem = 1,
    #[error("expected a constant name")]
    ExpectedConstName = 2,
    #[error("expected '=' in const definition")]
    ExpectedEqInConst = 3,
    #[error("expected ';' after const definition")]
    ExpectedSemiAfterConst = 4,
    #[error("expected a struct name")]
    ExpectedStructName = 5,
    #[error("expected ';' after struct definition")]
    ExpectedSemiAfterStruct = 6,
    #[error("expected a union name")]
    ExpectedUnionName = 7,
    #[error("expected ';' after union definition")]
    ExpectedSemiAfterUnion = 8,
    #[error("expected '{{' to open extern block")]
    ExpectedExternOpen = 9,
    #[error("expected an extern function signature")]
    ExpectedExternSignature = 10,
    #[error("expected '}}' to close extern block")]
    ExpectedExternClose = 11,
    #[error("expected ';' after extern signature")]
    ExpectedSemiAfterExternSig = 12,
    #[error("expected '{{' to open field list")]
    ExpectedFieldListOpen = 13,
    #[error("expected ',' after field")]
    ExpectedCommaAfterField = 14,
    #[error("expected a field")]
    ExpectedField = 15,
    #[error("expected '}}' to close field list")]
    ExpectedFieldListClose = 16,
    #[error("expected a field name")]
    ExpectedFieldName = 17,
    #[error("expected enum name")]
    ExpectedEnumName = 18,
    #[error("expected '=' after enum name")]
    ExpectedEqAfterEnumName = 19,
    #[error("expected at least one variant")]
    ExpectedAtLeastOneVariant = 20,
    #[error("expected variant name after '|'")]
    ExpectedVariantNameAfterPipe = 21,
    #[error("expected ';' after enum definition")]
    ExpectedSemiAfterEnum = 22,
    #[error("expected ';' between array element type and length")]
    ExpectedSemiInArrayType = 23,
    #[error("expected ']' to close array type")]
    ExpectedArrayTypeClose = 24,
    #[error("expected a type")]
    ExpectedType = 25,
    #[error("expected '('")]
    ExpectedOpenParen = 26,
    #[error("expected parameter name")]
    ExpectedParamName = 27,
    #[error("expected ')'")]
    ExpectedCloseParen = 28,
    #[error("expected '{{' to open block")]
    ExpectedBlockOpen = 29,
    #[error("expected ';' after expression")]
    ExpectedSemiAfterExpr = 30,
    #[error("expected a statement")]
    ExpectedStatement = 31,
    #[error("expected '}}' to close block")]
    ExpectedBlockClose = 32,
    #[error("expected a binding name")]
    ExpectedBindingName = 33,
    #[error("expected '=' in binding")]
    ExpectedEqInBinding = 34,
    #[error("expected ';' after statement")]
    ExpectedSemiAfterStatement = 35,
    #[error("expected ']' to close index")]
    ExpectedIndexClose = 36,
    #[error("expected field identifier after '.'")]
    ExpectedFieldIdentAfterDot = 37,
    #[error("expected '{{' to open match arms")]
    ExpectedMatchArmsOpen = 38,
    #[error("expected a match arm")]
    ExpectedMatchArm = 39,
    #[error("expected ',' between match arms")]
    ExpectedCommaBetweenMatchArms = 40,
    #[error("expected '}}' to close match arms")]
    ExpectedMatchArmsClose = 41,
    #[error("expected '->' after match pattern")]
    ExpectedArrowAfterPattern = 42,
    #[error("expected variant name after '.'")]
    ExpectedVariantNameAfterDot = 43,
    #[error("expected a pattern")]
    ExpectedPattern = 44,
    #[error("expected an expression")]
    ExpectedExpression = 45,
    #[error("expected an array element")]
    ExpectedArrayElement = 46,
    #[error("expected ']' to close array literal")]
    ExpectedArrayLitClose = 47,
    #[error("expected ')' to close parenthesized expression")]
    ExpectedParenExprClose = 48,
    #[error("expected ')' to close argument list")]
    ExpectedArgListClose = 49,
    #[error("expected '{{' to open struct literal")]
    ExpectedStructLitOpen = 50,
    #[error("expected a field initializer")]
    ExpectedFieldInit = 51,
    #[error("expected '}}' to close struct literal")]
    ExpectedStructLitClose = 52,
    #[error("expected a type name after `type`")]
    ExpectedExternTypeName = 53,
    #[error("expected ';' after extern type declaration")]
    ExpectedSemiAfterExternType = 54,
}

impl Diagnostic for SyntaxError {
    fn code(&self) -> Code {
        // The discriminant IS the stable number; see the module-level note.
        Code::new(Class::Syntax, *self as u16)
    }
}

/// A deliberate grammar rejection (class `G`): input that parses but the
/// language bans on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[repr(u16)]
pub enum GrammarError {
    #[error("comparison operators do not chain; parenthesize one side, e.g. `(a < b) < c`")]
    ComparisonChain = 1,
    #[error("assignment is not allowed in an `if` condition; use `==` to compare")]
    AssignInIfCondition = 2,
    #[error(
        "struct patterns are not yet supported in match arms; bind the value and destructure with `let`"
    )]
    StructPatInMatchArm = 3,
    #[error("`...` is only allowed in an `extern` signature")]
    VariadicOutsideExtern = 4,
    #[error("`...` must be the last parameter")]
    VariadicNotLast = 5,
    #[error("`...` requires at least one named parameter before it")]
    VariadicNeedsNamedParam = 6,
}

impl Diagnostic for GrammarError {
    fn code(&self) -> Code {
        Code::new(Class::Grammar, *self as u16)
    }
}

/// The carrier for every parser diagnostic. Partitioned by class so callers
/// emit a `SyntaxError`/`GrammarError` and the renderer routes on [`Code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    #[error(transparent)]
    Grammar(#[from] GrammarError),
}

impl Diagnostic for ParseError {
    fn code(&self) -> Code {
        match self {
            ParseError::Syntax(e) => e.code(),
            ParseError::Grammar(e) => e.code(),
        }
    }
}
