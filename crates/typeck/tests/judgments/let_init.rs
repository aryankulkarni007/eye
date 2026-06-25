use super::*;

// ---- let-initializer judgments: explicit type, array length, void value
// (moved from hir tests with the S2 step-b let-check migration) ----

/// an explicitly typed `let` whose call initializer has the wrong result type
/// is diagnosed.
#[test]
fn explicit_let_initializer_type_mismatch_is_diagnosed() {
    let hir = lower(
        "\
answer() -> int32 {
    42
}

main() {
    let string x = answer();
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::LetTypeMismatch { expected, got })
                if *expected == "string" && *got == "int32"
        )),
        "expected explicit let mismatch diagnostic, got: {:?}",
        diags(&hir)
    );
}

/// an initializer whose type is unknown (an unresolved call) must not cascade
/// into a spurious let-type mismatch.
#[test]
fn explicit_let_unknown_initializer_type_does_not_diagnose_mismatch() {
    let hir = lower(
        "\
main() {
    let int32 x = unknown();
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::LetTypeMismatch { .. }))),
        "unknown initializer type should not cascade into mismatch: {:?}",
        diags(&hir)
    );
}

/// an `if` used as a value must yield a value on every path. an else-less `if`
/// as a `let` initializer is rejected (it leaves the binding uninitialized when
/// the condition is false), while a diverging branch (`{ return; }`) is allowed
/// because it never falls through.
#[test]
fn else_less_if_in_value_position_rejected() {
    let reject = lower(
        "\
main() {
    let bool c = false;
    let int32 x = if c { 5 };
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&reject)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::VoidValueInValuePosition))),
        "expected VoidValueInValuePosition, got: {:?}",
        diags(&reject)
    );

    // a diverging then-branch is fine: the `else` supplies the value.
    let ok = lower(
        "\
pick(int32 c) -> int32 {
    let int32 x = if c < 0 { return 99; } else { 2 };
    x
}
main() { println(\"{}\", pick(5)); }
",
    );
    assert!(
        diags(&ok).is_empty(),
        "diverging then-branch must be clean, got: {:?}",
        diags(&ok)
    );
}

/// a typed array binding must initialize exactly the declared number of
/// elements. c accepts short initializers and zero-fills the rest; eye reports
/// the mismatch explicitly.
#[test]
fn array_decl_initializer_len_mismatch_emits_diagnostic() {
    let hir = lower(
        "\
main() {
    let [int32; 3] xs = [1, 2];
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArrayInitLenMismatch { .. }))),
        "expected ArrayInitLenMismatch, got: {:?}",
        diags(&hir)
    );
}

/// S3 struct-field value judgment: a field initialized with the wrong type is
/// rejected (`P { x: "hi" }` with `int32 x` reached clang before; only
/// missing/unknown fields were caught).
#[test]
fn struct_field_value_type_mismatch_is_rejected() {
    let hir = lower(
        "\
structure Point { int32 x, int32 y, }

main() {
    let Point p = Point { x: \"hi\", y: 2 };
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::StructFieldTypeMismatch { field, expected, .. })
                if field.as_str() == "x" && expected == "int32"
        )),
        "expected StructFieldTypeMismatch on `x`, got: {:?}",
        diags(&hir)
    );
}

/// L4 (S3): an array-literal element whose type does not match the declared
/// element type is rejected per element; a uniform literal is clean.
#[test]
fn array_element_type_mismatch_is_rejected() {
    let hir = lower(
        "\
main() {
    let [int32; 2] bad = [1, true];
    println(\"{}\", bad[0]);
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArrayElementTypeMismatch { index, expected, .. })
                if *index == 1 && expected == "int32"
        )),
        "expected ArrayElementTypeMismatch on element 1, got: {:?}",
        diags(&hir)
    );

    let clean = lower(
        "\
main() {
    let [int32; 3] ok = [1, 2, 3];
    println(\"{}\", ok[0]);
}
",
    );
    assert!(
        !diags(&clean).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArrayElementTypeMismatch { .. })
        )),
        "uniform int array must be clean, got: {:?}",
        diags(&clean)
    );
}

/// let-from-init: an untyped `let x = <init>` infers x's type from the
/// initializer's synthesized type, so it is no longer rejected (the T025 that
/// lowering used to emit is gone). covers a literal, an index, a call, and a
/// struct literal - each synthesizes a concrete type.
#[test]
fn untyped_let_infers_from_init() {
    let hir = lower(
        "\
structure Point { int32 x, int32 y, };
mk() -> int32 { 7 }
main() {
    let a = 5;
    let [int32; 3] xs = [1, 2, 3];
    let b = xs[0];
    let c = mk();
    let p = Point { x: 1, y: 2 };
    println(\"{} {} {} {}\", a, b, c, p.x);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MissingTypeAnnotation { .. }))),
        "untyped lets with concrete initializers must infer, not reject: {:?}",
        diags(&hir)
    );
}

/// the residual T025: an untyped `let x = <init>` whose initializer produces no
/// value (a `()`-returning call) has nothing to infer, so it still needs an
/// annotation.
#[test]
fn untyped_let_value_less_init_rejected() {
    let hir = lower(
        "\
noop() {}
main() {
    let x = noop();
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MissingTypeAnnotation { name }) if name == "x")),
        "a value-less initializer must still require an annotation: {:?}",
        diags(&hir)
    );
}

/// an untyped array literal has a single element type: a heterogeneous one is
/// rejected, not silently typed by its first element. footgun surfaced by
/// let-from-init (`let xs = [1, "two"]` would otherwise infer `[int32; 2]`). a
/// declared element type owns this check at the funnel; this covers the
/// no-expectation path.
#[test]
fn untyped_array_literal_must_be_homogeneous() {
    let bad = lower(
        "\
main() {
    let xs = [1, 2, \"three\"];
    println(\"{}\", xs[0]);
}
",
    );
    assert!(
        diags(&bad).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArrayElementTypeMismatch { index: 2, .. })
        )),
        "heterogeneous untyped array literal must be rejected: {:?}",
        diags(&bad)
    );

    let clean = lower(
        "\
main() {
    let xs = [1, 2, 3];
    println(\"{}\", xs[0]);
}
",
    );
    assert!(
        !diags(&clean).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArrayElementTypeMismatch { .. })
        )),
        "homogeneous untyped array literal must be clean: {:?}",
        diags(&clean)
    );
}
