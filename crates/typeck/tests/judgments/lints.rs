//! the T1 lints (P0): warning-severity judgments. each fires as a warning
//! (build proceeds) and never as an error - `diags` (error-only) must stay
//! empty on every fixture here.

use super::*;

// --- T50: unsigned compare tautology ---

#[test]
fn unsigned_le_zero_is_a_tautology_warning() {
    let hir = lower(
        "main() {\n    let usize n = 3;\n    if n <= 0 {\n        println(\"{}\", 0);\n    }\n}\n",
    );
    assert!(
        warnings(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::UnsignedCompareTautology { ty, .. }) if &**ty == "usize"
        )),
        "usize <= 0 must warn: {:?}",
        warnings(&hir)
    );
    assert!(diags(&hir).is_empty(), "a lint is not an error: {:?}", diags(&hir));
}

#[test]
fn unsigned_lt_zero_and_mirrored_forms_warn() {
    // u < 0 (always false), 0 >= u (== 0 disguise), 0 > u (always false).
    let hir = lower(
        "check(usize n) -> int32 {\n    if n < 0 { return 1; }\n    if 0 >= n { return 2; }\n    if 0 > n { return 3; }\n    0\n}\nmain() { println(\"{}\", check(1)); }\n",
    );
    let count = warnings(&hir)
        .iter()
        .filter(|e| matches!(e, HirError::Type(TypeError::UnsignedCompareTautology { .. })))
        .count();
    assert_eq!(count, 3, "all three forms warn: {:?}", warnings(&hir));
}

#[test]
fn signed_and_meaningful_unsigned_compares_do_not_warn() {
    // i <= 0 is meaningful on a signed type; u > 0 and u == 0 are meaningful
    // on an unsigned one.
    let hir = lower(
        "main() {\n    let int32 i = -1;\n    let usize u = 1;\n    if i <= 0 { println(\"{}\", 1); }\n    if u > 0 { println(\"{}\", 2); }\n    if u == 0 { println(\"{}\", 3); }\n}\n",
    );
    assert!(
        !warnings(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::UnsignedCompareTautology { .. })
        )),
        "no tautology here: {:?}",
        warnings(&hir)
    );
}

// --- T48: dead / pure expression statement ---

#[test]
fn pure_expression_statement_warns() {
    let hir = lower("main() {\n    let int32 x = 1;\n    x;\n    x + 2;\n    println(\"{}\", x);\n}\n");
    let count = warnings(&hir)
        .iter()
        .filter(|e| matches!(e, HirError::Type(TypeError::UnusedExpressionValue)))
        .count();
    assert_eq!(count, 2, "both pure statements warn: {:?}", warnings(&hir));
    assert!(diags(&hir).is_empty(), "a lint is not an error: {:?}", diags(&hir));
}

#[test]
fn effectful_statements_do_not_warn() {
    // a call, an assignment, and a statement-position `if` all have effect.
    let hir = lower(
        "f() -> int32 { 1 }\nmain() {\n    mut int32 x = 0;\n    f();\n    x = 2;\n    if x > 1 { println(\"{}\", x); }\n}\n",
    );
    assert!(
        !warnings(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::UnusedExpressionValue))),
        "no dead expression here: {:?}",
        warnings(&hir)
    );
}

// --- T49: unused binding ---

#[test]
fn unused_variable_and_parameter_warn() {
    let hir = lower("f(int32 used, int32 spare) -> int32 { used }\nmain() {\n    let int32 dead = 7;\n    println(\"{}\", f(1, 2));\n}\n");
    let ws = warnings(&hir);
    assert!(
        ws.iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::UnusedBinding { name, param: false }) if &**name == "dead"
        )),
        "unused variable warns: {ws:?}"
    );
    assert!(
        ws.iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::UnusedBinding { name, param: true }) if &**name == "spare"
        )),
        "unused parameter warns with param wording: {ws:?}"
    );
}

#[test]
fn underscore_prefix_opts_out_of_unused_binding() {
    let hir = lower("f(int32 _spare) -> int32 { 1 }\nmain() {\n    let int32 _scratch = 7;\n    println(\"{}\", f(1));\n}\n");
    assert!(
        !warnings(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::UnusedBinding { .. }))),
        "`_`-prefixed names are exempt: {:?}",
        warnings(&hir)
    );
}

#[test]
fn any_reference_counts_as_a_use() {
    // an assignment target and a `&x` operand both count as uses.
    let hir = lower(
        "main() {\n    mut int32 a = 0;\n    a = 1;\n    let int32 b = 5;\n    let int32* p = &b as int32*;\n    println(\"{}\", *p);\n}\n",
    );
    assert!(
        !warnings(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::UnusedBinding { .. }))),
        "assigned / referenced bindings are used: {:?}",
        warnings(&hir)
    );
}

// --- T51: returning a reference to a local ---

#[test]
fn returning_ref_to_local_warns() {
    let hir = lower(
        "leak() -> &int32 {\n    let int32 x = 5;\n    return &x;\n}\nmain() { println(\"{}\", *leak()); }\n",
    );
    assert!(
        warnings(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::RefToLocalReturned { name }) if &**name == "x"
        )),
        "return &local warns: {:?}",
        warnings(&hir)
    );
}

#[test]
fn tail_ref_to_local_warns() {
    let hir = lower("leak() -> &int32 {\n    let int32 x = 5;\n    &x\n}\nmain() { println(\"{}\", *leak()); }\n");
    assert!(
        warnings(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::RefToLocalReturned { name }) if &**name == "x"
        )),
        "the implicit-return tail is a return site: {:?}",
        warnings(&hir)
    );
}

#[test]
fn returning_ref_through_deref_does_not_warn() {
    // `&(*p)` points at the pointee's storage, not the local's; a param ref
    // passed straight through is a path, not a `&local`.
    let hir = lower(
        "extern {\n    malloc(usize n) -> ptr;\n}\npass(int32* p) -> &int32 {\n    &*p\n}\nmain() {\n    let int32* m = malloc(4) as int32*;\n    println(\"{}\", *pass(m));\n}\n",
    );
    assert!(
        !warnings(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::RefToLocalReturned { .. }))),
        "a deref-rooted ref is not a local escape: {:?}",
        warnings(&hir)
    );
}
