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
    let x = 0;
    let y = 0;
    mut Point p = Point { x, y };

    print(\"{}\", p.x);
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
        body.expr_types.get(call_id),
        Some(&TypeRef::Path("int32".into()))
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
        body.expr_types.get(call_id),
        Some(&TypeRef::Path("usize".into()))
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
                if expected.to_string() == "string" && got.to_string() == "int32"
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
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
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
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        Circle -> 0,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
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
         print(\"{{}}\", n);\n}}\n",
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
         print(\"{{}}\", n);\n}}\n",
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

#[test]
fn match_cross_enum_pattern_diagnosed() {
    let src = format!(
        "{}enum Option = Some | None ;\nmain() {{\n    \
         let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Option.Some -> 0,\n        _ -> 1,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
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
         print(\"{{}}\", n);\n}}\n",
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

#[test]
fn match_non_enum_scrut_diagnosed() {
    let src = "\
main() {
    let int32 x = 0;
    let int32 n = match x {
        _ -> 1,
    };
    print(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::MatchScrutineeNotEnum))),
        "expected non-enum diag, got: {:?}",
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
    let array_local = body.locals.iter().find_map(|(_, l)| match &l.ty {
        Some(TypeRef::Array { elem, len }) => Some((elem.clone(), *len)),
        _ => None,
    });
    let (elem, len) = array_local.expect("xs local has an Array type");
    assert_eq!(len, 3, "array length parsed from literal");
    assert_eq!(*elem, TypeRef::Path("int32".into()), "element type");

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

/// `[T; N]` currently requires `N` to be an integer literal. A name or other
/// expression is reserved for future compile-time constants and must not lower
/// silently as length 0.
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
            HirError::Const(ConstError::ArrayLenNotLiteral)
        ),
        "unexpected diagnostic: {:?}",
        diags(&hir)[0]
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
    // First arm is int32, a later arm is a string: the value-position match
    // has no single result type, so the mismatching arm is diagnosed.
    let src = format!(
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
         Circle -> 1,\n        Rectangle -> \"bad\",\n        Triangle -> 3,\n    }};\n    \
         print(\"{{}}\", n);\n}}\n",
        SHAPE_DECL
    );
    let hir = lower(&src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if expected.to_string() == "int32" && found.to_string() == "string"
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
    print(\"{}\", n);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if found.to_string() == "Point"
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
         print(\"{{}}\", n);\n}}\n",
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
    match body.expr_types.get(match_id) {
        Some(TypeRef::Path(n)) => assert_eq!(n.as_str(), "int64"),
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
         print(\"done\");\n}}\n",
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
         print(\"{{}}\", n);\n}}\n",
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
    print(\"{}\", a);
}
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { found, .. })
                if found.to_string() == "Color"
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
main() { print(\"{}\", sides(Red)); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::MatchArmTypeMismatch { expected, found })
                if expected.to_string() == "int32" && found.to_string() == "Color"
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
main() { print(\"{}\", bad()); }
";
    let hir = lower(src);
    assert!(
        diags(&hir).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::ReturnTypeMismatch { expected, found })
                if expected.to_string() == "int32" && found.to_string() == "Color"
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
main() { print(\"{}\", gt(3, 1)); }
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
        "main() {\n    mut int32 x = 0;\n    if x == 5 {\n        print(\"hi\");\n    }\n}\n",
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
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    print(\"{}\", xs[9]);\n}\n");
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
        lower("main() {\n    let [int32; 4] xs = [1, 2, 3, 4];\n    print(\"{}\", xs[3]);\n}\n");
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
        "main() {\n    let [int32; 5] xs = [1, 2, 3, 4, 5];\n    print(\"{}\", len(xs));\n}\n",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    // `len` folds to `(usize)5`: a usize-typed cast over the literal length, so
    // it prints with `%zu` without a varargs type mismatch.
    let has_len_const = body.exprs.iter().any(|(id, e)| {
        matches!(e, Expr::Cast { operand, .. }
            if matches!(body.exprs[*operand], Expr::Literal(Literal::Int(5))))
            && matches!(
                body.expr_types.get(id),
                Some(TypeRef::Path(n)) if n == "usize"
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
    let non_array = lower("main() {\n    let int32 x = 0;\n    print(\"{}\", len(x));\n}\n");
    assert_eq!(
        non_array.diagnostics.len(),
        1,
        "`len` on a non-array must diagnose; got: {:?}",
        non_array.diagnostics
    );

    let dot_form =
        lower("main() {\n    let [int32; 3] xs = [1, 2, 3];\n    print(\"{}\", xs.len);\n}\n");
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
        "mk() -> [int32; 3] {\n    [1, 2, 3]\n}\nmain() {\n    print(\"{}\", len(mk()));\n}\n",
    );
    assert_eq!(
        call_form.diagnostics.len(),
        1,
        "`len(f())` must be rejected (the call would be discarded); got: {:?}",
        call_form.diagnostics
    );

    let literal_form = lower("main() {\n    print(\"{}\", len([1, 2, 3]));\n}\n");
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
        "sum(&[int32; 3] r) -> usize {\n    len(r)\n}\nmain() {\n    mut [int32; 3] a = [1, 2, 3];\n    print(\"{}\", sum(&a));\n}\n",
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

/// `print` is a primitive-only intrinsic: a compound argument (array, struct,
/// union) has no format and is rejected.
#[test]
fn print_compound_is_rejected() {
    let arr = lower("main() {\n    let [int32; 2] a = [1, 2];\n    print(\"{}\", a);\n}\n");
    assert!(
        diags(&arr).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintCannotFormat { kind }) if *kind == "an array"
        )),
        "printing a whole array must be rejected; got: {:?}",
        arr.diagnostics
    );
    let strct = lower(
        "structure P { int32 x, };\nmain() {\n    let P p = P { x: 1 };\n    print(\"{}\", p);\n}\n",
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
    let hir = lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    print(\"{}\", a[-1]);\n}\n");
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
        lower("main() {\n    let [int32; 3] a = [1, 2, 3];\n    print(\"{}\", a[len(a)]);\n}\n");
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
    let hir = lower("main() {\n    let [int32; 0] a = [];\n    print(\"{}\", 0);\n}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Const(ConstError::ArrayLenZero))),
        "zero-length array must be rejected; got: {:?}",
        hir.diagnostics
    );
}

/// Arrays as struct/union fields are rejected this version (the wrapper typedef
/// would be emitted after the struct; needs a codegen type-dependency sort).
#[test]
fn array_struct_field_is_rejected() {
    let hir = lower("structure Buf { [int32; 4] data, };\nmain() {}\n");
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Unsupported(UnsupportedError::ArrayField))),
        "expected an array-field diagnostic, got: {:?}",
        hir.diagnostics
    );
}

/// L1: a value-position match in a ternary-shaped `if` branch is rejected
/// rather than emitting broken C.
#[test]
fn match_in_ternary_branch_is_rejected() {
    let hir = lower(
        "enum Color = Red | Blue;\n\
         pick(Color c) -> int32 {\n\
         \x20   let int32 r = if true { match c { Red -> 1, _ -> 0 } } else { 9 };\n\
         \x20   r\n\
         }\n",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|e| matches!(e, HirError::Unsupported(UnsupportedError::TernaryMatch))),
        "expected an unhoisted-match diagnostic, got: {:?}",
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
