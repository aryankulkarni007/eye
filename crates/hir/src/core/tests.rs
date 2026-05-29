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
    let x = 0;
    let y = 0;
    mut Point p = Point { x, y };

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
        two.diagnostics
            .iter()
            .any(|d| d.msg.contains("must set exactly one field")),
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("let initializer type mismatch")
                && d.msg.contains("expected string")
                && d.msg.contains("got int32")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("let initializer type mismatch")),
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
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
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
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
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
         let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
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
        "{}main() {{\n    let Shape sh = Circle;\n    \
         let int32 n = match sh {{\n        \
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
    let int32 x = 0;
    let int32 n = match x {
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
        hir.diagnostics[0]
            .msg
            .contains("array length must be an integer literal"),
        "unexpected diagnostic: {}",
        hir.diagnostics[0].msg
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
        hir.diagnostics[0]
            .msg
            .contains("array initializer length mismatch"),
        "unexpected diagnostic: {}",
        hir.diagnostics[0].msg
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")
                && d.msg.contains("expected int32")
                && d.msg.contains("produces string")),
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")
                && d.msg.contains("produces Point")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")),
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch") && d.msg.contains("produces Color")),
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")
                && d.msg.contains("expected int32")
                && d.msg.contains("produces Color")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("match arm type mismatch")),
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
        hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("return type mismatch")
                && d.msg.contains("returns int32")
                && d.msg.contains("produces Color")),
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
        !hir.diagnostics
            .iter()
            .any(|d| d.msg.contains("return type mismatch")),
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
