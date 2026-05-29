//! Typed parser diagnostics. Two classes:
//! - [`SyntaxError`] (`S`): a token or node the grammar required but did not
//!   find.
//! - [`GrammarError`] (`G`): a deliberate rejection of input the grammar *could*
//!   parse but the language bans (footguns).
//!
//! Both are carried by [`ParseError`]; the prose message is the
//! [`Display`](std::fmt::Display) rendering via `thiserror`, never stored as the
//! source of truth.

use diagnostics::{Class, Code, Diagnostic};

/// A missing-token / missing-node syntax error (class `S`). One variant per
/// distinct grammar message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyntaxError {
    #[error("expected an item")]
    ExpectedItem,
    #[error("expected a struct name")]
    ExpectedStructName,
    #[error("expected ';' after struct definition")]
    ExpectedSemiAfterStruct,
    #[error("expected a union name")]
    ExpectedUnionName,
    #[error("expected ';' after union definition")]
    ExpectedSemiAfterUnion,
    #[error("expected '{{' to open extern block")]
    ExpectedExternOpen,
    #[error("expected an extern function signature")]
    ExpectedExternSignature,
    #[error("expected '}}' to close extern block")]
    ExpectedExternClose,
    #[error("expected ';' after extern signature")]
    ExpectedSemiAfterExternSig,
    #[error("expected '{{' to open field list")]
    ExpectedFieldListOpen,
    #[error("expected ',' after field")]
    ExpectedCommaAfterField,
    #[error("expected a field")]
    ExpectedField,
    #[error("expected '}}' to close field list")]
    ExpectedFieldListClose,
    #[error("expected a field name")]
    ExpectedFieldName,
    #[error("expected enum name")]
    ExpectedEnumName,
    #[error("expected '=' after enum name")]
    ExpectedEqAfterEnumName,
    #[error("expected at least one variant")]
    ExpectedAtLeastOneVariant,
    #[error("expected variant name after '|'")]
    ExpectedVariantNameAfterPipe,
    #[error("expected ';' after enum definition")]
    ExpectedSemiAfterEnum,
    #[error("expected ';' between array element type and length")]
    ExpectedSemiInArrayType,
    #[error("expected ']' to close array type")]
    ExpectedArrayTypeClose,
    #[error("expected a type")]
    ExpectedType,
    #[error("expected '('")]
    ExpectedOpenParen,
    #[error("expected parameter name")]
    ExpectedParamName,
    #[error("expected ')'")]
    ExpectedCloseParen,
    #[error("expected '{{' to open block")]
    ExpectedBlockOpen,
    #[error("expected ';' after expression")]
    ExpectedSemiAfterExpr,
    #[error("expected a statement")]
    ExpectedStatement,
    #[error("expected '}}' to close block")]
    ExpectedBlockClose,
    #[error("expected a binding name")]
    ExpectedBindingName,
    #[error("expected '=' in binding")]
    ExpectedEqInBinding,
    #[error("expected ';' after statement")]
    ExpectedSemiAfterStatement,
    #[error("expected ']' to close index")]
    ExpectedIndexClose,
    #[error("expected field identifier after '.'")]
    ExpectedFieldIdentAfterDot,
    #[error("expected '{{' to open match arms")]
    ExpectedMatchArmsOpen,
    #[error("expected a match arm")]
    ExpectedMatchArm,
    #[error("expected ',' between match arms")]
    ExpectedCommaBetweenMatchArms,
    #[error("expected '}}' to close match arms")]
    ExpectedMatchArmsClose,
    #[error("expected '->' after match pattern")]
    ExpectedArrowAfterPattern,
    #[error("expected variant name after '.'")]
    ExpectedVariantNameAfterDot,
    #[error("expected a pattern")]
    ExpectedPattern,
    #[error("expected an expression")]
    ExpectedExpression,
    #[error("expected an array element")]
    ExpectedArrayElement,
    #[error("expected ']' to close array literal")]
    ExpectedArrayLitClose,
    #[error("expected ')' to close parenthesized expression")]
    ExpectedParenExprClose,
    #[error("expected ')' to close argument list")]
    ExpectedArgListClose,
    #[error("expected '{{' to open struct literal")]
    ExpectedStructLitOpen,
    #[error("expected a field initializer")]
    ExpectedFieldInit,
    #[error("expected '}}' to close struct literal")]
    ExpectedStructLitClose,
}

impl Diagnostic for SyntaxError {
    fn code(&self) -> Code {
        // Numbering is assigned positionally; the class letter is what matters.
        Code::new(Class::Syntax, *self as u16 + 1)
    }
}

/// A deliberate grammar rejection (class `G`): input that parses but the
/// language bans on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrammarError {
    #[error("comparison operators do not chain; parenthesize one side, e.g. `(a < b) < c`")]
    ComparisonChain,
    #[error("assignment is not allowed in an `if` condition; use `==` to compare")]
    AssignInIfCondition,
}

impl Diagnostic for GrammarError {
    fn code(&self) -> Code {
        Code::new(Class::Grammar, *self as u16 + 1)
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
