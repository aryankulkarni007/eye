use super::*;

/// `print` of a compound argument (array/struct/union) has no `{}` rendering and
/// is rejected (printcannotformat) - relocated from lowering to the typeck pass
/// at S2C C5, since it needs the argument type.
#[test]
fn print_compound_is_rejected() {
    let arr = lower("main() {\n    let [int32; 2] a = [1, 2];\n    println(\"{}\", a);\n}\n");
    assert!(
        diags(&arr).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintCannotFormat { kind }) if *kind == "an array"
        )),
        "printing a whole array must be rejected; got: {:?}",
        diags(&arr)
    );
    let strct = lower(
        "structure P { int32 x, };\nmain() {\n    let P p = P { x: 1 };\n    println(\"{}\", p);\n}\n",
    );
    assert!(
        diags(&strct).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintCannotFormat { kind }) if *kind == "a struct"
        )),
        "printing a struct must be rejected; got: {:?}",
        diags(&strct)
    );
}

/// `len` on a non-array argument (lennotarray) and `.len` field syntax on an
/// array (lenfieldonarray) are both diagnostics - relocated from lowering to the
/// typeck pass at S2C C5, since they need the operand's type.
#[test]
fn len_misuse_is_diagnosed() {
    let non_array = lower("main() {\n    let int32 x = 0;\n    println(\"{}\", len(x));\n}\n");
    assert!(
        diags(&non_array)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::LenNotArray))),
        "`len` on a non-array must diagnose; got: {:?}",
        diags(&non_array)
    );
    let dot_form =
        lower("main() {\n    let [int32; 3] xs = [1, 2, 3];\n    println(\"{}\", xs.len);\n}\n");
    assert!(
        diags(&dot_form)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::LenFieldOnArray))),
        "`.len` field form must steer to `len(x)`; got: {:?}",
        diags(&dot_form)
    );
}

/// a call's return type resolves to the callee's declared return - a user fn
/// here - so a `let` of the matching type compiles clean. (was a lowering
/// expr-type-stamp assertion; the stamp moved to typeck at S2C C5, so this now
/// checks the type end-to-end via the let-type judgment.)
#[test]
fn call_return_type_resolves_user_fn() {
    let hir = lower("answer() -> int32 {\n    42\n}\nmain() {\n    let int32 x = answer();\n}\n");
    assert!(diags(&hir).is_empty(), "expected clean: {:?}", diags(&hir));
}

/// an extern call's return type resolves the same way (`strlen -> usize`).
#[test]
fn call_return_type_resolves_extern_fn() {
    let hir = lower(
        "extern {\n    strlen(ptr s) -> usize;\n}\nmain() {\n    let usize n = strlen(\"abc\" as ptr);\n}\n",
    );
    assert!(diags(&hir).is_empty(), "expected clean: {:?}", diags(&hir));
}

/// calling a value that is not a function pointer (`let int32 x = 5; x(3);`) is a
/// `CallNonFunction` diagnostic (relocated from lowering to the typeck pass at
/// S2C C5), not a raw clang error.
#[test]
fn calling_a_non_function_is_rejected() {
    let hir = lower("main() {\n    let int32 x = 5;\n    println(\"{}\", x(3));\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::CallNonFunction { .. }))),
        "calling a non-function must be rejected; got: {:?}",
        diags(&hir)
    );
}

/// S3 argument type judgment: a call argument whose type does not match the
/// parameter is rejected. arity was checked before; types were not (swapped
/// args slipped through to clang).
#[test]
fn call_argument_type_mismatch_is_rejected() {
    let hir = lower(
        "\
take(int32 n) -> int32 { n }

main() {
    take(\"hello\");
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArgTypeMismatch { index, expected, .. })
                if *index == 1 && expected == "int32"
        )),
        "expected ArgTypeMismatch (string into int32 param), got: {:?}",
        diags(&hir)
    );

    // both args wrong: a bool into the int32 param and an int literal into the
    // bool param (no implicit int->bool, so the literal is rejected too).
    let swapped = lower(
        "\
combine(int32 n, bool b) -> int32 { n }

main() {
    combine(true, 7);
}
",
    );
    let bad: Vec<usize> = diags(&swapped)
        .iter()
        .filter_map(|e| match e {
            HirError::Type(TypeError::ArgTypeMismatch { index, .. }) => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(
        bad,
        [1, 2],
        "both wrong-typed arguments are flagged: {:?}",
        diags(&swapped)
    );
}

/// correct argument types, integer-width adoption, and the pointer escapes
/// (`&[T; N] -> &T` decay, a typed reference widening into `ptr`) must stay
/// clean - the check must not over-reject the kernel's FFI conventions.
#[test]
fn call_argument_correct_and_escapes_are_clean() {
    let ok = lower(
        "\
extern free(ptr p)

scale(usize n) -> usize { n * 2 }

main() {
    let usize n = 4;
    let int32 x = 9;
    scale(n);
    scale(7);
    free(&x);
}
",
    );
    assert!(
        !diags(&ok)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArgTypeMismatch { .. }))),
        "correct args + ptr-widening escape must not be rejected: {:?}",
        diags(&ok)
    );
}

/// boundary strict width (the `types_compatible` integer-family leniency is
/// gone): a non-literal `int64` argument to an `int8` parameter rejects - the
/// arg-boundary analogue of M2b. a literal would adopt the param width instead.
#[test]
fn mismatched_width_argument_is_rejected() {
    let hir = lower(
        "\
take(int8 x) -> int8 { x }
f(int64 n) -> int8 { take(n) }
main() -> int32 { 0 }
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Type(TypeError::ArgTypeMismatch { .. }))),
        "an int64 arg to an int8 param must reject: {:?}",
        diags(&hir)
    );
}

/// a safe reference `&T` widens into a raw typed-pointer slot `T*` (same
/// pointee) at a coercion site - argument, struct field, and return - without a
/// cast, since both are a `T*` in C and using a valid reference as a raw pointer
/// is lossless. (#372 ref/ptr auto-conversion, the safe direction.)
#[test]
fn ref_widens_to_typed_pointer() {
    let ok = lower(
        "\
structure Box { int32* p, };
take(int32* p) -> int32 { *p }
ret(&int32 r) -> int32* { r }
main() {
    let int32 x = 5;
    let int32 a = take(&x);
    let Box b = Box { p: &x };
    println(\"{}\", a);
}
",
    );
    assert!(
        !diags(&ok).iter().any(|e| matches!(
            e,
            HirError::Type(
                TypeError::ArgTypeMismatch { .. }
                    | TypeError::StructFieldTypeMismatch { .. }
                    | TypeError::ReturnTypeMismatch { .. }
            )
        )),
        "&T must widen into a T* slot at arg/field/return: {:?}",
        diags(&ok)
    );
}

/// the reverse stays gated: a raw `T*` does NOT implicitly become a `&T` (that
/// would fabricate the safety guarantee); it needs an explicit cast.
#[test]
fn typed_pointer_does_not_narrow_to_ref() {
    let bad = lower(
        "\
want(&int32 r) -> int32 { *r }
forward(int32* p) -> int32 { want(p) }
main() {
    let int32 x = 5;
    println(\"{}\", forward(&x));
}
",
    );
    assert!(
        diags(&bad)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArgTypeMismatch { .. }))),
        "a raw T* into a &T slot must be rejected (needs a cast): {:?}",
        diags(&bad)
    );
}

/// the two-span diagnostic (TYPECK.md tier 2): a type mismatch at a coercion
/// site carries the imposing declaration's span (`decl`) for the secondary
/// "declared here" label. covers the argument (parameter decl), struct field,
/// and return-type sites.
#[test]
fn type_mismatch_carries_declaration_span() {
    // argument: the callee parameter's decl span.
    let arg = lower(
        "\
take(int32 x) -> int32 { x }
main() {
    let bool b = true;
    take(b);
}
",
    );
    assert!(
        diags(&arg).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArgTypeMismatch { decl: Some(_), .. })
        )),
        "ArgTypeMismatch must carry the parameter decl span: {:?}",
        diags(&arg)
    );

    // struct field: the field's decl span.
    let field = lower(
        "\
structure P { int32 x, };
main() {
    let P p = P { x: true };
}
",
    );
    assert!(
        diags(&field).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::StructFieldTypeMismatch { decl: Some(_), .. })
        )),
        "StructFieldTypeMismatch must carry the field decl span: {:?}",
        diags(&field)
    );

    // return: the return-type annotation span.
    let ret = lower(
        "\
f() -> int32 { true }
main() {}
",
    );
    assert!(
        diags(&ret).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnTypeMismatch { decl: Some(_), .. })
        )),
        "ReturnTypeMismatch must carry the return-type decl span: {:?}",
        diags(&ret)
    );
}
