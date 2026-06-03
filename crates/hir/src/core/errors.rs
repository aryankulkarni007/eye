//! Typed HIR diagnostics, partitioned into the five HIR error classes:
//! [`ResolveError`] (`R`), [`TypeError`] (`T`), [`PatternError`] (`P`),
//! [`ConstError`] (`C`), and [`UnsupportedError`] (`U`). All are carried by
//! [`HirError`], the single accumulator kind, so lowering keeps one
//! [`Sink`](diagnostics::Sink) while every entry stays concretely typed for
//! in-crate `matches!` assertions.
//!
//! `Display` is hand-written rather than derived via `thiserror`: several
//! messages need list-joining and pluralization that a static `#[error("...")]`
//! template cannot express. The `Display + Debug` bound is what the
//! [`Diagnostic`](diagnostics::Diagnostic) trait requires; how each impl
//! supplies it is a per-crate choice.

use std::fmt;

use diagnostics::{Class, Code, Diagnostic};

use crate::core::{Text, TypeRef};

/// `R`: name-resolution failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    DuplicateItem {
        name: Text,
    },
    DuplicateVariantDecl {
        variant: Text,
        enum_name: Text,
    },
    UnknownEnumInPattern {
        enum_name: Text,
    },
    PatternEnumMismatch {
        pattern_enum: Text,
        scrutinee_enum: Text,
    },
    NoSuchVariant {
        enum_name: Text,
        variant: Text,
    },
    UnknownVariantInPattern {
        variant: Text,
    },
    EnumNameAsValue {
        name: Text,
    },
    /// A name in value position resolves to nothing: not a local, function,
    /// type, or variant. The `print`/`len` call intrinsics are excepted (they
    /// are sniffed by their unresolved name). This is the principled
    /// replacement for the old unresolved-name accident, where the HIR-walk
    /// backend emitted any unknown identifier verbatim as C (an undeclared
    /// `printf(...)` the linker resolved, the bare `return` keyword); MIR is a
    /// resolved IR, so it diagnoses here instead. See `docs/DEFER.md`.
    UnresolvedName {
        name: Text,
    },
    /// A struct type name used in value position (`let x = Point;`). A struct is
    /// a type, not a value. Sibling of [`ResolveError::EnumNameAsValue`]; a
    /// struct *literal* (`Point { .. }`) is a separate, valid form.
    StructNameAsValue {
        name: Text,
    },
    /// A function name used as a value (`let x = f;`). Eye has no function
    /// pointers, so a function is callable but not a value; it is valid only as
    /// a call callee.
    FnAsValue {
        name: Text,
    },
}

/// `T`: type-rule violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeError {
    ArrayInitLenMismatch { declared: u64, found: u64 },
    LetTypeMismatch { expected: TypeRef, got: TypeRef },
    MatchArmTypeMismatch { expected: TypeRef, found: TypeRef },
    ReturnTypeMismatch { expected: TypeRef, found: TypeRef },
    UnionLiteralFieldCount { name: Text, found: usize },
    StructLitMissingFields { name: Text, fields: Vec<Text> },
    StructLitUnknownFields { name: Text, fields: Vec<Text> },
    OpOnArray { op: &'static str },
    LenFieldOnArray,
    PrintCannotFormat { kind: &'static str },
    LenArity { found: usize },
    LenNotAPlace,
    LenNotArray,
    MatchScrutineeNotEnum,
}

/// `P`: match-analysis errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    UnreachableAfterWildcard,
    DuplicateArm { variant: Text },
    NonExhaustive { enum_name: Text, missing: Vec<Text> },
}

/// `C`: array-shape / compile-time-constant errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstError {
    ArrayLenNotLiteral,
    ArrayLenZero,
    ArrayLenTooLarge,
    IndexOutOfBounds { index: u128, len: u64 },
    NegativeIndex,
}

/// `U`: deferred features that are rejected for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedError {
    ArrayField,
}

/// The single HIR diagnostic kind. Lowering accumulates a `Sink<HirError>`;
/// the renderer routes on [`Code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirError {
    Resolve(ResolveError),
    Type(TypeError),
    Pattern(PatternError),
    Const(ConstError),
    Unsupported(UnsupportedError),
}

impl From<ResolveError> for HirError {
    fn from(e: ResolveError) -> Self {
        HirError::Resolve(e)
    }
}
impl From<TypeError> for HirError {
    fn from(e: TypeError) -> Self {
        HirError::Type(e)
    }
}
impl From<PatternError> for HirError {
    fn from(e: PatternError) -> Self {
        HirError::Pattern(e)
    }
}
impl From<ConstError> for HirError {
    fn from(e: ConstError) -> Self {
        HirError::Const(e)
    }
}
impl From<UnsupportedError> for HirError {
    fn from(e: UnsupportedError) -> Self {
        HirError::Unsupported(e)
    }
}

/// Join names as a comma-separated list of backtick-quoted items.
fn join_ticked(items: &[Text]) -> String {
    items
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `field` / `fields` depending on count.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::DuplicateItem { name } => write!(f, "duplicate item `{name}`"),
            ResolveError::DuplicateVariantDecl { variant, enum_name } => write!(
                f,
                "variant `{variant}` already declared in enum `{enum_name}`"
            ),
            ResolveError::UnknownEnumInPattern { enum_name } => {
                write!(f, "unknown enum `{enum_name}` in match pattern")
            }
            ResolveError::PatternEnumMismatch {
                pattern_enum,
                scrutinee_enum,
            } => write!(
                f,
                "pattern is from enum `{pattern_enum}`, but scrutinee is `{scrutinee_enum}`"
            ),
            ResolveError::NoSuchVariant { enum_name, variant } => {
                write!(f, "enum `{enum_name}` has no variant `{variant}`")
            }
            ResolveError::UnknownVariantInPattern { variant } => {
                write!(f, "unknown variant `{variant}` in match pattern")
            }
            ResolveError::EnumNameAsValue { name } => {
                write!(f, "`{name}` is an enum type, not a value")
            }
            ResolveError::UnresolvedName { name } => {
                write!(f, "use of undeclared name `{name}`")
            }
            ResolveError::StructNameAsValue { name } => {
                write!(f, "`{name}` is a struct type, not a value")
            }
            ResolveError::FnAsValue { name } => {
                write!(f, "`{name}` is a function, not a value")
            }
        }
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeError::ArrayInitLenMismatch { declared, found } => write!(
                f,
                "array initializer length mismatch: declared length {declared}, initializer has {found} element(s)"
            ),
            TypeError::LetTypeMismatch { expected, got } => {
                write!(
                    f,
                    "let initializer type mismatch: expected {expected}, got {got}"
                )
            }
            TypeError::MatchArmTypeMismatch { expected, found } => write!(
                f,
                "match arm type mismatch: expected {expected}, this arm produces {found}"
            ),
            TypeError::ReturnTypeMismatch { expected, found } => write!(
                f,
                "return type mismatch: function returns {expected}, tail expression produces {found}"
            ),
            TypeError::UnionLiteralFieldCount { name, found } => write!(
                f,
                "union literal `{name}` must set exactly one field, found {found}"
            ),
            TypeError::StructLitMissingFields { name, fields } => write!(
                f,
                "struct literal `{name}` is missing field{}: {}",
                plural(fields.len()),
                join_ticked(fields)
            ),
            TypeError::StructLitUnknownFields { name, fields } => write!(
                f,
                "struct literal `{name}` has unknown field{}: {}",
                plural(fields.len()),
                join_ticked(fields)
            ),
            TypeError::OpOnArray { op } => write!(f, "cannot apply `{op}` to an array"),
            TypeError::LenFieldOnArray => write!(f, "no `.len` field on arrays; use `len(x)`"),
            TypeError::PrintCannotFormat { kind } => write!(f, "`print` cannot format {kind}"),
            TypeError::LenArity { found } => write!(f, "`len` takes one argument, got {found}"),
            TypeError::LenNotAPlace => write!(f, "`len` takes an array variable, not a value"),
            TypeError::LenNotArray => write!(f, "`len` argument is not an array"),
            TypeError::MatchScrutineeNotEnum => {
                write!(f, "match scrutinee type is not a known enum")
            }
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternError::UnreachableAfterWildcard => {
                write!(f, "unreachable match arm after `_` wildcard")
            }
            PatternError::DuplicateArm { variant } => {
                write!(f, "duplicate match arm for variant `{variant}`")
            }
            PatternError::NonExhaustive { enum_name, missing } => write!(
                f,
                "non-exhaustive match on enum `{enum_name}`: missing {}",
                join_ticked(missing)
            ),
        }
    }
}

impl fmt::Display for ConstError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstError::ArrayLenNotLiteral => write!(f, "array length must be an integer literal"),
            ConstError::ArrayLenZero => write!(f, "array length cannot be zero"),
            ConstError::ArrayLenTooLarge => {
                write!(f, "array length integer literal is too large")
            }
            ConstError::IndexOutOfBounds { index, len } => {
                write!(f, "array index {index} is out of bounds for `[_; {len}]`")
            }
            ConstError::NegativeIndex => write!(f, "array index cannot be negative"),
        }
    }
}

impl fmt::Display for UnsupportedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsupportedError::ArrayField => {
                write!(f, "arrays as struct or union fields are not supported yet")
            }
        }
    }
}

impl fmt::Display for HirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HirError::Resolve(e) => e.fmt(f),
            HirError::Type(e) => e.fmt(f),
            HirError::Pattern(e) => e.fmt(f),
            HirError::Const(e) => e.fmt(f),
            HirError::Unsupported(e) => e.fmt(f),
        }
    }
}

impl Diagnostic for HirError {
    fn code(&self) -> Code {
        // FIXME: numbers are positional placeholders; the class letter is the
        // meaningful part until the code registry is finalized.
        match self {
            HirError::Resolve(_) => Code::new(Class::Resolve, 1),
            HirError::Type(_) => Code::new(Class::Type, 1),
            HirError::Pattern(_) => Code::new(Class::Pattern, 1),
            HirError::Const(_) => Code::new(Class::Const, 1),
            HirError::Unsupported(_) => Code::new(Class::Unsupported, 1),
        }
    }
}
