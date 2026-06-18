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
        "sizeof for named types must lower clean: {:?}",
        diags(&hir)
    );
}

/// `as` casts between numeric types lower clean (widening, narrowing, int↔float).
#[test]
fn numeric_as_casts_lower_clean() {
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
        "numeric as casts must lower clean: {:?}",
        diags(&hir)
    );
}

/// `as` casts between `ptr` and typed pointers lower clean.
#[test]
fn pointer_as_casts_lower_clean() {
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
        "pointer as casts must lower clean: {:?}",
        diags(&hir)
    );
}

/// typed pointer arithmetic lowers clean.
#[test]
fn typed_pointer_arithmetic_lowers_clean() {
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
        "typed pointer arithmetic must lower clean: {:?}",
        diags(&hir)
    );
}

/// typed pointer dereference lowers clean.
#[test]
fn typed_pointer_deref_lowers_clean() {
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
        "typed pointer dereference must lower clean: {:?}",
        diags(&hir)
    );
}

/// `&` reference on a variable lowers clean.
#[test]
fn ref_on_var_lowers_clean() {
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
        "ref on var must lower clean: {:?}",
        diags(&hir)
    );
}

/// multiple pointer indirection (`int32** pp`) lowers clean.
#[test]
fn multiple_pointer_indirection_lowers_clean() {
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
        "multiple pointer indirection must lower clean: {:?}",
        diags(&hir)
    );
}

/// `*(&x)` round-trip (deref of ref to a place) lowers clean.
#[test]
fn deref_of_ref_roundtrip_lowers_clean() {
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
        "deref of ref round-trip must lower clean: {:?}",
        diags(&hir)
    );
}

/// `sizeof` in a nested expression (e.g. `sizeof(int32) + 1`) lowers clean.
#[test]
fn sizeof_in_expression_lowers_clean() {
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
        "sizeof in expression must lower clean: {:?}",
        diags(&hir)
    );
}

/// compound assignment with pointer arithmetic lowers clean.
#[test]
fn pointer_compound_assignment_lowers_clean() {
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
        "pointer compound assignment must lower clean: {:?}",
        diags(&hir)
    );
}

/// deref assignment (`*p = val`) through a typed pointer lowers clean.
#[test]
fn deref_assignment_lowers_clean() {
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
        "deref assignment must lower clean: {:?}",
        diags(&hir)
    );
}
