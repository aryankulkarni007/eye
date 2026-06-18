//! typed HIR diagnostics, partitioned into the HIR error classes:
//! [`ResolveError`] (`R`), [`TypeError`] (`T`), [`PatternError`] (`P`),
//! [`ConstError`] (`C`), and [`EffectError`] (`E`). (the `U` "unsupported"
//! class exists in the diagnostics taxonomy but currently has no HIR members.)
//! `EffectError` data lives here (the `HirError` aggregate keeps the crate
//! graph acyclic) but its only producer is `crates/effect` (EFFECT.md). all
//! are carried by
//! [`HirError`], the single accumulator kind, so lowering keeps one
//! [`Sink`](diagnostics::sink) while every entry stays concretely typed for
//! in-crate `matches!` assertions.
//!
//! `Display` is hand-written rather than derived via `thiserror`: several
//! messages need list-joining and pluralization that a static `#[error("...")]`
//! template cannot express. the `Display + Debug` bound is what the
//! [`Diagnostic`](diagnostics::diagnostic) trait requires; how each impl
//! supplies it is a per-crate choice.

use std::fmt;

use diagnostics::{Class, Code, Diagnostic};

use crate::core::Text;

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
    DuplicateParam {
        name: Text,
        function: Text,
    },
    NameIsReserved {
        name: Text,
        what: &'static str,
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
    /// a name in value position resolves to nothing: not a local, function,
    /// type, or variant. the `print`/`len` call intrinsics are excepted (they
    /// are sniffed by their unresolved name). this is the principled
    /// replacement for the old unresolved-name accident, where the HIR-walk
    /// backend emitted any unknown identifier verbatim as c (an undeclared
    /// `printf(...)` the linker resolved, the bare `return` keyword); MIR is a
    /// resolved IR, so it diagnoses here instead. see `docs/planning/DEFER.md`.
    UnresolvedName {
        name: Text,
    },
    /// a struct type name used in value position (`let x = Point;`). a struct is
    /// a type, not a value. sibling of [`ResolveError::EnumNameAsValue`]; a
    /// struct *literal* (`Point { .. }`) is a separate, valid form.
    StructNameAsValue {
        name: Text,
    },
    /// an item, field, or parameter name that is a c keyword (`struct`,
    /// `register`, ...). the c backend emits these names verbatim (fields as
    /// `.name`, parameters and items as bare identifiers), so a keyword would
    /// produce illegal c. rejected at collection rather than mangled: a mangled
    /// name would diverge from the source in the emitted c and any debugger.
    NameIsCKeyword {
        name: Text,
        /// what kind of declaration carried the name (`"field"`, `"parameter"`,
        /// `"function"`, ...), for the message.
        what: &'static str,
    },
    /// a struct literal whose name is not a declared struct or union
    /// (`Foo { x: 1 }` with no `Foo`). without this check the literal emits
    /// `(Foo){ .x = 1 }` and clang reports an undeclared identifier.
    UnknownStructLiteral {
        name: Text,
    },
    /// a type annotation names a type that is not declared: not a primitive,
    /// struct, union, enum, or opaque extern type. without this check the name
    /// is emitted verbatim and clang reports "unknown type name". checked on
    /// every declared type: fields, parameters, return types, globals, consts,
    /// `let` annotations, and casts. `sizeof` arguments are deliberately
    /// exempt - `sizeof(ctype)` leans on the c backend as the layout
    /// authority (docs/features/SIZEOF.md).
    UnknownTypeName {
        name: Text,
    },
    /// a `let`/`mut`/`const` binding whose name is already bound in the same
    /// lexical scope (ruled 2026-06-12). shadowing in a *nested* scope stays
    /// legal; same-scope rebinding is rejected conservatively (it can be
    /// relaxed to a shadowing rule later; the reverse would break programs).
    DuplicateLocal {
        name: Text,
    },
}

/// `T`: type-rule violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeError {
    ArrayInitLenMismatch {
        declared: u64,
        found: u64,
    },
    LetTypeMismatch {
        expected: String,
        got: String,
    },
    MatchArmTypeMismatch {
        expected: String,
        found: String,
    },
    ReturnTypeMismatch {
        expected: String,
        found: String,
    },
    /// `return;` with no value in a function that declares a return type.
    ReturnMissingValue {
        expected: String,
    },
    /// `return expr;` with a value in a function that returns nothing.
    ReturnValueInVoid,
    /// a struct/union that contains itself by value (directly, mutually, or
    /// through an array), making it infinite-size. the cycle must be broken with
    /// a pointer (`Node* next`, not `Node next`).
    RecursiveValueType {
        name: Text,
    },
    /// a call `e(...)` whose callee `e` is a value that is not a function
    /// pointer (e.g. `let int32 x = 5; x(3);`).
    CallNonFunction {
        found: String,
    },
    /// `main` declares parameters. the c entry shim calls it with none, so a
    /// parameterized `main` would emit c that clang rejects. (any *return* type
    /// is allowed - the shim adapts it to the process exit code.)
    MainHasParams,
    UnionLiteralFieldCount {
        name: Text,
        found: usize,
    },
    StructLitMissingFields {
        name: Text,
        fields: Vec<Text>,
    },
    StructLitUnknownFields {
        name: Text,
        fields: Vec<Text>,
    },
    /// a call argument whose type does not match the parameter's declared
    /// type (e.g. a `string` passed where an `int32` is declared, or two
    /// swapped arguments). arity is checked separately (`CallArityMismatch`);
    /// this is the per-argument type judgment. integer widths stay lenient
    /// (the strict-width rule is deferred, M2b); the `&[T; N] -> &T` decay and
    /// any pointer widening into the untyped `ptr` are accepted.
    ArgTypeMismatch {
        /// 1-based argument position, for the message.
        index: usize,
        expected: String,
        found: String,
    },
    /// a struct- or union-literal field initialized with a value of the wrong
    /// type (`P { x: "hello" }` where `x` is `int32`). missing/unknown fields
    /// are caught separately; this is the field *value* judgment.
    StructFieldTypeMismatch {
        field: Text,
        expected: String,
        found: String,
    },
    /// an assignment (`x = y`, `x += y`) used where a value is expected - a
    /// `let` initializer, an argument, a condition, an operand, a
    /// value-producing branch tail. assignment is ruled non-value (no
    /// footgun: `if x = y` reads as a typo for `==`), so it is legal only in
    /// statement position or a discarded (void) tail.
    AssignInValuePosition,
    /// unary `-` applied to an unsigned integer (`-(uint32)`). the C result
    /// silently wraps modulo 2^N; Rust rejects it (no negative unsigned). `~`
    /// (bitwise complement) stays legal - it is well-defined on unsigned and
    /// the standard way to build all-ones masks.
    NegationOnUnsigned {
        ty: Text,
    },
    /// a value-position `if` whose `then` and `else` branches have
    /// incompatible types (`if c { 1 } else { true }`). the `match` analogue
    /// is `MatchArmTypeMismatch`; this runs the same branch-consistency
    /// judgment for `if`.
    IfBranchTypeMismatch {
        expected: String,
        found: String,
    },
    /// an array-literal element whose type does not match the array's element
    /// type (`[1, true, "x"]` against `[int32; 3]`). the length and
    /// literal-typing halves are checked elsewhere; this is the per-element
    /// value judgment (L4).
    ArrayElementTypeMismatch {
        /// 0-based element position, for the message.
        index: usize,
        expected: String,
        found: String,
    },
    /// an `as` cast between types the cast lattice does not permit (an
    /// aggregate - array/struct/union - on either side). scalar<->scalar,
    /// pointer<->pointer, and pointer<->integer casts are allowed; aggregates
    /// have no value-level conversion.
    CastNotAllowed {
        from: String,
        to: String,
    },
    /// M2b: a binary on two distinct concrete integer widths (neither a literal
    /// that would adopt) silently narrows; the user must cast explicitly.
    MixedIntegerWidths {
        left: String,
        right: String,
    },
    OpOnArray {
        op: &'static str,
    },
    ModuloOnFloat,
    LenFieldOnArray,
    PrintCannotFormat {
        kind: &'static str,
    },
    LenArity {
        found: usize,
    },
    LenNotAPlace,
    LenNotArray,
    /// `sizeof` takes exactly one argument (a type), got a different count.
    SizeofArity {
        found: usize,
    },
    /// `sizeof`'s argument is not a named type. at the floor only a bare type
    /// name is accepted (`sizeof(int32)`, `sizeof(Point)`); compound types
    /// (`sizeof(&T)`, `sizeof([T; N])`) and value expressions are rejected.
    SizeofNotAType,
    MatchScrutineeNotEnum,
    /// assignment to a binding declared immutable with `let` (eye is
    /// immutable-by-default; `mut` opts in). covers compound assignment and
    /// writes through a field/index projection rooted in the binding. a write
    /// through a pointer (`*p = ..`) is not tracked - the raw-pointer escape.
    AssignToImmutable {
        name: Text,
    },
    /// a call expression that produces no value (void) where a value is
    /// expected - e.g. `let int32 x = f()` where `f` returns nothing, or
    /// `return f()` in a typed function.
    VoidValueInValuePosition,
    /// a `let` / `mut` binding with no type annotation. type inference is on
    /// hiatus, so an explicit type is required; without it the binding would
    /// reach codegen as an `Error` type.
    MissingTypeAnnotation {
        name: Text,
    },
    /// a call with the wrong number of arguments (CLEAK L3). for a variadic
    /// extern signature `expected` is the minimum (the named parameters); a
    /// non-variadic call must match the count exactly. argument *types* are
    /// not checked here (that is the typeck pass); the count never needs
    /// inference.
    CallArityMismatch {
        name: Text,
        expected: usize,
        found: usize,
        /// `true` when the callee is variadic, so `expected` is a minimum.
        variadic: bool,
    },
    /// indexing a value of type `ptr` (CLEAK L7). `ptr` is the untyped
    /// pointer (c `void*`): it has no element type, so `p[i]` cannot be
    /// sized and clang rejects the subscript.
    IndexOnPtr,
    /// indexing a non-indexable value (`x[i]` where `x` is a scalar, struct,
    /// union, enum, `()` or `!`). only an array or a pointer/reference has
    /// elements; a plain value would emit invalid c.
    IndexOfNonIndexable {
        found: String,
    },
    /// dereferencing a value of type `ptr`. it has no pointee type, so `*p`
    /// has no value type; clang rejects the indirection under `-pedantic`.
    DerefOfPtr,
    /// dereferencing a non-pointer value (`*x` where `x` is not a `&T`/`T*`).
    /// `*` is the deref operator; a plain value has nothing to indirect through,
    /// so it would emit invalid c (`(*x)` on a non-pointer).
    DerefOfNonPointer {
        found: String,
    },
    /// arithmetic or bitwise operation on a value of type `ptr` (CLEAK P1).
    /// `void*` arithmetic is a GNU extension, not standard c, and there is no
    /// element size to scale by. comparisons (`==`, `<`, ...) stay allowed.
    ArithmeticOnPtr {
        op: &'static str,
    },
    /// an integer literal whose value does not fit the integer type the
    /// context gives it (CLEAK M1): the declared type at a `let`, argument,
    /// return, or field, or the `int32` literal default. without this check
    /// the raw decimal is emitted into c and the value silently truncates
    /// (clang only warns).
    IntLiteralOutOfRange {
        /// the literal as written, including a leading `-` when negated.
        value: String,
        ty: Text,
        min: String,
        max: String,
    },
    /// a struct literal field with no field name (`Point { 1, 2 }`).
    /// positional initialization is not supported: lowering carries fields by
    /// name only, so a positional value would be silently dropped (the struct
    /// would be zero-initialized).
    StructLitPositional,
    /// `println` placeholder/argument count mismatch (U5): the `{}` count in a
    /// literal format string must equal the value-argument count. unchecked,
    /// an exhausted placeholder emitted `%d` with no matching argument and
    /// extra arguments were forwarded to printf - varargs UB both ways.
    PrintlnArityMismatch {
        placeholders: usize,
        args: usize,
    },
    /// `println` with no arguments at all: there is no format string, and the
    /// bare `printf()` it emitted is not legal c.
    PrintlnMissingFormat,
    /// a char literal outside ASCII: `char` is one byte, but the literal's
    /// UTF-8 encoding is more than one, and the multibyte c char constant it
    /// emitted has an implementation-defined value.
    CharLiteralNotAscii {
        ch: char,
    },
    /// arithmetic or bitwise operation on an enum value (ruled 2026-06-12:
    /// enums are opaque, not ordinal). comparisons stay allowed; `as` casts
    /// to an integer type stay as the explicit escape.
    ArithmeticOnEnum {
        op: &'static str,
        enum_name: Text,
    },
    /// `&` of a non-place expression (`&(a + b)`) (ruled 2026-06-12). MIR
    /// would spill the value to a temp and silently take the temp's address,
    /// with no visible lifetime. `&` requires a place: a variable, global,
    /// field, index, or dereference.
    RefOfNonPlace,
}

/// `P`: match-analysis errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    UnreachableAfterWildcard,
    DuplicateArm {
        variant: Text,
    },
    NonExhaustive {
        enum_name: Text,
        missing: Vec<Text>,
    },
    /// a primitive-domain match is not total. `bool` is finite-provable, so
    /// `missing` names the uncovered values (`true`/`false`); `int`/`char` are
    /// too large to enumerate, so `missing` is empty and the fix is a `_` arm.
    NonExhaustivePrimitive {
        ty: Text,
        missing: Vec<Text>,
    },
    /// a pattern that cannot belong to the scrutinee's domain - a literal against
    /// an enum, a variant against a primitive, or a `bool` literal against an
    /// integer (and vice versa).
    PatternDomainMismatch {
        scrutinee: Text,
        pattern: Text,
    },
    /// a struct destructure (`let Point { .. } = p`) names a type that is not a
    /// known struct.
    DestructureNotAStruct {
        ty: Text,
    },
    /// a struct destructure binds a field the struct does not have.
    DestructureUnknownField {
        ty: Text,
        field: Text,
    },
    /// a struct destructure binds the same field twice.
    DestructureDuplicateField {
        field: Text,
    },
    /// a struct destructure does not bind every field. destructuring is
    /// exhaustive at the floor (no `..`/ignore yet), so a missing field is an
    /// error.
    DestructureNonExhaustive {
        ty: Text,
        missing: Vec<Text>,
    },
}

/// `C`: array-shape / compile-time-constant errors. not `Copy`: the const-expr
/// variants carry the offending name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstError {
    ArrayLenNotLiteral,
    ArrayLenZero,
    ArrayLenTooLarge,
    IndexOutOfBounds {
        index: u128,
        len: u64,
    },
    NegativeIndex,
    /// a `const` whose initializer references itself, directly or through a
    /// chain of other consts.
    ConstCycle {
        name: Text,
    },
    /// a name in a const-expr that does not resolve to another `const` (a local,
    /// a function, an undeclared name - none are compile-time values).
    ConstUnknownName {
        name: Text,
    },
    /// an operation a const-expr cannot fold: a function call (that is CTFE,
    /// far-future), or any non-constant operand.
    NotAConstExpr,
    /// integer division or modulo by a zero constant.
    ConstDivByZero,
    /// `&const` - taking the address of a value that has none.
    RefOfConst {
        name: Text,
    },
    /// assigning to a `const` - it is a value, not storage.
    AssignToConst {
        name: Text,
    },
    /// a `const` used as an array length whose folded value is not a
    /// non-negative integer.
    ArrayLenNotInteger,
    /// a `const`/global whose folded integer value does not fit its declared
    /// integer type (`const int8 X = 200`). an explicit `as` cast to the type
    /// is the blessed truncation; a bare out-of-range value is rejected (U2).
    ConstValueOutOfRange {
        value: String,
        ty: Text,
        min: String,
        max: String,
    },
    /// the folded value's kind does not match the declared type's kind
    /// (`const bool B = 5`, `const char C = 65`, `const int32 X = true`).
    /// matches the cast lattice: no implicit `int -> bool` / `int -> char`.
    ConstTypeMismatch {
        ty: Text,
        found: Text,
    },
}

/// `E`: machine-EFFECT contract failures (EFFECT.md). the data lives here so the
/// `HirError` aggregate stays the single accumulator kind; `crates/effect` is
/// the only producer (it owns the atom set and runs inference).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectError {
    /// an effect annotation names something outside the atom set (`pure`, `io`,
    /// `ffi`, `state`). effect names are contextual keywords, so this is a
    /// semantic error at validation, not a parse error - better recovery.
    UnknownEffect { name: Text },
    /// a fn's declared effect set does not equal its inferred set (the
    /// exact-match contract). `declared`/`inferred` are rendered set
    /// descriptions (`pure`, or `io | state`); `witness` is an optional
    /// "why" trail to a concrete primitive (EFFECT.md witness edges).
    EffectMismatch {
        function: Text,
        declared: String,
        inferred: String,
        witness: Option<String>,
    },
}

/// the single HIR diagnostic kind. lowering accumulates a `Sink<HirError>`;
/// the renderer routes on [`Code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirError {
    Resolve(ResolveError),
    Type(TypeError),
    Pattern(PatternError),
    Const(ConstError),
    Effect(EffectError),
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
impl From<EffectError> for HirError {
    fn from(e: EffectError) -> Self {
        HirError::Effect(e)
    }
}

/// join names as a comma-separated list of backtick-quoted items.
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
            ResolveError::DuplicateParam { name, function } => write!(
                f,
                "parameter `{name}` declared more than once in `{function}`"
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
            ResolveError::NameIsCKeyword { name, what } => {
                write!(
                    f,
                    "`{name}` cannot be used as a {what} name: it is a C keyword, and the C backend emits the name verbatim"
                )
            }
            ResolveError::NameIsReserved { name, what } => {
                write!(
                    f,
                    "`{name}` cannot be used as a {what} name: it is reserved for the compiler's C output"
                )
            }
            ResolveError::UnknownStructLiteral { name } => {
                write!(f, "`{name}` is not a declared struct or union")
            }
            ResolveError::UnknownTypeName { name } => {
                write!(f, "unknown type name `{name}`")
            }
            ResolveError::DuplicateLocal { name } => write!(
                f,
                "`{name}` is already declared in this scope; shadowing needs a nested block scope"
            ),
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
                "return type mismatch: function returns {expected}, but this produces {found}"
            ),
            TypeError::ReturnMissingValue { expected } => write!(
                f,
                "`return;` has no value but the function returns {expected}"
            ),
            TypeError::ReturnValueInVoid => {
                write!(f, "`return` has a value but the function returns nothing")
            }
            TypeError::RecursiveValueType { name } => write!(
                f,
                "`{name}` contains itself by value (infinite size); break the cycle with a pointer (`{name}*`)"
            ),
            TypeError::CallNonFunction { found } => {
                write!(
                    f,
                    "cannot call a value of type `{found}`; it is not a function"
                )
            }
            TypeError::MainHasParams => write!(f, "`main` cannot take parameters"),
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
            TypeError::ArgTypeMismatch {
                index,
                expected,
                found,
            } => write!(
                f,
                "argument {index} type mismatch: expected {expected}, got {found}"
            ),
            TypeError::StructFieldTypeMismatch {
                field,
                expected,
                found,
            } => write!(
                f,
                "field `{field}` type mismatch: expected {expected}, got {found}"
            ),
            TypeError::AssignInValuePosition => write!(
                f,
                "assignment produces no value and cannot be used where a value is expected"
            ),
            TypeError::NegationOnUnsigned { ty } => {
                write!(f, "cannot apply unary `-` to unsigned type `{ty}`")
            }
            TypeError::IfBranchTypeMismatch { expected, found } => write!(
                f,
                "`if` branches have incompatible types: expected {expected}, got {found}"
            ),
            TypeError::ArrayElementTypeMismatch {
                index,
                expected,
                found,
            } => write!(
                f,
                "array element {index} type mismatch: expected {expected}, got {found}"
            ),
            TypeError::CastNotAllowed { from, to } => {
                write!(f, "cannot cast {from} to {to}")
            }
            TypeError::MixedIntegerWidths { left, right } => write!(
                f,
                "mismatched integer types `{left}` and `{right}`: cast one operand to match"
            ),
            TypeError::OpOnArray { op } => write!(f, "cannot apply `{op}` to an array"),
            TypeError::ModuloOnFloat => {
                write!(
                    f,
                    "cannot apply `%` to a float; `%` is integer-only (use `fmod` for floats)"
                )
            }
            TypeError::LenFieldOnArray => write!(f, "no `.len` field on arrays; use `len(x)`"),
            TypeError::PrintCannotFormat { kind } => write!(f, "`println` cannot format {kind}"),
            TypeError::LenArity { found } => write!(f, "`len` takes one argument, got {found}"),
            TypeError::LenNotAPlace => write!(f, "`len` takes an array variable, not a value"),
            TypeError::LenNotArray => write!(f, "`len` argument is not an array"),
            TypeError::SizeofArity { found } => {
                write!(f, "`sizeof` takes one type argument, got {found}")
            }
            TypeError::SizeofNotAType => {
                write!(
                    f,
                    "`sizeof` takes a named type, not a value or compound type"
                )
            }
            TypeError::MatchScrutineeNotEnum => {
                write!(
                    f,
                    "match scrutinee is not a matchable domain (enum, int, char, or bool)"
                )
            }
            TypeError::AssignToImmutable { name } => write!(
                f,
                "cannot assign to `{name}`, which is immutable; declare it with `mut` to allow mutation"
            ),
            TypeError::VoidValueInValuePosition => {
                write!(
                    f,
                    "expression produces no value (void) where a value is expected"
                )
            }
            TypeError::MissingTypeAnnotation { name } => write!(
                f,
                "binding `{name}` has no type to infer from; its initializer produces no value, so give `{name}` a type annotation, e.g. `let int32 {name} = ...`"
            ),
            TypeError::CallArityMismatch {
                name,
                expected,
                found,
                variadic,
            } => {
                let args = if *expected == 1 {
                    "argument"
                } else {
                    "arguments"
                };
                let were = if *found == 1 { "was" } else { "were" };
                let at_least = if *variadic { "at least " } else { "" };
                write!(
                    f,
                    "`{name}` takes {at_least}{expected} {args}, but {found} {were} given"
                )
            }
            TypeError::IndexOnPtr => write!(
                f,
                "cannot index `ptr`: `ptr` has no element type; cast to a pointer type first"
            ),
            TypeError::IndexOfNonIndexable { found } => write!(
                f,
                "cannot index `{found}`: it has no elements; only an array or a pointer can be indexed"
            ),
            TypeError::DerefOfPtr => write!(
                f,
                "cannot dereference `ptr`: `ptr` has no pointee type; cast to a pointer type first"
            ),
            TypeError::DerefOfNonPointer { found } => write!(
                f,
                "cannot dereference `{found}`: it is not a pointer; `*` indirects through a `&T`/`T*`, use `&` to take a value's address"
            ),
            TypeError::ArithmeticOnPtr { op } => write!(
                f,
                "cannot apply `{op}` to `ptr`: `ptr` is untyped, so there is no element size; cast to a pointer type or an integer first"
            ),
            TypeError::IntLiteralOutOfRange {
                value,
                ty,
                min,
                max,
            } => write!(
                f,
                "integer literal `{value}` does not fit in `{ty}` (range {min}..={max})"
            ),
            TypeError::StructLitPositional => write!(
                f,
                "struct literal fields must be named (`Point {{ x: 1, y: 2 }}`); positional initialization is not supported"
            ),
            TypeError::PrintlnArityMismatch { placeholders, args } => {
                let ph = if *placeholders == 1 {
                    "placeholder"
                } else {
                    "placeholders"
                };
                let arg = if *args == 1 { "argument" } else { "arguments" };
                write!(
                    f,
                    "`println` format string has {placeholders} `{{}}` {ph} but {args} value {arg}"
                )
            }
            TypeError::PrintlnMissingFormat => write!(
                f,
                "`println` requires a format string as its first argument"
            ),
            TypeError::CharLiteralNotAscii { ch } => write!(
                f,
                "char literal `'{ch}'` does not fit in one byte: `char` is a single byte; use a string literal for non-ASCII text"
            ),
            TypeError::ArithmeticOnEnum { op, enum_name } => write!(
                f,
                "cannot apply `{op}` to enum `{enum_name}`: enum values are opaque; cast with `as` to do integer arithmetic"
            ),
            TypeError::RefOfNonPlace => write!(
                f,
                "cannot take the address of this expression: `&` requires a place (a variable, field, index, or dereference)"
            ),
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternError::UnreachableAfterWildcard => {
                write!(
                    f,
                    "unreachable match arm: an earlier arm (`_` or a bare-ident binding) already matches every value"
                )
            }
            PatternError::DuplicateArm { variant } => {
                write!(f, "duplicate match arm for variant `{variant}`")
            }
            PatternError::NonExhaustive { enum_name, missing } => write!(
                f,
                "non-exhaustive match on enum `{enum_name}`: missing {}",
                join_ticked(missing)
            ),
            PatternError::NonExhaustivePrimitive { ty, missing } => {
                if missing.is_empty() {
                    write!(
                        f,
                        "non-exhaustive match on `{ty}`: add a `_` arm (the domain is too large to enumerate)"
                    )
                } else {
                    write!(
                        f,
                        "non-exhaustive match on `{ty}`: missing {}",
                        join_ticked(missing)
                    )
                }
            }
            PatternError::PatternDomainMismatch { scrutinee, pattern } => write!(
                f,
                "pattern `{pattern}` does not match scrutinee type `{scrutinee}`"
            ),
            PatternError::DestructureNotAStruct { ty } => {
                write!(f, "cannot destructure `{ty}`: it is not a known struct")
            }
            PatternError::DestructureUnknownField { ty, field } => {
                write!(f, "struct `{ty}` has no field `{field}`")
            }
            PatternError::DestructureDuplicateField { field } => {
                write!(
                    f,
                    "field `{field}` is bound more than once in this destructure"
                )
            }
            PatternError::DestructureNonExhaustive { ty, missing } => write!(
                f,
                "destructure of `{ty}` is missing field{}: {} (destructuring binds every field)",
                plural(missing.len()),
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
            ConstError::ConstCycle { name } => {
                write!(f, "constant `{name}` references itself")
            }
            ConstError::ConstUnknownName { name } => {
                write!(
                    f,
                    "`{name}` is not a constant; only constants may appear in a const expression"
                )
            }
            ConstError::NotAConstExpr => write!(
                f,
                "not a constant expression (function calls and non-constant operands are not allowed)"
            ),
            ConstError::ConstDivByZero => write!(f, "division by zero in a constant expression"),
            ConstError::RefOfConst { name } => write!(
                f,
                "cannot take the address of constant `{name}`; a constant is a value, not a location"
            ),
            ConstError::AssignToConst { name } => {
                write!(f, "cannot assign to constant `{name}`")
            }
            ConstError::ArrayLenNotInteger => {
                write!(f, "array length constant must be a non-negative integer")
            }
            ConstError::ConstValueOutOfRange {
                value,
                ty,
                min,
                max,
            } => write!(
                f,
                "constant value {value} is out of range for type `{ty}` ({min}..{max})"
            ),
            ConstError::ConstTypeMismatch { ty, found } => write!(
                f,
                "constant of type `{ty}` cannot be initialized with {found} value"
            ),
        }
    }
}

impl fmt::Display for EffectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectError::UnknownEffect { name } => write!(
                f,
                "unknown effect `{name}`; the effects are `pure`, `io`, `ffi`, `state`"
            ),
            EffectError::EffectMismatch {
                function,
                declared,
                inferred,
                ..
            } => write!(
                f,
                "`{function}` declares `{declared}` but its effect is `{inferred}`"
            ),
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
            HirError::Effect(e) => e.fmt(f),
        }
    }
}

impl Diagnostic for HirError {
    fn code(&self) -> Code {
        match self {
            HirError::Resolve(e) => match e {
                ResolveError::DuplicateItem { .. } => Code::new(Class::Resolve, 1),
                ResolveError::DuplicateVariantDecl { .. } => Code::new(Class::Resolve, 2),
                ResolveError::UnknownEnumInPattern { .. } => Code::new(Class::Resolve, 3),
                ResolveError::PatternEnumMismatch { .. } => Code::new(Class::Resolve, 4),
                ResolveError::NoSuchVariant { .. } => Code::new(Class::Resolve, 5),
                ResolveError::UnknownVariantInPattern { .. } => Code::new(Class::Resolve, 6),
                ResolveError::EnumNameAsValue { .. } => Code::new(Class::Resolve, 7),
                ResolveError::UnresolvedName { .. } => Code::new(Class::Resolve, 8),
                ResolveError::StructNameAsValue { .. } => Code::new(Class::Resolve, 9),
                ResolveError::NameIsCKeyword { .. } => Code::new(Class::Resolve, 10),
                ResolveError::UnknownStructLiteral { .. } => Code::new(Class::Resolve, 11),
                ResolveError::UnknownTypeName { .. } => Code::new(Class::Resolve, 12),
                ResolveError::DuplicateParam { .. } => Code::new(Class::Resolve, 13),
                ResolveError::NameIsReserved { .. } => Code::new(Class::Resolve, 14),
                ResolveError::DuplicateLocal { .. } => Code::new(Class::Resolve, 15),
            },
            HirError::Type(e) => match e {
                TypeError::ArrayInitLenMismatch { .. } => Code::new(Class::Type, 1),
                TypeError::LetTypeMismatch { .. } => Code::new(Class::Type, 2),
                TypeError::MatchArmTypeMismatch { .. } => Code::new(Class::Type, 3),
                TypeError::ReturnTypeMismatch { .. } => Code::new(Class::Type, 4),
                TypeError::ReturnMissingValue { .. } => Code::new(Class::Type, 5),
                TypeError::ReturnValueInVoid => Code::new(Class::Type, 6),
                TypeError::RecursiveValueType { .. } => Code::new(Class::Type, 7),
                TypeError::CallNonFunction { .. } => Code::new(Class::Type, 8),
                TypeError::MainHasParams => Code::new(Class::Type, 9),
                TypeError::UnionLiteralFieldCount { .. } => Code::new(Class::Type, 10),
                TypeError::StructLitMissingFields { .. } => Code::new(Class::Type, 11),
                TypeError::StructLitUnknownFields { .. } => Code::new(Class::Type, 12),
                TypeError::OpOnArray { .. } => Code::new(Class::Type, 13),
                TypeError::ModuloOnFloat => Code::new(Class::Type, 14),
                TypeError::LenFieldOnArray => Code::new(Class::Type, 15),
                TypeError::PrintCannotFormat { .. } => Code::new(Class::Type, 16),
                TypeError::LenArity { .. } => Code::new(Class::Type, 17),
                TypeError::LenNotAPlace => Code::new(Class::Type, 18),
                TypeError::LenNotArray => Code::new(Class::Type, 19),
                TypeError::SizeofArity { .. } => Code::new(Class::Type, 20),
                TypeError::SizeofNotAType => Code::new(Class::Type, 21),
                TypeError::MatchScrutineeNotEnum => Code::new(Class::Type, 22),
                TypeError::AssignToImmutable { .. } => Code::new(Class::Type, 23),
                TypeError::VoidValueInValuePosition => Code::new(Class::Type, 24),
                TypeError::MissingTypeAnnotation { .. } => Code::new(Class::Type, 25),
                TypeError::CallArityMismatch { .. } => Code::new(Class::Type, 26),
                TypeError::IndexOnPtr => Code::new(Class::Type, 27),
                TypeError::IndexOfNonIndexable { .. } => Code::new(Class::Type, 46),
                TypeError::DerefOfPtr => Code::new(Class::Type, 28),
                TypeError::DerefOfNonPointer { .. } => Code::new(Class::Type, 45),
                TypeError::ArithmeticOnPtr { .. } => Code::new(Class::Type, 29),
                TypeError::IntLiteralOutOfRange { .. } => Code::new(Class::Type, 30),
                TypeError::StructLitPositional => Code::new(Class::Type, 31),
                TypeError::PrintlnArityMismatch { .. } => Code::new(Class::Type, 32),
                TypeError::PrintlnMissingFormat => Code::new(Class::Type, 33),
                TypeError::CharLiteralNotAscii { .. } => Code::new(Class::Type, 34),
                TypeError::ArithmeticOnEnum { .. } => Code::new(Class::Type, 35),
                TypeError::RefOfNonPlace => Code::new(Class::Type, 36),
                TypeError::ArgTypeMismatch { .. } => Code::new(Class::Type, 37),
                TypeError::StructFieldTypeMismatch { .. } => Code::new(Class::Type, 38),
                TypeError::AssignInValuePosition => Code::new(Class::Type, 39),
                TypeError::NegationOnUnsigned { .. } => Code::new(Class::Type, 40),
                TypeError::IfBranchTypeMismatch { .. } => Code::new(Class::Type, 41),
                TypeError::ArrayElementTypeMismatch { .. } => Code::new(Class::Type, 42),
                TypeError::CastNotAllowed { .. } => Code::new(Class::Type, 43),
                TypeError::MixedIntegerWidths { .. } => Code::new(Class::Type, 44),
            },
            HirError::Pattern(e) => match e {
                PatternError::UnreachableAfterWildcard => Code::new(Class::Pattern, 1),
                PatternError::DuplicateArm { .. } => Code::new(Class::Pattern, 2),
                PatternError::NonExhaustive { .. } => Code::new(Class::Pattern, 3),
                PatternError::NonExhaustivePrimitive { .. } => Code::new(Class::Pattern, 4),
                PatternError::PatternDomainMismatch { .. } => Code::new(Class::Pattern, 5),
                PatternError::DestructureNotAStruct { .. } => Code::new(Class::Pattern, 6),
                PatternError::DestructureUnknownField { .. } => Code::new(Class::Pattern, 7),
                PatternError::DestructureDuplicateField { .. } => Code::new(Class::Pattern, 8),
                PatternError::DestructureNonExhaustive { .. } => Code::new(Class::Pattern, 9),
            },
            HirError::Const(e) => match e {
                ConstError::ArrayLenNotLiteral => Code::new(Class::Const, 1),
                ConstError::ArrayLenZero => Code::new(Class::Const, 2),
                ConstError::ArrayLenTooLarge => Code::new(Class::Const, 3),
                ConstError::IndexOutOfBounds { .. } => Code::new(Class::Const, 4),
                ConstError::NegativeIndex => Code::new(Class::Const, 5),
                ConstError::ConstCycle { .. } => Code::new(Class::Const, 6),
                ConstError::ConstUnknownName { .. } => Code::new(Class::Const, 7),
                ConstError::NotAConstExpr => Code::new(Class::Const, 8),
                ConstError::ConstDivByZero => Code::new(Class::Const, 9),
                ConstError::RefOfConst { .. } => Code::new(Class::Const, 10),
                ConstError::AssignToConst { .. } => Code::new(Class::Const, 11),
                ConstError::ArrayLenNotInteger => Code::new(Class::Const, 12),
                ConstError::ConstValueOutOfRange { .. } => Code::new(Class::Const, 13),
                ConstError::ConstTypeMismatch { .. } => Code::new(Class::Const, 14),
            },
            HirError::Effect(e) => match e {
                EffectError::UnknownEffect { .. } => Code::new(Class::Effect, 1),
                EffectError::EffectMismatch { .. } => Code::new(Class::Effect, 2),
            },
        }
    }

    fn notes(&self) -> Vec<std::borrow::Cow<'static, str>> {
        match self {
            // the witness trail to the concrete primitive that produced the
            // unexpected EFFECT (EFFECT.md witness edges).
            HirError::Effect(EffectError::EffectMismatch {
                witness: Some(w), ..
            }) => vec![std::borrow::Cow::Owned(w.clone())],
            _ => Vec::new(),
        }
    }
}
