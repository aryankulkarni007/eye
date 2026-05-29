use super::*;
use ast::{AstNode, SourceFile};
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};

fn emit(src: &str) -> String {
    let source = SourceText::new(src.to_string());
    let tokens = Lexer::new(&source).tokenize().tokens;
    let parse = parser::parse(&tokens, &source);
    assert!(parse.errors.is_empty(), "parse errors: {:?}", parse.errors);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    let hir = lower_source_file(file);
    assert!(
        hir.diagnostics.is_empty(),
        "hir diagnostics: {:?}",
        hir.diagnostics
    );
    CGen::new(&hir).gen_all()
}

/// Canonical `main.eye`. Pinning the C output cements the v0.1 codegen
/// behaviour - any incidental change downstream forces a snapshot
/// review.
#[test]
fn main_eye_c_output_snapshot() {
    let src = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    let int32 x = 0;
    let int32 y = 0;
    mut Point p = Point { x, y };

    print(\"{}\", p.x);
    print(\"{}\", p.y);
}
";
    insta::assert_snapshot!(emit(src));
}

/// Regression for the call-arg emission bug: the loop used to guard
/// `gen_expr` behind `if i > 0`, dropping arg 0 for every non-`print`
/// call. The v0.1 parser has no fn-call-with-args path outside
/// `print(...)`, so we build the HIR directly and feed it into the
/// expression generator.
#[test]
fn user_fn_call_emits_every_argument_in_order() {
    use hir::core::{Body, Expr, Function, HIR, Literal, Resolution, Stmt};

    let mut hir = HIR::default();

    let callee_fn = hir.functions.alloc(Function {
        name: "add".into(),
        params: Vec::new(),
        ret: None,
        body: None,
        is_extern: false,
    });

    let mut body = Body::default();
    let callee = body.exprs.alloc(Expr::Path(Resolution::Fn(callee_fn)));
    let a1 = body.exprs.alloc(Expr::Literal(Literal::Int(1)));
    let a2 = body.exprs.alloc(Expr::Literal(Literal::Int(2)));
    let a3 = body.exprs.alloc(Expr::Literal(Literal::Int(3)));
    let call = body.exprs.alloc(Expr::Call {
        callee,
        args: vec![a1, a2, a3],
    });
    let call_stmt = body.stmts.alloc(Stmt::Expr(call));
    body.block.push(call_stmt);
    let body_id = hir.bodies.alloc(body);

    let main_fn = hir.functions.alloc(Function {
        name: "main".into(),
        params: Vec::new(),
        ret: None,
        body: Some(body_id),
        is_extern: false,
    });
    hir.items.functions.insert("main".into(), main_fn);
    hir.items.functions.insert("add".into(), callee_fn);

    let c = CGen::new(&hir).gen_all();
    assert!(
        c.contains("add(1, 2, 3)"),
        "expected `add(1, 2, 3)` in output, got:\n{c}"
    );
    assert!(
        !c.contains("add(, "),
        "leading separator should not appear, got:\n{c}"
    );
}

/// Regression for nested field access: the HIR previously used
/// `NameRef::nth(1)`, which returns `None` when the base is itself
/// a `FieldExpr`. The C output should contain the chained `.y`.
#[test]
fn nested_field_access_lowers_correctly() {
    let src = "\
structure Inner {
    int32 y,
};

structure Outer {
    Inner i,
};

main() {
    let Inner i = Inner { y: 42 };
    let Outer o = Outer { i: i };
    print(\"{}\", o.i.y);
}
";
    let c = emit(src);
    assert!(
        c.contains("o.i.y"),
        "expected chained field access `o.i.y` in output, got:\n{c}"
    );
}

/// Reference type in a fn param plus the address-of prefix at the call
/// site. Auto-deref turns `v.x` inside the callee into `v->x` in C.
#[test]
fn reference_and_pointer_codegen_v02() {
    let src = "\
structure Vector {
    int32 x,
    int32 y,
};

update_vector(&Vector v) {
    -- Eye auto-dereferences references for field access
    print(\"{}\", v.x);
}

main() {
    mut Vector vec = Vector { x: 10, y: 20 };
    -- Pass by reference
    update_vector(&vec);
}
";
    let c_output = emit(src);

    // 1. parameter type lowers to a pointer
    assert!(
        c_output.contains("update_vector(Vector* v)"),
        "expected `update_vector(Vector* v)` in output, got:\n{c_output}"
    );
    // 2. auto-deref translates `.` to `->` on a ref-typed base
    assert!(
        c_output.contains("v->x"),
        "expected `v->x` in output, got:\n{c_output}"
    );
    // 3. address-of expression translation
    assert!(
        c_output.contains("update_vector(&vec)"),
        "expected `update_vector(&vec)` in output, got:\n{c_output}"
    );
}

/// Each primitive arg in a `print(...)` should map to its correct printf
/// specifier. Previously every `{}` lowered to `%d`, so strings, floats,
/// chars, and pointers came out garbled.
#[test]
fn print_format_specifiers_match_primitive_types() {
    let src = "\
main() {
    let int32 i = 7;
    let float64 f = 3.14;
    let bool b = true;
    let char c = 'x';
    print(\"i={} f={} b={} c={}\", i, f, b, c);
    print(\"lit s={} lit i={} lit f={} lit b={} lit c={}\", \"hi\", 1, 2.5, false, 'q');
}
";
    let c = emit(src);
    assert!(
        c.contains("\"i=%d f=%f b=%d c=%c\\n\""),
        "expected per-type specifiers for typed locals, got:\n{c}"
    );
    assert!(
        c.contains("\"lit s=%s lit i=%d lit f=%f lit b=%d lit c=%c\\n\""),
        "expected literal-driven specifiers, got:\n{c}"
    );
}

/// A `&T`-typed value should print with `%p` rather than the default
/// `%d`, otherwise the C compiler issues a format-mismatch warning.
#[test]
fn print_format_specifier_for_reference_is_pointer() {
    let src = "\
structure P { int32 x, };

main() {
    mut P p = P { x: 1 };
    mut &P r = &p;
    print(\"{}\", r);
}
";
    let c = emit(src);
    assert!(
        c.contains("\"%p\\n\""),
        "expected `%p` specifier for reference value, got:\n{c}"
    );
}

/// v0.4 sized/unsigned integer types lower to their `<stdint.h>` C types and
/// pick the right printf specifier: `%d` for signed widths up to 32, `%lld`
/// for `int64`, `%u` for unsigned widths up to 32, `%llu` for `uint64`.
#[test]
fn sized_integer_types_map_to_stdint_and_specifiers() {
    let src = "\
main() {
    let int8 a = 1;
    let int16 b = 2;
    let int64 c = 3;
    let uint8 d = 4;
    let uint16 e = 5;
    let uint32 f = 6;
    let uint64 g = 7;
    print(\"{}\", a);
    print(\"{}\", b);
    print(\"{}\", c);
    print(\"{}\", d);
    print(\"{}\", e);
    print(\"{}\", f);
    print(\"{}\", g);
}
";
    let c = emit(src);

    assert!(c.contains("int8_t a"), "expected `int8_t a`, got:\n{c}");
    assert!(c.contains("int16_t b"), "expected `int16_t b`, got:\n{c}");
    assert!(c.contains("int64_t c"), "expected `int64_t c`, got:\n{c}");
    assert!(c.contains("uint8_t d"), "expected `uint8_t d`, got:\n{c}");
    assert!(c.contains("uint16_t e"), "expected `uint16_t e`, got:\n{c}");
    assert!(c.contains("uint32_t f"), "expected `uint32_t f`, got:\n{c}");
    assert!(c.contains("uint64_t g"), "expected `uint64_t g`, got:\n{c}");

    assert!(
        c.contains("\"%lld\\n\""),
        "expected `%lld` for int64, got:\n{c}"
    );
    assert!(
        c.contains("\"%llu\\n\""),
        "expected `%llu` for uint64, got:\n{c}"
    );
    assert!(
        c.contains("\"%u\\n\""),
        "expected `%u` for unsigned widths up to 32, got:\n{c}"
    );
}

/// `as` casts lower to a C cast `(T)operand`. The cast binds tighter than a
/// binary `+`, so `a + b as int64` emits `(a + (int64_t)b)`.
#[test]
fn cast_expr_lowers_to_c_cast() {
    let src = "\
main() {
    let int64 a = 1;
    let uint8 b = a as uint8;
    let int64 c = a + b as int64;
    print(\"{}\", b);
    print(\"{}\", c);
}
";
    let c = emit(src);
    assert!(
        c.contains("(uint8_t)a"),
        "expected `(uint8_t)a` cast, got:\n{c}"
    );
    // cast binds tighter than `+`: the cast wraps `b`, inside the binary.
    assert!(
        c.contains("(a + (int64_t)b)"),
        "expected cast to bind tighter than `+`, got:\n{c}"
    );
}

/// A `union` decl lowers to `typedef union`, and a union member's type drives
/// its print specifier the same as a struct field (proves the field-type
/// lookup spans unions, not just structs).
#[test]
fn union_def_lowers_to_typedef_union_with_typed_members() {
    let src = "\
union Bits {
    int64 i,
    float64 f,
};

main() {
    mut Bits b = Bits { i: 42 };
    mut Bits g = Bits { f: 3.5 };
    print(\"{}\", b.i);
    print(\"{}\", g.f);
}
";
    let c = emit(src);
    assert!(
        c.contains("typedef union {"),
        "expected `typedef union`, got:\n{c}"
    );
    // one-member designated init, and per-member specifiers resolved.
    assert!(c.contains(".i = 42"), "expected designated union init, got:\n{c}");
    assert!(c.contains("%lld"), "int64 member should print %lld, got:\n{c}");
    assert!(c.contains("%f"), "float64 member should print %f, got:\n{c}");
}

/// An `extern` block lowers to bare C prototypes (no body), with `ptr` mapping
/// to `void*`. The prototypes precede `main` so call sites always see them.
#[test]
fn extern_block_lowers_to_prototypes_with_ptr_as_void_star() {
    let src = "\
extern {
    malloc(uint64 size) -> ptr;
    free(ptr p);
}

main() {
    mut ptr p = malloc(8);
    free(p);
}
";
    let c = emit(src);
    assert!(
        c.contains("void *malloc(uint64_t);") || c.contains("void* malloc(uint64_t);"),
        "expected a malloc prototype with void* return, got:\n{c}"
    );
    assert!(
        c.contains("void free(void *);") || c.contains("void free(void*);"),
        "expected a free prototype taking void*, got:\n{c}"
    );
    // prototype precedes the definition that calls it.
    let proto = c.find("malloc(uint64_t)").expect("prototype present");
    let call = c.find("malloc(8)").expect("call present");
    assert!(proto < call, "prototype must precede the call site:\n{c}");
}

/// Statement-position match lowers to a bare `switch` with one `case` per
/// variant and no hoisted temp - the arm bodies run for effect only.
#[test]
fn statement_position_match_emits_switch_without_temp() {
    let src = "\
enum Color = Red | Green;

main() {
    let Color c = Red;
    match c {
        Red -> print(\"r\"),
        Green -> print(\"g\"),
    };
}
";
    let c = emit(src);
    assert!(
        c.contains("switch (c) {"),
        "expected `switch (c) {{` in output, got:\n{c}"
    );
    assert!(
        c.contains("case Red:") && c.contains("case Green:"),
        "expected a `case` per variant, got:\n{c}"
    );
    assert!(
        !c.contains("_match"),
        "statement-position match must not hoist a temp, got:\n{c}"
    );
}

/// Value-position match into a `let` hoists `int32_t _match0;`, fills it
/// with an assigning `switch`, then the `let` reads the temp - in that
/// order.
#[test]
fn value_position_match_hoists_temp_then_reads_it() {
    let src = "\
enum Color = Red | Green;

main() {
    let Color c = Red;
    let int32 n = match c {
        Red -> 1,
        Green -> 2,
    };
    print(\"{}\", n);
}
";
    let c = emit(src);

    assert!(
        c.contains("int32_t _match0;"),
        "expected temp declaration, got:\n{c}"
    );
    assert!(
        c.contains("_match0 = 1;"),
        "expected assigning arm, got:\n{c}"
    );
    assert!(
        c.contains("const int32_t n = _match0;"),
        "expected `let` to read the temp, got:\n{c}"
    );

    let decl = c.find("int32_t _match0;").unwrap();
    let assign = c.find("_match0 = 1;").unwrap();
    let read = c.find("const int32_t n = _match0;").unwrap();
    assert!(
        decl < assign && assign < read,
        "expected decl -> switch assign -> read order, got:\n{c}"
    );
}

/// A wildcard arm lowers to `default:`; the explicit variant becomes a
/// `case`, and the variants the wildcard subsumes do not get their own
/// `case` labels.
#[test]
fn wildcard_arm_emits_default() {
    let src = "\
enum Color = Red | Green | Blue;

main() {
    let Color c = Red;
    let int32 n = match c {
        Red -> 1,
        _ -> 0,
    };
    print(\"{}\", n);
}
";
    let c = emit(src);
    assert!(
        c.contains("case Red:"),
        "expected `case Red:` in output, got:\n{c}"
    );
    assert!(
        c.contains("default:"),
        "expected wildcard to lower to `default:`, got:\n{c}"
    );
    assert!(
        !c.contains("case Green:") && !c.contains("case Blue:"),
        "variants covered by the wildcard must not get their own case, got:\n{c}"
    );
}

/// Two value-position matches in one function get distinct temps:
/// `_match0` then `_match1`. Pins the per-statement counter increment.
#[test]
fn two_matches_in_one_function_use_distinct_temps() {
    let src = "\
enum Color = Red | Green;

main() {
    let Color c = Red;
    let int32 x = match c {
        Red -> 1,
        Green -> 2,
    };
    let int32 y = match c {
        Red -> 3,
        Green -> 4,
    };
    print(\"{}\", x);
    print(\"{}\", y);
}
";
    let c = emit(src);
    assert!(
        c.contains("int32_t _match0;") && c.contains("int32_t _match1;"),
        "expected both `_match0` and `_match1` temps, got:\n{c}"
    );
}

/// The match temp counter resets at each function boundary, so a match in
/// one function and a match in the next both name their temp `_match0`.
/// Pins `gen_function`'s per-function reset.
#[test]
fn match_counter_resets_per_function() {
    let src = "\
enum Color = Red | Green;

helper() {
    let Color c = Red;
    let int32 x = match c {
        Red -> 1,
        Green -> 2,
    };
    print(\"{}\", x);
}

main() {
    let Color c = Green;
    let int32 y = match c {
        Red -> 3,
        Green -> 4,
    };
    print(\"{}\", y);
}
";
    let c = emit(src);
    assert!(
        c.contains("int32_t _match0;"),
        "expected `_match0` temp, got:\n{c}"
    );
    assert!(
        !c.contains("_match1"),
        "counter must reset per function, so `_match1` must not appear, got:\n{c}"
    );
}
