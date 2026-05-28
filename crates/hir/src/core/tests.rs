use super::*;
use ast::{AstNode, SourceFile};
use lexer::{Lexer, SourceText};

fn lower(src: &str) -> HIR {
    let source = SourceText::new(src.to_string());
    let tokens = Lexer::new(&source).tokenize().tokens;
    let parse = parser::parse(&tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    lower_source_file(file)
}

const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    const x = 0;
    const y = 0;
    var Point p = Point { x, y };

    print(\"{}\", p);
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
        hir.diagnostics[0].msg.contains("duplicate item `Point`"),
        "unexpected message: {}",
        hir.diagnostics[0].msg
    );
    // both struct arena slots persist so existing IDs stay valid
    assert_eq!(hir.structs.len(), 2);
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
        hir.diagnostics[0].msg.contains("duplicate item `main`"),
        "unexpected message: {}",
        hir.diagnostics[0].msg
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
        hir.diagnostics[0].msg.contains("duplicate item `Foo`"),
        "unexpected message: {}",
        hir.diagnostics[0].msg
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
    print(\"{}\", a.b.c);
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
    var P p = P { x: 0 };
    var &P p_ref = &p;
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
    let unresolved_p_ref = body.exprs.iter().any(
        |(_, e)| matches!(e, Expr::Path(Resolution::Unresolved(n)) if n.as_str() == "p_ref"),
    );
    assert!(
        !unresolved_p_ref,
        "p_ref inside the tail loop body did not resolve to the outer local"
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
        "{}main() {{\n    const Shape sh = Rectangle;\n    \
         const int32 n = match sh {{\n        \
         Shape.Circle -> 0,\n        \
         Rectangle -> 1,\n        \
         Triangle -> 2,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
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
    match body.expr_types.get(scrut) {
        Some(TypeRef::Path(n)) => assert_eq!(n.as_str(), "Shape"),
        other => panic!("scrut type missing/wrong: {other:?}"),
    }
    // match expression itself carries the arm-body type so M5 codegen can
    // declare `int32 _matchN;` for the hoist temp.
    match body.expr_types.get(match_id) {
        Some(TypeRef::Path(n)) => assert_eq!(n.as_str(), "int32"),
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
        "{}main() {{\n    const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        \
         Circle -> 0,\n        \
         _ -> 99,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
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
        "{}main() {{\n    const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        Circle -> 0,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    let msgs: Vec<&str> = hir.diagnostics.iter().map(|d| d.msg.as_str()).collect();
    let exhaustive = msgs
        .iter()
        .find(|m| m.starts_with("non-exhaustive match"))
        .copied()
        .unwrap_or_else(|| panic!("missing non-exhaustive diag: {msgs:?}"));
    assert!(exhaustive.contains("`Rectangle`"), "got: {exhaustive}");
    assert!(exhaustive.contains("`Triangle`"), "got: {exhaustive}");
}

#[test]
fn match_duplicate_arm_diagnosed() {
    let src = format!(
        "{}main() {{\n    const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        \
         Circle -> 0,\n        Rectangle -> 1,\n        \
         Circle -> 2,\n        Triangle -> 3,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    let dup = hir
        .diagnostics
        .iter()
        .find(|d| d.msg.starts_with("duplicate match arm"))
        .unwrap_or_else(|| panic!("missing dup diag: {:?}", hir.diagnostics));
    assert!(dup.msg.contains("`Circle`"), "got: {}", dup.msg);
}

#[test]
fn match_arm_after_wildcard_is_unreachable() {
    let src = format!(
        "{}main() {{\n    const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        \
         _ -> 0,\n        Triangle -> 1,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("unreachable match arm after `_`")),
        "expected unreachable diag, got: {:?}",
        hir.diagnostics
    );
}

#[test]
fn match_cross_enum_pattern_diagnosed() {
    let src = format!(
        "{}enum Option = Some | None ;\nmain() {{\n    \
         const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        \
         Option.Some -> 0,\n        _ -> 1,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("pattern is from enum `Option`")),
        "expected cross-enum diag, got: {:?}",
        hir.diagnostics
    );
}

#[test]
fn match_unknown_variant_diagnosed() {
    let src = format!(
        "{}main() {{\n    const Shape sh = Circle;\n    \
         const int32 n = match sh {{\n        \
         Square -> 0,\n        _ -> 1,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("enum `Shape` has no variant `Square`")),
        "expected unknown-variant diag, got: {:?}",
        hir.diagnostics
    );
}

#[test]
fn match_non_enum_scrut_diagnosed() {
    let src = "\
main() {
    const int32 x = 0;
    const int32 n = match x {
        _ -> 1,
    };
    print(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match scrutinee type is not a known enum")),
        "expected non-enum diag, got: {:?}",
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
