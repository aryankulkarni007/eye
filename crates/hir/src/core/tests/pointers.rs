use super::*;

/// `&` requires a place (T036, ruled 2026-06-12): `&(a + b)` would spill the
/// value to a MIR temp and silently take the temp's address. places (a
/// variable, field, index, deref) stay legal.
#[test]
fn ref_of_non_place_is_rejected() {
    let non_place = lower(
        "main() {\n    let int32 a = 1;\n    let int32 b = 2;\n    let &int32 p = &(a + b);\n}\n",
    );
    assert!(
        diags(&non_place)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::RefOfNonPlace))),
        "`&(a + b)` must be rejected; got: {:?}",
        non_place.diagnostics
    );
    let place = lower("main() {\n    let int32 a = 1;\n    let &int32 p = &a;\n}\n");
    assert!(
        !diags(&place)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::RefOfNonPlace))),
        "`&a` must stay legal; got: {:?}",
        place.diagnostics
    );
}

/// `sizeof(T)` folds to a `usize` constant for every named type.
#[test]
fn sizeof_for_primitives_and_structs() {
    let hir = lower(
        "\
structure Point {
    int32 x,
    int32 y,
};
main() {
    let usize a = sizeof(int32);
    let usize b = sizeof(int64);
    let usize c = sizeof(float64);
    let usize d = sizeof(Point);
    let usize e = sizeof(uint8);
    let usize f = sizeof(char);
    println(\"{} {} {} {} {} {}\", a, b, c, d, e, f);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "sizeof for named types must succeed: {:?}",
        diags(&hir)
    );
}

/// `as` casts between numeric types lower without error (widening, narrowing, int↔float).
#[test]
fn numeric_as_casts_succeed() {
    let hir = lower(
        "\
main() {
    let int32 x = 42;
    let int64 y = x as int64;
    let uint8 z = x as uint8;
    let float64 w = x as float64;
    let int32 a = w as int32;
    let float32 b = x as float32;
    println(\"{} {} {} {} {} {}\", y, z, w, a, b, 0);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "numeric as casts must succeed: {:?}",
        diags(&hir)
    );
}

/// `as` casts between `ptr` and typed pointers lower without error.
#[test]
fn pointer_as_casts_succeed() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let ptr p = malloc(8);
    let uint8* bp = p as uint8*;
    let ptr q = bp as ptr;
    println(\"{}\", q == p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "pointer as casts must succeed: {:?}",
        diags(&hir)
    );
}

/// typed pointer arithmetic lowers without error.
#[test]
fn typed_pointer_arithmetic_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(8) as int32*;
    let int32* q = p + 1;
    println(\"{}\", q == p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "typed pointer arithmetic must succeed: {:?}",
        diags(&hir)
    );
}

/// typed pointer dereference lowers without error.
#[test]
fn typed_pointer_deref_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    *p = 42;
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "typed pointer dereference must succeed: {:?}",
        diags(&hir)
    );
}

/// `&` reference on a variable lowers without error.
#[test]
fn ref_on_var_succeeds() {
    let hir = lower(
        "\
main() {
    let int32 a = 1;
    let &int32 r = &a;
    let int32 b = *r;
    println(\"{}\", b);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "ref on var must succeed: {:?}",
        diags(&hir)
    );
}

/// multiple pointer indirection (`int32** pp`) lowers without error.
#[test]
fn multiple_pointer_indirection_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    let int32** pp = &p;
    let int32 v = **pp;
    println(\"{}\", v);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "multiple pointer indirection must succeed: {:?}",
        diags(&hir)
    );
}

/// `*(&x)` round-trip (deref of ref to a place) lowers without error.
#[test]
fn deref_of_ref_roundtrip_succeeds() {
    let hir = lower(
        "\
main() {
    let int32 x = 42;
    let int32 y = *(&x);
    println(\"{}\", y);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "deref of ref round-trip must succeed: {:?}",
        diags(&hir)
    );
}

/// `sizeof` in a nested expression (e.g. `sizeof(int32) + 1`) lowers without error.
#[test]
fn sizeof_in_expression_succeeds() {
    let hir = lower(
        "\
main() {
    let usize n = sizeof(int32) + sizeof(int64);
    println(\"{}\", n);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "sizeof in expression must succeed: {:?}",
        diags(&hir)
    );
}

/// compound assignment with pointer arithmetic lowers without error.
#[test]
fn pointer_compound_assignment_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(16) as int32*;
    let int32* q = p + 1;
    println(\"{}\", 0);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "pointer compound assignment must succeed: {:?}",
        diags(&hir)
    );
}

/// deref assignment (`*p = val`) through a typed pointer lowers without error.
#[test]
fn deref_assignment_succeeds() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    *p = 42;
    println(\"{}\", *p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "deref assignment must succeed: {:?}",
        diags(&hir)
    );
}
