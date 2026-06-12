//! End-to-end driver tests. Each test invokes the built `eye` binary on a
//! `.eye` source file, then runs the resulting native binary and inspects
//! its stdout. These tests cement the externally visible v0.1 behaviour:
//! the public surface is "I hand you a `.eye` file and the program runs".

use std::process::Command;

mod common;

/// The canonical v0.1 program. Captures every node kind the language ships
/// with: struct def, fn def, typed and inferred lets, struct literal with
/// shorthand, field access, `print` lowering.
#[test]
fn main_eye_compiles_runs_and_prints_expected_output() {
    let source = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    let int32 x = 0;
    let int32 y = 0;
    mut Point p = Point { x, y };

    println(\"{}\", p.x);
    println(\"{}\", p.y);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "0\n0\n",
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Exercises the binop and prefix operator codegen path end-to-end.
#[test]
fn arithmetic_expression_evaluates_correctly() {
    let source = "\
main() {
    let int32 x = -1 + 2 * 3;
    println(\"{}\", x);
}
";
    let (out, _) = common::run_program(source);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5\n");
}

/// Exercises every primitive `print` format specifier plus reference-to-struct
/// (`%p`). Source lives in `eyesrc/lang/print.eye` so the file stays authoritative.
#[test]
fn print_eye_covers_every_format_specifier() {
    let source = include_str!("../eyesrc/lang/print.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    assert_eq!(
        lines.len(),
        9,
        "unexpected line count; full stdout:\n{stdout}"
    );
    assert_eq!(lines[0], "int32      i = 42");
    assert_eq!(lines[1], "float32    f32 = 1.500000");
    assert_eq!(lines[2], "float64    f64 = 3.141590");
    assert_eq!(lines[3], "bool       t = 1  f = 0");
    assert_eq!(lines[4], "char       c = A");
    assert_eq!(lines[5], "string     s = hello");
    // pointer address is non-deterministic; only assert the prefix + `0x` form.
    assert!(
        lines[6].starts_with("&Box       r = 0x"),
        "expected pointer print, got: {}",
        lines[6]
    );
    assert_eq!(lines[7], "mixed      i=42 f64=3.141590 c=A s=world bool=1");
    assert_eq!(lines[8], "literals   100 2.710000 Z lit 0");
}

/// v0.3 end-to-end: enum decls, a statement-position match (printed for
/// effect) and two value-position matches (one exhaustive, one with a
/// wildcard) returning into typed `let`s. Source lives in `eyesrc/lang/enums.eye`
/// so the file stays authoritative. Locks the externally visible v0.3
/// behaviour.
#[test]
fn v03_eye_lowers_match_and_prints_expected_output() {
    let source = include_str!("../eyesrc/lang/enums.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "0\n1\nboxy\n4\n0\n",
        "unexpected v0.3 stdout"
    );
}

/// Horizon 0 / Component 4 (S1): primitive-domain match - int, char, and bool
/// scrutinees with literal arms, in both value and statement position. Source
/// lives in `eyesrc/lang/match_prim.eye`. Locks the literal `ArmTest::Const` lowering.
#[test]
fn match_primitive_domains_run() {
    let source = include_str!("../eyesrc/lang/match_prim.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "grade1 A\n\
         grade3 C\n\
         gradeX F\n\
         flipT  0\n\
         flipF  1\n\
         vowelA 1\n\
         vowelZ 0\n\
         two\n",
        "unexpected primitive-match stdout"
    );
}

/// NOTE: EXPERIMENTAL - Match arm guards: `A if flag -> body`. Verifies that
/// a simple bool-variable guard is ANDed with the variant test at codegen time
/// and short-circuits correctly.
#[test]
fn match_guard_runs() {
    let source = "\
enum E = A | B ;
main() {
    mut bool flag = true;
    let E e = A;
    let int32 r1 = match e {
        A if flag -> 1,
        B -> 2,
        _ -> 3,
    };
    println(\"{}\", r1);
    flag = false;
    let int32 r2 = match e {
        A if flag -> 4,
        B -> 5,
        _ -> 6,
    };
    println(\"{}\", r2);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1\n6\n",
        "unexpected guard-match stdout"
    );
}

/// Horizon 0 / Component 4 (S2): `let` struct destructuring - shorthand, rename,
/// call-result init (spilled to a temp), and a nested struct value. Source lives
/// in `eyesrc/lang/destructure.eye`.
#[test]
fn struct_destructure_let_runs() {
    let source = include_str!("../eyesrc/lang/destructure.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "x=10 y=20\n\
         px=10 py=20\n\
         cx=3 cy=4\n\
         a.x=1 b.y=6\n",
        "unexpected destructure stdout"
    );
}

/// v0.4 end-to-end: every sized/unsigned integer type compiles under clang
/// and prints its value with the correct printf specifier (catches a `%lld` /
/// `%llu` width mismatch that would only surface at C-compile or run time).
#[test]
fn sized_integer_types_compile_and_println() {
    let source = "\
main() {
    let int8 a = 1;
    let int16 b = 2;
    let int64 c = 3;
    let uint8 d = 4;
    let uint16 e = 5;
    let uint32 f = 6;
    let uint64 g = 7;
    println(\"{}\", a);
    println(\"{}\", b);
    println(\"{}\", c);
    println(\"{}\", d);
    println(\"{}\", e);
    println(\"{}\", f);
    println(\"{}\", g);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "1\n2\n3\n4\n5\n6\n7\n",
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.4 end-to-end: `as` casts compile under clang and carry C cast
/// semantics. `300 as uint8` truncates to `44`; an int promoted to float
/// divides as floating point.
#[test]
fn cast_expr_compiles_and_truncates() {
    let source = "\
main() {
    let int32 big = 300;
    let uint8 small = big as uint8;
    let int32 n = 7;
    let float64 half = n as float64 / 2.0;
    println(\"{}\", small);
    println(\"{}\", half);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "44\n3.500000\n",
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// v0.4 end-to-end: the canonical v0.4 showcase. Every sized/unsigned integer
/// primitive plus the `as` cast paths (truncation, int->float promotion, tight
/// binding in a widening add, and a widen/narrow roundtrip). Source lives in
/// `eyesrc/lang/integers.eye` so the file stays authoritative.
#[test]
fn v04_eye_lowers_primitives_and_casts() {
    let source = include_str!("../eyesrc/lang/integers.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "signed     1 2 3 4\n\
         unsigned   5 6 7 8\n\
         truncate   44\n\
         promote    3.500000\n\
         widen-add  30\n\
         roundtrip  5\n",
        "unexpected v0.4 stdout"
    );
}

/// v0.4 end-to-end: the FFI + union substrate. An `extern` block binds libc
/// `malloc`/`free` (resolved at link), `ptr` is the opaque untyped pointer
/// bridged to `Point*` via `as`, and a `union` gives overlapping storage whose
/// members print with their own specifiers. Source lives in `eyesrc/ffi/ffi.eye`.
#[test]
fn ffi_eye_links_libc_and_lowers_union() {
    let source = include_str!("../eyesrc/ffi/ffi.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "union i = 42\nunion f = 3.500000\nfreed\n",
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--dump-ast` is the user-facing typed-AST smoke test. Keep it aligned with
/// the current syntax surface, including params, returns, externs, unions,
/// arrays, casts, indexing, assignment, control flow, refs/derefs, and match.
#[test]
fn dump_ast_covers_current_surface_without_opaque_placeholders() {
    let dir = common::fixture_dir();
    let src_path = common::write_source(
        &dir,
        "dump.eye",
        "\
structure Point {
    int32 x,
    int32 y,
};

union Bits {
    int32 i,
    float32 f,
};

enum Shape = Circle | Square | Other;

extern {
    println(string fmt, int32 value);
}

id(int32 value) -> int32 {
    value
}

main() {
    let Point p = Point { x: 1, y: 2 };
    mut [int32; 3] xs = [10, 20, 30];
    let int32 sides = match Square {
        Shape.Circle -> 1,
        Square -> 2,
        _ -> 3,
    };
    xs[1] = id(sides) as int32;
    let &Point rp = &p;
    let int32 y = p.y;
    let &int32 ry = &y;
    let int32 z = *ry;
    loop {
        if z > 0 {
            break;
        } else {
            continue;
        }
    };
}
",
    );

    let out = Command::new(common::DRIVER)
        .arg(&src_path)
        .arg("--dump-ast")
        .output()
        .expect("invoke driver");
    assert!(
        out.status.success(),
        "driver failed: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "structure Point",
        "union Bits",
        "enum Shape",
        "extern fn println(string fmt, int32 value) -> void",
        "fn id(int32 value) -> int32",
        "tail name value",
        "fn main() -> void",
        "let Point p = Point { x: Int(1), y: Int(2) }",
        "mut [int32; Int(3)] xs = [Int(10), Int(20), Int(30)]",
        "let int32 sides = match name Square { Shape.Circle -> Int(1), Square -> Int(2), _ -> Int(3) }",
        "expr (index name xs[Int(1)] = (name id(name sides) as int32))",
        "let &Point rp = &name p",
        "let int32 y = name p.y",
        "let &int32 ry = &name y",
        "let int32 z = *name ry",
        "expr loop { 0 stmt(s); tail if (name z Gt Int(0)) { 1 stmt(s); tail <none> } else { 1 stmt(s); tail <none> } }",
    ] {
        assert!(
            stdout.contains(expected),
            "missing AST dump fragment `{expected}` in:\n{stdout}"
        );
    }

    for stale in [
        "<assign>",
        "<if>",
        "<loop>",
        "<break>",
        "<continue>",
        "<ref>",
        "<deref>",
        "<match>",
    ] {
        assert!(
            !stdout.contains(stale),
            "stale placeholder `{stale}` remained in AST dump:\n{stdout}"
        );
    }
}

/// v0.6 end-to-end: operator completeness - modulo, bitwise binary, prefix
/// complement/not, compound assignment. Source lives in `eyesrc/lang/operators.eye`
/// so the file stays authoritative. Locks the externally visible v0.6 behaviour.
#[test]
fn v06_eye_runs_operators_and_prints_expected_output() {
    let source = include_str!("../eyesrc/lang/operators.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        10,
        "unexpected line count; full stdout:\n{stdout}"
    );
    assert_eq!(lines[0], "mod        2");
    assert_eq!(lines[1], "bitand     8");
    assert_eq!(lines[2], "bitor      14");
    assert_eq!(lines[3], "bitxor     6");
    assert_eq!(lines[4], "shl        48");
    assert_eq!(lines[5], "shr        6");
    assert_eq!(lines[6], "bitnot     -13");
    assert_eq!(lines[7], "lognot     1");
    assert_eq!(lines[8], "compound   85");
    assert_eq!(lines[9], "grouped    14");
}

/// Track 2 vertical slice: a straight-line program lowered HIR -> MIR -> C.
/// Locks the Segment 1 seam (Let/Binary/Call/Literal); an output assertion, not
/// a C-text one (codegen makes no decisions, so output is the oracle - R1).
#[test]
fn mir_path_compiles_and_runs_straight_line_slice() {
    let (out, _) =
        common::run_program("main() {\n    let int32 x = 1 + 2;\n    println(\"{}\", x);\n}\n");
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n");
}

/// Track 2 Segment 2: statement-position control flow. Exercises `loop`,
/// statement `if`, `break`, `match` -> tag-dispatch, `Assign`, enum values, and
/// a value-returning function (the `add` helper, compiled but not called - locks
/// `Return` emission). The `match` arm `Stop -> break` proves the break targets
/// the enclosing loop: the emitter renders the match as an `if`/`else if` chain,
/// not a C `switch` (which would capture the `break` for the switch and loop
/// forever). An output assertion (R1).
#[test]
fn mir_path_lowers_loop_match_break_and_value_return() {
    let (out, _) = common::run_program(
        "enum Sig = Stop | Go;\n\
         \n\
         add(int32 a, int32 b) -> int32 { a + b }\n\
         \n\
         main() {\n\
         \x20   mut int32 i = 0;\n\
         \x20   mut Sig s = Go;\n\
         \x20   loop {\n\
         \x20       match s {\n\
         \x20           Stop -> break,\n\
         \x20           Go -> println(\"{}\", i),\n\
         \x20       };\n\
         \x20       i = i + 1;\n\
         \x20       if i >= 3 { s = Stop; }\n\
         \x20   }\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n1\n2\n");
}

/// Track 2 Segment 2: the control-flow branches the break test above does not
/// reach - `if`/`else`, `continue`, a `match` wildcard arm (`default`), and
/// compound assignment (`+=`, `-=`). Output assertion (R1).
#[test]
fn mir_path_lowers_else_continue_default_and_compound_assign() {
    let (out, _) = common::run_program(
        "enum Tag = A | B;\n\
         \n\
         main() {\n\
         \x20   mut int32 n = 0;\n\
         \x20   loop {\n\
         \x20       n += 1;\n\
         \x20       if n >= 4 { break; }\n\
         \x20       if n == 2 {\n\
         \x20           continue;\n\
         \x20       } else {\n\
         \x20           let Tag t = B;\n\
         \x20           match t {\n\
         \x20               A -> println(\"a\"),\n\
         \x20               _ -> println(\"b\"),\n\
         \x20           };\n\
         \x20       }\n\
         \x20       n -= 0;\n\
         \x20       println(\"{}\", n);\n\
         \x20   }\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "b\n1\nb\n3\n");
}

/// Track 2 Segment 3, the REDESIGN I3 acid test. A value-position `match` nested
/// inside the `then` branch of a value-position `if`, bound to a `let`. Lowering
/// emits the match *in place* against the if-temp, assigning it per arm inside
/// the branch - the shape the deleted `TernaryMatch` ban once rejected (hoisting
/// the match out of the branch would run it even when the condition is false).
/// Also exercises a general `Call` (`sides(shape)`) and a value-`match` as a
/// function-body tail (`sides`). Output assertion (R1); mirrors `eyesrc/programs/wierd.eye`.
#[test]
fn mir_path_acid_test_value_match_nested_in_if_branch() {
    let (out, _) = common::run_program(
        "enum Shape = Circle | Square | Triangle;\n\
         \n\
         sides(Shape s) -> int32 {\n\
         \x20   match s {\n\
         \x20       Circle -> 0,\n\
         \x20       Square -> 4,\n\
         \x20       Triangle -> 3,\n\
         \x20   }\n\
         }\n\
         \n\
         main() {\n\
         \x20   let Shape shape = Square;\n\
         \x20   let int32 result =\n\
         \x20       if sides(shape) > 3 {\n\
         \x20           match shape {\n\
         \x20               Circle -> 100,\n\
         \x20               Square -> 200,\n\
         \x20               Triangle -> 300,\n\
         \x20           }\n\
         \x20       } else {\n\
         \x20           0\n\
         \x20       };\n\
         \x20   println(\"{}\", result);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "200\n");
}

/// Driver should refuse non-`.eye` input rather than overwriting an
/// arbitrary file with generated C.
#[test]
fn driver_rejects_non_eye_extension() {
    let dir = common::fixture_dir();
    let bad = common::write_source(&dir, "prog.txt", "main() {}\n");

    let status = Command::new(common::DRIVER).arg(&bad).status().unwrap();
    assert!(!status.success(), "driver should have rejected non-.eye");
}

/// v0.7 end-to-end: fixed arrays as a first-class value type - lvalue index,
/// `len(x)`, value-copy independence, return-by-value, `&[T; N]` reference, and
/// multi-dimensional nesting. Source is `eyesrc/lang/arrays.eye`. Locks that the
/// struct-wrap representation behaves as real value semantics at runtime.
#[test]
fn arrays_eye_runs_value_semantics_and_prints_expected_output() {
    let source = include_str!("../eyesrc/lang/arrays.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        7,
        "unexpected line count; full stdout:\n{stdout}"
    );
    assert_eq!(lines[0], "idx        99");
    assert_eq!(lines[1], "len        4");
    // value copy: `a` is untouched by mutating its copy `b`.
    assert_eq!(lines[2], "copy       1 77");
    assert_eq!(lines[3], "return     30");
    assert_eq!(lines[4], "sumref     6");
    assert_eq!(lines[5], "grid       3");
    assert_eq!(lines[6], "usize      200");
}

/// Track 2 Segment 4, the REDESIGN I5 short-circuit proof. `&&`/`||` must lower
/// to control flow, not to an eager `RValue::Binary`: the right-hand operand
/// runs only when the left does not already decide the result. The operands
/// here have an observable side effect (each prints), so eager evaluation would
/// show up as extra output. `sidet() || sidef()` must print only `T` (the `||`
/// is already true, so `sidef` never runs); `sidef() && sidet()` must print only
/// `F` (the `&&` is already false, so `sidet` never runs). The regression would
/// be invisible without a side-effecting operand, which no corpus program had.
#[test]
fn mir_path_short_circuits_or_and_and_skipping_side_effects() {
    let (out, _) = common::run_program(
        "sidef() -> bool { println(\"F\"); false }\n\
         sidet() -> bool { println(\"T\"); true }\n\
         \n\
         main() {\n\
         \x20   let bool a = sidet() || sidef();\n\
         \x20   println(\"a={}\", a);\n\
         \x20   let bool b = sidef() && sidet();\n\
         \x20   println(\"b={}\", b);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // No stray `F` before `a=1` and no stray `T` before `b=0`: the
    // short-circuited operand was not evaluated.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "T\na=1\nF\nb=0\n");
}

/// Track 2 Segment 4: structs, references, and pointers. Source is
/// `eyesrc/programs/example.eye`, which exercises struct literals (shorthand and
/// explicit), field access via `.` and via a pointer (`d.x` -> `d->x`), a
/// `malloc(...) as Vec3*` cast of a call result, a deref lvalue (`*d = ...`) and
/// a deref operand (`print_vec(*d)`), plus arrays of field reads. Output oracle
/// (R1).
#[test]
fn mir_path_runs_struct_pointer_and_deref() {
    let (out, _) = common::run_program(include_str!("../eyesrc/programs/example.eye"));
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(2, 2, 2)\n(2, 2, 2)\n(6, 6)\n",
    );
}

/// Same-scope redeclaration is rejected (R015, ruled 2026-06-12):
/// `let x = 1; let x = 2;` in one block is an error, not shadowing.
/// Shadowing in a nested block scope stays legal (see
/// `nested_block_shadowing_is_legal`). This reverses the Track 2 Segment 4
/// pin (`mir_path_allows_same_block_shadowing`): the conservative reject can
/// be relaxed to a shadowing rule later; the reverse would break programs.
#[test]
fn same_scope_redeclaration_is_rejected() {
    let out = common::compile_expect_failure(
        "main() {\n\
         \x20   let int32 x = 1;\n\
         \x20   let int32 x = 2;\n\
         \x20   println(\"{}\", x);\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("already declared in this scope"),
        "expected a duplicate-local diagnostic, got:\n{rendered}"
    );
}

/// Shadowing in a *nested* block scope stays legal under R015: the inner
/// binding wins inside the block, the outer binding is visible again after.
/// The emitter suffixes every non-parameter local with its `LocalId`
/// (`x_0`, `x_1`), so the two declarations never collide in one C scope.
#[test]
fn nested_block_shadowing_is_legal() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   let int32 x = 1;\n\
         \x20   if true {\n\
         \x20       let int32 x = 2;\n\
         \x20       println(\"{}\", x);\n\
         \x20   }\n\
         \x20   println(\"{}\", x);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n1\n");
}

/// `{{`/`}}` print a literal brace in a `println` format string (ruled
/// 2026-06-12, Rust-style); `{}` stays a placeholder and a lone brace still
/// prints literally. Output oracle (R1).
#[test]
fn println_brace_escapes_print_literal_braces() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   println(\"{{}} {} {{{}}}\", 1, 2);\n\
         \x20   println(\"lone { brace\");\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "{} 1 {2}\nlone { brace\n"
    );
}

/// Track 2 cutover (I2): a call to an undeclared name is rejected with a clean
/// `use of undeclared name` diagnostic instead of emitting verbatim C. MIR is a
/// resolved IR with no node for an unresolved call, so the rejection lives in
/// HIR lowering. (`bubblesort`/`file` used to pin this - they called libc
/// without declaring it - but Rust-style FFI restored them; see
/// `bubblesort_runs` and the C-seam tests below.)
#[test]
fn cutover_rejects_undeclared_name_programs() {
    let out = common::compile_expect_failure(
        "main() {\n\
         \x20   frobnicate(1);\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("use of undeclared name"),
        "expected an undeclared-name diagnostic, got:\n{rendered}"
    );
}

/// Early return: `eyesrc/programs/floodfill.eye` is a recursive flood fill that uses bare
/// `return;` as a guard at the top of the recursion (out-of-bounds, already
/// filled, wrong colour). It was rejected before early-return landed; it now
/// compiles and runs. Output oracle (R1): the program prints three counts.
#[test]
fn floodfill_runs() {
    let (out, _) = common::run_program(include_str!("../eyesrc/programs/floodfill.eye"));
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "9\n9\n1\n");
}

/// Early return in value position diverges correctly. `return` appears as an
/// `if`-branch tail inside a `let` initializer: when taken it returns from the
/// function (the binding and the code after it never run); when not taken the
/// other branch supplies the value. This is the case that, without an
/// `Expr::Return` arm in MIR `lower_into`, would route through `lower_rvalue`
/// and hit its `unreachable!`. `pick(5)` takes the else branch (2 + 1 = 3);
/// `pick(-1)` takes the `return 99`.
#[test]
fn early_return_in_value_position_diverges() {
    let (out, _) = common::run_program(
        "pick(int32 c) -> int32 {\n\
         \x20   let int32 x = if c < 0 { return 99; } else { 2 };\n\
         \x20   x + 1\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", pick(5));\n\
         \x20   println(\"{}\", pick(0 - 1));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n99\n");
}

/// A `loop` in value position used to panic the compiler at an MIR `unreachable!`.
/// It now lowers like the other divergent control flow (`return`/`break`): the
/// loop runs as a statement and yields the poison `0` (break is valueless today;
/// break-with-value is Fork D). The program compiles and runs instead of crashing
/// the compiler.
#[test]
fn value_position_loop_does_not_panic() {
    let (out, _) = common::run_program(
        "pick() -> int32 {\n\
         \x20   loop { break; }\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", pick());\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0\n");
}

/// A complex (temp-spilling) match guard falls through correctly when false.
/// `A if x > 0` spills a temp for the comparison; a false guard used to dead-end
/// (no fallthrough, the value-match temp read uninitialized). The flag-gated
/// chain routes a false guard to the next arm. `guard_example.eye` only covered
/// simple bare-local guards (the one shape that already worked).
#[test]
fn complex_match_guard_falls_through() {
    let (out, _) = common::run_program(
        "enum E = A | B ;\n\
         classify(E e, int32 x) -> int32 {\n\
         \x20   match e {\n\
         \x20       A if x > 0 -> 1,\n\
         \x20       B -> 2,\n\
         \x20       _ -> 9,\n\
         \x20   }\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", classify(A, 5));\n\
         \x20   println(\"{}\", classify(A, 0));\n\
         \x20   println(\"{}\", classify(B, 0));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // A,x=5: guard true -> 1. A,x=0: guard false -> falls past B to `_` -> 9.
    // B: -> 2.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n9\n2\n");
}

/// A parameter literally named like a generated local (`x_1` vs local `x`
/// with MIR id 1, `_t2` vs a temp) must not collide: parameters keep their
/// bare source name, so colliding generated names are repaired with a
/// trailing `_` (generated names never end in `_`, keeping the scheme
/// injective).
#[test]
fn param_named_like_mangled_local_does_not_collide() {
    let (out, _) = common::run_program(
        "f(int32 x_1) -> int32 {\n\
         \x20   let int32 x = 5;\n\
         \x20   x + x_1\n\
         }\n\
         g(int32 _t2) -> int32 {\n\
         \x20   let int32 a = _t2 + 1;\n\
         \x20   a * 2\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", f(100));\n\
         \x20   println(\"{}\", g(10));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "105\n22\n");
}

/// A guarded switch whose UNGUARDED arms prove exhaustiveness (no `_`, no
/// default) must still initialize the value-match temp on every path: the
/// last unguarded arm is emitted gated on the flag alone (no scrutinee test),
/// the guarded chain's analogue of the unguarded chain's `else`. Behavior is
/// unchanged; the generated C is checked for the flag-only arm so a rogue
/// scrutinee (bad FFI cast) cannot read the temp uninitialized.
#[test]
fn guarded_exhaustive_switch_has_unconditional_tail() {
    let (out, dir) = common::run_program(
        "enum E = A | B ;\n\
         pick(E e, bool c) -> int32 {\n\
         \x20   match e {\n\
         \x20       A if c -> 1,\n\
         \x20       A -> 2,\n\
         \x20       B -> 3,\n\
         \x20   }\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", pick(A, true));\n\
         \x20   println(\"{}\", pick(A, false));\n\
         \x20   println(\"{}\", pick(B, false));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n2\n3\n");
    let c = std::fs::read_to_string(dir.join("prog.c")).expect("read generated C");
    assert!(
        c.contains("if (!_g0) {"),
        "exhaustive guarded switch must end in a flag-only arm; got:\n{c}"
    );
}

/// A guarded wildcard catch-all (`_ if cond`) falls through when the guard is
/// false. In value position the match is hoisted to a temp; a false guard must
/// route to the trailing unconditional `_` so the temp is written. Before the
/// `Always`-arm lowering this shape was rejected outright.
#[test]
fn guarded_wildcard_catchall_falls_through() {
    let (out, _) = common::run_program(
        "enum E = A | B ;\n\
         classify(E e, bool flag) -> int32 {\n\
         \x20   let int32 r = match e {\n\
         \x20       A -> 1,\n\
         \x20       _ if flag -> 9,\n\
         \x20       _ -> 0,\n\
         \x20   };\n\
         \x20   return r;\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", classify(A, true));\n\
         \x20   println(\"{}\", classify(B, true));\n\
         \x20   println(\"{}\", classify(B, false));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // A: first arm -> 1. B,true: A fails, `_ if true` -> 9. B,false: `_ if false`
    // falls through to `_` -> 0.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n9\n0\n");
}

/// A guarded bare-ident catch-all (`x if cond`) binds the scrutinee, evaluates
/// the guard against the binding, and falls through when false. The match is the
/// tail of an `int32`-returning fn (value position - its value is returned). The
/// binding must be in scope for both the guard and the body.
#[test]
fn guarded_binding_catchall_falls_through() {
    let (out, _) = common::run_program(
        "classify(int32 n) -> int32 {\n\
         \x20   match n {\n\
         \x20       0 -> 100,\n\
         \x20       x if x > 10 -> x,\n\
         \x20       _ -> 0,\n\
         \x20   }\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", classify(0));\n\
         \x20   println(\"{}\", classify(20));\n\
         \x20   println(\"{}\", classify(5));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // 0: first arm -> 100. 20: `x=20 > 10` -> binding x -> 20. 5: `x=5 > 10`
    // false, falls through to `_` -> 0.
    assert_eq!(String::from_utf8_lossy(&out.stdout), "100\n20\n0\n");
}

/// A guarded binding catch-all in genuine statement (discard) position - the
/// match is run only for the side effects in its arm bodies, not hoisted to a
/// temp. Exercises the statement-position lowering path (`lower_expr_stmt`),
/// which differs from value position only in how the body is lowered; the guard
/// and binding handling is shared.
#[test]
fn guarded_binding_catchall_in_statement_position() {
    let (out, _) = common::run_program(
        "side_effect(int32 n) {\n\
         \x20   match n {\n\
         \x20       0 -> println(\"zero\"),\n\
         \x20       x if x > 10 -> println(\"big {}\", x),\n\
         \x20       _ -> println(\"other\"),\n\
         \x20   }\n\
         }\n\
         main() {\n\
         \x20   side_effect(0);\n\
         \x20   side_effect(20);\n\
         \x20   side_effect(5);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // 0: "zero". 20: guard true, binding x -> "big 20". 5: guard false, falls to
    // `_` -> "other".
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "zero\nbig 20\nother\n"
    );
}

/// `main` is an ordinary function; the C entry point is a backend shim. An
/// integer-returning `main` forwards its value as the process exit code
/// (`int main(void) { return (int)__eye_main(); }`), so a non-zero return is
/// observable as the exit status. A bare void `main()` keeps exiting 0.
#[test]
fn int_returning_main_sets_exit_code() {
    let (out, _) = common::run_program(
        "main() -> int32 {\n\
         \x20   println(\"bye\");\n\
         \x20   return 2;\n\
         }\n",
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "main's int32 return must be the exit code; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "bye\n");
}

/// `main` may return any type - the C entry shim adapts it. A non-integer
/// return (here a struct) is computed for effect and the process exits 0; the
/// program must still compile and run cleanly (no raw clang error from
/// returning a struct out of `int main`).
#[test]
fn non_int_returning_main_compiles_and_exits_zero() {
    let (out, _) = common::run_program(
        "structure P { int32 x, int32 y, };\n\
         main() -> P {\n\
         \x20   println(\"hi\");\n\
         \x20   P { x: 1, y: 2 }\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "a struct-returning main must compile and exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
}

/// Early return *directly* in rvalue position (`let x = return 5;`), not wrapped
/// in an `if`/`match`. This routes through MIR `lower_rvalue` rather than
/// `lower_into`; without a diverging arm there it would hit the
/// `non-value expression in rvalue position` `unreachable!` and panic the
/// compiler. The return diverges, so the binding is dead and `g` returns 5.
/// (Mirrors Rust, where `let x = return;` is legal.)
#[test]
fn early_return_as_direct_rvalue_diverges() {
    let (out, _) = common::run_program(
        "g() -> int32 {\n\
         \x20   let int32 x = return 5;\n\
         \x20   x\n\
         }\n\
         main() {\n\
         \x20   println(\"{}\", g());\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5\n");
}

/// Object topology: type declarations are emitted in dependency order with
/// forward declarations, so a struct may be declared before the types it
/// embeds, hold a union field, an array field, a nested struct field, and a
/// self-referential pointer field - all in one program. `Outer` is declared
/// before `Inner`/`Tag`; the codegen topo sort places their definitions first.
#[test]
fn topology_orders_nested_aggregate_fields() {
    let (out, _) = common::run_program(
        "structure Outer { Inner in, [int32; 3] vals, Tag t, };\n\
         union Tag { int32 i, float32 f, };\n\
         structure Inner { int32 x, int32 y, };\n\
         structure Node { int32 v, Node* next, };\n\
         main() {\n\
         \x20   let Inner i = Inner { x: 3, y: 4 };\n\
         \x20   let Outer o = Outer { in: i, vals: [10, 20, 30], t: Tag { i: 7 } };\n\
         \x20   println(\"{}\", o.in.x);\n\
         \x20   println(\"{}\", o.vals[1]);\n\
         \x20   println(\"{}\", o.t.i);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n20\n7\n");
}

/// A value-recursive struct (infinite size) is rejected in HIR with a clear
/// diagnostic rather than leaking a raw clang error. Pins that the
/// value-recursion check and the codegen ordering agree.
#[test]
fn value_recursive_struct_diagnostic_renders() {
    let out = common::compile_expect_failure("structure A { A a, };\nmain() {}\n");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("contains itself by value"),
        "expected a value-recursion diagnostic, got:\n{rendered}"
    );
}

/// Function pointers: a function name decays to a value of its function type, is
/// stored in a `let`, and called through indirectly (`op(2, 3)`).
#[test]
fn function_pointer_value_and_call() {
    let (out, _) = common::run_program(
        "add(int32 a, int32 b) -> int32 { a + b }\n\
         main() {\n\
         \x20   let (int32, int32) -> int32 op = add;\n\
         \x20   println(\"{}\", op(2, 3));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5\n");
}

/// Function pointers as struct fields - a hand-written dispatch table (vtable),
/// the case object topology (B) and function pointers (C) exercise together. The
/// `Ops` struct holds two function-pointer fields; `run` dispatches through them
/// indirectly and takes the struct by value as a parameter.
#[test]
fn function_pointer_struct_vtable() {
    let (out, _) = common::run_program(
        "inc(int32 x) -> int32 { x + 1 }\n\
         at_three(int32 x) -> bool { x == 3 }\n\
         structure Ops { (int32) -> int32 step, (int32) -> bool stop, };\n\
         run(Ops ops, int32 n) -> int32 {\n\
         \x20   if ops.stop(n) { return n; }\n\
         \x20   run(ops, ops.step(n))\n\
         }\n\
         main() {\n\
         \x20   let Ops o = Ops { step: inc, stop: at_three };\n\
         \x20   println(\"{}\", run(o, 0));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "3\n");
}

/// A higher-order function: a function-pointer parameter, called indirectly.
#[test]
fn higher_order_function_pointer_param() {
    let (out, _) = common::run_program(
        "apply(int32 x, (int32) -> int32 f) -> int32 { f(x) }\n\
         double_it(int32 n) -> int32 { n * 2 }\n\
         main() {\n\
         \x20   println(\"{}\", apply(21, double_it));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");
}

/// A function that returns a function pointer, then calls the result:
/// `get()(5)`. The callee of the second call is an rvalue (a call result), not a
/// place, so it spills to a temp - a distinct lowering path from a let/param/
/// field callee. The postfix `()()` chain must also parse.
#[test]
fn function_returning_function_pointer() {
    let (out, _) = common::run_program(
        "inc(int32 x) -> int32 { x + 1 }\n\
         get() -> (int32) -> int32 { inc }\n\
         main() { println(\"{}\", get()(5)); }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6\n");
}

/// An array of function pointers, indexed and called: `tbl[i](x)`. Exercises the
/// array wrapper of a function-pointer typedef (the wrapper hard-deps the Fn
/// node, the Fn node has no hard deps, so the topology orders them correctly).
#[test]
fn array_of_function_pointers() {
    let (out, _) = common::run_program(
        "inc(int32 x) -> int32 { x + 1 }\n\
         dec(int32 x) -> int32 { x - 1 }\n\
         main() {\n\
         \x20   let [(int32) -> int32; 2] tbl = [inc, dec];\n\
         \x20   println(\"{}\", tbl[0](10));\n\
         \x20   println(\"{}\", tbl[1](10));\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "11\n9\n");
}

/// Integer literals carry an optional base prefix: `0x`/`0X` hex, `0b`/`0B`
/// binary, `0o`/`0O` octal. The value is parsed in HIR and emitted in decimal,
/// so this pins the parse, not C's literal grammar. `0xFF == 255`,
/// `0b1010 == 10`, `0o17 == 15`.
#[test]
fn integer_base_prefixes_parse() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   println(\"{}\", 0xFF);\n\
         \x20   println(\"{}\", 0b1010);\n\
         \x20   println(\"{}\", 0o17);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "255\n10\n15\n");
}

/// Every compound assignment form desugars to `a = a <op> b`. Threads one
/// mutable accumulator through arithmetic, bitwise, and shift compounds:
/// `10 *= 3 -> 30`, `/= 2 -> 15`, `+= 1 -> 16`, `-= 4 -> 12`, `%= 7 -> 5`,
/// `<<= 2 -> 20`, `>>= 1 -> 10`, `&= 12 -> 8`, `|= 1 -> 9`, `^= 3 -> 10`.
#[test]
fn compound_assignment_forms_evaluate() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   mut int32 a = 10;\n\
         \x20   a *= 3;\n\
         \x20   a /= 2;\n\
         \x20   a += 1;\n\
         \x20   a -= 4;\n\
         \x20   a %= 7;\n\
         \x20   a <<= 2;\n\
         \x20   a >>= 1;\n\
         \x20   a &= 12;\n\
         \x20   a |= 1;\n\
         \x20   a ^= 3;\n\
         \x20   println(\"{}\", a);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "10\n");
}

/// Immutable-by-default: a `mut` binding accepts reassignment and compound
/// assignment, and the value updates as expected.
#[test]
fn mut_binding_allows_reassignment() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   mut int32 x = 5;\n\
         \x20   x = 6;\n\
         \x20   x += 4;\n\
         \x20   println(\"{}\", x);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "10\n");
}

/// Immutable-by-default: assigning to a `let` binding is rejected in HIR with a
/// clear diagnostic, not leaked as a C `const` error.
#[test]
fn assign_to_let_binding_rejected() {
    let out = common::compile_expect_failure("main() {\n    let int32 x = 5;\n    x = 6;\n}\n");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("immutable"),
        "expected an immutability diagnostic, got:\n{rendered}"
    );
}

/// Immutable-by-default reaches through a projection: mutating a field of a
/// `let`-bound struct is rejected, because the write roots in the immutable
/// binding.
#[test]
fn field_assign_through_let_binding_rejected() {
    let out = common::compile_expect_failure(
        "structure P { int32 a, };\n\
         main() {\n\
         \x20   let P p = P { a: 1 };\n\
         \x20   p.a = 9;\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("immutable"),
        "expected an immutability diagnostic, got:\n{rendered}"
    );
}

/// No-footgun F2 extends to compound assignment: an assignment in an `if`
/// condition is rejected, so `if x += 5` cannot silently become `if (x = x + 5)`
/// the way it does in C. Pins that the guard covers every assignment operator,
/// not just plain `=`. (`mut` avoids an immutability error masking the result.)
#[test]
fn compound_assignment_in_if_condition_rejected() {
    let out =
        common::compile_expect_failure("main() {\n    mut int32 x = 0;\n    if x += 5 { }\n}\n");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("assignment is not allowed in an `if` condition"),
        "expected an assign-in-condition diagnostic, got:\n{rendered}"
    );
}

/// Horizon 0, Component 1 (const): compile-time constants. Exercises a scalar
/// const, a const-expr referencing another const, a negative fold, the bitwise
/// operator set, a comparison folding to bool, float arithmetic, a char const,
/// and a const (and const-expr) driving a fixed-array length (A6); plus the
/// block-scope form: a local const referencing top-level and earlier local
/// consts, inner-block shadowing, a local const as an array length, and a
/// negative local fold. A const is a value, not storage - the values are
/// inlined, so this is an output oracle (R1). Source lives in
/// `eyesrc/lang/const.eye` so the file stays authoritative.
#[test]
fn const_eye_folds_and_inlines_compile_time_values() {
    let source = include_str!("../eyesrc/lang/const.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "max        100\n\
         dbl        200\n\
         neg        -5\n\
         bits       255\n\
         big        1\n\
         tau        6.000000\n\
         mark       A\n\
         itau       6\n\
         len        4\n\
         xs0        100\n\
         ys-len     8\n\
         loc        101\n\
         loc2       202\n\
         inner      7\n\
         outer      101\n\
         pair-len   2\n\
         negl       -3\n",
        "unexpected const stdout"
    );
}

/// Block-scope `const` misuse is rejected like the top-level form: assignment
/// (a const is a value, not a place) and `&` (it has no address).
#[test]
fn local_const_assign_and_ref_are_rejected() {
    let out = common::compile_expect_failure(
        "main() {\n\
         \x20   const int32 N = 5;\n\
         \x20   N = 6;\n\
         \x20   let &int32 r = &N;\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("cannot assign to constant `N`"),
        "expected an assign-to-const diagnostic, got:\n{rendered}"
    );
    assert!(
        rendered.contains("cannot take the address of constant `N`"),
        "expected a ref-of-const diagnostic, got:\n{rendered}"
    );
}

/// A block-scope const's initializer is the same bounded const-expr fold as the
/// top level: a runtime local in it is rejected (not a constant), and the const
/// itself is not visible outside its declaring block.
#[test]
fn local_const_runtime_init_and_scope_escape_are_rejected() {
    let out = common::compile_expect_failure(
        "main() {\n\
         \x20   let int32 x = 1;\n\
         \x20   const int32 BAD = x + 1;\n\
         \x20   if true {\n\
         \x20       const int32 INNER = 2;\n\
         \x20   }\n\
         \x20   println(\"{}\", INNER);\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("`x` is not a constant"),
        "expected a non-const-initializer diagnostic, got:\n{rendered}"
    );
    assert!(
        rendered.contains("use of undeclared name `INNER`"),
        "expected an out-of-scope diagnostic, got:\n{rendered}"
    );
}

/// `sizeof(T)` lowers to C `sizeof(ctype)`: fixed-width types have guaranteed
/// sizes (int8=1, int32=4, int64=8, char=1), a struct of two int32 is 8, and
/// `count * sizeof(T)` (the malloc-argument shape) folds into a usize value.
#[test]
fn sizeof_eye_reports_type_sizes() {
    let source = include_str!("../eyesrc/lang/sizeof.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "int8   1\n\
         int32  4\n\
         int64  8\n\
         char   1\n\
         Point  8\n\
         bytes  32\n",
        "unexpected sizeof stdout"
    );
}

/// `sizeof` takes a *type*, not a value or a compound type. A compound-type
/// argument (`sizeof(&z)`) is rejected at the floor (deferred), not silently
/// miscompiled.
#[test]
fn sizeof_compound_type_is_rejected() {
    let out = common::compile_expect_failure(
        "main() {\n\
         \x20   let int32 z = 0;\n\
         \x20   println(\"{}\", sizeof(&z));\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("takes a named type"),
        "expected a sizeof-not-a-type diagnostic, got:\n{rendered}"
    );
}

/// String literals as `&[uint8; N]` (Component 3, Part B): a literal printed
/// directly renders as text (`%s`), a stored string round-trips, `len` reports
/// the visible byte count, and indexing yields the byte value (`uint8` prints as
/// `%d`, so `s[0]` = 104, `s[4]` = 111). A `char`-typed result prints as `%c`, so
/// `first`/`last` render `h`/`o`. Closes the old `print`-renders-`%d` string bug.
#[test]
fn string_eye_byte_array_refs() {
    let source = include_str!("../eyesrc/lang/string.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "greet  world\n\
         stored hello\n\
         len    5\n\
         s[0]   104\n\
         s[4]   111\n\
         first  h\n\
         last   o\n\
         esclen 3\n\
         strvar hi\n",
        "unexpected string stdout"
    );
}

/// caesar.eye: the integrating string program - it decays a stored string to a
/// `string` parameter, calls libc `strlen`/`putchar` over it (FFI), indexes it,
/// and does char arithmetic. Runs end to end with the correct cipher output.
#[test]
fn caesar_eye_string_decay_and_ffi() {
    let source = include_str!("../eyesrc/ffi/caesar.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "plain:\n\
         hello caeser\n\
         cipher (+3):\n\
         khoor fdhvhu\n",
        "unexpected caesar stdout"
    );
}

/// Top-level globals (Component 3): a `let` read-only static and a `mut`
/// writable static, the latter initialized from a const, mutated through a
/// function, and read back through `&counter` (a global is addressable, unlike
/// a const).
#[test]
fn global_eye_static_storage() {
    let source = include_str!("../eyesrc/lang/global.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "origin   0\n\
         counter  10\n\
         counter  12\n\
         enabled  1\n\
         via-ptr  12\n",
        "unexpected global stdout"
    );
}

/// A `let` global is read-only static storage; assigning it is rejected with the
/// same immutable-by-default diagnostic as a `let` local. A `mut` global opts in.
#[test]
fn global_assign_to_let_is_rejected() {
    let out = common::compile_expect_failure(
        "let int32 X = 5;\n\
         main() {\n\
         \x20   X = 9;\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("immutable"),
        "expected an immutable-assignment diagnostic, got:\n{rendered}"
    );
}

/// A const is a value with no address: `&MAX` is rejected in HIR with a clear
/// diagnostic rather than silently taking the address of an inlined temp.
#[test]
fn const_address_of_is_rejected() {
    let out = common::compile_expect_failure(
        "const int32 MAX = 5;\n\
         main() {\n\
         \x20   let int32* p = &MAX;\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("address of constant"),
        "expected an address-of-const diagnostic, got:\n{rendered}"
    );
}

/// A const whose initializer is not a const-expr (here a function call - that
/// is compile-time *execution*, the far-future prime layer) is rejected.
#[test]
fn const_non_const_initializer_is_rejected() {
    let out = common::compile_expect_failure(
        "f() -> int32 { 1 }\n\
         const int32 X = f();\n\
         main() {}\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("not a constant expression"),
        "expected a non-const-expr diagnostic, got:\n{rendered}"
    );
}

/// The raw-pointer escape: a write *through* a `let`-bound pointer (`*p = v`)
/// is not an assignment to the binding itself, so it is allowed and runs. This
/// is Eye's runtime-freedom seam - immutability tracks the binding, not memory
/// reached through a pointer.
/// Documentation example: match arm guards -- checks guard-true, guard-false
/// fallthrough to wildcard, and multiple guarded arms.
#[test]
fn docs_guard_example_compiles_and_runs() {
    let source = include_str!("../eyesrc/lang/guard_example.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "r1 = 1\nr2 = 6\nr3 = 8\nr4 = 20\n",
        "unexpected guard-example output"
    );
}

/// Documentation example: println intrinsic -- every supported type and mixed
/// multi-arg formatting.
#[test]
fn docs_println_example_compiles_and_runs() {
    let source = include_str!("../eyesrc/lang/println_example.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "int32  i   = 42\n\
         int64  big = 1000000\n\
         float32 f   = 2.500000\n\
         float64 d   = 3.141593\n\
         true   = 1\n\
         false  = 0\n\
         char   = Z\n\
         string = hello world\n\
         mixed  i=42 f=3.141593 c=X\n",
        "unexpected println-example output"
    );
}

#[test]
fn write_through_let_pointer_allowed() {
    let (out, _) = common::run_program(
        "main() {\n\
         \x20   mut int32 x = 5;\n\
         \x20   let int32* p = &x;\n\
         \x20   *p = 99;\n\
         \x20   println(\"{}\", x);\n\
         }\n",
    );
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "99\n");
}

/// Array repeat literal `[value; N]`: scalar fill, const-length fill, element
/// coercion, struct value fill, nested/multi-dim, and evaluate-once semantics.
/// Source lives in `eyesrc/lang/array_fill.eye` so the file stays authoritative.
#[test]
fn array_fill_eye_repeat_literal() {
    let source = include_str!("../eyesrc/lang/array_fill.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            "a 7 7 7 len 3",
            "flags 1 1 len 4",
            "big 0 0",
            "ps 1 2 1 2",
            "grid 9 9 9",
            // next() ran exactly once; the temp was copied 4 times.
            "same 1 1 1 1 calls 1",
        ],
        "full stdout:\n{stdout}"
    );
}

/// C seam: a variadic extern (`printf(string fmt, ...)`). The prototype gains
/// `...`, calls pass extra trailing arguments, and no `<stdio.h>` is included -
/// the extern block is the sole prototype.
#[test]
fn variadic_extern_printf_runs() {
    let source = "\
extern {
    printf(string fmt, ...) -> int32;
}

main() {
    let int32 n = 42;
    printf(\"n=%d s=%s\\n\", n, \"mixed\");
    printf(\"no extras\\n\");
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "n=42 s=mixed\nno extras\n"
    );
}

/// C seam: an opaque FFI type (`extern { type FILE; }`) used behind a pointer.
/// `fopen`/`fclose` round-trip a `FILE*` value; the emitted C declares
/// `typedef struct FILE FILE;` and no definition.
#[test]
fn opaque_extern_type_fopen_roundtrip() {
    let source = "\
extern {
    type FILE;
    fopen(string path, string mode) -> FILE*;
    fclose(FILE* f) -> int32;
}

main() {
    mut FILE* f = fopen(\"/dev/null\", \"r\");
    if f == (0 as FILE*) {
        println(\"open failed\");
        return;
    }
    let int32 rc = fclose(f);
    println(\"closed {}\", rc);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "closed 0\n");
}

/// The whole C-seam corpus program: opaque `FILE`, variadic `printf`, and
/// `println` together in one unit (the auto printf prototype is skipped
/// because the program declares its own). Source lives in
/// `eyesrc/programs/bubblesort.eye` so the file stays authoritative.
#[test]
fn bubblesort_runs() {
    let source = include_str!("../eyesrc/programs/bubblesort.eye");
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "9 2 7 1 8 3 5 4 \n1 2 3 4 5 7 8 9 \n"
    );
}

/// `...` is an extern-only C-ABI marker: a defined fn cannot take it (Eye has
/// no varargs access), it must be the last parameter, and it needs at least
/// one named parameter before it (the C calling convention requires one).
#[test]
fn variadic_misuse_is_rejected() {
    let out = common::compile_expect_failure(
        "log(string fmt, ...) {\n\
         }\n\
         main() {\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("`...` is only allowed in an `extern` signature"),
        "expected a variadic-outside-extern diagnostic, got:\n{rendered}"
    );

    let out = common::compile_expect_failure(
        "extern {\n\
         \x20   bad(string fmt, ..., int32 n) -> int32;\n\
         }\n\
         main() {\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("`...` must be the last parameter"),
        "expected a variadic-not-last diagnostic, got:\n{rendered}"
    );

    let out = common::compile_expect_failure(
        "extern {\n\
         \x20   bare(...) -> int32;\n\
         }\n\
         main() {\n\
         }\n",
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rendered.contains("`...` requires at least one named parameter before it"),
        "expected a variadic-needs-param diagnostic, got:\n{rendered}"
    );
}

// ---- snapshot tests live in tests/snapshots.rs ----

/// CLEAK L1/L2 (coercion-point unification): string decay at struct-literal
/// fields and array-literal elements, through codegen to a running binary.
/// Also covers an integer literal adopting a wide annotated type (M1's
/// positive side): the printed `int64` value only survives if the literal's
/// C temp is 64-bit.
#[test]
fn coercion_point_decay_and_wide_literals_run() {
    let source = "\
structure Syllable {
    string sound,
};

main() {
    let Syllable s = Syllable { sound: \"cvc\" };
    let [string; 2] xs = [\"ab\", \"cd\"];
    let int64 big = 5000000000;
    println(\"{}\", s.sound);
    println(\"{} {}\", xs[0], xs[1]);
    println(\"{}\", big);
}
";
    let (out, _) = common::run_program(source);
    assert!(
        out.status.success(),
        "program exited {}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "cvc\nab cd\n5000000000\n",
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
