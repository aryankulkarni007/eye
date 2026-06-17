//! the free type predicates and the `as` cast lattice (CAST.md) shared across
//! the walker, the funnel, and the judgments. no `InferCtx` state - pure
//! functions over `TypeRef` and the interner.

use ast::BinOp;
use hir::core::{HIR, Literal, Text, TypeInterner, TypeKind, TypeRef, VisitTypeRef};

/// the source spelling of a binary operator, for diagnostics.
pub(crate) fn bin_op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        Eq => "==",
        Neq => "!=",
        Lt => "<",
        Gt => ">",
        Leq => "<=",
        Geq => ">=",
        And => "&&",
        Or => "||",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

pub(crate) fn is_comparison(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq
            | BinOp::Neq
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::Leq
            | BinOp::Geq
            | BinOp::And
            | BinOp::Or
    )
}

pub(crate) fn is_int_type_name(n: &str) -> bool {
    int_type_range(n).is_some()
}

/// whether `ty` is an array, peeling one ref/ptr (so `&[T; N]` counts) - the
/// `len` / `.len` array-ness test, ported from lowering's `peeled_array_len`
/// (S2C C5).
pub(crate) fn peeled_array(ty: TypeRef, types: &TypeInterner) -> bool {
    match types.lookup(ty) {
        TypeKind::Array { .. } => true,
        &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => {
            matches!(types.lookup(inner), TypeKind::Array { .. })
        }
        _ => false,
    }
}
/// display text for a literal pattern, used in a domain-mismatch diagnostic.
/// moved with the match judgments from lowering (S2C C2).
pub(crate) fn literal_pat_text(lit: &Literal) -> Text {
    match lit {
        Literal::Int(v) => Text::from(v.to_string()),
        Literal::Char(c) => Text::from(format!("'{c}'")),
        Literal::Bool(b) => Text::from(if *b { "true" } else { "false" }),
        // float / string never reach a pattern (the parser excludes them).
        Literal::Float(s) | Literal::String(s) => Text::from(s.as_str()),
    }
}

/// the value range of a primitive integer type, as `(negative magnitude
/// bound, positive bound)`: a literal `n` must satisfy `n <= max`, a negated
/// literal `-N` must satisfy `N <= neg`. `usize`/`isize` use 64-bit ranges
/// (LP64 targets; a 32-bit target needs a target description). `None` for
/// any non-integer name. moved from lowering's coerce module (S2 step b).
pub(crate) fn int_type_range(name: &str) -> Option<(u128, u128)> {
    Some(match name {
        "int8" => (1 << 7, (1 << 7) - 1),
        "int16" => (1 << 15, (1 << 15) - 1),
        "int32" => (1 << 31, (1 << 31) - 1),
        "int64" | "isize" => (1 << 63, (1 << 63) - 1),
        "uint8" => (0, (1 << 8) - 1),
        "uint16" => (0, (1 << 16) - 1),
        "uint32" => (0, (1 << 32) - 1),
        "uint64" | "usize" => (0, u64::MAX as u128),
        _ => return None,
    })
}

/// a type tree containing an `Error` anywhere (poison absorber for
/// `types_compatible`).
pub(crate) struct ContainsError(bool);

impl VisitTypeRef for ContainsError {
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
        let is_error = matches!(types.lookup(ty), TypeKind::Error);
        if is_error {
            self.0 = true;
        }
        !is_error
    }
}

pub(crate) fn type_ref_contains_error(ty: TypeRef, types: &TypeInterner) -> bool {
    let mut v = ContainsError(false);
    types.walk(ty, &mut v);
    v.0
}

/// compatibility test for value-position match-arm types. compatible when
/// either side carries an `Error` (no follow-on cascade), when either side is
/// `Never` (the bottom type coerces to anything), or by exact `TypeRef`
/// equality. the old integer-family leniency (any int ~ any int) is gone (M2b):
/// a literal adopts the expected width at the coercion site before this runs,
/// so a surviving mismatch is two distinct concrete widths.
pub(crate) fn types_compatible(a: TypeRef, b: TypeRef, types: &TypeInterner) -> bool {
    if type_ref_contains_error(a, types) || type_ref_contains_error(b, types) {
        return true;
    }
    // `!` (never) is the bottom type: a diverging value is acceptable wherever
    // any type is expected, so a `Never` branch never forces a mismatch
    // (`if c { 5 } else { return }` returns the `5`'s type, `loop {}` satisfies
    // any return type).
    if matches!(types.lookup(a), TypeKind::Never) || matches!(types.lookup(b), TypeKind::Never) {
        return true;
    }
    // exact-width equality: the integer-family leniency (any int ~ any int) is
    // gone (M2b at the boundaries). a literal already adopts the expected width
    // at the coercion site before this runs, so a surviving mismatch is two
    // distinct concrete widths - the same silent-narrowing footgun M2b rejects
    // for operands, here for arguments / fields / returns.
    a == b
}

/// whether a `&[T; N]` (`found`) decays to `declared`: `declared` is `&T` / `T*`
/// with the same element type, or `string` (the byte-pointer view of a
/// `&[uint8; N]`). mirrors `coerce.rs::array_ref_decays_to`; the coercion sites
/// accept this pairing without a mismatch, and `record_decay` files the cast
/// MIR applies. directional (found -> declared) so it never relaxes a symmetric
/// equality and mask a real mismatch.
pub(crate) fn array_ref_decays_to(declared: TypeRef, found: TypeRef, types: &TypeInterner) -> bool {
    let &TypeKind::Ref(arr) = types.lookup(found) else {
        return false;
    };
    let &TypeKind::Array { elem, .. } = types.lookup(arr) else {
        return false;
    };
    match types.lookup(declared) {
        TypeKind::Path(n) if n == "string" => {
            matches!(types.lookup(elem), TypeKind::Path(e) if e == "uint8")
        }
        // `&T`/`T*` accepts `&[T; N]` on an exact element match, plus the
        // `char`<->`uint8` byte pun: a string literal is `&[uint8; N]`, so this
        // lets it decay into a `char*` slot (`let [char*; N] = ["a", ...]`, FFI
        // string args). both are one-byte types; the decay emits an explicit
        // `(char*)` cast, which is well-defined and silences `-Wpointer-sign`.
        TypeKind::Ref(t) | TypeKind::Ptr(t) => *t == elem || byte_pun(*t, elem, types),
        _ => false,
    }
}

/// whether two scalar type handles are the interchangeable one-byte pair
/// `char` and `uint8` (a string literal's element type vs a C `char*`).
pub(crate) fn byte_pun(a: TypeRef, b: TypeRef, types: &TypeInterner) -> bool {
    let name = |t: TypeRef| match types.lookup(t) {
        TypeKind::Path(n) => Some(n.clone()),
        _ => None,
    };
    matches!(
        (name(a).as_deref(), name(b).as_deref()),
        (Some("char"), Some("uint8")) | (Some("uint8"), Some("char"))
    )
}

/// whether a value of type `found` is acceptable at a site expecting
/// `expected` - a call argument or a struct/union-literal field - after the
/// coercion-site adjustments (`site_coerce`) have run. accepts an
/// equal/integer-family-compatible type, the `&[T; N] -> &T` / `string` decay
/// (`record_decay` files the cast MIR applies), and any pointer-shaped value
/// widening into the untyped `ptr` (`void*` absorbs any pointer - the FFI
/// escape). `Error` on either side is silent via `types_compatible`. the
/// integer-family leniency defers the strict-width rule (M2b) until a corpus
/// program needs it, matching every other coercion site.
pub(crate) fn site_assignable(expected: TypeRef, found: TypeRef, types: &TypeInterner) -> bool {
    types_compatible(found, expected, types)
        || array_ref_decays_to(expected, found, types)
        || (matches!(types.lookup(expected), TypeKind::RawPtr) && is_pointer_shaped(found, types))
}

/// whether `ty` is a pointer-shaped value (a typed reference/pointer, or the
/// untyped `ptr`): the values that widen into `ptr` without an explicit cast.
pub(crate) fn is_pointer_shaped(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::RawPtr
    )
}

/// the name of an unsigned integer type, or `None` for anything else - the F2
/// test for `-` rejection.
pub(crate) fn unsigned_int_name(ty: TypeRef, types: &TypeInterner) -> Option<Text> {
    match types.lookup(ty) {
        TypeKind::Path(name)
            if matches!(
                name.as_str(),
                "uint8" | "uint16" | "uint32" | "uint64" | "usize"
            ) =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

/// whether `name` is a float type (the F3 adoption test).
pub(crate) fn is_float_type_name(n: &str) -> bool {
    matches!(n, "float32" | "float64")
}

/// the cast-lattice class of a type (S3, CAST.md). the ratified `as` ruling is
/// directional - `char`/`bool`/`enum` widen OUT to an integer but cannot be
/// fabricated IN - so each scalar keeps its own class rather than collapsing to
/// one "scalar". `Unknown` (an `Error` or an unresolved type name - a type
/// parameter the floor cannot resolve) stays lenient.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CastClass {
    Int,
    Float,
    Bool,
    Char,
    Enum,
    Pointer,
    Aggregate,
    Fn,
    Unknown,
}

pub(crate) fn cast_class(ty: TypeRef, scope: &HIR, types: &TypeInterner) -> CastClass {
    match types.lookup(ty) {
        TypeKind::Error => CastClass::Unknown,
        // `()` / `!` have no value-level representation to cast through; the
        // aggregate class rejects every cast pair involving them.
        TypeKind::Array { .. } | TypeKind::Unit | TypeKind::Never => CastClass::Aggregate,
        TypeKind::Fn { .. } => CastClass::Fn,
        TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::RawPtr => CastClass::Pointer,
        TypeKind::Path(name) => {
            if is_int_type_name(name) {
                CastClass::Int
            } else if is_float_type_name(name) {
                CastClass::Float
            } else if name == "bool" {
                CastClass::Bool
            } else if name == "char" {
                CastClass::Char
            } else if scope.items.enums.contains_key(name) {
                CastClass::Enum
            } else if scope.items.structs.contains_key(name)
                || scope.items.unions.contains_key(name)
            {
                CastClass::Aggregate
            } else {
                CastClass::Unknown
            }
        }
    }
}

/// whether an `as` cast from `from` to `to` is in the cast lattice (CAST.md).
/// the allowed directed pairs are listed explicitly; everything else rejects.
/// an `Unknown` side is lenient (no cascade).
pub(crate) fn cast_allowed(from: TypeRef, to: TypeRef, scope: &HIR, types: &TypeInterner) -> bool {
    use CastClass::*;
    match (cast_class(from, scope, types), cast_class(to, scope, types)) {
        (Unknown, _) | (_, Unknown) => true,
        // numeric <-> numeric.
        (Int, Int) | (Int, Float) | (Float, Int) | (Float, Float) => true,
        // pointer puns and the integer<->pointer bridge.
        (Pointer, Pointer) | (Int, Pointer) | (Pointer, Int) => true,
        // the tagged scalars widen OUT to an integer, never the reverse.
        (Char, Int) | (Bool, Int) | (Enum, Int) => true,
        // everything else - `_ -> bool`/`_ -> char`, `int -> enum`,
        // float<->pointer, any aggregate/fn - is rejected.
        _ => false,
    }
}

pub(crate) fn is_integer_path(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Path(name)
            if matches!(
                name.as_str(),
                "int8" | "int16" | "int32" | "int64"
                    | "uint8" | "uint16" | "uint32" | "uint64"
                    | "usize" | "isize"
            )
    )
}
