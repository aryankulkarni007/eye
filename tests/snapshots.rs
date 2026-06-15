//! snapshot tests for IR and codegen output. each test writes a minimal `.eye`
//! program, runs the driver with the corresponding `--dump-*` flag, and captures
//! the relevant section of stdout as an `insta` snapshot. review and accept
//! changes with `cargo insta review` when output changes intentionally.

mod common;

/// MIR dump snapshot: run `--dump-mir-raw` on a minimal program and snapshot
/// the concrete MIR.
#[test]
fn mir_dump_snapshot() {
    let mir = common::run_driver_dump(
        "\
main() {
    let int32 x = 42;
    let int32 y = x + 1;
    println(\"{}\", y);
}
",
        &["--dump-mir-raw"],
        "--- MIR (raw) ---",
        "c source written",
    );
    insta::assert_snapshot!("mir_dump", mir);
}

/// c codegen snapshot: run `--dump-c` on a minimal program and snapshot the
/// generated c.
#[test]
fn c_codegen_snapshot() {
    let c = common::run_driver_dump(
        "\
main() {
    let int32 x = 42;
    println(\"{}\", x);
}
",
        &["--dump-c"],
        "--- generated C ---",
        "c source written",
    );
    insta::assert_snapshot!("c_codegen", c);
}

/// HIR dump snapshot: run `--dump-hir` on a minimal program and snapshot the
/// readable HIR summary.
#[test]
fn hir_dump_snapshot() {
    let hir = common::run_driver_dump(
        "\
main() {
    let int32 x = 42;
    println(\"{}\", x);
}
",
        &["--dump-hir"],
        "--- HIR ---",
        "c source written",
    );
    insta::assert_snapshot!("hir_dump", hir);
}

/// HIR raw dump snapshot: run `--dump-hir-raw` on a minimal program and
/// snapshot the full debug representation.
#[test]
fn hir_raw_dump_snapshot() {
    let hir = common::run_driver_dump(
        "\
main() {
    let int32 x = 42;
    println(\"{}\", x);
}
",
        &["--dump-hir-raw"],
        "--- HIR (raw) ---",
        "c source written",
    );
    insta::assert_snapshot!("hir_raw_dump", hir);
}
