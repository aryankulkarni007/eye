//! Database wiring tests: memoization within a revision, invalidation across
//! revisions, and agreement between the per-fn and whole-file paths.

use salsa::Setter as _;

use crate::*;

const PROGRAM: &str = "\
structure Point { int32 x, int32 y, };

add(int32 a, int32 b) -> int32 {
    a + b
}

main() -> int32 {
    add(1, 2)
}
";

fn file(db: &Database, text: &str) -> SourceFileInput {
    SourceFileInput::new(db, "test.eye".to_owned(), text.to_owned())
}

#[test]
fn queries_are_memoized_within_a_revision() {
    let db = Database::default();
    let input = file(&db, PROGRAM);
    // Same revision: the second call must return the cached value (same Arc).
    assert!(database_eq(&lex(&db, input), &lex(&db, input)));
    assert!(database_eq(&parse(&db, input), &parse(&db, input)));
    assert!(database_eq(&lowered_file(&db, input), &lowered_file(&db, input)));
    assert!(database_eq(&mir_map(&db, input), &mir_map(&db, input)));
    assert!(database_eq(&c_code(&db, input), &c_code(&db, input)));
}

fn database_eq<T>(a: &Memo<T>, b: &Memo<T>) -> bool {
    a == b
}

#[test]
fn set_text_invalidates_and_recomputes() {
    let mut db = Database::default();
    let input = file(&db, PROGRAM);
    let before = c_code(&db, input);
    assert!(before.contains("int32_t add"), "C contains add: {}", *before);
    assert!(!before.contains("int32_t sub"));

    input
        .set_text(&mut db)
        .to(PROGRAM.replace("add", "sub"));
    let after = c_code(&db, input);
    assert!(after.contains("int32_t sub"), "C contains sub: {}", *after);
    assert!(!after.contains("int32_t add"));
}

#[test]
fn diagnostics_gate_c_code() {
    let db = Database::default();
    let input = file(&db, "main() { let int32 x = undeclared_name; }");
    assert!(!hir_diagnostics(&db, input).is_empty());
    assert!(c_code(&db, input).is_empty());
}

#[test]
fn per_fn_path_agrees_with_whole_file_path() {
    let db = Database::default();
    let input = file(&db, PROGRAM);

    // No diagnostics either way on a clean program.
    assert!(hir_diagnostics(&db, input).is_empty());
    assert!(lowered_file(&db, input).diagnostics.is_empty());

    // The per-fn path lowers every collected function.
    let scope = item_scope(&db, input);
    assert_eq!(scope.fns.len(), 2, "add + main");
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(&db, input, ptr);
        let lowered = lower_fn(&db, fn_id);
        assert!(lowered.diagnostics.is_empty());
        assert!(!lowered.body.exprs.is_empty());
    }
}

#[test]
fn per_fn_diagnostics_localize_to_the_broken_body() {
    let db = Database::default();
    let input = file(
        &db,
        "good() -> int32 { 1 }\nbad() -> int32 { undeclared_name }\n",
    );
    let scope = item_scope(&db, input);
    let mut per_fn: Vec<usize> = Vec::new();
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(&db, input, ptr);
        per_fn.push(lower_fn(&db, fn_id).diagnostics.len());
    }
    assert_eq!(per_fn, vec![0, 1], "only `bad` carries a diagnostic");
}
