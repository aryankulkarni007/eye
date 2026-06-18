use super::*;

/// `[T; N]` requires `N` to be a compile-time value: an integer literal or a
/// `const` (a const-expr over those). a runtime local is not a constant, so it
/// is rejected (and must not lower silently as length 0).
#[test]
fn non_integer_literal_array_len_emits_diagnostic() {
    let hir = lower(
        "\
main() {
    let int32 n = 3;
    let [int32; n] xs = [1, 2, 3];
}
",
    );

    assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
    assert!(
        matches!(
            diags(&hir)[0],
            HirError::Const(ConstError::ConstUnknownName { .. })
        ),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
}

/// the const-expr evaluator folds literals, the operator set, and references to
/// other consts into a scalar [`ConstValue`] on each [`Const`].
#[test]
fn consts_fold_to_values() {
    let hir = lower(
        "\
const int32 MAX = 100;
const int32 DBL = MAX * 2;
const int32 NEG = 0 - 5;
const bool BIG = MAX > 50;
const char MARK = 'A';
const float64 HALF = 3.0 / 2.0;
const int32 TRUNC = HALF as int32;
main() {}
",
    );
    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
    let val = |name: &str| hir.consts[hir.items.consts[name]].value.clone();
    assert_eq!(val("MAX"), Some(ConstValue::Int(100)));
    assert_eq!(val("DBL"), Some(ConstValue::Int(200)));
    assert_eq!(val("NEG"), Some(ConstValue::Int(-5)));
    assert_eq!(val("BIG"), Some(ConstValue::Bool(true)));
    assert_eq!(val("MARK"), Some(ConstValue::Char('A')));
    assert_eq!(val("HALF"), Some(ConstValue::Float(1.5)));
    // a numeric `as` cast folds: float 1.5 truncates to int 1.
    assert_eq!(val("TRUNC"), Some(ConstValue::Int(1)));
}

/// a `const` whose initializer references itself (directly or through a chain)
/// is a definition cycle, diagnosed once and left poisoned (`value == None`).
#[test]
fn const_cycle_is_rejected() {
    let hir = lower(
        "\
const int32 A = B;
const int32 B = A;
main() {}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Const(ConstError::ConstCycle { .. }))),
        "expected a const cycle diagnostic: {:?}",
        diags(&hir)
    );
    assert_eq!(hir.consts[hir.items.consts["A"]].value, None);
}

/// the const value-vs-type *kind* check (the const analogue of the cast
/// lattice): a wrong-kind initializer is rejected. no implicit `int -> bool`
/// or `int -> char`, so `const bool B = 5` and `const char C = 65` are errors,
/// as is `const int32 X = true`.
#[test]
fn const_value_kind_mismatch_is_rejected() {
    let hir = lower(
        "\
const bool B = 5;
const char C = 65;
const int32 X = true;
main() {}
",
    );
    let n = diags(&hir)
        .iter()
        .filter(|d| matches!(d, HirError::Const(ConstError::ConstTypeMismatch { .. })))
        .count();
    assert_eq!(
        n,
        3,
        "all three kind mismatches must be rejected: {:?}",
        diags(&hir)
    );
}

/// matching kinds, an `int` literal widening into a `float` const, and the
/// `ptr <- int` address idiom all stay legal (the corpus exercises each).
#[test]
fn const_matching_and_widening_kinds_accepted() {
    let hir = lower(
        "\
const bool B = true;
const char C = 'A';
const float64 F = 3.0;
const float64 R = 100;
const ptr P = 0 as ptr;
const int32 I = 7;
main() {}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Const(ConstError::ConstTypeMismatch { .. }))),
        "matching kinds + int->float widening + ptr<-int must be accepted: {:?}",
        diags(&hir)
    );
}

/// a top-level `const` integer resolves as an array length (A6,
/// `docs/design/HORIZON0.md`): `const usize N = 4; [int32; N]` lowers to a length-4
/// array, and a const-expr (`N * 2`) folds too.
#[test]
fn const_array_length_resolves() {
    let hir = lower(
        "\
const usize N = 4;
main() {
    let [int32; N] xs = [1, 2, 3, 4];
    let [int32; N * 2] ys = [0, 0, 0, 0, 0, 0, 0, 0];
}
",
    );

    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    let types = &hir.types;
    let lens: Vec<u64> = body
        .stmts
        .iter()
        .filter_map(|(_, s)| match s {
            Stmt::Let { ty: Some(ty), .. } => ty.as_array(types).map(|(_, len)| len),
            _ => None,
        })
        .collect();
    assert_eq!(lens, vec![4, 8]);
}

/// the repeat literal `[value; N]` resolves its count via the same const
/// machinery as a `[T; N]` type length: a literal or a `const`.
#[test]
fn array_repeat_resolves_const_count() {
    let hir = lower(
        "\
const usize N = 4;
main() {
    let [int32; N] xs = [7; N];
    println(\"{}\", xs[0]);
}
",
    );
    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
}

/// a repeat literal with a non-const count is a `Const` error - a runtime local
/// cannot be a compile-time array length.
#[test]
fn array_repeat_non_const_count_rejected() {
    let hir = lower(
        "\
main() {
    let int32 n = 3;
    let [int32; 3] xs = [0; n];
    println(\"{}\", xs[0]);
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(e, HirError::Const(_))),
        "expected a Const diagnostic for a non-const repeat count, got: {:?}",
        hir.diagnostics
    );
}

/// an untyped `let` is rejected. type inference is on hiatus, so a binding needs
/// an explicit type; without one it would reach codegen as an `Error` placeholder
/// (`void* /* ERROR TY */`).
#[test]
fn untyped_let_requires_annotation() {
    let hir = lower(
        "\
main() {
    let x = 5;
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MissingTypeAnnotation { .. }))),
        "expected MissingTypeAnnotation, got: {:?}",
        hir.diagnostics
    );
}

/// a zero-length array `[T; 0]` lowers to a nonstandard c zero-length array, so
/// it is rejected.
#[test]
fn zero_length_array_is_rejected() {
    let hir = lower("main() {\n    let [int32; 0] a = [];\n    println(\"{}\", 0);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::ArrayLenZero))),
        "zero-length array must be rejected; got: {:?}",
        hir.diagnostics
    );
}
