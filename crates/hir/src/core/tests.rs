use super::*;
use ast::{AstNode, SourceFile};
use lexer::{Lexer, SourceText};

fn lower(src: &str) -> HIR {
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parser::parse(&lexed.tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    lower_source_file(file, &lexed.interner)
}

/// The concrete diagnostic kinds, for structural assertions. Tests match on
/// variants (and payloads) rather than message text, so rewording a message
/// never breaks a test.
fn diags(hir: &HIR) -> Vec<&HirError> {
    hir.diagnostics.entries().iter().map(|(_, e)| e).collect()
}

const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    let int32 x = 0;
    let int32 y = 0;
    mut Point p = Point { x, y };

    println(\"{}\", p.x);
}
";

#[test]
fn items_collected() {
    let hir = lower(MAIN_EYE);
    assert_eq!(hir.structs.len(), 1);
    assert_eq!(hir.functions.len(), 1);
    assert!(hir.items.structs.contains_key("Point"));
    assert!(hir.items.functions.contains_key("main"));
}

#[test]
fn shorthand_struct_lit_desugared() {
    let hir = lower(MAIN_EYE);
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    // find the StructLit init of `p`
    let mut sl_field_count = 0;
    for (_, expr) in body.exprs.iter() {
        if let Expr::StructLit { fields, .. } = expr {
            sl_field_count = fields.len();
            for f in fields {
                // shorthand must be materialized: every field has a real
                // ExprId (no Option). The synthesized expr resolves to
                // the local of the same name.
                let inner = &body.exprs[f.value];
                match inner {
                    Expr::Path(Resolution::Local(_)) => {}
                    other => panic!(
                        "shorthand field {} did not desugar to a Local path: {:?}",
                        f.name, other
                    ),
                }
            }
        }
    }
    assert_eq!(sl_field_count, 2, "Point literal has two fields");
}

#[test]
fn duplicate_struct_emits_diagnostic() {
    let hir = lower(
        "\
structure Point {
    int32 x,
};

structure Point {
    int32 y,
};

main() {}
",
    );
    assert_eq!(
        hir.diagnostics.len(),
        1,
        "expected one diagnostic, got: {:?}",
        hir.diagnostics
    );
    assert!(
        matches!(diags(&hir)[0], HirError::Resolve(ResolveError::DuplicateItem { name }) if name == "Point"),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
    // both struct arena slots persist so existing IDs stay valid
    assert_eq!(hir.structs.len(), 2);
}

/// A union literal must set exactly one member - overlapping storage means
/// a second field silently overwrites the first. One field is clean; two
/// emits a diagnostic.
#[test]
fn union_literal_must_set_exactly_one_field() {
    let one = lower(
        "\
union Bits {
    int64 i,
    float64 f,
};

main() {
    mut Bits b = Bits { i: 1 };
}
",
    );
    assert!(one.diagnostics.is_empty(), "{:?}", one.diagnostics);

    let two = lower(
        "\
union Bits {
    int64 i,
    float64 f,
};

main() {
    mut Bits b = Bits { i: 1, f: 2.0 };
}
",
    );
    assert!(
        diags(&two)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::UnionLiteralFieldCount { .. }))),
        "expected a one-field diagnostic, got: {:?}",
        two.diagnostics
    );
}

#[test]
fn duplicate_fn_emits_diagnostic() {
    let hir = lower(
        "\
main() {}
main() {}
",
    );
    assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
    assert!(
        matches!(diags(&hir)[0], HirError::Resolve(ResolveError::DuplicateItem { name }) if name == "main"),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
    assert_eq!(hir.functions.len(), 2);
}

#[test]
fn fn_and_struct_with_same_name_collide() {
    // Cross-namespace collision should still be flagged: in v0.1 the
    // resolver treats both namespaces as one for name-resolution.
    let hir = lower(
        "\
structure Foo {
    int32 x,
};

Foo() {}
",
    );
    assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
    assert!(
        matches!(diags(&hir)[0], HirError::Resolve(ResolveError::DuplicateItem { name }) if name == "Foo"),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
}

#[test]
fn well_formed_program_has_no_diagnostics() {
    let hir = lower(MAIN_EYE);
    assert!(
        hir.diagnostics.is_empty(),
        "expected zero diagnostics, got: {:?}",
        hir.diagnostics
    );
}

/// Regression for the `NameRef::nth(1)` bug: when the base of a field
/// access is itself a field expression (`a.b.c`), the outer FieldExpr
/// has only one direct NameRef child (the field name); `nth(1)` would
/// silently return `None` and drop the name.
#[test]
fn nested_field_access_resolves_field_name() {
    let src = "\
main() {
    println(\"{}\", a.b.c);
}
";
    let hir = lower(src);
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    // collect every Expr::Field name; expect `c` and `b` to be present.
    let mut names: Vec<&str> = body
        .exprs
        .iter()
        .filter_map(|(_, e)| match e {
            Expr::Field { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["b", "c"], "nested field access dropped a name");
}

/// Regression for the lower-block ordering bug: a block's tail expression
/// (typically a `loop { ... }` body) used to be lowered *before* the
/// preceding stmts, so locals defined by those stmts were not yet in
/// scope. NameRefs inside the loop body fell through to
/// `Resolution::Unresolved`, which downstream made auto-deref on field
/// access impossible.
#[test]
fn tail_expression_sees_locals_defined_by_preceding_stmts() {
    let src = "\
structure P {
    int32 x,
};

main() {
    mut P p = P { x: 0 };
    mut &P p_ref = &p;
    loop {
        if p_ref.x > 10 { break; }
        p_ref.x = p_ref.x + 1;
    }
}
";
    let hir = lower(src);
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    // Every `Path` expression that names `p_ref` must resolve to a
    // Local, not fall through to Unresolved.
    let unresolved_p_ref = body
        .exprs
        .iter()
        .any(|(_, e)| matches!(e, Expr::Path(Resolution::Unresolved(n)) if n.as_str() == "p_ref"));
    assert!(
        !unresolved_p_ref,
        "p_ref inside the tail loop body did not resolve to the outer local"
    );
}

#[test]
fn call_expr_records_user_function_return_type() {
    let hir = lower(
        "\
answer() -> int32 {
    42
}

main() {
    let int32 x = answer();
}
",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        hir.diagnostics
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    let call_id = body
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, Expr::Call { .. }).then_some(id))
        .expect("main contains a call");

    assert_eq!(
        body.expr_types.get(call_id.into()),
        Some(&hir.types.int32_ty())
    );
}

#[test]
fn call_expr_records_extern_function_return_type() {
    let hir = lower(
        "\
extern {
    strlen(ptr s) -> usize;
}

main() {
    let usize n = strlen(\"abc\" as ptr);
}
",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        hir.diagnostics
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    let call_id = body
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, Expr::Call { .. }).then_some(id))
        .expect("main contains a call");

    assert_eq!(
        body.expr_types.get(call_id.into()),
        Some(&hir.types.usize_ty())
    );
}

#[test]
fn explicit_let_initializer_type_mismatch_is_diagnosed() {
    let hir = lower(
        "\
answer() -> int32 {
    42
}

main() {
    let string x = answer();
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::LetTypeMismatch { expected, got })
                if *expected == "string" && *got == "int32"
        )),
        "expected explicit let mismatch diagnostic, got: {:?}",
        hir.diagnostics
    );
}

#[test]
fn explicit_let_unknown_initializer_type_does_not_diagnose_mismatch() {
    let hir = lower(
        "\
main() {
    let int32 x = unknown();
}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::LetTypeMismatch { .. }))),
        "unknown initializer type should not cascade into mismatch: {:?}",
        hir.diagnostics
    );
}

// ---- v0.3 match lowering ----

/// Walk the HIR for the `main` body and return the first `Expr::Match`
/// it finds. Tests assume exactly one per fixture.
fn first_match(hir: &HIR) -> (&Body, ExprId, &[MatchArm], ExprId) {
    let main_id = *hir.items.functions.get("main").expect("main fn");
    let body_id = hir.functions[main_id].body.expect("main body");
    let body = &hir.bodies[body_id];
    for (id, expr) in body.exprs.iter() {
        if let Expr::Match { scrut, arms } = expr {
            return (body, id, arms.as_slice(), *scrut);
        }
    }
    panic!("no Expr::Match in main body");
}

const SHAPE_DECL: &str = "enum Shape = Circle | Rectangle | Triangle ;\n";

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
    let (body, match_id, arms, scrut) = first_match(&hir);
    assert_eq!(arms.len(), 3);
    // scrutinee type pinned to Shape.
    let types = &hir.types;
    let scrut_ty = body
        .expr_types
        .get(scrut.into())
        .map(|ty| types.lookup(*ty));
    match scrut_ty {
        Some(TypeKind::Path(n)) => assert_eq!(n.as_str(), "Shape"),
        other => panic!("scrut type missing/wrong: {other:?}"),
    }
    // match expression itself carries the arm-body type so M5 codegen can
    // declare `int32 _matchN;` for the hoist temp.
    let match_ty = body
        .expr_types
        .get(match_id.into())
        .map(|ty| types.lookup(*ty));
    match match_ty {
        Some(TypeKind::Path(n)) => assert_eq!(n.as_str(), "int32"),
        other => panic!("match expr type missing/wrong: {other:?}"),
    }
    // every arm resolved to a distinct Variant pat.
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
        .unwrap_or_else(|| panic!("missing non-exhaustive diag: {:?}", hir.diagnostics));
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
        hir.diagnostics
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
        hir.diagnostics
    );
}

/// Multiple irrefutable arms are rejected at HIR: an arm after a catch-all
/// (a `_` wildcard OR a bare-ident binding, both irrefutable over a primitive)
/// is unreachable. This guards the MIR `ArmKind::Bind`/`Default` paths, where two
/// irrefutable arms would otherwise both write the default slot and the last
/// would silently win. Covers `bind -> _` and `bind -> bind`.
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
            hir.diagnostics
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
        hir.diagnostics
    );
}

#[test]
fn match_unknown_variant_diagnosed() {
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Square -> 0,\n        _ -> 1,\n    }};\n    \
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

// A scrutinee whose type is not a matchable domain (enum / int / char / bool) is
// diagnosed. `float64` is a scalar but not discrete, so it is rejected. (An int
// scrutinee, by contrast, is now matchable - see `match_int_literal_arms`.)
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
        hir.diagnostics
    );
}

// An int scrutinee with literal arms plus a `_` is a valid, total match - no
// diagnostics. Exercises the int domain and `ArmTest::Const` lowering.
#[test]
fn match_int_literal_arms() {
    let src = "\
main() {
    let int32 x = 2;
    let int32 n = match x {
        1 -> 10,
        2 -> 20,
        _ -> 0,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).is_empty(),
        "expected a clean int match, got: {:?}",
        hir.diagnostics
    );
}

// An int match with no `_` is non-exhaustive (the domain is too large to
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
        hir.diagnostics
    );
}

// A bool match covering both `true` and `false` is total without a `_`.
#[test]
fn match_bool_both_values_is_exhaustive() {
    let src = "\
main() {
    let bool b = true;
    let int32 n = match b {
        true -> 1,
        false -> 0,
    };
    println(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).is_empty(),
        "expected a clean bool match, got: {:?}",
        hir.diagnostics
    );
}

// A bool match missing `false` is non-exhaustive, naming the missing value.
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
        hir.diagnostics
    );
}

// A `let` struct destructure binding every field lowers cleanly.
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

// Destructuring is exhaustive: a pattern that omits a field is an error naming
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
// These tests are marked experimental because they cover error-reporting paths
// that are correct today but whose diagnostic text or anchor placement may
// change as the pattern-matching surface stabilises through S3-S5.

// A struct destructure binding a field the type does not declare is an error.
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

// A struct destructure binding the same field twice is an error (each field must
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

// A `let` destructure whose type name does not resolve to a known struct is an
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

// A bare ident over a primitive scrutinee is a binding, not a (failed) variant
// lookup - the type-directed bare-ident rule. The match is total.
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

// A literal pattern whose domain disagrees with the scrutinee (a bool literal
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
        hir.diagnostics
    );
}

// Match arm guard lowering tests.

/// A match arm guard (`A if flag -> body`) lowers without diagnostics and the
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

/// A guard on a bare-ident binding catch-all (`x if cond`) is supported: the
/// binding is in scope for the guard, and an unconditional `_` makes the match
/// exhaustive. Compiles with no pattern diagnostic.
#[test]
fn match_guard_on_binding_arm_supported() {
    let src = "\
main() {
    let int32 x = 5;
    let int32 r = match x {
        y if y > 0 -> 1,
        _ -> 0,
    };
    println(\"{}\", r);
}
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded binding catch-all should compile cleanly, got: {:?}",
        hir.diagnostics
    );
}

/// A guarded arm does not discharge coverage of its discriminant: a full-variant
/// match with a guarded arm and no `_` is non-exhaustive, since the guard may be
/// false. This prevents a value-position match's hoist temp being read
/// uninitialized when no arm fires.
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
        hir.diagnostics
    );
}

/// A guard on a wildcard arm (`_ if cond`) is supported when a later
/// unconditional catch-all keeps the match exhaustive. The guarded `_` does not
/// discharge coverage, so the trailing `_ -> 0` is required and reachable.
#[test]
fn match_guard_on_wildcard_arm_supported() {
    let src = "\
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
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Pattern(_))),
        "guarded wildcard with trailing catch-all should compile cleanly, got: {:?}",
        hir.diagnostics
    );
}

/// A guarded wildcard with NO unconditional catch-all is non-exhaustive: the
/// guard may be false for an uncovered case, leaving the match with no arm. This
/// is the safety property that keeps a value-position hoist temp initialized.
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
        hir.diagnostics
    );
}

/// `[T; N]` lowers to `TypeRef::Array` with the literal length, an `[...]`
/// initializer to `Expr::ArrayLit`, and `xs[i]` to `Expr::Index`.
#[test]
fn array_type_literal_and_index_lower() {
    let hir = lower(
        "\
main() {
    let [int32; 3] xs = [1, 2, 3];
    let int32 a = xs[0];
}
",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body_id = hir.functions[main_id].body.expect("main has body");
    let body = &hir.bodies[body_id];

    // the `xs` local carries an Array type with the parsed length.
    let int32_ty = hir.types.int32_ty();
    let types = &hir.types;
    let array_local = body
        .locals
        .iter()
        .find_map(|(_, l)| l.ty.and_then(|ty| ty.as_array(&types)));
    let (elem, len) = array_local.expect("xs local has an Array type");
    assert_eq!(len, 3, "array length parsed from literal");
    assert_eq!(elem, int32_ty, "element type");

    // both an ArrayLit and an Index expression were produced.
    assert!(
        body.exprs
            .iter()
            .any(|(_, e)| matches!(e, Expr::ArrayLit(v) if v.len() == 3)),
        "expected a 3-element ArrayLit"
    );
    assert!(
        body.exprs
            .iter()
            .any(|(_, e)| matches!(e, Expr::Index { .. })),
        "expected an Index expr"
    );
}

/// `[T; N]` requires `N` to be a compile-time value: an integer literal or a
/// `const` (a const-expr over those). A runtime local is not a constant, so it
/// is rejected (and must not lower silently as length 0).
#[test]
fn non_integer_literal_array_len_emits_diagnostic() {
    let hir = lower(
        "\
main() {
    let int32 n = 3;
    let [int32; n] xs = [1, 2, 3];
}
",
    );

    assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
    assert!(
        matches!(
            diags(&hir)[0],
            HirError::Const(ConstError::ConstUnknownName { .. })
        ),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
}

/// The const-expr evaluator folds literals, the operator set, and references to
/// other consts into a scalar [`ConstValue`] on each [`Const`].
#[test]
fn consts_fold_to_values() {
    let hir = lower(
        "\
const int32 MAX = 100;
const int32 DBL = MAX * 2;
const int32 NEG = 0 - 5;
const bool BIG = MAX > 50;
const char MARK = 'A';
const float64 HALF = 3.0 / 2.0;
const int32 TRUNC = HALF as int32;
main() {}
",
    );
    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
    let val = |name: &str| hir.consts[hir.items.consts[name]].value.clone();
    assert_eq!(val("MAX"), Some(ConstValue::Int(100)));
    assert_eq!(val("DBL"), Some(ConstValue::Int(200)));
    assert_eq!(val("NEG"), Some(ConstValue::Int(-5)));
    assert_eq!(val("BIG"), Some(ConstValue::Bool(true)));
    assert_eq!(val("MARK"), Some(ConstValue::Char('A')));
    assert_eq!(val("HALF"), Some(ConstValue::Float(1.5)));
    // a numeric `as` cast folds: float 1.5 truncates to int 1.
    assert_eq!(val("TRUNC"), Some(ConstValue::Int(1)));
}

/// A `const` whose initializer references itself (directly or through a chain)
/// is a definition cycle, diagnosed once and left poisoned (`value == None`).
#[test]
fn const_cycle_is_rejected() {
    let hir = lower(
        "\
const int32 A = B;
const int32 B = A;
main() {}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Const(ConstError::ConstCycle { .. }))),
        "expected a const cycle diagnostic: {:?}",
        diags(&hir)
    );
    assert_eq!(hir.consts[hir.items.consts["A"]].value, None);
}

/// A const sharing a name with another item is a duplicate, like any item clash.
#[test]
fn duplicate_const_name_is_rejected() {
    let hir = lower(
        "\
const int32 X = 1;
const int32 X = 2;
main() {}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Resolve(ResolveError::DuplicateItem { .. }))),
        "expected a duplicate-item diagnostic: {:?}",
        diags(&hir)
    );
}

/// A name the C backend emits verbatim (field, parameter, function, enum
/// variant) that is a C keyword is rejected at collection (R010): emitted
/// verbatim it would be illegal C (`.struct = ...`). Non-keyword names that
/// merely *look* C-ish (`data`, `value`) are untouched.
#[test]
fn c_keyword_names_are_rejected() {
    let hir = lower(
        "\
structure Syllable {
    string struct,
    int32 data,
};
typedef(int32 switch) {}
main() {}
",
    );
    let keyword_errors: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Resolve(ResolveError::NameIsCKeyword { name, .. }) => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        keyword_errors,
        ["struct", "typedef", "switch"],
        "expected exactly the keyword names rejected: {:?}",
        diags(&hir)
    );
}

/// A top-level `const` integer resolves as an array length (A6,
/// `docs/design/HORIZON0.md`): `const usize N = 4; [int32; N]` lowers to a length-4
/// array, and a const-expr (`N * 2`) folds too.
#[test]
fn const_array_length_resolves() {
    let hir = lower(
        "\
const usize N = 4;
main() {
    let [int32; N] xs = [1, 2, 3, 4];
    let [int32; N * 2] ys = [0, 0, 0, 0, 0, 0, 0, 0];
}
",
    );

    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    let types = &hir.types;
    let lens: Vec<u64> = body
        .stmts
        .iter()
        .filter_map(|(_, s)| match s {
            Stmt::Let { ty: Some(ty), .. } => ty.as_array(&types).map(|(_, len)| len),
            _ => None,
        })
        .collect();
    assert_eq!(lens, vec![4, 8]);
}

/// The repeat literal `[value; N]` resolves its count via the same const
/// machinery as a `[T; N]` type length: a literal or a `const`.
#[test]
fn array_repeat_resolves_const_count() {
    let hir = lower(
        "\
const usize N = 4;
main() {
    let [int32; N] xs = [7; N];
    println(\"{}\", xs[0]);
}
",
    );
    assert_eq!(hir.diagnostics.len(), 0, "{:?}", hir.diagnostics);
}

/// A repeat literal with a non-const count is a `Const` error - a runtime local
/// cannot be a compile-time array length.
#[test]
fn array_repeat_non_const_count_rejected() {
    let hir = lower(
        "\
main() {
    let int32 n = 3;
    let [int32; 3] xs = [0; n];
    println(\"{}\", xs[0]);
}
",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(e, HirError::Const(_))),
        "expected a Const diagnostic for a non-const repeat count, got: {:?}",
        hir.diagnostics
    );
}

/// An `if` used as a value must yield a value on every path. An else-less `if`
/// as a `let` initializer is rejected (it leaves the binding uninitialized when
/// the condition is false), while a diverging branch (`{ return; }`) is allowed
/// because it never falls through.
#[test]
fn else_less_if_in_value_position_rejected() {
    let reject = lower(
        "\
main() {
    let bool c = false;
    let int32 x = if c { 5 };
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&reject)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::VoidValueInValuePosition))),
        "expected VoidValueInValuePosition, got: {:?}",
        reject.diagnostics
    );

    // A diverging then-branch is fine: the `else` supplies the value.
    let ok = lower(
        "\
pick(int32 c) -> int32 {
    let int32 x = if c < 0 { return 99; } else { 2 };
    x
}
main() { println(\"{}\", pick(5)); }
",
    );
    assert_eq!(ok.diagnostics.len(), 0, "{:?}", ok.diagnostics);
}

/// An untyped `let` is rejected. Type inference is on hiatus, so a binding needs
/// an explicit type; without one it would reach codegen as an `Error` placeholder
/// (`void* /* ERROR TY */`).
#[test]
fn untyped_let_requires_annotation() {
    let hir = lower(
        "\
main() {
    let x = 5;
    println(\"{}\", x);
}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MissingTypeAnnotation { .. }))),
        "expected MissingTypeAnnotation, got: {:?}",
        hir.diagnostics
    );
}

/// A typed array binding must initialize exactly the declared number of
/// elements. C accepts short initializers and zero-fills the rest, but Eye
/// reports the mismatch explicitly.
#[test]
fn array_decl_initializer_len_mismatch_emits_diagnostic() {
    let hir = lower(
        "\
main() {
    let [int32; 3] xs = [1, 2];
}
",
    );

    assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
    assert!(
        matches!(
            diags(&hir)[0],
            HirError::Type(TypeError::ArrayInitLenMismatch { .. })
        ),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
    );
}

// ---- value-position match result type (MATCH.md steps 1-5) ----

#[test]
fn match_value_position_heterogeneous_arms_diagnosed() {
    // First arm is int32, a later arm is a string (`&[uint8; N]`, HORIZON0 C3):
    // the value-position match has no single result type, so the mismatching arm
    // is diagnosed.
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
        hir.diagnostics
    );
}

#[test]
fn match_value_position_call_arm_type_mismatch_diagnosed() {
    // Regression for the review finding: a `Point`-returning call in an arm of
    // an int-typed match used to silently emit ill-typed C. Now diagnosed.
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
        hir.diagnostics
    );
}

#[test]
fn match_wide_int_let_records_binding_type_without_false_positive() {
    // A wider explicit binding (int64) over int-literal arms (typed int32):
    // integer leniency means no mismatch diag, and the explicit type is
    // re-recorded as the match type so codegen declares an `int64_t` temp.
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
        hir.diagnostics
    );
    let (body, match_id, _, _) = first_match(&hir);
    let types = &hir.types;
    match body
        .expr_types
        .get(match_id.into())
        .map(|ty| types.lookup(*ty))
    {
        Some(TypeKind::Path(n)) => assert_eq!(n.as_str(), "int64"),
        other => panic!("explicit binding type not recorded on match: {other:?}"),
    }
}

#[test]
fn statement_position_match_heterogeneous_arms_not_diagnosed() {
    // Statement-position match has no result type requirement (MATCH.md), so
    // differing arm-body types are not a mismatch - only let-bound matches are
    // checked.
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
        hir.diagnostics
    );
}

#[test]
fn untyped_let_homogeneous_match_arms_are_clean() {
    // No explicit binding type: result type falls back to the first known arm
    // (int32); homogeneous arms produce no mismatch.
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
        hir.diagnostics
    );
}

#[test]
fn fn_arg_value_position_match_heterogeneous_arms_diagnosed() {
    // A match in a non-`let` value position (function-call argument) is still
    // result-type checked: heterogeneous arms used to silently coerce in C.
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
        hir.diagnostics
    );
}

#[test]
fn return_tail_match_heterogeneous_arms_diagnosed() {
    // A match as a function's implicit-return tail is value-position; the
    // declared return type is the result type, so a mismatching arm is caught.
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
        hir.diagnostics
    );
}

#[test]
fn void_tail_match_heterogeneous_arms_not_diagnosed() {
    // A match that is the tail of a body whose value is discarded (no declared
    // return) runs for effect like a statement-position match - no result type,
    // so differing arm types are not a mismatch.
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
        hir.diagnostics
    );
}

#[test]
fn return_type_mismatch_non_match_tail_diagnosed() {
    // The general tail-vs-declared-return-type check: a function returning
    // int32 whose tail produces an enum value is diagnosed.
    let src = "\
enum Color = Red | Green | Blue ;
bad() -> int32 { Red }
main() { println(\"{}\", bad()); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnTypeMismatch { expected, found })
                if *expected == "int32" && *found == "Color"
        )),
        "expected return-type-mismatch diag, got: {:?}",
        hir.diagnostics
    );
}

#[test]
fn bool_returning_comparison_tail_is_clean() {
    // Comparison operators are typed `bool`, so a `-> bool` function whose tail
    // is a comparison must NOT be flagged as a return-type mismatch. Guards the
    // false positive that motivated typing comparison results as bool.
    let src = "\
gt(int32 a, int32 b) -> bool { a > b }
main() { println(\"{}\", gt(3, 1)); }
";
    let hir = lower(src);
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "comparison tail must type as bool and not mismatch a bool return, got: {:?}",
        hir.diagnostics
    );
}

/// Manual dump - run with `cargo test -p eye-hir dump -- --nocapture`.
#[test]
fn dump_main_eye() {
    let hir = lower(MAIN_EYE);
    eprintln!("---- HIR.items ----\n{:#?}", hir.items);
    eprintln!("---- HIR.structs ----\n{:#?}", hir.structs);
    eprintln!("---- HIR.fields ----\n{:#?}", hir.fields);
    eprintln!("---- HIR.functions ----\n{:#?}", hir.functions);
    for (id, body) in hir.bodies.iter() {
        eprintln!("---- Body {:?} ----", id);
        eprintln!("locals: {:#?}", body.locals);
        eprintln!("pats:   {:#?}", body.pats);
        eprintln!("stmts:  {:#?}", body.stmts);
        eprintln!("exprs:  {:#?}", body.exprs);
        eprintln!("block:  {:?}", body.block);
    }
}

/// F3 / S1: a struct literal omitting a declared field is an error, naming the
/// missing field. Garbage-in-C otherwise.
#[test]
fn incomplete_struct_literal_emits_diagnostic() {
    let hir = lower(
        "structure Point { int32 x, int32 y, int32 z, };\n\
         main() {\n    let Point p = Point { x: 1, y: 2 };\n}\n",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::StructLitMissingFields { fields, .. })
                if fields.iter().any(|f| f == "z")
        )),
        "expected a missing-field diagnostic naming `z`, got: {:?}",
        hir.diagnostics
    );
}

/// F3: a struct literal naming a field the type does not declare is an error.
#[test]
fn unknown_struct_field_emits_diagnostic() {
    let hir = lower(
        "structure Point { int32 x, int32 y, };\n\
         main() {\n    let Point p = Point { x: 1, y: 2, w: 9 };\n}\n",
    );
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::StructLitUnknownFields { fields, .. })
                if fields.iter().any(|f| f == "w")
        )),
        "expected an unknown-field diagnostic naming `w`, got: {:?}",
        hir.diagnostics
    );
}

/// A struct literal that names every declared field exactly once is clean.
#[test]
fn complete_struct_literal_has_no_diagnostic() {
    let hir = lower(
        "structure Point { int32 x, int32 y, };\n\
         main() {\n    let Point p = Point { x: 1, y: 2 };\n}\n",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "expected no diagnostics, got: {:?}",
        hir.diagnostics
    );
}

/// F2 (`if x = 5`) is rejected in the parser now
/// (GrammarError::AssignInIfCondition); see the parser crate's
/// `assignment_in_if_condition_is_rejected`. A genuine comparison stays clean.
#[test]
fn comparison_in_if_condition_is_clean() {
    let hir = lower(
        "main() {\n    mut int32 x = 0;\n    if x == 5 {\n        println(\"hi\");\n    }\n}\n",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "expected no diagnostics, got: {:?}",
        hir.diagnostics
    );
}

// --- v0.7 arrays first-class + latent gaps ---

/// A4: a literal index past a fixed array's length is a hard error - C would
/// only warn.
#[test]
fn literal_array_index_out_of_bounds_is_rejected() {
    let hir =
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    println(\"{}\", xs[9]);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::IndexOutOfBounds { .. }))),
        "expected an out-of-bounds diagnostic, got: {:?}",
        hir.diagnostics
    );
}

/// An in-bounds literal index stays clean (control for A4).
#[test]
fn in_bounds_literal_index_is_clean() {
    let hir =
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    println(\"{}\", xs[3]);\n}\n");
    assert!(
        hir.diagnostics.is_empty(),
        "expected no diagnostics, got: {:?}",
        hir.diagnostics
    );
}

/// A3: the `len(xs)` intrinsic lowers to a `usize` integer constant carrying
/// the type's length, not a call.
#[test]
fn array_len_is_usize_constant() {
    let hir = lower(
        "main() {\n    let [int32; 5] xs = [1, 2, 3, 4, 5];\n    println(\"{}\", len(xs));\n}\n",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    // `len` folds to `(usize)5`: a usize-typed cast over the literal length, so
    // it prints with `%zu` without a varargs type mismatch.
    let types = &hir.types;
    let has_len_const = body.exprs.iter().any(|(id, e)| {
        matches!(e, Expr::Cast { operand, .. }
            if matches!(body.exprs[*operand], Expr::Literal(Literal::Int(5))))
            && matches!(
                body.expr_types.get(id.into()).map(|ty| types.lookup(*ty)),
                Some(TypeKind::Path(n)) if n == "usize"
            )
    });
    assert!(
        has_len_const,
        "expected `len(xs)` to lower to a usize constant 5; exprs: {:?}",
        body.exprs.iter().collect::<Vec<_>>()
    );
    // and the call to `len` did not survive as a call expression.
    assert!(
        !body.exprs.iter().any(|(_, e)| matches!(
            e,
            Expr::Call { callee, .. }
                if matches!(&body.exprs[*callee], Expr::Path(Resolution::Unresolved(n)) if n == "len")
        )),
        "`len(xs)` must not lower to a call"
    );
    assert!(
        hir.diagnostics.is_empty(),
        "`len(xs)` on an array is valid; diagnostics: {:?}",
        hir.diagnostics
    );
}

/// `len` on a non-array argument, and `.len` field syntax on an array, are both
/// diagnostics: the first is a type error, the second steers to `len(x)`.
#[test]
fn len_misuse_is_diagnosed() {
    let non_array = lower("main() {\n    let int32 x = 0;\n    println(\"{}\", len(x));\n}\n");
    assert_eq!(
        non_array.diagnostics.len(),
        1,
        "`len` on a non-array must diagnose; got: {:?}",
        non_array.diagnostics
    );

    let dot_form =
        lower("main() {\n    let [int32; 3] xs = [1, 2, 3];\n    println(\"{}\", xs.len);\n}\n");
    assert_eq!(
        dot_form.diagnostics.len(),
        1,
        "`.len` field form must steer to `len(x)`; got: {:?}",
        dot_form.diagnostics
    );
}

/// `len` never evaluates its operand (it reads the length from the type), so a
/// computed operand like `len(f())` would silently discard the call. The
/// operand is restricted to a place (variable/field/index/deref); a call or an
/// array literal is rejected.
#[test]
fn len_of_computed_value_is_rejected() {
    let call_form = lower(
        "mk() -> [int32; 3] {\n    [1, 2, 3]\n}\nmain() {\n    println(\"{}\", len(mk()));\n}\n",
    );
    assert_eq!(
        call_form.diagnostics.len(),
        1,
        "`len(f())` must be rejected (the call would be discarded); got: {:?}",
        call_form.diagnostics
    );

    let literal_form = lower("main() {\n    println(\"{}\", len([1, 2, 3]));\n}\n");
    assert_eq!(
        literal_form.diagnostics.len(),
        1,
        "`len([..])` must be rejected (not a place); got: {:?}",
        literal_form.diagnostics
    );
}

/// `len` on an array reference still works without an explicit deref: `&[T; N]`
/// is a place and one ref is peeled. `len(*r)` works too. Both fold to the
/// length with no diagnostic.
#[test]
fn len_through_reference_is_accepted() {
    let hir = lower(
        "sum(&[int32; 3] r) -> usize {\n    len(r)\n}\nmain() {\n    mut [int32; 3] a = [1, 2, 3];\n    println(\"{}\", sum(&a));\n}\n",
    );
    assert!(
        hir.diagnostics.is_empty(),
        "`len(r)` on `&[T; N]` is valid (auto-deref kept); diagnostics: {:?}",
        hir.diagnostics
    );
}

/// A whole array is a struct in the C backend, so a binary operator on it would
/// emit invalid C. Every operator family is rejected in lowering.
#[test]
fn binary_op_on_array_is_rejected() {
    for op in ["==", "+", "<"] {
        let src = format!(
            "main() {{\n    let [int32; 2] a = [1, 2];\n    let [int32; 2] b = [3, 4];\n    let x = a {op} b;\n}}\n"
        );
        let hir = lower(&src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::OpOnArray { .. }))),
            "`a {op} b` on arrays must be rejected; got: {:?}",
            hir.diagnostics
        );
    }
}

/// `%` is integer-only: on a float it would lower to invalid C (`double %
/// double`). Rejected in lowering whether the float is on the left or right.
#[test]
fn modulo_on_float_is_rejected() {
    for src in [
        "main() {\n    let float64 a = 5.5;\n    let x = a % 2.0;\n}\n",
        "main() {\n    let float32 a = 5.5;\n    let x = a % 2.0;\n}\n",
        "main() {\n    let float64 a = 5.5;\n    let int32 b = 2;\n    let x = b % a;\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::ModuloOnFloat))),
            "`%` on a float must be rejected; got: {:?}",
            hir.diagnostics
        );
    }
}

/// Integer `%` stays clean - the float guard must not catch it.
#[test]
fn modulo_on_int_is_clean() {
    let hir = lower("main() {\n    let int32 a = 5;\n    let x = a % 2;\n}\n");
    assert!(
        !diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ModuloOnFloat))),
        "integer `%` must not trip the float guard; got: {:?}",
        hir.diagnostics
    );
}

/// `return expr;` in a void function is rejected (it reaches clang as a value
/// returned from a `void` function, a hard error). Caught in HIR instead.
#[test]
fn return_value_in_void_is_rejected() {
    let hir = lower("f() {\n    return 5;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnValueInVoid))),
        "`return <value>` in a void function must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// `return;` with no value in a typed function is rejected (clang would reject
/// the missing value). `main` is an ordinary function (the C entry point is a
/// backend shim), so a bare void `main()` is NOT typed and a bare `return;` in
/// it is clean - see `bare_return_in_void_main_is_clean`.
#[test]
fn return_missing_value_is_rejected() {
    let hir = lower("g() -> int32 {\n    return;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnMissingValue { .. }))),
        "bare `return;` in a typed function must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// `main` is no longer special-cased as `int`-returning in the front end: a bare
/// void `main()` may use `return;` like any other void function. (The C entry
/// point's `int` return is supplied by a backend shim, not a language rule.)
#[test]
fn bare_return_in_void_main_is_clean() {
    let hir = lower("main() {\n    println(\"x\");\n    return;\n}\n");
    assert!(
        !diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnMissingValue { .. } | TypeError::ReturnValueInVoid)
        )),
        "a bare `return;` in a void `main` must be clean; got: {:?}",
        hir.diagnostics
    );
}

/// `return expr;` whose value type does not match the declared return type is
/// rejected, same as a mismatching tail expression.
#[test]
fn return_wrong_type_is_rejected() {
    let hir = lower("h() -> int32 {\n    return true;\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "a wrong-typed `return` must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// A well-formed early return trips none of the return diagnostics: a matching
/// typed `return expr;` and a bare `return;` in a void function are both clean.
#[test]
fn well_formed_early_return_is_clean() {
    for src in [
        "k() -> int32 {\n    return 7;\n}\n",
        "v() {\n    println(\"x\");\n    return;\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            !diags(&hir).iter().any(|e| matches!(
                e,
                HirError::Type(
                    TypeError::ReturnValueInVoid
                        | TypeError::ReturnMissingValue { .. }
                        | TypeError::ReturnTypeMismatch { .. }
                )
            )),
            "a well-formed early return must be clean; got: {:?}",
            hir.diagnostics
        );
    }
}

/// `print` is a primitive-only intrinsic: a compound argument (array, struct,
/// union) has no format and is rejected.
#[test]
fn print_compound_is_rejected() {
    let arr = lower("main() {\n    let [int32; 2] a = [1, 2];\n    println(\"{}\", a);\n}\n");
    assert!(
        diags(&arr).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintCannotFormat { kind }) if *kind == "an array"
        )),
        "printing a whole array must be rejected; got: {:?}",
        arr.diagnostics
    );
    let strct = lower(
        "structure P { int32 x, };\nmain() {\n    let P p = P { x: 1 };\n    println(\"{}\", p);\n}\n",
    );
    assert!(
        diags(&strct).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintCannotFormat { kind }) if *kind == "a struct"
        )),
        "printing a struct must be rejected; got: {:?}",
        strct.diagnostics
    );
}

/// A statically negative literal index is out of bounds for any length, so it is
/// rejected like a too-large literal index (A4).
#[test]
fn negative_literal_index_is_rejected() {
    let hir =
        lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    println(\"{}\", a[-1]);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::NegativeIndex))),
        "negative literal index must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// `len(a)` folds to `(usize)N`, so `a[len(a)]` is a static off-by-one: the
/// bounds check peels the fold's cast and still flags it.
#[test]
fn len_as_index_is_caught_out_of_bounds() {
    let hir =
        lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    println(\"{}\", a[len(a)]);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::IndexOutOfBounds { .. }))),
        "`a[len(a)]` must be caught as out of bounds; got: {:?}",
        hir.diagnostics
    );
}

/// A zero-length array `[T; 0]` lowers to a nonstandard C zero-length array, so
/// it is rejected.
#[test]
fn zero_length_array_is_rejected() {
    let hir = lower("main() {\n    let [int32; 0] a = [];\n    println(\"{}\", 0);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::ArrayLenZero))),
        "zero-length array must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// Arrays as struct/union fields are accepted now that codegen orders type
/// declarations by dependency (the wrapper typedef is emitted before the struct
/// that embeds it). No diagnostic.
#[test]
fn array_struct_field_is_accepted() {
    let hir = lower("structure Buf { [int32; 4] data, };\nmain() {}\n");
    assert!(
        diags(&hir).is_empty(),
        "an array struct field must lower clean; got: {:?}",
        hir.diagnostics
    );
}

/// A struct that embeds itself by value has infinite size: rejected with
/// `RecursiveValueType`. Covers a direct self field, mutual recursion, and
/// recursion through an array. A pointer field (`Node* next`) breaks the cycle
/// and stays clean - see `pointer_self_reference_is_accepted`.
#[test]
fn value_recursive_struct_is_rejected() {
    for src in [
        "structure A { A a, };\nmain() {}\n",
        "structure A { B b, };\nstructure B { A a, };\nmain() {}\n",
        "structure A { [A; 4] xs, };\nmain() {}\n",
    ] {
        let hir = lower(src);
        assert!(
            diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::RecursiveValueType { .. }))),
            "a value-recursive type must be rejected; got: {:?}",
            hir.diagnostics
        );
    }
}

/// Mutual recursion is a single infinite-size cycle, so it is reported once (on
/// the first-declared member), not once per member.
#[test]
fn mutual_recursion_reports_one_diagnostic() {
    let hir = lower("structure A { B b, };\nstructure B { A a, };\nmain() {}\n");
    let count = diags(&hir)
        .iter()
        .filter(|e| matches!(e, HirError::Type(TypeError::RecursiveValueType { .. })))
        .count();
    assert_eq!(
        count, 1,
        "a mutual recursion must report once, got {count}: {:?}",
        hir.diagnostics
    );
}

/// `main` is the entry point; the C backend wraps it in `int main(void)` and
/// calls it with no arguments, so a parameterized `main` is rejected (it would
/// otherwise emit C that clang rejects).
#[test]
fn main_with_params_is_rejected() {
    let hir = lower("main(int32 x) {\n    println(\"{}\", x);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MainHasParams))),
        "a parameterized `main` must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// `main` may return any type - the C entry shim adapts it to the exit code -
/// so a non-integer return is accepted, not rejected. (Only declaring
/// parameters is an error; see `main_with_params_is_rejected`.)
#[test]
fn main_with_any_return_is_accepted() {
    for src in [
        "main() {\n    println(\"x\");\n}\n",
        "main() -> int32 {\n    return 0;\n}\n",
        "main() -> bool {\n    true\n}\n",
        "main() -> float64 {\n    1.5\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            !diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::MainHasParams))),
            "any `main` return type must be accepted; got: {:?}",
            hir.diagnostics
        );
    }
}

/// A bare function name in value position is a function pointer of the
/// function's signature, not an error (the old `FnAsValue` rejection is gone). A
/// correctly-typed binding lowers clean.
#[test]
fn function_name_as_value_is_accepted() {
    let hir = lower(
        "add(int32 a, int32 b) -> int32 { a + b }\n\
         main() {\n    let (int32, int32) -> int32 op = add;\n    println(\"{}\", op(1, 2));\n}\n",
    );
    assert!(
        diags(&hir).is_empty(),
        "a function name as a value must lower clean; got: {:?}",
        hir.diagnostics
    );
}

/// Calling a value that is not a function pointer (`let int32 x = 5; x(3);`) is a
/// `CallNonFunction` diagnostic, not a raw clang error.
#[test]
fn calling_a_non_function_is_rejected() {
    let hir = lower("main() {\n    let int32 x = 5;\n    println(\"{}\", x(3));\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::CallNonFunction { .. }))),
        "calling a non-function must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// A struct that refers to itself only through a pointer is finite and legal:
/// the pointer is a soft edge (the forward-declared tag suffices). No diagnostic.
#[test]
fn pointer_self_reference_is_accepted() {
    for src in [
        // pointer to self
        "structure Node { int32 v, Node* next, };\nmain() {}\n",
        // array of pointers to self
        "structure Node { int32 v, [&Node; 4] kids, };\nmain() {}\n",
        // pointer to an array of self (finite: the named-tag wrapper is
        // forward-declared, so the pointer is a soft edge)
        "structure Node { int32 v, &[Node; 4] kids, };\nmain() {}\n",
    ] {
        let hir = lower(src);
        assert!(
            !diags(&hir)
                .iter()
                .any(|e| matches!(e, HirError::Type(TypeError::RecursiveValueType { .. }))),
            "a pointer self-reference must be clean; got: {:?}",
            hir.diagnostics
        );
    }
}

/// REDESIGN I3: a value-position match in a ternary-shaped `if` branch lowers
/// clean. The HIR ban (`UnsupportedError::TernaryMatch`) that rejected this
/// shape was deleted at the Track 2 cutover; MIR lowers the nested match in
/// place inside the branch (proven end-to-end by the e2e acid test on
/// `eyesrc/wierd.eye`). This guards that nothing reintroduces the ban.
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
        "value-position ternary match must lower clean, got: {:?}",
        hir.diagnostics
    );
}

/// A let-bound match (the normal value position) stays clean - control against
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

/// A literal-array return whose element type differs from the declared return
/// type is clean (the element type is coerced); a wrong *length* still errors.
/// Guards that the element coercion does not mask an arity mismatch.
#[test]
fn array_literal_return_coercion_keeps_length_check() {
    let ok = lower("g() -> [usize; 3] {\n    [1, 2, 3]\n}\nmain() {}\n");
    assert!(
        ok.diagnostics.is_empty(),
        "element coercion should make this clean, got: {:?}",
        ok.diagnostics
    );

    let bad = lower("g() -> [int32; 3] {\n    [1, 2]\n}\nmain() {}\n");
    assert!(
        diags(&bad)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::ReturnTypeMismatch { .. }))),
        "a wrong-length literal return must still error, got: {:?}",
        bad.diagnostics
    );
}

/// Track 2 cutover (I2): every reachable name in value position that does not
/// denote a value is rejected in HIR, so MIR's lowering of a `Path` is
/// `unreachable!` for the non-value resolutions. Covers the full `Resolution`
/// set: an undeclared name (call callee, bare value, struct-literal shorthand),
/// a struct type name, a function name, and the `print`/`len` intrinsics outside
/// callee position. Values (a local, an enum variant) and valid callees (a
/// function call, `println(...)`) must stay clean.
#[test]
fn non_value_name_uses_are_rejected() {
    let resolve_err = |src: &str| -> Vec<ResolveError> {
        let hir = lower(src);
        hir.diagnostics
            .entries()
            .iter()
            .filter_map(|(_, e)| match e {
                HirError::Resolve(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    };
    use ResolveError::*;
    let has = |src: &str, pred: fn(&ResolveError) -> bool| resolve_err(src).iter().any(pred);

    // Undeclared name: call callee, bare value, and struct-literal shorthand.
    assert!(
        has("main() {\n    printf(\"x\");\n}\n", |e| matches!(
            e,
            UnresolvedName { .. }
        )),
        "undeclared call must be rejected"
    );
    assert!(
        has("main() {\n    let int32 x = nope;\n}\n", |e| matches!(
            e,
            UnresolvedName { .. }
        )),
        "bare undeclared value must be rejected"
    );
    assert!(
        has(
            "structure P { int32 x, };\nmain() {\n    let P p = P { x };\n}\n",
            |e| matches!(e, UnresolvedName { .. })
        ),
        "undeclared shorthand field must be rejected"
    );
    // A struct type name as a value (and so a struct as a callee, `P()`).
    assert!(
        has(
            "structure P { int32 x, };\nmain() {\n    let int32 y = P;\n}\n",
            |e| matches!(e, StructNameAsValue { .. })
        ),
        "struct name in value position must be rejected"
    );
    // The `print`/`len` intrinsics outside callee position are undeclared.
    assert!(
        has("main() {\n    let int32 p = print;\n}\n", |e| matches!(
            e,
            UnresolvedName { .. }
        )),
        "bare `print` value must be rejected"
    );

    // Controls: real values and valid callees stay clean.
    assert!(
        resolve_err("f() -> int32 { 1 }\nmain() {\n    println(\"{}\", f());\n}\n").is_empty(),
        "a function call and `println(...)` are valid, not errors"
    );
    assert!(
        resolve_err("enum E = A | B;\nmain() {\n    let E y = A;\n}\n").is_empty(),
        "an enum variant is a value, not an error"
    );
}

/// C seam: a variadic extern signature sets `Function::variadic`; an opaque
/// `type Name;` lands in the opaque arena + namespace. A defined fn is never
/// variadic (the parser rejects `...` outside extern).
#[test]
fn extern_variadic_and_opaque_type_collected() {
    let hir = lower(
        "extern {\n\
         \x20   type FILE;\n\
         \x20   printf(string fmt, ...) -> int32;\n\
         \x20   fclose(FILE* f) -> int32;\n\
         }\n\
         main() {\n\
         }\n",
    );
    assert!(diags(&hir).is_empty(), "{:?}", diags(&hir));

    let printf = hir.items.functions["printf"];
    assert!(hir.functions[printf].is_extern);
    assert!(hir.functions[printf].variadic);
    assert_eq!(hir.functions[printf].params.len(), 1);

    let fclose = hir.items.functions["fclose"];
    assert!(!hir.functions[fclose].variadic);

    assert_eq!(hir.opaques.len(), 1);
    let file = hir.items.opaques["FILE"];
    assert_eq!(hir.opaques[file].name, "FILE");

    let main = hir.items.functions["main"];
    assert!(!hir.functions[main].variadic);
}

/// An opaque type name collides with the nominal-type namespaces: redeclaring
/// a struct's name as `type Name;` is a duplicate-item error.
#[test]
fn opaque_type_duplicate_name_is_rejected() {
    let hir = lower(
        "structure FILE {\n\
         \x20   int32 x,\n\
         };\n\
         extern {\n\
         \x20   type FILE;\n\
         }\n\
         main() {\n\
         }\n",
    );
    assert!(
        diags(&hir).iter().any(|d| matches!(
            d,
            HirError::Resolve(ResolveError::DuplicateItem { name }) if name == "FILE"
        )),
        "expected a duplicate-item diagnostic, got: {:?}",
        diags(&hir)
    );
}

/// CLEAK L3: the argument count of a direct call is checked - exact for a
/// defined function, a minimum for a variadic extern.
#[test]
fn call_arity_is_checked() {
    let hir = lower(
        "\
extern {
    printf(string fmt, ...) -> int32;
}
add(int32 a, int32 b) -> int32 { a + b }
main() {
    add(1, 2, 3);
    add(1);
    add(1, 2);
    printf(\"ok\");
    printf(\"%d %d\", 1, 2);
    printf();
}
",
    );
    let arity: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Type(TypeError::CallArityMismatch {
                name,
                expected,
                found,
                variadic,
            }) => Some((name.as_str().to_owned(), *expected, *found, *variadic)),
            _ => None,
        })
        .collect();
    assert_eq!(
        arity,
        [
            ("add".to_owned(), 2, 3, false),
            ("add".to_owned(), 2, 1, false),
            ("printf".to_owned(), 1, 0, true),
        ],
        "expected exactly the three arity mismatches: {:?}",
        diags(&hir)
    );
}

/// CLEAK L5 (R011): a struct literal naming no declared struct or union is
/// rejected instead of emitting `(Foo){..}` into C.
#[test]
fn unknown_struct_literal_rejected() {
    let hir = lower("main() { Foo { x: 1 }; }");
    assert!(
        diags(&hir).iter().any(|d| matches!(
            d,
            HirError::Resolve(ResolveError::UnknownStructLiteral { name }) if name == "Foo"
        )),
        "expected an unknown-struct-literal diagnostic, got: {:?}",
        diags(&hir)
    );
}

/// CLEAK L6 (R012): a type annotation naming an undeclared type is rejected
/// at every declaration site - field, parameter, return, global, `let`, and
/// cast - instead of emitting the name verbatim into C. A forward reference
/// to an item declared later in the file stays legal.
#[test]
fn unknown_type_names_rejected() {
    let hir = lower(
        "\
structure Arena {
    off off,
    Late ok_forward_ref,
};
f(wat x) -> huh { 0 }
let gee g = 0;
structure Late { int32 v, };
main() {
    let blah b = 0;
    let int32 c = 0 as zap;
}
",
    );
    let unknown: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Resolve(ResolveError::UnknownTypeName { name }) => {
                Some(name.as_str().to_owned())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        unknown,
        // Globals are collected (pass 1b) before items (pass 1), so `gee`
        // is recorded first.
        ["gee", "off", "wat", "huh", "blah", "zap"],
        "expected exactly the undeclared type names (and not `Late`): {:?}",
        diags(&hir)
    );
}

/// CLEAK L7 / P1: `ptr` (the untyped pointer) cannot be indexed,
/// dereferenced, or used in arithmetic; comparisons stay allowed.
#[test]
fn ptr_index_deref_arithmetic_rejected() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let ptr p = malloc(8);
    p[0];
    *p;
    p + 4;
    if p == 0 as ptr { };
}
",
    );
    let ds = diags(&hir);
    assert!(
        ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::IndexOnPtr))),
        "expected IndexOnPtr: {ds:?}"
    );
    assert!(
        ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::DerefOfPtr))),
        "expected DerefOfPtr: {ds:?}"
    );
    assert!(
        ds.iter().any(
            |d| matches!(d, HirError::Type(TypeError::ArithmeticOnPtr { op }) if *op == "+")
        ),
        "expected ArithmeticOnPtr: {ds:?}"
    );
    // The comparison must not be rejected.
    assert!(
        !ds.iter()
            .any(|d| matches!(d, HirError::Type(TypeError::ArithmeticOnPtr { op }) if *op == "==")),
        "comparison on ptr must stay legal: {ds:?}"
    );
}

/// CLEAK M1: an integer literal must fit the integer type its context gives
/// it. Out of range - at an annotated site, negated into an unsigned type, or
/// over the bare `int32` default - is an error; a wide literal under a wide
/// annotation is clean.
#[test]
fn int_literal_range_is_checked() {
    let hir = lower(
        "\
main() {
    let int32 a = 5000000000;
    let uint8 b = -1;
    let int8 c = 300;
    println(\"{}\", 6000000000);
}
",
    );
    let out_of_range: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Type(TypeError::IntLiteralOutOfRange { value, ty, .. }) => {
                Some((value.clone(), ty.as_str().to_owned()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        out_of_range,
        [
            ("5000000000".to_owned(), "int32".to_owned()),
            ("-1".to_owned(), "uint8".to_owned()),
            ("300".to_owned(), "int8".to_owned()),
            ("6000000000".to_owned(), "int32".to_owned()),
        ],
        "expected exactly the four out-of-range literals: {:?}",
        diags(&hir)
    );

    // In range under the declared type: clean, including both int32 bounds
    // and a 64-bit literal under an int64/usize annotation.
    let hir = lower(
        "\
main() {
    let int64 a = 5000000000;
    let usize b = 18446744073709551615;
    let int32 c = -2147483648;
    let int32 d = 2147483647;
    let uint8 e = 255;
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "in-range literals must be clean: {:?}",
        diags(&hir)
    );
}

/// CLEAK M4: a positional struct literal (`Point { 1, 2 }`) is rejected -
/// lowering carries fields by name, so the values would be silently dropped
/// and the struct zero-initialized.
#[test]
fn positional_struct_literal_rejected() {
    let hir = lower(
        "\
structure Point {
    int32 x,
    int32 y,
};
main() {
    let Point p = Point { 1, 2 };
}
",
    );
    assert_eq!(
        diags(&hir)
            .iter()
            .filter(|d| matches!(d, HirError::Type(TypeError::StructLitPositional)))
            .count(),
        2,
        "expected one diagnostic per positional field: {:?}",
        diags(&hir)
    );
}

/// CLEAK L1 + L2: string decay through the unified coercion point at the two
/// sites it was missing - struct-literal fields and array-literal elements.
/// Both must lower clean (the decay cast satisfies the declared type).
#[test]
fn string_decay_at_struct_fields_and_array_elements() {
    let hir = lower(
        "\
structure Syllable {
    string sound,
};
main() {
    let Syllable s = Syllable { sound: \"cvc\" };
    let [string; 2] xs = [\"ab\", \"cd\"];
    println(\"{} {} {}\", s.sound, xs[0], xs[1]);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "decay at struct fields and array elements must be clean: {:?}",
        diags(&hir)
    );
}
