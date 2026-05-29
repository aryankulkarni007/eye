use super::*;
use ast::{AstNode, SourceFile};
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};
use smallvec::SmallVec;
use thin_vec::thin_vec;

fn emit(src: &str) -> String {
    let source = SourceText::new(src.to_string());
    let tokens = Lexer::new(&source).tokenize().tokens;
    let parse = parser::parse(&tokens, &source);
    assert!(
        parse.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        parse.diagnostics
    );
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
        params: SmallVec::new(),
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
        args: thin_vec![a1, a2, a3],
    });
    let call_stmt = body.stmts.alloc(Stmt::Expr(call));
    body.block.push(call_stmt);
    let body_id = hir.bodies.alloc(body);

    let main_fn = hir.functions.alloc(Function {
        name: "main".into(),
        params: SmallVec::new(),
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

/// A literal `%` in the format string must be escaped to `%%` so printf does
/// not read it as a conversion spec. The `{}`-driven specs stay single-`%`.
#[test]
fn print_escapes_literal_percent() {
    let src = "\
main() {
    let int32 done = 50;
    print(\"{}% done\", done);
}
";
    let c = emit(src);
    assert!(
        c.contains("\"%d%% done\\n\""),
        "expected literal `%` escaped to `%%` with single-`%` spec, got:\n{c}"
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

/// Pointer-width integers map to the platform-defined libc types `size_t` /
/// `ptrdiff_t` (the FFI seam: malloc, sizeof, indexing) and pick the C99
/// length-modified printf specifiers `%zu` / `%td`.
#[test]
fn pointer_width_integers_map_to_size_types_and_specifiers() {
    let src = "\
main() {
    let usize a = 1;
    let isize b = 2;
    print(\"{}\", a);
    print(\"{}\", b);
}
";
    let c = emit(src);

    assert!(c.contains("size_t a"), "expected `size_t a`, got:\n{c}");
    assert!(
        c.contains("ptrdiff_t b"),
        "expected `ptrdiff_t b`, got:\n{c}"
    );
    assert!(
        c.contains("#include <stddef.h>"),
        "expected `<stddef.h>` include for size_t/ptrdiff_t, got:\n{c}"
    );
    assert!(
        c.contains("\"%zu\\n\""),
        "expected `%zu` for usize, got:\n{c}"
    );
    assert!(
        c.contains("\"%td\\n\""),
        "expected `%td` for isize, got:\n{c}"
    );
}

/// `else if` lowers flat: a chain emits C `} else if (` rather than nesting
/// `else { if ... }` braces (the parser desugars to the nested form; codegen
/// flattens it back). An `if` is never a ternary - a value-position chain is
/// hoisted into an `_ifN` temp, each branch assigning the temp, exactly like a
/// value-position `match`.
#[test]
fn else_if_chain_lowers_flat_statement_and_hoisted_value() {
    let src = "\
pick(int32 n) {
    if n < 0 {
        print(\"neg\");
    } else if n == 0 {
        print(\"zero\");
    } else {
        print(\"pos\");
    }
}

main() {
    let int32 g = if 1 == 0 { 10 } else if 1 == 1 { 20 } else { 30 };
    print(\"{}\", g);
}
";
    let c = emit(src);

    // Both chains flatten: the statement chain in `pick` and the hoisted value
    // chain in `main` each emit one flat `else if`, never a nested `else { if }`.
    assert!(
        c.contains("} else if ("),
        "expected flat `else if`, got:\n{c}"
    );
    assert_eq!(
        c.matches("else if (").count(),
        2,
        "expected two flattened `else if` arms, got:\n{c}"
    );
    // No ternary is ever emitted for `if`.
    assert!(!c.contains('?'), "expected no ternary, got:\n{c}");
    // The value-position chain is hoisted into an `_if0` temp declared ahead of
    // the `let`, each branch assigns it, and the `let` reads it back.
    assert!(
        c.contains("int32_t _if0;"),
        "expected hoisted `_if0` temp, got:\n{c}"
    );
    assert!(
        c.contains("_if0 = 10;") && c.contains("_if0 = 20;") && c.contains("_if0 = 30;"),
        "expected each branch to assign the temp, got:\n{c}"
    );
    assert!(
        c.contains("const int32_t g = _if0;"),
        "expected the `let` to read the hoisted temp, got:\n{c}"
    );
}

#[test]
fn _scratch_dump() {
    let src = "\
main() {
    let int32 x = if 1 == 1 { if 2 == 2 { 10 } else { 20 } } else { 30 };
    print(\"{}\", x);
}
";
    std::fs::write("/tmp/eye_nested.c", emit(src)).unwrap();
}

/// Regression: a statement-position `if`/`else if` with no final `else` whose
/// branch bodies are tail expressions (assignments written without a trailing
/// `;`) once miscompiled into a broken `(cond ? a : if (...) { ... })` - a
/// ternary whose else operand was an `if` statement. Statement position must
/// always emit control flow: each tail assignment becomes a plain statement and
/// the chain stays flat.
#[test]
fn elseless_statement_if_chain_with_tail_assignments_is_not_a_ternary() {
    let src = "\
enum Coin = Head | Tail;

main() {
    mut int32 d = 0;
    mut Coin coin = Head;
    if d == 0 {
        coin = Coin.Tail
    } else if d == 1 {
        coin = Coin.Head
    }
}
";
    let c = emit(src);

    // No ternary, and crucially no `if` smuggled into a `:` else operand.
    assert!(!c.contains('?'), "expected no ternary, got:\n{c}");
    // The chain is a flat statement: each branch assigns as a statement.
    assert!(
        c.contains("coin = Tail;") && c.contains("coin = Head;"),
        "expected branch assignments as statements, got:\n{c}"
    );
    assert!(
        c.contains("} else if ("),
        "expected a flat `else if`, got:\n{c}"
    );
}

/// Fixed-size arrays are value types via struct-wrap: `[T; N]` renders as its
/// wrapper typedef, `[...]` is a compound literal of that wrapper, indexing
/// reaches through `.data[i]` (rvalue and lvalue), and `mut` drops the `const`.
#[test]
fn fixed_array_decl_literal_and_index() {
    let src = "\
main() {
    let [int32; 3] xs = [10, 20, 30];
    mut [int32; 2] ys = [1, 2];
    ys[0] = xs[2];
    print(\"{}\", xs[1]);
}
";
    let c = emit(src);

    // An array is a value: it renders as its struct-wrap typedef, and a literal
    // is a compound literal of that wrapper.
    assert!(
        c.contains("typedef struct { int32_t data[3]; } __eye_arr_3_5int32;"),
        "expected wrapper typedef, got:\n{c}"
    );
    assert!(
        c.contains("const __eye_arr_3_5int32 xs = (__eye_arr_3_5int32){{ 10, 20, 30 }}"),
        "expected wrapped value initializer, got:\n{c}"
    );
    // `mut` array is non-const and mutable.
    assert!(
        c.contains("__eye_arr_2_5int32 ys = (__eye_arr_2_5int32){{ 1, 2 }}")
            && !c.contains("const __eye_arr_2_5int32 ys"),
        "expected non-const mut array, got:\n{c}"
    );
    // index reaches through the wrapper's `data` field, lvalue and rvalue.
    assert!(
        c.contains("ys.data[0] = xs.data[2]"),
        "expected index assignment through .data, got:\n{c}"
    );
    // element type drives the print specifier through the index.
    assert!(
        c.contains("printf(\"%d\\n\", xs.data[1])"),
        "expected %d for int32 element, got:\n{c}"
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
    assert!(
        c.contains(".i = 42"),
        "expected designated union init, got:\n{c}"
    );
    assert!(
        c.contains("%lld"),
        "int64 member should print %lld, got:\n{c}"
    );
    assert!(
        c.contains("%f"),
        "float64 member should print %f, got:\n{c}"
    );
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

/// An explicit binding type drives the hoist-temp declaration: a wider
/// `int64` binding over int-literal arms (typed `int32`) declares an
/// `int64_t` temp, not `int32_t`. Confirms the HIR result-type override
/// reaches codegen.
#[test]
fn value_position_match_uses_binding_type_for_temp() {
    let src = "\
enum Color = Red | Green;

main() {
    let Color c = Red;
    let int64 n = match c {
        Red -> 1,
        Green -> 2,
    };
    print(\"{}\", n);
}
";
    let c = emit(src);
    assert!(
        c.contains("int64_t _match0;"),
        "expected `int64_t` temp from the binding type, got:\n{c}"
    );
    assert!(
        !c.contains("int32_t _match0;"),
        "temp must not fall back to the first arm's int32, got:\n{c}"
    );
    assert!(
        c.contains("const int64_t n = _match0;"),
        "expected `let` to read the temp, got:\n{c}"
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

/// A value-position match as a function's implicit-return tail must be hoisted
/// into a temp and then returned (`return _match0;`), not emitted as an
/// unhoisted placeholder. Regression for the `/* UNHOISTED MATCH */` bug.
#[test]
fn return_tail_match_hoists_and_returns_temp() {
    let src = "\
enum Color = Red | Green;

sides(Color c) -> int32 {
    match c {
        Red -> 10,
        Green -> 20,
    }
}

main() {
    print(\"{}\", sides(Red));
}
";
    let c = emit(src);
    assert!(
        !c.contains("UNHOISTED"),
        "return-tail match must be hoisted, got:\n{c}"
    );
    assert!(
        c.contains("int32_t _match0;"),
        "expected hoisted temp for the return-tail match, got:\n{c}"
    );
    assert!(
        c.contains("return _match0;"),
        "expected the hoisted temp to be returned, got:\n{c}"
    );
    let decl = c.find("int32_t _match0;").unwrap();
    let ret = c.find("return _match0;").unwrap();
    assert!(
        decl < ret,
        "temp must be declared before the return, got:\n{c}"
    );
}

/// The declared return type drives the return-tail match's hoist temp: an
/// `int64` return over int-literal arms (typed `int32`) declares `int64_t`,
/// confirming `enforce_fn_return_type` re-records the return type onto the
/// match and that reaches codegen.
#[test]
fn return_tail_match_uses_return_type_for_temp() {
    let src = "\
enum Color = Red | Green;

big(Color c) -> int64 {
    match c {
        Red -> 1,
        Green -> 2,
    }
}

main() {
    print(\"{}\", big(Red));
}
";
    let c = emit(src);
    assert!(
        c.contains("int64_t _match0;"),
        "expected `int64_t` temp from the return type, got:\n{c}"
    );
    assert!(
        !c.contains("int32_t _match0;"),
        "temp must not fall back to the first arm's int32, got:\n{c}"
    );
}

/// O1 modulo lowers to native C `%`.
#[test]
fn modulo_operator_emits_native_c_rem() {
    let c = emit("main() {\n    let int32 r = 17 % 5;\n    print(\"{}\", r);\n}\n");
    assert!(c.contains("17 % 5"), "expected `17 % 5`, got:\n{c}");
}

/// O3 bitwise binary operators each pass straight through to native C.
#[test]
fn bitwise_operators_emit_native_c() {
    let c = emit(
        "main() {\n    let int32 a = 12;\n    let int32 b = 10;\n    \
         print(\"{}\", a & b);\n    print(\"{}\", a | b);\n    print(\"{}\", a ^ b);\n    \
         print(\"{}\", a << 2);\n    print(\"{}\", a >> 1);\n}\n",
    );
    for frag in ["a & b", "a | b", "a ^ b", "a << 2", "a >> 1"] {
        assert!(c.contains(frag), "expected `{frag}` in output, got:\n{c}");
    }
}

/// O2 prefix `~` (complement) and `!` (logical-not) emit native C prefixes.
#[test]
fn prefix_complement_and_not_emit_native_c() {
    let c = emit(
        "main() {\n    let int32 a = 12;\n    let bool f = false;\n    \
         print(\"{}\", ~a);\n    print(\"{}\", !f);\n}\n",
    );
    assert!(c.contains("~a"), "expected `~a`, got:\n{c}");
    assert!(c.contains("!f"), "expected `!f`, got:\n{c}");
}

/// `!` types `bool`: a fn declared `-> bool` whose tail is `!n` must pass the
/// return-type check, so `emit` (which asserts no HIR diagnostics) succeeding
/// is the proof. `==` (also bool) is checked alongside as a control.
#[test]
fn logical_not_and_neq_type_bool() {
    let c =
        emit("not(int32 n) -> bool { !n }\nne(int32 a, int32 b) -> bool { a != b }\nmain() {}\n");
    assert!(c.contains("!n"), "expected `!n`, got:\n{c}");
    assert!(c.contains("a != b"), "expected `a != b`, got:\n{c}");
}

/// O4 compound assignment emits the native C compound operator, not a desugar.
#[test]
fn compound_assignment_emits_native_c() {
    let c = emit(
        "main() {\n    mut int32 c = 100;\n    c += 5;\n    c -= 20;\n    print(\"{}\", c);\n}\n",
    );
    assert!(c.contains("c += 5"), "expected `c += 5`, got:\n{c}");
    assert!(c.contains("c -= 20"), "expected `c -= 20`, got:\n{c}");
}

/// A parenthesized group overrides precedence: codegen brackets each binary by
/// its parse structure, so `a * (b + c)` nests the add inside the multiply,
/// whereas the unparenthesized `a * b + c` does not.
#[test]
fn paren_group_overrides_precedence_in_emission() {
    let grouped = emit(
        "main() {\n    let int32 a = 2;\n    let int32 b = 3;\n    let int32 c = 4;\n    \
         print(\"{}\", a * (b + c));\n}\n",
    );
    assert!(
        grouped.contains("(a * (b + c))"),
        "expected the add nested inside the multiply, got:\n{grouped}"
    );
}

// --- v0.7 arrays first-class + latent gaps ---

/// A1: a function returns a fixed array by value - the wrapper struct is the
/// return type and the body `return`s it as a value (no decayed pointer, no
/// dangling-stack UB).
#[test]
fn array_returns_by_value() {
    let c = emit(
        "mkarr() -> [int32; 3] {\n    let [int32; 3] a = [7, 8, 9];\n    a\n}\n\
         main() {\n    let [int32; 3] r = mkarr();\n    print(\"{}\", r[0]);\n}\n",
    );
    assert!(
        c.contains("__eye_arr_3_5int32 mkarr("),
        "expected wrapper return type, got:\n{c}"
    );
    assert!(
        c.contains("return a;"),
        "expected a by-value array return, got:\n{c}"
    );
}

/// A3: the `len(xs)` intrinsic emits the compile-time length constant, with no
/// `len(` call and no `.data` surviving to C.
#[test]
fn array_len_emits_constant() {
    let c = emit(
        "main() {\n    let [int32; 5] xs = [1, 2, 3, 4, 5];\n    print(\"{}\", len(xs));\n}\n",
    );
    assert!(
        c.contains("printf(\"%zu\\n\", (size_t)5)"),
        "expected the length to emit as a size_t-typed constant 5, got:\n{c}"
    );
    assert!(
        !c.contains("len("),
        "`len(xs)` must fold away, not survive as a call, got:\n{c}"
    );
}

/// A2: indexing a reference to an array reaches through `->data[i]` (the
/// reference is a pointer to the wrapper struct), while a value array uses
/// `.data[i]`.
#[test]
fn ref_to_array_indexes_through_arrow() {
    let c = emit(
        "bump(&[int32; 3] r) {\n    r[0] = 42;\n}\n\
         main() {\n    mut [int32; 3] a = [1, 2, 3];\n    bump(&a);\n    print(\"{}\", a[0]);\n}\n",
    );
    assert!(
        c.contains("__eye_arr_3_5int32* r"),
        "expected a pointer-to-wrapper parameter, got:\n{c}"
    );
    assert!(
        c.contains("r->data[0] = 42"),
        "expected index through the reference, got:\n{c}"
    );
    assert!(
        c.contains("a.data[0]"),
        "expected value-array index through .data, got:\n{c}"
    );
}

/// L3: a multibyte UTF-8 character in a format string is preserved byte-for-byte
/// (not re-encoded per byte as Latin-1).
#[test]
fn print_preserves_utf8() {
    let c = emit("main() {\n    print(\"café {}\", 1);\n}\n");
    assert!(
        c.contains("\"café %d\\n\""),
        "expected the UTF-8 to survive intact, got:\n{c}"
    );
}

/// An array literal in argument position is coerced to the parameter's array
/// type. Without it, the literal's int32-default elements would produce a
/// wrapper type that mismatches a `[usize; N]` parameter (a C type error). The
/// `emit` helper also asserts no HIR diagnostics, so this guards both ends.
#[test]
fn array_literal_arg_coerced_to_param_type() {
    let c = emit(
        "f([usize; 2] xs) -> usize {\n    xs[0]\n}\n\
         main() {\n    print(\"{}\", f([10, 20]));\n}\n",
    );
    assert!(
        c.contains("f((__eye_arr_2_5usize){{ 10, 20 }})"),
        "expected the literal arg coerced to the usize wrapper, got:\n{c}"
    );
}

/// An array literal in return position is coerced to the declared array return
/// type (element type), so no spurious return-type mismatch and a matching
/// wrapper.
#[test]
fn array_literal_return_coerced_to_ret_type() {
    let c =
        emit("g() -> [usize; 3] {\n    [1, 2, 3]\n}\nmain() {\n    let [usize; 3] r = g();\n}\n");
    assert!(
        c.contains("return (__eye_arr_3_5usize){{ 1, 2, 3 }};"),
        "expected the literal return coerced to the usize wrapper, got:\n{c}"
    );
}
