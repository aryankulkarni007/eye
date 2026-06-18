use super::*;

#[test]
fn match_lowers_arms_and_pins_scrut_enum() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Rectangle;\n    \
         let int32 n = match sh {{\n        \
         Shape.Circle -> 0,\n        \
         Rectangle -> 1,\n        \
         Triangle -> 2,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        hir.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        hir.diagnostics
    );
    let (body, _match_id, arms, _scrut) = first_match(&hir);
    assert_eq!(arms.len(), 3);
    // structural lowering only: every arm (qualified `Shape.Circle` or bare
    // `Rectangle`/`Triangle`) resolves to a distinct variant pat. the scrutinee
    // and match result types are a typeck concern now (S2C C5).
    let mut seen = Vec::new();
    for arm in arms {
        match &body.pats[arm.pat] {
            Pat::Variant { idx, .. } => seen.push(*idx),
            other => panic!("non-variant pat: {other:?}"),
        }
    }
    seen.sort();
    assert_eq!(seen, vec![0, 1, 2]);
}

#[test]
fn match_wildcard_covers_remaining_variants() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 0,\n        \
         _ -> 99,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        hir.diagnostics.is_empty(),
        "wildcard should silence exhaustiveness: {:?}",
        hir.diagnostics
    );
    let (body, _, arms, _) = first_match(&hir);
    assert_eq!(arms.len(), 2);
    assert!(matches!(body.pats[arms[1].pat], Pat::Wildcard));
}

/// a QUALIFIED `Enum.Variant` pattern naming a variant the enum does not have
/// is a name-resolution error, which stays in lowering (no scrutinee type
/// needed). a BARE unknown ident, by contrast, is now a binding, not an error
/// (the name-based rule, S2C C2) - see the typeck crate's
/// `match_bare_unknown_ident_is_binding_not_error`. the type-directed match
/// judgments (domain, coverage, exhaustiveness, duplicate, unreachable,
/// cross-enum) moved to the typeck pass; their tests live in
/// `crates/typeck/tests/judgments.rs`.
#[test]
fn match_qualified_unknown_variant_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Shape.Square -> 0,\n        _ -> 1,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Resolve(ResolveError::NoSuchVariant { enum_name, variant })
                if enum_name == "Shape" && variant == "Square"
        )),
        "expected unknown-variant diag, got: {:?}",
        hir.diagnostics
    );
}

// a `let` struct destructure binding every field succeeds.
#[test]
fn destructure_binds_all_fields() {
    let src = "\
structure P { int32 x, int32 y, };
main() {
    let P p = P { x: 1, y: 2 };
    let P { x, y } = p;
    println(\"{} {}\", x, y);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).is_empty(),
        "expected a clean destructure, got: {:?}",
        hir.diagnostics
    );
}

// destructuring is exhaustive: a pattern that omits a field is an error naming
// the missing field.
#[test]
fn destructure_missing_field_is_non_exhaustive() {
    let src = "\
structure P { int32 x, int32 y, };
main() {
    let P p = P { x: 1, y: 2 };
    let P { x } = p;
    println(\"{}\", x);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DestructureNonExhaustive { ty, missing })
                if ty == "P" && missing.iter().any(|m| m == "y")
        )),
        "expected destructure non-exhaustive diag, got: {:?}",
        hir.diagnostics
    );
}

// --- EXPERIMENTAL: destructure error paths added 2026-06-07 ---
//
// these tests are marked experimental because they cover error-reporting paths
// that are correct today but whose diagnostic text or anchor placement may
// change as the pattern-matching surface stabilises through S3-S5.

// a struct destructure binding a field the type does not declare is an error.
#[test]
fn destructure_unknown_field_diagnosed() {
    let src = "\
structure P { int32 x, int32 y, };
main() {
    let P p = P { x: 1, y: 2 };
    let P { x, z } = p;
    println(\"{}\", x);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DestructureUnknownField { ty, field })
                if ty == "P" && field == "z"
        )),
        "expected unknown-field diagnostic naming `z`, got: {:?}",
        hir.diagnostics
    );
}

// a struct destructure binding the same field twice is an error (each field must
// be bound exactly once).
#[test]
fn destructure_duplicate_field_diagnosed() {
    let src = "\
structure P { int32 x, int32 y, };
main() {
    let P p = P { x: 1, y: 2 };
    let P { x, x } = p;
    println(\"{}\", x);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DestructureDuplicateField { field })
                if field == "x"
        )),
        "expected duplicate-field diagnostic, got: {:?}",
        hir.diagnostics
    );
}

// a `let` destructure whose type name does not resolve to a known struct is an
// error (e.g. a typo or a type from another namespace).
#[test]
fn destructure_not_a_struct_diagnosed() {
    let src = "\
main() {
    let int32 x = 5;
    let NotAStruct { x } = x;
    println(\"{}\", x);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DestructureNotAStruct { ty })
                if ty == "NotAStruct"
        )),
        "expected not-a-struct diagnostic, got: {:?}",
        hir.diagnostics
    );
}

// --- end experimental ---

// a bare ident over a primitive scrutinee is a binding, not a (failed) variant
// lookup - the type-directed bare-ident rule. the match is total.
#[test]
fn match_bare_ident_binds_over_primitive() {
    let src = "\
main() {
    let int32 n = 5;
    let int32 r = match n {
        0 -> 100,
        x -> x + 1,
    };
    println(\"{}\", r);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).is_empty(),
        "expected a clean binding match, got: {:?}",
        hir.diagnostics
    );
}

// match arm guard lowering tests.

/// a match arm guard (`A if flag -> body`) lowers without diagnostics and the
/// guard expression is recorded in `MatchArm::guard`.
#[test]
fn match_guard_lowers_cleanly() {
    let src = "\
enum E = A | B ;
main() {
    let bool flag = true;
    let E e = A;
    let int32 r = match e {
        A if flag -> 1,
        B -> 2,
        _ -> 0,
    };
    println(\"{}\", r);
}
";
    let hir = lower(src);
    assert!(
        hir.diagnostics.is_empty(),
        "expected clean guard match, got: {:?}",
        hir.diagnostics
    );
    let (_, _, arms, _) = first_match(&hir);
    assert!(arms[0].guard.is_some(), "expected guard on first arm");
    assert!(arms[1].guard.is_none(), "expected no guard on second arm");
}

/// REDESIGN I3: a value-position match in a ternary-shaped `if` branch lowers
/// clean. the HIR ban (`UnsupportedError::TernaryMatch`) that rejected this
/// shape was deleted at the track 2 cutover; MIR lowers the nested match in
/// place inside the branch (proven end-to-end by the e2e acid test on
/// `eyesrc/wierd.eye`). this guards that nothing reintroduces the ban.
#[test]
fn match_in_ternary_branch_is_accepted() {
    let hir = lower(
        "enum Color = Red | Blue;\n\
         pick(Color c) -> int32 {\n\
         \x20   let int32 r = if true { match c { Red -> 1, _ -> 0 } } else { 9 };\n\
         \x20   r\n\
         }\n",
    );
    assert!(
        diags(&hir).is_empty(),
        "value-position ternary match must succeed, got: {:?}",
        hir.diagnostics
    );
}

/// a let-bound match (the normal value position) stays clean - control against
/// a false positive in the ternary check.
#[test]
fn let_bound_match_is_clean() {
    let hir = lower(
        "enum Color = Red | Blue;\n\
         rank(Color c) -> int32 {\n\
         \x20   let int32 r = match c { Red -> 1, _ -> 0 };\n\
         \x20   r\n\
         }\n",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "expected no diagnostics, got: {:?}",
        hir.diagnostics
    );
}
