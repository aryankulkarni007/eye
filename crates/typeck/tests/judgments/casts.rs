use super::*;

/// cast lattice (S3): an `as` cast with an aggregate (array) operand has no
/// value-level conversion and is rejected; scalar<->scalar, pointer<->pointer,
/// and pointer<->integer casts stay clean.
#[test]
fn aggregate_cast_is_rejected() {
    let hir = lower(
        "\
main() {
    let [int32; 2] a = [1, 2];
    let int32 b = a as int32;
    println(\"{}\", b);
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::CastNotAllowed { from, .. }) if from.contains("int32")
        )),
        "expected CastNotAllowed casting an array to int32, got: {:?}",
        diags(&hir)
    );
}

#[test]
fn scalar_and_pointer_casts_are_clean() {
    let hir = lower(
        "\
main() {
    let int32 i = 5;
    let int64 j = i as int64;
    let int32* q = &i;
    let usize addr = q as usize;
    let ptr z = 0 as ptr;
    println(\"{}\", j);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::CastNotAllowed { .. }))),
        "scalar/pointer/int casts must be clean, got: {:?}",
        diags(&hir)
    );
}

/// U2/U4 (S3): a bare out-of-range const value is rejected; the same magnitude
/// behind an explicit `as` cast to the type is the blessed truncation (U4 folds
/// it to the wrapped value, which is in range) and stays clean.
#[test]
fn const_value_out_of_range_is_rejected() {
    let hir = lower(
        "\
const int8 BIG = 200;
const int8 OK = 200 as int8;
const int8 FINE = 100;

main() {
    println(\"{}\", FINE);
}
",
    );
    let oor: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Const(ConstError::ConstValueOutOfRange { value, ty, .. }) => {
                Some((value.clone(), ty.as_str().to_owned()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        oor,
        [("200".to_owned(), "int8".to_owned())],
        "only the bare `200` const should be out of range (the `200 as int8` is blessed), got: {:?}",
        diags(&hir)
    );
}

/// a string literal (`&[uint8; N]`) decays into a `char*` slot - both a scalar
/// `let` and an array element - via the `char`<->`uint8` byte pun. closes the
/// string/char duality gap; no type error at any of the three sites.
#[test]
fn string_literal_decays_to_char_ptr() {
    let hir = lower(
        "\
greet(char* s) -> int32 { 0 }

main() -> int32 {
    let char* s = \"hi\";
    let [char*; 2] xs = [\"a\", \"b\"];
    greet(s) + greet(xs[0])
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "string -> char* decay (scalar + array element + arg) must be accepted: {:?}",
        diags(&hir)
    );
}

/// the kernel cuts the implicit raw-`ptr` -> typed-pointer footgun: a `ptr`
/// (`malloc`) tail in an `int32*` value-position `if` rejects (the branch is
/// consistent with its sibling but not the declared type). an explicit
/// `as int32*` is the escape.
#[test]
fn raw_ptr_into_typed_pointer_branch_is_rejected() {
    let hir = lower(
        "\
extern { malloc(usize n) -> ptr; }
bad(int32 c) -> int32 { let int32* x = if c > 0 { malloc(8) } else { malloc(4) }; 0 }
main() -> int32 { 0 }
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Type(TypeError::IfBranchTypeMismatch { .. }))),
        "raw ptr -> int32* if-branch must reject: {:?}",
        diags(&hir)
    );
}

#[test]
fn raw_ptr_into_typed_pointer_with_cast_is_accepted() {
    let hir = lower(
        "\
extern { malloc(usize n) -> ptr; }
good(int32 c) -> int32 { let int32* x = if c > 0 { malloc(8) as int32* } else { malloc(4) as int32* }; 0 }
main() -> int32 { 0 }
",
    );
    assert!(
        !diags(&hir).iter().any(|d| matches!(d, HirError::Type(_))),
        "explicit `as int32*` must be accepted: {:?}",
        diags(&hir)
    );
}
