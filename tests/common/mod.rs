#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cargo writes the driver here for integration tests.
pub const DRIVER: &str = env!("CARGO_BIN_EXE_eye");

/// Monotonic counter so parallel tests never clash on temp paths.
static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

/// Create and return a unique per-test fixture directory.
pub fn fixture_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("target").join("e2e-fixtures").join(format!(
        "case-{}",
        FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

/// Write `source` into `dir / name` and return the full path.
pub fn write_source(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write source");
    path
}

/// Arrange `source` into a fixture, compile it, run the binary, return output.
pub fn run_program(source: &str) -> (std::process::Output, PathBuf) {
    let dir = fixture_dir();
    let src_path = write_source(&dir, "prog.eye", source);

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

/// Compile `source` expecting the driver to REJECT it, returning the driver's
/// captured output. Asserts a non-zero exit.
pub fn compile_expect_failure(source: &str) -> std::process::Output {
    let dir = fixture_dir();
    let src_path = write_source(&dir, "prog.eye", source);

    let out = Command::new(DRIVER)
        .arg(&src_path)
        .output()
        .expect("invoke driver");
    assert!(
        !out.status.success(),
        "driver unexpectedly accepted the program"
    );
    out
}

/// Run the driver with the given extra args, extract the section between
/// `header` and `terminator`, and return it trimmed.
pub fn run_driver_dump(
    source: &str,
    extra_args: &[&str],
    header: &str,
    terminator: &str,
) -> String {
    let dir = fixture_dir();
    let src_path = write_source(&dir, "snap.eye", source);

    let out = Command::new(DRIVER)
        .arg(&src_path)
        .args(extra_args)
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
    stdout
        .split(header)
        .nth(1)
        .unwrap_or(&stdout)
        .split(terminator)
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}
