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
    const int32 x = 0;
    const int32 y = 0;
    var Point p = Point { x, y };

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
    const int32 x = -1 + 2 * 3;
    print(\"{}\", x);
}
";
    let (out, _) = run_program(source);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5\n");
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
