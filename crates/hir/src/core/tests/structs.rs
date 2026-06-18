use super::*;

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

    // find the structlit init of `p`
    let mut sl_field_count = 0;
    for (_, expr) in body.exprs.iter() {
        if let Expr::StructLit { fields, .. } = expr {
            sl_field_count = fields.len();
            for f in fields {
                // shorthand must be materialized: every field has a real
                // exprid (no option). the synthesized expr resolves to
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

/// a union literal must set exactly one member - overlapping storage means
/// a second field silently overwrites the first. one field is clean; two
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
fn well_formed_program_has_no_diagnostics() {
    let hir = lower(MAIN_EYE);
    assert!(
        hir.diagnostics.is_empty(),
        "expected zero diagnostics, got: {:?}",
        hir.diagnostics
    );
}

/// F3 / S1: a struct literal omitting a declared field is an error, naming the
/// missing field. produces undefined behavior in C otherwise.
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

/// a struct literal that names every declared field exactly once is clean.
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
/// (grammarerror::assigninifcondition); see the parser crate's
/// `assignment_in_if_condition_is_rejected`. a genuine comparison stays clean.
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

/// arrays as struct/union fields are accepted now that codegen orders type
/// declarations by dependency (the wrapper typedef is emitted before the struct
/// that embeds it). no diagnostic.
#[test]
fn array_struct_field_is_accepted() {
    let hir = lower("structure Buf { [int32; 4] data, };\nmain() {}\n");
    assert!(
        diags(&hir).is_empty(),
        "an array struct field must succeed; got: {:?}",
        hir.diagnostics
    );
}

/// a struct that embeds itself by value has infinite size: rejected with
/// `RecursiveValueType`. covers a direct self field, mutual recursion, and
/// recursion through an array. a pointer field (`Node* next`) breaks the cycle
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

/// mutual recursion is a single infinite-size cycle, so it is reported once (on
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

/// a struct that refers to itself only through a pointer is finite and legal:
/// the pointer is a soft edge (the forward-declared tag suffices). no diagnostic.
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
/// both must lower without error (the decay cast satisfies the declared type).
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
