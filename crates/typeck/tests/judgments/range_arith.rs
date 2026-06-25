use super::*;

/// CLEAK M1: an integer literal must fit the integer type its context gives
/// it. out of range - at an annotated site, negated into an unsigned type, or
/// over the bare `int32` default - is an error; a wide literal under a wide
/// annotation is clean. (moved from hir tests with the S2 check migration.)
#[test]
fn int_literal_range_is_checked() {
    let hir = lower(
        "\
main() {
    let int32 a = 5000000000;
    let uint8 b = -1;
    let int8 c = 300;
    println(\"{}\", 6000000000);
}
",
    );
    let out_of_range: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Type(TypeError::IntLiteralOutOfRange { value, ty, .. }) => {
                Some((value.clone(), ty.as_str().to_owned()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        out_of_range,
        [
            ("5000000000".to_owned(), "int32".to_owned()),
            ("-1".to_owned(), "uint8".to_owned()),
            ("300".to_owned(), "int8".to_owned()),
            ("6000000000".to_owned(), "int32".to_owned()),
        ],
        "expected exactly the four out-of-range literals: {:?}",
        diags(&hir)
    );

    // in range under the declared type: clean, including both int32 bounds
    // and a 64-bit literal under an int64/usize annotation.
    let hir = lower(
        "\
main() {
    let int64 a = 5000000000;
    let usize b = 18446744073709551615;
    let int32 c = -2147483648;
    let int32 d = 2147483647;
    let uint8 e = 255;
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "in-range literals must be clean: {:?}",
        diags(&hir)
    );
}

/// a user-written cast operand keeps its range check via the cast target
/// at S3 (the cast lattice); the literal synthesized by the `len` fold is
/// skipped by its shared syntax pointer. this pins the skip's precision:
/// `len(xs)` produces no range diagnostic.
#[test]
fn len_fold_literal_is_not_range_checked() {
    let hir = lower(
        "\
main() {
    let [int32; 3] xs = [1, 2, 3];
    println(\"{}\", len(xs));
}
",
    );
    assert!(diags(&hir).is_empty(), "unexpected: {:?}", diags(&hir));
}

/// a whole array is a struct in the c backend, so a binary operator on it
/// would emit invalid c. every operator family is rejected. (moved from hir
/// tests with the S2 step-b operator-judgment migration.)
#[test]
fn binary_op_on_array_is_rejected() {
    for op in ["==", "+", "<"] {
        let src = format!(
            "main() {{\n    let [int32; 2] a = [1, 2];\n    let [int32; 2] b = [3, 4];\n    let x = a {op} b;\n}}\n"
        );
        let hir = lower(&src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::OpOnArray { .. }))),
            "`a {op} b` on arrays must be rejected; got: {:?}",
            diags(&hir)
        );
    }
}

/// `%` is integer-only: on a float it would lower to invalid c (`double %
/// double`). Rejected whether the float is on the left or right; integer `%`
/// stays clean (the float guard must not catch it).
#[test]
fn modulo_on_float_is_rejected() {
    for src in [
        "main() {\n    let float64 a = 5.5;\n    let x = a % 2.0;\n}\n",
        "main() {\n    let float32 a = 5.5;\n    let x = a % 2.0;\n}\n",
        "main() {\n    let float64 a = 5.5;\n    let int32 b = 2;\n    let x = b % a;\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::ModuloOnFloat))),
            "`%` on a float must be rejected; got: {:?}",
            diags(&hir)
        );
    }
    let int = lower("main() {\n    let int32 a = 5;\n    let x = a % 2;\n}\n");
    assert!(
        !diags(&int)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ModuloOnFloat))),
        "integer `%` must not trip the float guard; got: {:?}",
        diags(&int)
    );
}

/// enums are opaque, not ordinal (T035): arithmetic and bitwise operators on
/// an enum value are rejected; comparisons stay allowed and `as` to an integer
/// stays the explicit escape.
#[test]
fn enum_arithmetic_is_rejected() {
    let src = "enum E = A | B;\n\
               main() {\n    let E a = A;\n    let E b = B;\n";
    let plus = lower(&format!("{src}    let E c = a + b;\n}}\n"));
    assert!(
        diags(&plus)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArithmeticOnEnum { op, enum_name }) if *op == "+" && enum_name == "E")),
        "`+` on enum values must be rejected; got: {:?}",
        diags(&plus)
    );
    let neg = lower(&format!("{src}    let E c = -a;\n}}\n"));
    assert!(
        diags(&neg).iter().any(
            |e| matches!(e, HirError::Type(TypeError::ArithmeticOnEnum { op, .. }) if *op == "-")
        ),
        "unary `-` on an enum value must be rejected; got: {:?}",
        diags(&neg)
    );
    let cmp = lower(&format!("{src}    let bool eq = a == b;\n}}\n"));
    assert!(
        !diags(&cmp)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArithmeticOnEnum { .. }))),
        "`==` on enum values must stay legal; got: {:?}",
        diags(&cmp)
    );
    let cast = lower(&format!("{src}    let int32 n = (a as int32) + 1;\n}}\n"));
    assert!(
        !diags(&cast)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ArithmeticOnEnum { .. }))),
        "`as int32` then arithmetic must stay legal; got: {:?}",
        diags(&cast)
    );
}

/// A4: a literal index past a fixed array's length is a hard error - c would
/// only warn. an in-bounds literal index stays clean (the control).
#[test]
fn literal_array_index_out_of_bounds_is_rejected() {
    let oob =
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    println(\"{}\", xs[9]);\n}\n");
    assert!(
        diags(&oob)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::IndexOutOfBounds { .. }))),
        "expected an out-of-bounds diagnostic, got: {:?}",
        diags(&oob)
    );
    let ok =
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    println(\"{}\", xs[3]);\n}\n");
    assert!(
        diags(&ok).is_empty(),
        "in-bounds index must be clean, got: {:?}",
        diags(&ok)
    );
}

/// a statically negative literal index is out of bounds for any length, so it
/// is rejected like a too-large literal index (A4).
#[test]
fn negative_literal_index_is_rejected() {
    let hir =
        lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    println(\"{}\", a[-1]);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::NegativeIndex))),
        "negative literal index must be rejected; got: {:?}",
        diags(&hir)
    );
}

/// `len(a)` folds to `(usize)N`, so `a[len(a)]` is a static off-by-one: the
/// bounds check peels the fold's cast and still flags it.
#[test]
fn len_as_index_is_caught_out_of_bounds() {
    let hir =
        lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    println(\"{}\", a[len(a)]);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::IndexOutOfBounds { .. }))),
        "`a[len(a)]` must be caught as out of bounds; got: {:?}",
        diags(&hir)
    );
}

/// L7 / P1: the untyped `ptr` cannot be indexed, dereferenced, or used in
/// arithmetic; comparisons stay allowed.
#[test]
fn ptr_index_deref_arithmetic_rejected() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let ptr p = malloc(8);
    p[0];
    *p;
    p + 4;
    if p == 0 as ptr { };
}
",
    );
    let ds = diags(&hir);
    assert!(
        ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::IndexOnPtr))),
        "expected IndexOnPtr: {ds:?}"
    );
    assert!(
        ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::DerefOfPtr))),
        "expected DerefOfPtr: {ds:?}"
    );
    assert!(
        ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::ArithmeticOnPtr { op }) if *op == "+")),
        "expected ArithmeticOnPtr: {ds:?}"
    );
    // the comparison must not be rejected.
    assert!(
        !ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::ArithmeticOnPtr { op }) if *op == "==")),
        "comparison on ptr must stay legal: {ds:?}"
    );
}

/// CLEAK M2: a binary's result type is the operands' common integer width, so
/// a literal mixed with a `usize` operand types `usize`, not the literal's
/// `int32` (the prior LHS-only rule that truncated the c result). both operand
/// orders adopt the concrete width.
#[test]
fn mixed_width_arith_adopts_concrete_width() {
    use hir::core::{Expr, TypeKind};

    let src = "\
f(usize size) -> usize {
    let usize a = size + 1;
    let usize b = 1 + size;
    a + b
}
";
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parser::parse(&lexed.tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    let hir = hir::core::lower_source_file(file, &lexed.interner);
    let typeck = typeck::check_file(&hir);

    let (fn_id, function) = hir
        .functions
        .iter()
        .find(|(_, f)| f.body.is_some())
        .expect("one function with a body");
    let body = &hir.bodies[function.body.expect("has body")];
    let results = &typeck[&fn_id];

    let widths: Vec<String> = body
        .exprs
        .iter()
        .filter(|(_, e)| matches!(e, Expr::Binary { .. }))
        .map(
            |(idx, _)| match hir.types.lookup(results.expr_types[idx.into()]) {
                TypeKind::Path(n) => n.as_str().to_owned(),
                other => format!("{other:?}"),
            },
        )
        .collect();

    assert!(!widths.is_empty(), "expected binary expressions");
    assert!(
        widths.iter().all(|n| n == "usize"),
        "every binary types usize (literal adopted the concrete width): {widths:?}"
    );
}

/// F2 (S3): unary `-` on an unsigned value wraps in C; rejected (Rust parity).
/// a negated literal (a signed constant) and `-` on a signed value stay legal.
#[test]
fn negation_on_unsigned_is_rejected() {
    let hir = lower(
        "\
main() {
    mut uint32 u = 7;
    let uint32 a = -u;
    let int32 s = -5;
    println(\"{}\", a);
    println(\"{}\", s);
}
",
    );
    let neg: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Type(TypeError::NegationOnUnsigned { ty }) => Some(ty.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        neg,
        ["uint32".to_owned()],
        "expected exactly one NegationOnUnsigned (on `-u`), got: {:?}",
        diags(&hir)
    );
}

/// F3 (S3): a float literal adopts the expected float width, so a `float32`
/// array literal types each element `float32` and the L4 element judgment does
/// not falsely reject the `float64`-default literals. (an F3-less stamp would
/// surface here as a spurious ArrayElementTypeMismatch.)
#[test]
fn float_literal_adopts_expected_width() {
    let hir = lower(
        "\
main() {
    let float32 f = 1.5;
    let [float32; 2] xs = [1.5, 2.5];
    println(\"{}\", f);
}
",
    );
    assert!(
        !diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ArrayElementTypeMismatch { .. })
        )),
        "float literals must adopt float32; no element mismatch expected, got: {:?}",
        diags(&hir)
    );
}

/// M2b: a binary on two distinct *concrete* integer widths (neither a literal)
/// silently narrows in C; reject and make the user cast (Rust's strict-width
/// rule). the literal-adoption case (M2) and equal widths stay legal.
#[test]
fn mixed_integer_widths_are_rejected() {
    let hir = lower(
        "\
f(int8 a, int64 b) -> int8 { a + b }
main() -> int32 { 0 }
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Type(TypeError::MixedIntegerWidths { .. }))),
        "int8 + int64 must reject (M2b): {:?}",
        diags(&hir)
    );
}

#[test]
fn matching_width_and_literal_binaries_accepted() {
    let hir = lower(
        "\
same(int8 a, int8 b) -> int8 { a + b }
adopt(int8 a) -> int8 { a + 5 }
wide(usize n) -> usize { n - 1 }
main() -> int32 { 0 }
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Type(TypeError::MixedIntegerWidths { .. }))),
        "equal widths + literal adoption must be accepted: {:?}",
        diags(&hir)
    );
}

/// `*x` on a non-pointer value (`int32`, a struct, ...) is rejected: `*` is the
/// deref operator and a plain value has nothing to indirect through, so it would
/// emit invalid c (the C-brain footgun - you meant `&` address-of). a genuine
/// `&T`/`T*` deref stays clean.
#[test]
fn deref_of_non_pointer_is_rejected() {
    let bad = lower(
        "\
main() {
    let int32 x = 5;
    let int32 y = *x;
    println(\"{}\", y);
}
",
    );
    assert!(
        diags(&bad)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::DerefOfNonPointer { .. }))),
        "deref of a non-pointer must be rejected: {:?}",
        diags(&bad)
    );

    let ok = lower(
        "\
main() {
    let int32 x = 5;
    let &int32 r = &x;
    let int32 y = *r;
    println(\"{}\", y);
}
",
    );
    assert!(
        !diags(&ok)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::DerefOfNonPointer { .. }))),
        "deref of a ref must stay clean: {:?}",
        diags(&ok)
    );
}

/// `x[i]` on a non-indexable value (a scalar, struct, ...) is rejected: only an
/// array or a pointer has elements. an array index stays clean.
#[test]
fn index_of_non_indexable_is_rejected() {
    let bad = lower(
        "main() {\n    let int32 x = 5;\n    let int32 y = x[0];\n    println(\"{}\", y);\n}\n",
    );
    assert!(
        diags(&bad)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::IndexOfNonIndexable { .. }))),
        "indexing a scalar must be rejected: {:?}",
        diags(&bad)
    );
    let ok =
        lower("main() {\n    let [int32; 3] xs = [1, 2, 3];\n    println(\"{}\", xs[0]);\n}\n");
    assert!(
        !diags(&ok)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::IndexOfNonIndexable { .. }))),
        "indexing an array must stay clean: {:?}",
        diags(&ok)
    );
}
