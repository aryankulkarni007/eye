//! database wiring tests: memoization within a revision, invalidation across
//! revisions, and agreement between the per-fn and whole-file paths.

use std::sync::Arc;

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
    // same revision: the second call must return the cached value (same arc).
    assert!(database_eq(&lex(&db, input), &lex(&db, input)));
    assert!(database_eq(&parse(&db, input), &parse(&db, input)));
    assert!(database_eq(
        &lowered_file(&db, input),
        &lowered_file(&db, input)
    ));
    assert!(database_eq(&mir_map(&db, input), &mir_map(&db, input)));
    assert!(database_eq(&c_code(&db, input), &c_code(&db, input)));
}

/// true identity (the *same* cached `Arc`), for the within-revision
/// memoization checks. distinct from `Memo`'s `PartialEq`, which is now a
/// content-digest backdating test ([`MemoEq`]), not pointer identity.
fn database_eq<T>(a: &Memo<T>, b: &Memo<T>) -> bool {
    Arc::ptr_eq(&a.0, &b.0)
}

#[test]
fn set_text_invalidates_and_recomputes() {
    let mut db = Database::default();
    let input = file(&db, PROGRAM);
    let before = c_code(&db, input);
    assert!(
        before.contains("int32_t add"),
        "C contains add: {}",
        *before
    );
    assert!(!before.contains("int32_t sub"));

    input.set_text(&mut db).to(PROGRAM.replace("add", "sub"));
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

    // no diagnostics either way on a clean program.
    assert!(hir_diagnostics(&db, input).is_empty());
    assert!(lowered_file(&db, input).hir.diagnostics.is_empty());

    // the per-fn path lowers every collected function.
    let scope = item_scope(&db, input);
    assert_eq!(scope.fns.len(), 2, "add + main");
    for &(_, ptr) in &scope.fns {
        let fn_id = StableFnId::new(&db, input, ptr);
        let lowered = lower_fn(&db, fn_id);
        assert!(lowered.lowered.diagnostics.is_empty());
        assert!(!lowered.lowered.body.exprs.is_empty());
    }
}

#[test]
fn lowered_file_carries_the_effect_map() {
    // the fused walk stores the whole-program effect verdict alongside the type
    // results: main calls println, so its total effect includes io.
    let db = Database::default();
    let input = file(&db, "main() {\n    println(\"{}\", 1);\n}\n");
    let checked = lowered_file(&db, input);
    let main = *checked.hir.items.functions.get("main").expect("main exists");
    assert!(
        checked.effects.effect_of(main).contains(effect::Atom::Io),
        "main's effect map entry must record io"
    );
}

#[test]
fn effect_contract_mismatch_is_reported_and_gates_c() {
    // `pure` declared on a fn that calls println: the e-class contract
    // diagnostic must surface and gate c generation, like a type error.
    let db = Database::default();
    let input = file(&db, "pure report(int32 n) {\n    println(\"{}\", n);\n}\n");
    let diags = hir_diagnostics(&db, input);
    assert!(
        diags
            .into_iter()
            .any(|(_, e)| matches!(e, hir::core::HirError::Effect(_))),
        "the effect mismatch must reach the file diagnostics"
    );
    assert!(
        c_code(&db, input).is_empty(),
        "an effect-contract violation gates C generation"
    );
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
        per_fn.push(lower_fn(&db, fn_id).lowered.diagnostics.len());
    }
    assert_eq!(per_fn, vec![0, 1], "only `bad` carries a diagnostic");
}

#[test]
fn typeck_fn_localizes_type_diagnostics() {
    // a *type* error (returning a `bool` from an `int32` fn) is a `typeck_fn`
    // diagnostic, not a lowering one (step d): only the broken body's query
    // carries it, and it still reaches the whole-file diagnostics.
    let db = Database::default();
    let input = file(&db, "good() -> int32 { 1 }\nbad() -> int32 { true }\n");
    let scope = item_scope(&db, input);
    let per_fn: Vec<usize> = scope
        .fns
        .iter()
        .map(|&(_, ptr)| {
            let fn_id = StableFnId::new(&db, input, ptr);
            // the error is type-only: lowering each body is clean.
            assert!(lower_fn(&db, fn_id).lowered.diagnostics.is_empty());
            typeck_fn(&db, fn_id).diagnostics.len()
        })
        .collect();
    assert_eq!(per_fn, vec![0, 1], "only `bad` carries a type diagnostic");
    assert!(
        hir_diagnostics(&db, input)
            .into_iter()
            .any(|(_, e)| matches!(e, hir::core::HirError::Type(_))),
        "the return-type mismatch must reach the file diagnostics via typeck_fn"
    );
}

// --- the signature firewall (S5) ---

const TWO_FNS: &str = "\
alpha() -> int32 {
    1
}

beta() -> int32 {
    2
}
";

/// editing one body must not re-run the *sibling* body's type check: with the
/// signature firewall, `item_scope` backdates (no signature moved) and the
/// unedited body's `lower_fn` backdates, so its `typeck_fn` is a cache hit -
/// observable as the *same* stored `Arc` across the revision. without the
/// firewall every body re-checks on every keystroke.
#[test]
fn body_edit_backdates_the_sibling_typeck() {
    let mut db = Database::default();
    let input = file(&db, TWO_FNS);

    // alpha is first, so editing beta's (later) body leaves alpha's node range -
    // and thus its StableFnId - identical across the edit.
    let alpha_ptr = item_scope(&db, input).fns[0].1;
    let alpha0 = typeck_fn(&db, StableFnId::new(&db, input, alpha_ptr)).0.clone();

    // edit only beta's body; every signature is untouched.
    input
        .set_text(&mut db)
        .to(TWO_FNS.replace("    2", "    2 + 2"));

    let alpha1 = typeck_fn(&db, StableFnId::new(&db, input, alpha_ptr)).0.clone();
    assert!(
        Arc::ptr_eq(&alpha0, &alpha1),
        "alpha's typeck_fn must cache-hit when only beta's body changed"
    );

    // and beta itself genuinely re-checked (its node moved, so a fresh key/value).
    let beta_ptr = item_scope(&db, input).fns[1].1;
    assert!(
        typeck_fn(&db, StableFnId::new(&db, input, beta_ptr))
            .diagnostics
            .is_empty(),
        "beta still type-checks clean after the edit"
    );
}

const FN_THEN_CONST: &str = "\
alpha() -> int32 {
    5
}

const int32 K = 10;
";

/// the firewall must not go stale: editing a *signature*-level item (here a
/// const initializer) changes the signature digest, so `item_scope` does not
/// backdate and every body re-runs - even one whose own text did not move.
/// alpha is before the const, so its node range/StableFnId is stable and the
/// re-run is observable as a *new* `Arc`.
#[test]
fn signature_edit_reruns_every_body() {
    let mut db = Database::default();
    let input = file(&db, FN_THEN_CONST);

    let alpha_ptr = item_scope(&db, input).fns[0].1;
    let alpha0 = typeck_fn(&db, StableFnId::new(&db, input, alpha_ptr)).0.clone();

    // edit the const's value; alpha's node range is unchanged (const is later).
    input.set_text(&mut db).to(FN_THEN_CONST.replace("= 10", "= 20"));

    let alpha1 = typeck_fn(&db, StableFnId::new(&db, input, alpha_ptr)).0.clone();
    assert!(
        !Arc::ptr_eq(&alpha0, &alpha1),
        "a signature/const edit must re-run every body's typeck (no stale skip)"
    );
}

// --------------------------------------------------------------------------
// S6 validation spike: parallel per-fn inference on salsa 0.27.
//
// Proves the version-sensitive parallel API shape before the segment's bigger
// pieces (the lock-free interner, the whole-file fan-out). The model salsa
// 0.27 gives: `Database: Send` (not `Sync`), so each worker owns a *clone* of
// the database (a cheap `Storage` handle bump onto the shared, internally
// synchronized memo tables) moved into the task; interned ids and inputs are
// valid across clones of the same storage. The per-fn `typeck_fn` query is
// already seal-isolated (its own interner clone), so this path parallelizes
// with no interner change at all - and is trivially deterministic, since no
// shared whole-file interner means no handle-order dependence (determinism
// law #2 is vacuous here; law #1 is enforced by the order-preserving collect).
// --------------------------------------------------------------------------

const FOUR_FNS: &str = "\
add(int32 a, int32 b) -> int32 { a + b }

sub(int32 a, int32 b) -> int32 { a - b }

mul(int32 a, int32 b) -> int32 { a * b }

neg(int32 x) -> int32 { 0 - x }

main() -> int32 { add(mul(2, 3), sub(neg(1), 4)) }
";

#[test]
fn parallel_per_fn_typeck_matches_serial() {
    use rayon::prelude::*;

    let db = Database::default();
    let input = file(&db, FOUR_FNS);
    let ptrs: Vec<SyntaxNodePtr> =
        item_scope(&db, input).fns.iter().map(|&(_, p)| p).collect();
    assert!(ptrs.len() >= 4, "spike needs several bodies to be meaningful");

    // serial: per-fn typeck in collection order. diagnostics carry baked-in
    // display strings + type *names* (not raw `TypeRef` handles), so their
    // debug form is handle-independent and safe to compare across runs.
    let serial: Vec<String> = ptrs
        .iter()
        .map(|&ptr| {
            let id = StableFnId::new(&db, input, ptr);
            format!("{:?}", typeck_fn(&db, id).diagnostics.entries())
        })
        .collect();

    // parallel: one owned db clone per body, moved into a rayon task via
    // `into_par_iter` (needs only `Database: Send`). order-preserving collect
    // keeps determinism law #1 (collection order, never completion order).
    let clones: Vec<(Database, SyntaxNodePtr)> =
        ptrs.iter().map(|&p| (db.clone(), p)).collect();
    let parallel: Vec<String> = clones
        .into_par_iter()
        .map(|(db2, ptr)| {
            let id = StableFnId::new(&db2, input, ptr);
            format!("{:?}", typeck_fn(&db2, id).diagnostics.entries())
        })
        .collect();

    assert_eq!(
        serial, parallel,
        "parallel per-fn typeck diverged from the serial order"
    );
}

// the whole-file fused walk (`effect::infer_file` -> `collect_results`) fans its
// per-body type+effect inference out across rayon, each body interning into the
// one shared lock-free interner. body-local types (string literals, array
// wrappers) are therefore interned in a nondeterministic order across runs, so
// their `TypeRef` handle *values* vary. this program has several bodies with
// such types; compiling it in independent fresh databases must still yield
// byte-identical C - the regression guard for determinism law #2 (no observable
// output may depend on handle numeric order; codegen emits typedefs in
// program-discovery order, never by handle).
const PARALLEL_DET_PROGRAM: &str = "\
structure Point { int32 x, int32 y, };

sum(int32 a, int32 b) -> int32 {
    let [int32; 3] xs = [a, b, 7];
    xs[0] + xs[1] + xs[2]
}

origin() -> int32 {
    let Point p = Point { x: 1, y: 2 };
    p.x + p.y
}

tag() -> int32 {
    let [int32; 2] ys = [9, 8];
    ys[0] - ys[1]
}

main() -> int32 {
    sum(1, 2) + origin() + tag()
}
";

#[test]
fn parallel_inference_is_deterministic() {
    // each fresh database builds its own interner under the parallel per-body
    // walk; equal C across them proves codegen never depends on handle values.
    let outputs: Vec<String> = (0..6)
        .map(|_| {
            let db = Database::default();
            let input = file(&db, PARALLEL_DET_PROGRAM);
            c_code(&db, input).to_string()
        })
        .collect();
    assert!(
        outputs[0].contains("int32_t sum") && outputs[0].len() > 200,
        "the determinism program must actually generate C (else the test is \
         vacuous); got {} bytes",
        outputs[0].len()
    );
    for (i, out) in outputs.iter().enumerate() {
        assert_eq!(
            out, &outputs[0],
            "C output of run {i} diverged - codegen depends on TypeRef handle \
             order (determinism law #2 violated under parallel interning)"
        );
    }
}
