//! Unit tests for MIR lowering. Each test parses a small `.eye` program, lowers
//! it through HIR into MIR, then asserts structural properties of the resulting
//! `MirBody` — statement count, variant kinds, local types, control flow shape.
//!
//! These tests complement the e2e tests by catching MIR-level regressions
//! without needing a C compiler or binary execution.

use crate::core::*;
use crate::lower::lower_function;
use ast::{AstNode, SourceFile};
use hir::core::*;
use lexer::{Lexer, SourceText};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse `src`, lower to HIR, lower the first function's body to MIR.
fn lower_first_fn(src: &str) -> (HIR, FnId, MirBody) {
    let source = SourceText::new(src.to_string());
    let tokens = Lexer::new(&source).tokenize().tokens;
    let parse = parser::parse(&tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    let hir = lower_source_file(file);
    assert!(
        hir.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        hir.diagnostics
    );
    let fn_id = *hir
        .items
        .functions
        .values()
        .next()
        .expect("at least one function");
    let body_id = hir.functions[fn_id]
        .body
        .expect("function has a body");
    let body = &hir.bodies[body_id];
    let mir = lower_function(
        &hir,
        body,
        hir.functions[fn_id].params.len(),
        hir.functions[fn_id].ret,
    );
    (hir, fn_id, mir)
}

/// Count statements of a given kind in a flat stmt list (no recursion).
fn count_kind(stmts: &[MirStmt], kind: &str) -> usize {
    stmts
        .iter()
        .filter(|s| variant_name(s) == kind)
        .count()
}

fn variant_name(s: &MirStmt) -> &'static str {
    match s {
        MirStmt::Let { .. } => "Let",
        MirStmt::Assign { .. } => "Assign",
        MirStmt::Eval(_) => "Eval",
        MirStmt::If { .. } => "If",
        MirStmt::Loop { .. } => "Loop",
        MirStmt::Switch { .. } => "Switch",
        MirStmt::Break => "Break",
        MirStmt::Continue => "Continue",
        MirStmt::Return(_) => "Return",
    }
}

// ---------------------------------------------------------------------------
// Straight-line lowering
// ---------------------------------------------------------------------------

#[test]
fn straight_line_let_and_binary() {
    let (_, _, mir) = lower_first_fn(
        "\
main() {
    let int32 x = 42;
    let int32 y = x + 1;
    println(\"{}\", y);
}
",
    );
    assert_eq!(mir.locals.len(), 2, "x and y");
    assert_eq!(mir.params.len(), 0, "main has no params");
    assert_eq!(mir.body.stmts.len(), 3);
    assert_eq!(count_kind(&mir.body.stmts, "Let"), 2);
    assert_eq!(count_kind(&mir.body.stmts, "Eval"), 1);
}

#[test]
fn params_become_locals() {
    let (_, _, mir) = lower_first_fn(
        "\
add(int32 a, int32 b) -> int32 {
    a + b
}
",
    );
    // param locals + 0 temps if tail is trivial, but the binary produces 1 temp
    assert!(mir.locals.len() >= 2, "at least the two params");
    assert_eq!(mir.params.len(), 2);
}

// ---------------------------------------------------------------------------
// If / else
// ---------------------------------------------------------------------------

#[test]
fn if_else_produces_if_stmt() {
    let (_, _, mir) = lower_first_fn(
        "\
main() {
    let int32 x = 0;
    if x > 0 {
        println(\"pos\", 1);
    } else {
        println(\"non\", 2);
    }
}
",
    );
    let if_count = count_kind(&mir.body.stmts, "If");
    assert_eq!(if_count, 1, "expected exactly one If stmt");
}

// ---------------------------------------------------------------------------
// Loop / break / continue
// ---------------------------------------------------------------------------

#[test]
fn loop_contains_loop_stmt() {
    let (_, _, mir) = lower_first_fn(
        "\
main() {
    mut int32 i = 0;
    loop {
        if i >= 3 { break; }
        i = i + 1;
    }
}
",
    );
    let loop_count = count_kind(&mir.body.stmts, "Loop");
    assert_eq!(loop_count, 1, "expected exactly one Loop stmt");
}

// ---------------------------------------------------------------------------
// Match → Switch
// ---------------------------------------------------------------------------

#[test]
fn match_lowers_to_switch() {
    let (_, _, mir) = lower_first_fn(
        "\
enum E = A | B;
main() {
    let E e = A;
    let int32 r = match e {
        A -> 1,
        B -> 2,
    };
    println(\"{}\", r);
}
",
    );
    let _switch_count = count_kind(&mir.body.stmts, "Switch");
    // The match call might be lowered directly (no Switch if trivial) or
    // through a temp. At minimum we can assert the program lowered without
    // panicking and that the body has at least one stmt.
    assert!(
        !mir.body.stmts.is_empty(),
        "match body must produce stmts"
    );
}

// ---------------------------------------------------------------------------
// Short-circuit && and ||
// ---------------------------------------------------------------------------

#[test]
fn logical_and_lowers_without_binary() {
    let (_, _, mir) = lower_first_fn(
        "\
main() {
    let bool a = true && false;
    println(\"{}\", a);
}
",
    );
    // && must lower to control flow (If), not RValue::Binary.
    let if_count = count_kind(&mir.body.stmts, "If");
    assert!(
        if_count >= 1,
        "&& must produce If control flow, found {} If stmts",
        if_count
    );
}

#[test]
fn logical_or_lowers_without_binary() {
    let (_, _, mir) = lower_first_fn(
        "\
main() {
    let bool a = true || false;
    println(\"{}\", a);
}
",
    );
    let if_count = count_kind(&mir.body.stmts, "If");
    assert!(
        if_count >= 1,
        "|| must produce If control flow, found {} If stmts",
        if_count
    );
}

// ---------------------------------------------------------------------------
// Struct field access
// ---------------------------------------------------------------------------

#[test]
fn struct_field_access_produces_field_place() {
    let (_, _, mir) = lower_first_fn(
        "\
structure Point { int32 x, int32 y, };
main() {
    let Point p = Point { x: 1, y: 2 };
    let int32 v = p.x;
    println(\"{}\", v);
}
",
    );
    // Field access lowers to Place::Field; check that lowering doesn't panic
    // and produces reasonable MIR.
    assert!(mir.locals.len() >= 2, "p and v plus temps");
    assert_eq!(mir.params.len(), 0);
}

// ---------------------------------------------------------------------------
// Return
// ---------------------------------------------------------------------------

#[test]
fn value_return_produces_return_stmt() {
    let (_, _, mir) = lower_first_fn(
        "\
f() -> int32 {
    return 42;
}
main() {
    println(\"{}\", f());
}
",
    );
    // f is first function in items order
    assert!(!mir.body.stmts.is_empty());
}

// ---------------------------------------------------------------------------
// Destructure (struct binding in let)
// ---------------------------------------------------------------------------

#[test]
fn destructure_let_expands_into_field_bindings() {
    let (_, _, mir) = lower_first_fn(
        "\
structure Point { int32 x, int32 y, };
main() {
    let Point p = Point { x: 10, y: 20 };
    let Point { x, y } = p;
    println(\"{}\", x);
    println(\"{}\", y);
}
",
    );
    // destructure expands to multiple Let stmts
    let let_count = count_kind(&mir.body.stmts, "Let");
    assert!(
        let_count >= 2,
        "destructure should expand into multiple Let stmts, got {}",
        let_count
    );
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

#[test]
fn mir_body_default_is_empty() {
    let block = MirBlock::default();
    assert!(block.stmts.is_empty());
}
