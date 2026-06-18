use super::*;

/// S3 assignment-non-value: an assignment used where a value is expected (here
/// a `let` initializer) is rejected. assignment is statement-only.
#[test]
fn value_position_assignment_is_rejected() {
    let hir = lower(
        "\
main() {
    mut int32 y = 0;
    let int32 x = y = 5;
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::AssignInValuePosition))),
        "expected AssignInValuePosition for the `y = 5` initializer, got: {:?}",
        diags(&hir)
    );
}

/// statement-position assignments stay legal: a bare `x = y;`, a compound
/// `x += y;`, and - critically - the branch-tail assignments of an `if` used
/// as a statement (those tails are discarded, not value-producing).
#[test]
fn statement_position_assignments_are_clean() {
    let hir = lower(
        "\
counter(bool up) {
    mut int32 n = 0;
    n = 1;
    n += 5;
    if up { n = 10 } else { n = 20 }
}

main() { counter(true); }
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::AssignInValuePosition))),
        "statement-position assignments must be clean, got: {:?}",
        diags(&hir)
    );
}

/// F1 (S3): a value-position `if` whose branches have incompatible types is
/// rejected (the `if` analogue of match-arm consistency).
#[test]
fn value_position_if_branch_mismatch_is_rejected() {
    let hir = lower(
        "\
main() {
    let bool c = true;
    let int32 x = if c { 1 } else { true };
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::IfBranchTypeMismatch { found, .. }) if found == "bool"
        )),
        "expected IfBranchTypeMismatch (then int32 vs else bool), got: {:?}",
        diags(&hir)
    );
}

/// a consistent value-`if` and a statement-position `if` with differing branch
/// types (its values discarded) both stay clean.
#[test]
fn if_branch_consistency_clean_cases() {
    let hir = lower(
        "\
main() {
    let bool c = true;
    let int32 x = if c { 1 } else { 2 };
    if c { 1 } else { true };
    println(\"{}\", x);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::IfBranchTypeMismatch { .. }))),
        "consistent value-if and statement-if must be clean, got: {:?}",
        diags(&hir)
    );
}

/// a literal in a value-position `if`/`match` branch adopts the expected width
/// (`site_coerce` forwards the expectation into branches), so a wider declared
/// type stays consistent - no spurious arm/branch mismatch.
#[test]
fn value_position_branch_literals_adopt_width() {
    let hir = lower(
        "\
choose(int64 k) -> int64 {
    let int64 x = match k { 0 -> 1, _ -> 2 };
    let int64 y = if k > 0 { 10 } else { 20 };
    x + y
}
main() -> int32 { 0 }
",
    );
    assert!(
        !diags(&hir).iter().any(|d| matches!(d, HirError::Type(_))),
        "branch literals must adopt int64: {:?}",
        diags(&hir)
    );
}
