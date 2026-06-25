use super::*;

// ---- return-type enforcement: implicit-return tail + explicit `return`
// (moved from hir tests with the S2 step-b return-enforcement migration) ----

/// the general tail-vs-declared-return-type check: a function returning int32
/// whose tail produces an enum value is diagnosed.
#[test]
fn return_type_mismatch_non_match_tail_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
bad() -> int32 { Red }
main() { println(\"{}\", bad()); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnTypeMismatch { expected, found, .. })
                if *expected == "int32" && *found == "Color"
        )),
        "expected return-type-mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// comparison operators are typed `bool`, so a `-> bool` function whose tail is
/// a comparison must NOT be flagged as a return-type mismatch. guards the false
/// positive that motivated typing comparison results as bool.
#[test]
fn bool_returning_comparison_tail_is_clean() {
    let src = "\
gt(int32 a, int32 b) -> bool { a > b }
main() { println(\"{}\", gt(3, 1)); }
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "comparison tail must type as bool and not mismatch a bool return, got: {:?}",
        diags(&hir)
    );
}

/// `return expr;` in a void function is rejected (it reaches clang as a value
/// returned from a `void` function, a hard error).
#[test]
fn return_value_in_void_is_rejected() {
    let hir = lower("f() {\n    return 5;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnValueInVoid))),
        "`return <value>` in a void function must be rejected; got: {:?}",
        diags(&hir)
    );
}

/// `return;` with no value in a typed function is rejected (clang would reject
/// the missing value). `main` is an ordinary function (the c entry point is a
/// backend shim), so a bare void `main()` is NOT typed and a bare `return;` in
/// it is clean - see `bare_return_in_void_main_is_clean`.
#[test]
fn return_missing_value_is_rejected() {
    let hir = lower("g() -> int32 {\n    return;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnMissingValue { .. }))),
        "bare `return;` in a typed function must be rejected; got: {:?}",
        diags(&hir)
    );
}

/// `main` is not special-cased as `int`-returning in the front end: a bare void
/// `main()` may use `return;` like any other void function. (the c entry
/// point's `int` return is supplied by a backend shim, not a language rule.)
#[test]
fn bare_return_in_void_main_is_clean() {
    let hir = lower("main() {\n    println(\"x\");\n    return;\n}\n");
    assert!(
        !diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnMissingValue { .. } | TypeError::ReturnValueInVoid)
        )),
        "a bare `return;` in a void `main` must be clean; got: {:?}",
        diags(&hir)
    );
}

/// `return expr;` whose value type does not match the declared return type is
/// rejected, same as a mismatching tail expression.
#[test]
fn return_wrong_type_is_rejected() {
    let hir = lower("h() -> int32 {\n    return true;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "a wrong-typed `return` must be rejected; got: {:?}",
        diags(&hir)
    );
}

/// a well-formed early return trips none of the return diagnostics: a matching
/// typed `return expr;` and a bare `return;` in a void function are both clean.
#[test]
fn well_formed_early_return_is_clean() {
    for src in [
        "k() -> int32 {\n    return 7;\n}\n",
        "v() {\n    println(\"x\");\n    return;\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            !diags(&hir).iter().any(|e| matches!(
                e,
                HirError::Type(
                    TypeError::ReturnValueInVoid
                        | TypeError::ReturnMissingValue { .. }
                        | TypeError::ReturnTypeMismatch { .. }
                )
            )),
            "a well-formed early return must be clean; got: {:?}",
            diags(&hir)
        );
    }
}

/// a literal-array return whose element type differs from the declared return
/// type is clean (the element type is coerced); a wrong *length* still errors.
/// guards that the element coercion does not mask an arity mismatch.
#[test]
fn array_literal_return_coercion_keeps_length_check() {
    let ok = lower("g() -> [usize; 3] {\n    [1, 2, 3]\n}\nmain() {}\n");
    assert!(
        diags(&ok).is_empty(),
        "element coercion should make this clean, got: {:?}",
        diags(&ok)
    );

    let bad = lower("g() -> [int32; 3] {\n    [1, 2]\n}\nmain() {}\n");
    assert!(
        diags(&bad)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "a wrong-length literal return must still error, got: {:?}",
        diags(&bad)
    );
}

// ---- unit (`()`) / never (`!`) types ----

/// the explicit unit type `()` is spellable as a return type and behaves like
/// the implicit void return: a tail-less body is clean (it completes with
/// unit), and binding the `()` result in value position is the void-value error.
#[test]
fn explicit_unit_return_type_is_void_like() {
    let clean = lower("noop() -> () { }\nmain() { noop(); }\n");
    assert!(
        diags(&clean).is_empty(),
        "`-> ()` no-op function must be clean: {:?}",
        diags(&clean)
    );

    let rejected = lower("noop() -> () { }\nmain() { let int32 x = noop(); }\n");
    assert!(
        rejected
            .diagnostics
            .entries()
            .iter()
            .any(|(_, e)| matches!(e, HirError::Type(TypeError::VoidValueInValuePosition))),
        "binding a `()` value must be the void-value error: {:?}",
        diags(&rejected)
    );
}

/// a value-position `if` whose branches yield no value (tail-less, so the whole
/// `if` is `()`) is rejected wherever it sits - here as a `let` initializer and,
/// crucially, buried as a `Binary` operand (the lang.eye MIR-ICE repro). a
/// statement-position void `if` stays legal (its value is discarded).
#[test]
fn value_position_void_if_is_rejected() {
    let init = lower("extern { f(); }\nmain() { let int32 x = if true { f(); } else { f(); }; }\n");
    assert!(
        init.diagnostics
            .entries()
            .iter()
            .any(|(_, e)| matches!(e, HirError::Type(TypeError::VoidValueInValuePosition))),
        "a void `if` bound to int32 must be rejected: {:?}",
        diags(&init)
    );

    // statement position: the `if` runs for effect, its `()` discarded.
    let stmt = lower("extern { f(); }\nmain() { if true { f(); } else { f(); } }\n");
    assert!(
        !stmt
            .diagnostics
            .entries()
            .iter()
            .any(|(_, e)| matches!(e, HirError::Type(TypeError::VoidValueInValuePosition))),
        "a statement-position void `if` must be clean: {:?}",
        diags(&stmt)
    );
}

/// a diverging branch has the never type (`!`), which coerces to any expected
/// type, so a value-position `if` whose other branch yields a value is clean -
/// `let int32 x = if c { return; } else { 2 }` types as the `2`.
#[test]
fn never_branch_coerces_to_value_branch() {
    let hir = lower(
        "pick(int32 c) -> int32 {\n    let int32 x = if c < 0 { return 0; } else { 2 };\n    x\n}\nmain() { }\n",
    );
    assert!(
        diags(&hir).is_empty(),
        "a `Never` (return) branch must coerce to the value branch: {:?}",
        diags(&hir)
    );
}
