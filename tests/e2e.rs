//! End-to-end driver tests. Each test invokes the built `eye` binary on a
//! `.eye` source file, then runs the resulting native binary and inspects
//! its stdout. These tests cement the externally visible v0.1 behaviour:
//! the public surface is "I hand you a `.eye` file and the program runs".

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cargo writes the driver here for integration tests.
const DRIVER: &str = env!("CARGO_BIN_EXE_eye");

/// Monotonic counter so parallel tests never clash on temp paths.
static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

/// Stage `source` into a per-test directory under `target/e2e-fixtures/`,
/// run the driver, run the produced binary, and return its captured output.
fn run_program(source: &str) -> (std::process::Output, PathBuf) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("target").join("e2e-fixtures").join(format!(
        "case-{}",
        FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");

    let src_path = dir.join("prog.eye");
    std::fs::write(&src_path, source).expect("write source");

    let driver_status = Command::new(DRIVER)
        .arg(&src_path)
        .status()
        .expect("invoke driver");
    assert!(driver_status.success(), "driver failed: {driver_status}");

    let bin_path = dir.join("prog");
    let out = Command::new(&bin_path)
        .output()
        .expect("execute produced binary");
    (out, dir)
}

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

    print(\"{}\", p.x);
    print(\"{}\", p.y);
}
";
    let (out, _) = run_program(source);
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
    print(\"{}\", x);
}
";
    let (out, _) = run_program(source);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5\n");
}

/// Exercises every primitive `print` format specifier plus reference-to-struct
/// (`%p`). Source lives in `eyesrc/print.eye` so the file stays authoritative.
#[test]
fn print_eye_covers_every_format_specifier() {
    let source = include_str!("../eyesrc/print.eye");
    let (out, _) = run_program(source);
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
/// wildcard) returning into typed `let`s. Source lives in `eyesrc/v03.eye`
/// so the file stays authoritative. Locks the externally visible v0.3
/// behaviour.
#[test]
fn v03_eye_lowers_match_and_prints_expected_output() {
    let source = include_str!("../eyesrc/v03.eye");
    let (out, _) = run_program(source);
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

/// v0.4 end-to-end: every sized/unsigned integer type compiles under clang
/// and prints its value with the correct printf specifier (catches a `%lld` /
/// `%llu` width mismatch that would only surface at C-compile or run time).
#[test]
fn sized_integer_types_compile_and_print() {
    let source = "\
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
    let (out, _) = run_program(source);
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
    print(\"{}\", small);
    print(\"{}\", half);
}
";
    let (out, _) = run_program(source);
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
/// `eyesrc/v04.eye` so the file stays authoritative.
#[test]
fn v04_eye_lowers_primitives_and_casts() {
    let source = include_str!("../eyesrc/v04.eye");
    let (out, _) = run_program(source);
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
/// members print with their own specifiers. Source lives in `eyesrc/ffi.eye`.
#[test]
fn ffi_eye_links_libc_and_lowers_union() {
    let source = include_str!("../eyesrc/ffi.eye");
    let (out, _) = run_program(source);
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
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("target").join("e2e-fixtures").join(format!(
        "case-{}",
        FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");

    let src_path = dir.join("dump.eye");
    std::fs::write(
        &src_path,
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
    print(string fmt, int32 value);
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
    )
    .expect("write source");

    let out = Command::new(DRIVER)
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
        "extern fn print(string fmt, int32 value) -> void",
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
/// complement/not, compound assignment. Source lives in `eyesrc/v06.eye` so
/// the file stays authoritative. Locks the externally visible v0.6 behaviour.
#[test]
fn v06_eye_runs_operators_and_prints_expected_output() {
    let source = include_str!("../eyesrc/v06.eye");
    let (out, _) = run_program(source);
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

/// Driver should refuse non-`.eye` input rather than overwriting an
/// arbitrary file with generated C.
#[test]
fn driver_rejects_non_eye_extension() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("target").join("e2e-fixtures").join(format!(
        "case-{}",
        FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("prog.txt");
    std::fs::write(&bad, "main() {}\n").unwrap();

    let status = Command::new(DRIVER).arg(&bad).status().unwrap();
    assert!(!status.success(), "driver should have rejected non-.eye");
}

/// v0.7 end-to-end: fixed arrays as a first-class value type - lvalue index,
/// `len(x)`, value-copy independence, return-by-value, `&[T; N]` reference, and
/// multi-dimensional nesting. Source is `eyesrc/arrays.eye`. Locks that the
/// struct-wrap representation behaves as real value semantics at runtime.
#[test]
fn arrays_eye_runs_value_semantics_and_prints_expected_output() {
    let source = include_str!("../eyesrc/arrays.eye");
    let (out, _) = run_program(source);
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
