use super::*;

/// `[T; N]` lowers to `TypeRef::Array` with the literal length, an `[...]`
/// initializer to `Expr::ArrayLit`, and `xs[i]` to `Expr::Index`.
#[test]
fn array_type_literal_and_index_lower() {
    let hir = lower(
        "\
main() {
    let [int32; 3] xs = [1, 2, 3];
    let int32 a = xs[0];
}
",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    // the `xs` local carries an array type with the parsed length.
    let int32_ty = hir.types.int32_ty();
    let types = &hir.types;
    let array_local = body
        .locals
        .iter()
        .find_map(|(_, l)| l.ty.and_then(|ty| ty.as_array(types)));
    let (elem, len) = array_local.expect("xs local has an Array type");
    assert_eq!(len, 3, "array length parsed from literal");
    assert_eq!(elem, int32_ty, "element type");

    // both an arraylit and an index expression were produced.
    assert!(
        body.exprs
            .iter()
            .any(|(_, e)| matches!(e, Expr::ArrayLit(v) if v.len() == 3)),
        "expected a 3-element ArrayLit"
    );
    assert!(
        body.exprs
            .iter()
            .any(|(_, e)| matches!(e, Expr::Index { .. })),
        "expected an Index expr"
    );
}

// --- v0.7 arrays first-class + latent gaps ---

/// the `len(xs)` intrinsic lowers to a `usize`-typed `Expr::Len` node, not a
/// call. the element count is folded at MIR lowering from the operand's type
/// (S2C C1); here we assert only the node and its `usize` type.
#[test]
fn array_len_lowers_to_len_node() {
    let hir = lower(
        "main() {\n    let [int32; 5] xs = [1, 2, 3, 4, 5];\n    println(\"{}\", len(xs));\n}\n",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    // `len(xs)` lowers to a `Len` node (typed `usize` by typeck; MIR folds it to
    // `(usize)5`, so it prints with `%zu`).
    let has_len_node = body.exprs.iter().any(|(_, e)| matches!(e, Expr::Len(_)));
    assert!(
        has_len_node,
        "expected `len(xs)` to lower to a `Len` node; exprs: {:?}",
        body.exprs.iter().collect::<Vec<_>>()
    );
    // and the call to `len` did not survive as a call expression.
    assert!(
        !body.exprs.iter().any(|(_, e)| matches!(
            e,
            Expr::Call { callee, .. }
                if matches!(&body.exprs[*callee], Expr::Path(Resolution::Unresolved(n)) if n == "len")
        )),
        "`len(xs)` must not lower to a call"
    );
    assert!(
        hir.diagnostics.is_empty(),
        "`len(xs)` on an array is valid; diagnostics: {:?}",
        hir.diagnostics
    );
}

// `len` on a non-array (lennotarray) and `.len` field syntax on an array
// (lenfieldonarray) both need the operand type, so they moved to the typeck pass
// at S2C C5; their tests live in `crates/typeck/tests/judgments.rs`. the
// place-restriction below (lennotaplace) is structural and stays in lowering.

/// `len` never evaluates its operand (it reads the length from the type), so a
/// computed operand like `len(f())` would silently discard the call. the
/// operand is restricted to a place (variable/field/index/deref); a call or an
/// array literal is rejected.
#[test]
fn len_of_computed_value_is_rejected() {
    let call_form = lower(
        "mk() -> [int32; 3] {\n    [1, 2, 3]\n}\nmain() {\n    println(\"{}\", len(mk()));\n}\n",
    );
    assert_eq!(
        call_form.diagnostics.len(),
        1,
        "`len(f())` must be rejected (the call would be discarded); got: {:?}",
        call_form.diagnostics
    );

    let literal_form = lower("main() {\n    println(\"{}\", len([1, 2, 3]));\n}\n");
    assert_eq!(
        literal_form.diagnostics.len(),
        1,
        "`len([..])` must be rejected (not a place); got: {:?}",
        literal_form.diagnostics
    );
}

/// `len` on an array reference still works without an explicit deref: `&[T; N]`
/// is a place and one ref is peeled. `len(*r)` works too. both fold to the
/// length with no diagnostic.
#[test]
fn len_through_reference_is_accepted() {
    let hir = lower(
        "sum(&[int32; 3] r) -> usize {\n    len(r)\n}\nmain() {\n    mut [int32; 3] a = [1, 2, 3];\n    println(\"{}\", sum(&a));\n}\n",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "`len(r)` on `&[T; N]` is valid (auto-deref kept); diagnostics: {:?}",
        hir.diagnostics
    );
}

/// array of pointers lowers without error.
#[test]
fn array_of_pointers_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let [ptr; 3] ps = [malloc(4), malloc(4), malloc(4)];
    println(\"{}\", ps[0] == ps[1]);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "array of pointers must succeed: {:?}",
        diags(&hir)
    );
}
