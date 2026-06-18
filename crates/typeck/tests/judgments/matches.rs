use super::*;

// ---- value-position MATCH-arm result-type consistency (MATCH.md steps 1-5,
// moved from hir tests with the S2 step-b migration) ----

const SHAPE_DECL: &str = "enum Shape = Circle | Rectangle | Triangle ;\n";

/// first arm is int32, a later arm is a string (`&[uint8; N]`): the
/// value-position match has no single result type, so the mismatching arm is
/// diagnosed.
#[test]
fn match_value_position_heterogeneous_arms_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> \"bad\",\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if *expected == "int32" && *found == "&[uint8; 3]"
        )),
        "expected arm-type-mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a `Point`-returning call in an arm of an int-typed match (regression: used
/// to silently emit ill-typed c).
#[test]
fn match_value_position_call_arm_type_mismatch_diagnosed() {
    let src = "\
structure Point { int32 x, int32 y, };
enum Color = Red | Green | Blue ;
unit() -> Point { Point { x: 1, y: 1 } }
pick() -> Color { Green }
main() {
    let int32 n = match pick() {
        Red -> 1,
        Green -> unit(),
        Blue -> 3,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if *found == "Point"
        )),
        "expected Point arm mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a match in a non-`let` value position (a call argument) is still
/// result-type checked.
#[test]
fn fn_arg_value_position_match_heterogeneous_arms_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
take(int32 n) -> int32 { n }
pick() -> Color { Green }
main() {
    let int32 a = take(match pick() { Red -> 1, Green -> pick(), Blue -> 3 });
    println(\"{}\", a);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if *found == "Color"
        )),
        "fn-arg value-position match must be arm-checked, got: {:?}",
        diags(&hir)
    );
}

/// a match as a function's implicit-return tail is value-position; the declared
/// return type is the result type, so a mismatching arm is caught.
#[test]
fn return_tail_match_heterogeneous_arms_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
pick() -> Color { Green }
sides(Color c) -> int32 {
    match c {
        Red -> 1,
        Green -> pick(),
        Blue -> 3,
    }
}
main() { println(\"{}\", sides(Red)); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if *expected == "int32" && *found == "Color"
        )),
        "return-tail match arm must be checked against the return type, got: {:?}",
        diags(&hir)
    );
}

/// statement-position MATCH has no result-type requirement (MATCH.md), so
/// differing arm-body types are not a mismatch.
#[test]
fn statement_position_match_heterogeneous_arms_not_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         match sh {{\n        \
         Circle -> 1,\n        Rectangle -> \"bad\",\n        Triangle -> 3,\n    }}\n    \
         println(\"done\");\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "statement-position match must not be result-type checked: {:?}",
        diags(&hir)
    );
}

/// no explicit binding type: the result type falls back to the first known arm
/// (int32); homogeneous arms produce no mismatch.
#[test]
fn untyped_let_homogeneous_match_arms_are_clean() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "homogeneous untyped match must be clean: {:?}",
        diags(&hir)
    );
}

/// a wider explicit binding (int64) over int-literal arms (typed int32): the
/// integer leniency means no false-positive mismatch.
#[test]
fn match_wide_int_let_no_false_positive() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int64 n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "integer widening must not false-positive: {:?}",
        diags(&hir)
    );
}

/// a match that is the tail of a body whose value is discarded (no declared
/// return) runs for effect like a statement-position match - no result type.
#[test]
fn void_tail_match_heterogeneous_arms_not_diagnosed() {
    let src = "\
enum Color = Red | Green | Blue ;
ic() -> int32 { 1 }
cc() -> Color { Red }
main() {
    match cc() {
        Red -> ic(),
        Green -> cc(),
        Blue -> ic(),
    }
}
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchArmTypeMismatch { .. }))),
        "value-discarded tail match must not be result-type checked, got: {:?}",
        diags(&hir)
    );
}

// ----------------------------------------------------------------------------
// match type-judgments (migrated from `crates/hir/src/core/tests.rs` with S2C
// C2). lowering now lowers match arms purely structurally; the scrutinee-domain,
// coverage, exhaustiveness, duplicate, unreachable, and domain-mismatch
// judgments are the typeck pass's (`check_matches`), so these run lowering +
// typeck. the structural lowering tests (arm shapes, scrutinee stamping) stay in
// the hir crate.
// ----------------------------------------------------------------------------

#[test]
fn match_non_exhaustive_diags_each_missing_variant() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        Circle -> 0,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    let missing = diags(&hir)
        .iter()
        .find_map(|e| match e {
            HirError::Pattern(PatternError::NonExhaustive { missing, .. }) => Some(missing.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing non-exhaustive diag: {:?}", diags(&hir)));
    assert!(missing.iter().any(|m| m == "Rectangle"), "got: {missing:?}");
    assert!(missing.iter().any(|m| m == "Triangle"), "got: {missing:?}");
}

#[test]
fn match_duplicate_arm_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 0,\n        Rectangle -> 1,\n        \
         Circle -> 2,\n        Triangle -> 3,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::DuplicateArm { variant }) if variant == "Circle"
        )),
        "missing dup diag: {:?}",
        diags(&hir)
    );
}

#[test]
fn match_arm_after_wildcard_is_unreachable() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         _ -> 0,\n        Triangle -> 1,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::UnreachableAfterWildcard))),
        "expected unreachable diag, got: {:?}",
        diags(&hir)
    );
}

/// an arm after a catch-all (a `_` wildcard OR a bare-ident binding, both
/// irrefutable) is unreachable. guards the MIR `ArmKind::Bind`/`Default` paths,
/// where two irrefutable arms would otherwise both write the default slot.
#[test]
fn match_multiple_irrefutable_arms_rejected() {
    for arms in ["n -> 1,\n        _ -> 2,", "n -> 1,\n        m -> 2,"] {
        let src = format!(
            "main() {{\n    let int32 x = 5;\n    \
             let int32 r = match x {{\n        {arms}\n    }};\n    \
             println(\"{{}}\", r);\n}}\n"
        );
        let hir = lower(&src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Pattern(PatternError::UnreachableAfterWildcard))),
            "expected unreachable diag for arms `{arms}`, got: {:?}",
            diags(&hir)
        );
    }
}

#[test]
fn match_cross_enum_pattern_diagnosed() {
    let src = format!(
        "{}enum Option = Some | None ;\nmain() {{\n    \
         let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Option.Some -> 0,\n        _ -> 1,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Resolve(ResolveError::PatternEnumMismatch { pattern_enum, .. })
                if pattern_enum == "Option"
        )),
        "expected cross-enum diag, got: {:?}",
        diags(&hir)
    );
}

// a scrutinee whose type is not a matchable domain (enum / int / char / bool) is
// diagnosed. `float64` is a scalar but not discrete, so it is rejected.
#[test]
fn match_non_matchable_scrut_diagnosed() {
    let src = "\
main() {
    let float64 x = 0.0;
    let int32 n = match x {
        _ -> 1,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchScrutineeNotEnum))),
        "expected non-matchable-domain diag, got: {:?}",
        diags(&hir)
    );
}

// an int match with no `_` is non-exhaustive (the domain is too large to
// enumerate).
#[test]
fn match_int_without_wildcard_is_non_exhaustive() {
    let src = "\
main() {
    let int32 x = 1;
    let int32 n = match x {
        1 -> 10,
        2 -> 20,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::NonExhaustivePrimitive { missing, .. }) if missing.is_empty()
        )),
        "expected open-domain non-exhaustive diag, got: {:?}",
        diags(&hir)
    );
}

// a bool match missing `false` is non-exhaustive, naming the missing value.
#[test]
fn match_bool_missing_value_is_non_exhaustive() {
    let src = "\
main() {
    let bool b = true;
    let int32 n = match b {
        true -> 1,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::NonExhaustivePrimitive { ty, missing })
                if ty == "bool" && missing.iter().any(|m| m == "false")
        )),
        "expected bool non-exhaustive diag, got: {:?}",
        diags(&hir)
    );
}

// a literal pattern whose domain disagrees with the scrutinee (a bool literal
// against an int scrutinee) is a domain mismatch.
#[test]
fn match_literal_domain_mismatch_diagnosed() {
    let src = "\
main() {
    let int32 x = 1;
    let int32 n = match x {
        true -> 1,
        _ -> 0,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Pattern(PatternError::PatternDomainMismatch { .. })
        )),
        "expected a domain-mismatch diag, got: {:?}",
        diags(&hir)
    );
}

/// a guarded arm does not discharge coverage of its discriminant: a full-variant
/// match with a guarded arm and no `_` is non-exhaustive, since the guard may be
/// false.
#[test]
fn guarded_arm_does_not_cover_for_exhaustiveness() {
    let hir = lower(
        "\
enum E = A | B ;
main() {
    let bool c = true;
    let E e = A;
    let int32 r = match e {
        A if c -> 1,
        B -> 2,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::NonExhaustive { .. }))),
        "expected non-exhaustive (guarded `A` does not cover), got: {:?}",
        diags(&hir)
    );
}

/// a guarded wildcard with NO unconditional catch-all is non-exhaustive: the
/// guard may be false for an uncovered case.
#[test]
fn match_guard_on_wildcard_without_catchall_rejected() {
    let src = "\
enum E = A | B;
main() {
    let E e = A;
    let bool flag = false;
    let int32 r = match e {
        A -> 1,
        _ if flag -> 9,
    };
    println(\"{}\", r);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(PatternError::NonExhaustive { .. }))),
        "expected non-exhaustive (guarded `_` does not cover B), got: {:?}",
        diags(&hir)
    );
}

// --- acceptance: clean matches the typeck pass must NOT diagnose ---

// an int scrutinee with literal arms plus a `_` is total - no diagnostics.
#[test]
fn match_int_literal_arms_clean() {
    let hir = lower(
        "\
main() {
    let int32 x = 2;
    let int32 n = match x {
        1 -> 10,
        2 -> 20,
        _ -> 0,
    };
    println(\"{}\", n);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_) | HirError::Type(_))),
        "expected a clean int match, got: {:?}",
        diags(&hir)
    );
}

// a bool match covering both `true` and `false` is total without a `_`.
#[test]
fn match_bool_both_values_is_exhaustive() {
    let hir = lower(
        "\
main() {
    let bool b = true;
    let int32 n = match b {
        true -> 1,
        false -> 0,
    };
    println(\"{}\", n);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "expected a clean bool match, got: {:?}",
        diags(&hir)
    );
}

/// a guard on a bare-ident binding catch-all (`x if cond`) is supported: the
/// binding is in scope for the guard, and an unconditional `_` makes the match
/// exhaustive.
#[test]
fn match_guard_on_binding_arm_supported() {
    let hir = lower(
        "\
main() {
    let int32 x = 5;
    let int32 r = match x {
        y if y > 0 -> 1,
        _ -> 0,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded binding catch-all should compile cleanly, got: {:?}",
        diags(&hir)
    );
}

/// a guard on a wildcard arm (`_ if cond`) is supported when a later
/// unconditional catch-all keeps the match exhaustive.
#[test]
fn match_guard_on_wildcard_arm_supported() {
    let hir = lower(
        "\
enum E = A | B;
main() {
    let E e = A;
    let bool flag = false;
    let int32 r = match e {
        A -> 1,
        _ if flag -> 9,
        _ -> 0,
    };
    println(\"{}\", r);
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded wildcard with trailing catch-all should compile cleanly, got: {:?}",
        diags(&hir)
    );
}

/// name-based classification (S2C C2): a bare ident that is NOT a known variant
/// is a binding (an irrefutable named wildcard), not an "unknown variant" error -
/// the rustc/rust-analyzer rule. over an enum scrutinee it is a catch-all, so the
/// match is exhaustive and clean. (the qualified form `Enum.Bad` still errors;
/// see the hir crate's `match_qualified_unknown_variant_diagnosed`.)
#[test]
fn match_bare_unknown_ident_is_binding_not_error() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         whatever -> 0,\n    }};\n    \
         println(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_) | HirError::Resolve(_))),
        "a bare unknown ident is a binding catch-all, not an error: {:?}",
        diags(&hir)
    );
}
