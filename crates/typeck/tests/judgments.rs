//! type-judgment diagnostics owned by the typeck pass. tests migrate here
//! from `crates/hir/src/core/tests.rs` as S2 step b moves each check
//! cluster out of lowering.

use ast::{AstNode, SourceFile};
use hir::core::{ConstError, HIR, HirError, PatternError, ResolveError, TypeError};
use lexer::{Lexer, SourceText};

/// lower + typeck, returning the HIR with lowering diagnostics and the
/// typeck diagnostics merged into one stream (fn order, like the driver).
fn lower(src: &str) -> HIR {
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parser::parse(&lexed.tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    let mut hir = hir::core::lower_source_file(file, &lexed.interner);
    let typeck = typeck::check_file(&mut hir);
    let mut fn_ids: Vec<_> = typeck.keys().copied().collect();
    fn_ids.sort_by_key(|id| id.raw_idx().into_u32());
    for fn_id in fn_ids {
        hir.diagnostics.extend(typeck[&fn_id].diagnostics.clone());
    }
    hir
}

fn diags(hir: &HIR) -> Vec<&HirError> {
    hir.diagnostics.entries().iter().map(|(_, e)| e).collect()
}

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

// ---- value-position MATCH-arm result-type consistency (MATCH.md steps 1-5,
// moved from hir tests with the S2 step-b migration) ----

const SHAPE_DECL: &str = "enum Shape = Circle | Rectangle | Triangle ;\n";

/// first arm is int32, a later arm is a string (`&[uint8; N]`): the
/// value-position match has no single result type, so the mismatching arm is
/// diagnosed.
#[test]
fn match_value_position_heterogeneous_arms_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> \"bad\",\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if *expected == "int32" && *found == "&[uint8; 3]"
        )),
        "expected arm-type-mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a `Point`-returning call in an arm of an int-typed match (regression: used
/// to silently emit ill-typed c).
#[test]
fn match_value_position_call_arm_type_mismatch_diagnosed() {
    let src = "\
structure Point { int32 x, int32 y, };
enum Color = Red | Green | Blue ;
unit() -> Point { Point { x: 1, y: 1 } }
pick() -> Color { Green }
main() {
    let int32 n = match pick() {
        Red -> 1,
        Green -> unit(),
        Blue -> 3,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if *found == "Point"
        )),
        "expected Point arm mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a match in a non-`let` value position (a call argument) is still
/// result-type checked.
#[test]
fn fn_arg_value_position_match_heterogeneous_arms_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
take(int32 n) -> int32 { n }
pick() -> Color { Green }
main() {
    let int32 a = take(match pick() { Red -> 1, Green -> pick(), Blue -> 3 });
    println(\"{}\", a);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if *found == "Color"
        )),
        "fn-arg value-position match must be arm-checked, got: {:?}",
        diags(&hir)
    );
}

/// a match as a function's implicit-return tail is value-position; the declared
/// return type is the result type, so a mismatching arm is caught.
#[test]
fn return_tail_match_heterogeneous_arms_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
pick() -> Color { Green }
sides(Color c) -> int32 {
    match c {
        Red -> 1,
        Green -> pick(),
        Blue -> 3,
    }
}
main() { println(\"{}\", sides(Red)); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if *expected == "int32" && *found == "Color"
        )),
        "return-tail match arm must be checked against the return type, got: {:?}",
        diags(&hir)
    );
}

/// statement-position MATCH has no result-type requirement (MATCH.md), so
/// differing arm-body types are not a mismatch.
#[test]
fn statement_position_match_heterogeneous_arms_not_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         match sh {{\n        \
         Circle -> 1,\n        Rectangle -> \"bad\",\n        Triangle -> 3,\n    }}\n    \
         println(\"done\");\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "statement-position match must not be result-type checked: {:?}",
        diags(&hir)
    );
}

/// no explicit binding type: the result type falls back to the first known arm
/// (int32); homogeneous arms produce no mismatch.
#[test]
fn untyped_let_homogeneous_match_arms_are_clean() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "homogeneous untyped match must be clean: {:?}",
        diags(&hir)
    );
}

/// a wider explicit binding (int64) over int-literal arms (typed int32): the
/// integer leniency means no false-positive mismatch.
#[test]
fn match_wide_int_let_no_false_positive() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int64 n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "integer widening must not false-positive: {:?}",
        diags(&hir)
    );
}

/// a match that is the tail of a body whose value is discarded (no declared
/// return) runs for effect like a statement-position match - no result type.
#[test]
fn void_tail_match_heterogeneous_arms_not_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
ic() -> int32 { 1 }
cc() -> Color { Red }
main() {
    match cc() {
        Red -> ic(),
        Green -> cc(),
        Blue -> ic(),
    }
}
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "value-discarded tail match must not be result-type checked, got: {:?}",
        diags(&hir)
    );
}

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
            HirError::Type(TypeError::ReturnTypeMismatch { expected, found })
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

// ----------------------------------------------------------------------------
// match type-judgments (migrated from `crates/hir/src/core/tests.rs` with S2C
// C2). lowering now lowers match arms purely structurally; the scrutinee-domain,
// coverage, exhaustiveness, duplicate, unreachable, and domain-mismatch
// judgments are the typeck pass's (`check_matches`), so these run lowering +
// typeck. the structural lowering tests (arm shapes, scrutinee stamping) stay in
// the hir crate.
// ----------------------------------------------------------------------------

#[test]
fn match_non_exhaustive_diags_each_missing_variant() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        Circle -> 0,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    let missing = diags(&hir)
        .iter()
        .find_map(|e| match e {
            HirError::Pattern(PatternError::NonExhaustive { missing, .. }) => Some(missing.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing non-exhaustive diag: {:?}", diags(&hir)));
    assert!(missing.iter().any(|m| m == "Rectangle"), "got: {missing:?}");
    assert!(missing.iter().any(|m| m == "Triangle"), "got: {missing:?}");
}

#[test]
fn match_duplicate_arm_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 0,\n        Rectangle -> 1,\n        \
         Circle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DuplicateArm { variant }) if variant == "Circle"
        )),
        "missing dup diag: {:?}",
        diags(&hir)
    );
}

#[test]
fn match_arm_after_wildcard_is_unreachable() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         _ -> 0,\n        Triangle -> 1,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::UnreachableAfterWildcard))),
        "expected unreachable diag, got: {:?}",
        diags(&hir)
    );
}

/// an arm after a catch-all (a `_` wildcard OR a bare-ident binding, both
/// irrefutable) is unreachable. guards the MIR `ArmKind::Bind`/`Default` paths,
/// where two irrefutable arms would otherwise both write the default slot.
#[test]
fn match_multiple_irrefutable_arms_rejected() {
    for arms in ["n -> 1,\n        _ -> 2,", "n -> 1,\n        m -> 2,"] {
        let src = format!(
            "main() {{\n    let int32 x = 5;\n    \
             let int32 r = match x {{\n        {arms}\n    }};\n    \
             println(\"{{}}\", r);\n}}\n"
        );
        let hir = lower(&src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Pattern(PatternError::UnreachableAfterWildcard))),
            "expected unreachable diag for arms `{arms}`, got: {:?}",
            diags(&hir)
        );
    }
}

#[test]
fn match_cross_enum_pattern_diagnosed() {
    let src = format!(
        "{}enum Option = Some | None ;\nmain() {{\n    \
         let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Option.Some -> 0,\n        _ -> 1,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Resolve(ResolveError::PatternEnumMismatch { pattern_enum, .. })
                if pattern_enum == "Option"
        )),
        "expected cross-enum diag, got: {:?}",
        diags(&hir)
    );
}

// a scrutinee whose type is not a matchable domain (enum / int / char / bool) is
// diagnosed. `float64` is a scalar but not discrete, so it is rejected.
#[test]
fn match_non_matchable_scrut_diagnosed() {
    let src = "\
main() {
    let float64 x = 0.0;
    let int32 n = match x {
        _ -> 1,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchScrutineeNotEnum))),
        "expected non-matchable-domain diag, got: {:?}",
        diags(&hir)
    );
}

// an int match with no `_` is non-exhaustive (the domain is too large to
// enumerate).
#[test]
fn match_int_without_wildcard_is_non_exhaustive() {
    let src = "\
main() {
    let int32 x = 1;
    let int32 n = match x {
        1 -> 10,
        2 -> 20,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::NonExhaustivePrimitive { missing, .. }) if missing.is_empty()
        )),
        "expected open-domain non-exhaustive diag, got: {:?}",
        diags(&hir)
    );
}

// a bool match missing `false` is non-exhaustive, naming the missing value.
#[test]
fn match_bool_missing_value_is_non_exhaustive() {
    let src = "\
main() {
    let bool b = true;
    let int32 n = match b {
        true -> 1,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::NonExhaustivePrimitive { ty, missing })
                if ty == "bool" && missing.iter().any(|m| m == "false")
        )),
        "expected bool non-exhaustive diag, got: {:?}",
        diags(&hir)
    );
}

// a literal pattern whose domain disagrees with the scrutinee (a bool literal
// against an int scrutinee) is a domain mismatch.
#[test]
fn match_literal_domain_mismatch_diagnosed() {
    let src = "\
main() {
    let int32 x = 1;
    let int32 n = match x {
        true -> 1,
        _ -> 0,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::PatternDomainMismatch { .. })
        )),
        "expected a domain-mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a guarded arm does not discharge coverage of its discriminant: a full-variant
/// match with a guarded arm and no `_` is non-exhaustive, since the guard may be
/// false.
#[test]
fn guarded_arm_does_not_cover_for_exhaustiveness() {
    let hir = lower(
        "\
enum E = A | B ;
main() {
    let bool c = true;
    let E e = A;
    let int32 r = match e {
        A if c -> 1,
        B -> 2,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::NonExhaustive { .. }))),
        "expected non-exhaustive (guarded `A` does not cover), got: {:?}",
        diags(&hir)
    );
}

/// a guarded wildcard with NO unconditional catch-all is non-exhaustive: the
/// guard may be false for an uncovered case.
#[test]
fn match_guard_on_wildcard_without_catchall_rejected() {
    let src = "\
enum E = A | B;
main() {
    let E e = A;
    let bool flag = false;
    let int32 r = match e {
        A -> 1,
        _ if flag -> 9,
    };
    println(\"{}\", r);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::NonExhaustive { .. }))),
        "expected non-exhaustive (guarded `_` does not cover B), got: {:?}",
        diags(&hir)
    );
}

// --- acceptance: clean matches the typeck pass must NOT diagnose ---

// an int scrutinee with literal arms plus a `_` is total - no diagnostics.
#[test]
fn match_int_literal_arms_clean() {
    let hir = lower(
        "\
main() {
    let int32 x = 2;
    let int32 n = match x {
        1 -> 10,
        2 -> 20,
        _ -> 0,
    };
    println(\"{}\", n);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_) | HirError::Type(_))),
        "expected a clean int match, got: {:?}",
        diags(&hir)
    );
}

// a bool match covering both `true` and `false` is total without a `_`.
#[test]
fn match_bool_both_values_is_exhaustive() {
    let hir = lower(
        "\
main() {
    let bool b = true;
    let int32 n = match b {
        true -> 1,
        false -> 0,
    };
    println(\"{}\", n);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "expected a clean bool match, got: {:?}",
        diags(&hir)
    );
}

/// a guard on a bare-ident binding catch-all (`x if cond`) is supported: the
/// binding is in scope for the guard, and an unconditional `_` makes the match
/// exhaustive.
#[test]
fn match_guard_on_binding_arm_supported() {
    let hir = lower(
        "\
main() {
    let int32 x = 5;
    let int32 r = match x {
        y if y > 0 -> 1,
        _ -> 0,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded binding catch-all should compile cleanly, got: {:?}",
        diags(&hir)
    );
}

/// a guard on a wildcard arm (`_ if cond`) is supported when a later
/// unconditional catch-all keeps the match exhaustive.
#[test]
fn match_guard_on_wildcard_arm_supported() {
    let hir = lower(
        "\
enum E = A | B;
main() {
    let E e = A;
    let bool flag = false;
    let int32 r = match e {
        A -> 1,
        _ if flag -> 9,
        _ -> 0,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded wildcard with trailing catch-all should compile cleanly, got: {:?}",
        diags(&hir)
    );
}

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

/// name-based classification (S2C C2): a bare ident that is NOT a known variant
/// is a binding (an irrefutable named wildcard), not an "unknown variant" error -
/// the rustc/rust-analyzer rule. over an enum scrutinee it is a catch-all, so the
/// match is exhaustive and clean. (the qualified form `Enum.Bad` still errors;
/// see the hir crate's `match_qualified_unknown_variant_diagnosed`.)
#[test]
fn match_bare_unknown_ident_is_binding_not_error() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         whatever -> 0,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_) | HirError::Resolve(_))),
        "a bare unknown ident is a binding catch-all, not an error: {:?}",
        diags(&hir)
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
    let mut hir = hir::core::lower_source_file(file, &lexed.interner);
    let typeck = typeck::check_file(&mut hir);

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
        .map(|(idx, _)| match hir.types.lookup(results.expr_types[idx.into()]) {
            TypeKind::Path(n) => n.as_str().to_owned(),
            other => format!("{other:?}"),
        })
        .collect();

    assert!(!widths.is_empty(), "expected binary expressions");
    assert!(
        widths.iter().all(|n| n == "usize"),
        "every binary types usize (literal adopted the concrete width): {widths:?}"
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

/// S3 struct-field value judgment: a field initialized with the wrong type is
/// rejected (`P { x: "hi" }` with `int32 x` reached clang before; only
/// missing/unknown fields were caught).
#[test]
fn struct_field_value_type_mismatch_is_rejected() {
    let hir = lower(
        "\
struct Point { int32 x, int32 y }

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
