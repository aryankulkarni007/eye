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

/// the concrete diagnostic kinds, for structural assertions. tests match on
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
    // both struct arena slots persist so existing ids stay valid
    assert_eq!(hir.structs.len(), 2);
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
    // cross-namespace collision should still be flagged: in v0.1 the
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

/// regression for the `NameRef::nth(1)` bug: when the base of a field
/// access is itself a field expression (`a.b.c`), the outer fieldexpr
/// has only one direct nameref child (the field name); `nth(1)` would
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

    // collect every expr::field name; expect `c` and `b` to be present.
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

/// regression for the lower-block ordering bug: a block's tail expression
/// (typically a `loop { ... }` body) used to be lowered *before* the
/// preceding stmts, so locals defined by those stmts were not yet in
/// scope. namerefs inside the loop body fell through to
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

    // every `Path` expression that names `p_ref` must resolve to a
    // local, not fall through to unresolved.
    let unresolved_p_ref = body
        .exprs
        .iter()
        .any(|(_, e)| matches!(e, Expr::Path(Resolution::Unresolved(n)) if n.as_str() == "p_ref"));
    assert!(
        !unresolved_p_ref,
        "p_ref inside the tail loop body did not resolve to the outer local"
    );
}

// call return-type resolution (user + extern fns) is now a typeck concern,
// covered end-to-end by the `let`-type check in `crates/typeck/tests/judgments.rs`
// (`call_return_type_resolves_*`) and the program corpus.

// ---- v0.3 match lowering ----

/// walk the HIR for the `main` body and return the first `Expr::Match`
/// it finds. tests assume exactly one per fixture.
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

// a `let` struct destructure binding every field lowers cleanly.
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

    // the `xs` local carries an array type with the parsed length.
    let int32_ty = hir.types.int32_ty();
    let types = &hir.types;
    let array_local = body
        .locals
        .iter()
        .find_map(|(_, l)| l.ty.and_then(|ty| ty.as_array(types)));
    let (elem, len) = array_local.expect("xs local has an Array type");
    assert_eq!(len, 3, "array length parsed from literal");
    assert_eq!(elem, int32_ty, "element type");

    // both an arraylit and an index expression were produced.
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
/// `const` (a const-expr over those). a runtime local is not a constant, so it
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

/// the const-expr evaluator folds literals, the operator set, and references to
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

/// a `const` whose initializer references itself (directly or through a chain)
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

/// a const sharing a name with another item is a duplicate, like any item clash.
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

/// a name the c backend emits verbatim (field, parameter, function, enum
/// variant) that is a c keyword is rejected at collection (R010): emitted
/// verbatim it would be illegal c (`.struct = ...`). non-keyword names that
/// merely *look* c-ish (`data`, `value`) are untouched.
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

/// non-ASCII char literals are rejected (T034) in both expression and
/// match-pattern position: `char` is one byte, and the multibyte c char
/// constant is implementation-defined. ASCII and escapes stay legal.
#[test]
fn non_ascii_char_literals_are_rejected() {
    let hir = lower(
        "\
main() {
    let char a = 'é';
    let char ok = 'x';
    let char esc = '\\n';
    match ok {
        '→' -> {},
        _ -> {},
    }
}
",
    );
    let rejected: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Type(TypeError::CharLiteralNotAscii { ch }) => Some(*ch),
            _ => None,
        })
        .collect();
    assert_eq!(
        rejected,
        ['é', '→'],
        "expected exactly the non-ASCII char literals rejected: {:?}",
        diags(&hir)
    );
}

/// compiler-reserved names are rejected (R014): `__eye`-prefixed names
/// collide with the backend's own symbols (string statics, array wrappers,
/// the `main` shim), and a non-extern `printf` collides with the libc symbol
/// the `println` intrinsic calls. an `extern` declaration of `printf` stays
/// legal - it names the same libc symbol.
#[test]
fn reserved_names_are_rejected() {
    let hir = lower(
        "\
printf(int32 x) -> int32 { x }
__eye_main() -> int32 { 7 }
main() {}
",
    );
    let reserved: Vec<_> = diags(&hir)
        .iter()
        .filter_map(|d| match d {
            HirError::Resolve(ResolveError::NameIsReserved { name, .. }) => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reserved,
        ["printf", "__eye_main"],
        "expected exactly the reserved names rejected: {:?}",
        diags(&hir)
    );
    let hir = lower(
        "\
extern {
    printf(string fmt, ...) -> int32;
}
main() { printf(\"x\"); }
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Resolve(ResolveError::NameIsReserved { .. }))),
        "extern printf declares the libc symbol and must stay legal: {:?}",
        diags(&hir)
    );
}

/// duplicate parameter names in a function definition are rejected (R013):
/// the c signature declares them verbatim, where a duplicate is a
/// redefinition error. extern prototypes are types-only and exempt.
#[test]
fn duplicate_param_names_are_rejected() {
    let hir = lower(
        "\
f(int32 x, int32 x) -> int32 { x }
main() {}
",
    );
    assert!(
        diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Resolve(ResolveError::DuplicateParam { .. }))),
        "expected a duplicate-parameter diagnostic: {:?}",
        diags(&hir)
    );
    let hir = lower(
        "\
extern {
    g(int32 x, int32 x) -> int32;
}
main() {}
",
    );
    assert!(
        !diags(&hir)
            .iter()
            .any(|d| matches!(d, HirError::Resolve(ResolveError::DuplicateParam { .. }))),
        "extern prototypes are types-only; duplicate names are not checked: {:?}",
        diags(&hir)
    );
}

/// a top-level `const` integer resolves as an array length (A6,
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
            Stmt::Let { ty: Some(ty), .. } => ty.as_array(types).map(|(_, len)| len),
            _ => None,
        })
        .collect();
    assert_eq!(lens, vec![4, 8]);
}

/// the repeat literal `[value; N]` resolves its count via the same const
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

/// a repeat literal with a non-const count is a `Const` error - a runtime local
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

/// an untyped `let` is rejected. type inference is on hiatus, so a binding needs
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

// ---- value-position MATCH result type (MATCH.md steps 1-5) ----
//
// the match's result type (and the explicitly-typed-`let` binding override that
// widens it, e.g. `let int64 n = match ..`) moved to the typeck pass at S2C C5;
// lowering no longer stamps it. the codegen-temp-width behavior is covered by
// the c_codegen snapshot + the program corpus.

/// manual dump - run with `cargo test -p eye-hir dump -- --nocapture`.
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
/// missing field. garbage-in-c otherwise.
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

// --- v0.7 arrays first-class + latent gaps ---

/// the `len(xs)` intrinsic lowers to a `usize`-typed `Expr::Len` node, not a
/// call. the element count is folded at MIR lowering from the operand's type
/// (S2C C1); here we assert only the node and its `usize` type.
#[test]
fn array_len_lowers_to_len_node() {
    let hir = lower(
        "main() {\n    let [int32; 5] xs = [1, 2, 3, 4, 5];\n    println(\"{}\", len(xs));\n}\n",
    );
    let main_id = *hir.items.functions.get("main").unwrap();
    let body = &hir.bodies[hir.functions[main_id].body.unwrap()];
    // `len(xs)` lowers to a `Len` node (typed `usize` by typeck; MIR folds it to
    // `(usize)5`, so it prints with `%zu`).
    let has_len_node = body.exprs.iter().any(|(_, e)| matches!(e, Expr::Len(_)));
    assert!(
        has_len_node,
        "expected `len(xs)` to lower to a `Len` node; exprs: {:?}",
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

// `len` on a non-array (lennotarray) and `.len` field syntax on an array
// (lenfieldonarray) both need the operand type, so they moved to the typeck pass
// at S2C C5; their tests live in `crates/typeck/tests/judgments.rs`. the
// place-restriction below (lennotaplace) is structural and stays in lowering.

/// `len` never evaluates its operand (it reads the length from the type), so a
/// computed operand like `len(f())` would silently discard the call. the
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
/// is a place and one ref is peeled. `len(*r)` works too. both fold to the
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

// `print` of a compound argument (array/struct/union, printcannotformat) needs
// the argument type, so it moved to the typeck pass at S2C C5; its test lives in
// `crates/typeck/tests/judgments.rs`. the placeholder-arity check below is
// structural and stays in lowering.

/// `println` placeholder/argument counts must match (U5): an exhausted
/// placeholder emitted `%d` with no argument, surplus arguments were
/// forwarded to printf - varargs UB both ways. `println()` with no
/// arguments has no format string at all (`printf()` is not legal c).
#[test]
fn println_placeholder_arity_is_checked() {
    let too_few = lower("main() {\n    println(\"{} {}\", 1);\n}\n");
    assert!(
        diags(&too_few).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintlnArityMismatch {
                placeholders: 2,
                args: 1
            })
        )),
        "2 placeholders with 1 argument must be rejected; got: {:?}",
        too_few.diagnostics
    );
    let too_many = lower("main() {\n    println(\"{}\", 1, 2);\n}\n");
    assert!(
        diags(&too_many).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintlnArityMismatch {
                placeholders: 1,
                args: 2
            })
        )),
        "1 placeholder with 2 arguments must be rejected; got: {:?}",
        too_many.diagnostics
    );
    let no_args = lower("main() {\n    println();\n}\n");
    assert!(
        diags(&no_args)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::PrintlnMissingFormat))),
        "println with no arguments must be rejected; got: {:?}",
        no_args.diagnostics
    );
    // matched counts are clean, and a lone `{` is not a placeholder.
    for src in [
        "main() {\n    println(\"{} {}\", 1, 2);\n}\n",
        "main() {\n    println(\"plain\");\n}\n",
        "main() {\n    println(\"{ {}\", 1);\n}\n",
    ] {
        let hir = lower(src);
        assert!(
            !diags(&hir).iter().any(|e| matches!(
                e,
                HirError::Type(
                    TypeError::PrintlnArityMismatch { .. } | TypeError::PrintlnMissingFormat
                )
            )),
            "matched counts must be clean for {src:?}; got: {:?}",
            hir.diagnostics
        );
    }
}

/// `{{` and `}}` escape a literal brace in a `println` format string (ruled
/// 2026-06-12, rust-style); only `{}` is a placeholder. the arity scan must
/// skip escapes with the same rule codegen renders them by.
#[test]
fn println_brace_escapes_are_not_placeholders() {
    // `{{}}` is the literal text `{}` - zero placeholders.
    let escaped = lower("main() {\n    println(\"{{}}\");\n}\n");
    assert!(
        !diags(&escaped).iter().any(|e| matches!(
            e,
            HirError::Type(
                TypeError::PrintlnArityMismatch { .. } | TypeError::PrintlnMissingFormat
            )
        )),
        "`{{{{}}}}` must count zero placeholders; got: {:?}",
        escaped.diagnostics
    );
    // `{{{}}}` is `{{` + `{}` + `}}` - exactly one placeholder.
    let mixed = lower("main() {\n    println(\"{{{}}}\", 1);\n}\n");
    assert!(
        !diags(&mixed)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::PrintlnArityMismatch { .. }))),
        "`{{{{{{}}}}}}` must count one placeholder; got: {:?}",
        mixed.diagnostics
    );
    // an argument against an all-escaped string is a mismatch.
    let surplus = lower("main() {\n    println(\"{{}}\", 1);\n}\n");
    assert!(
        diags(&surplus).iter().any(|e| matches!(
            e,
            HirError::Type(TypeError::PrintlnArityMismatch {
                placeholders: 0,
                args: 1
            })
        )),
        "an argument with zero placeholders must be rejected; got: {:?}",
        surplus.diagnostics
    );
}

/// same-scope redeclaration is rejected (R015, ruled 2026-06-12); shadowing
/// needs a nested block scope. covers `let` against `let`, `const` against
/// `let`, and a destructure rename colliding with an earlier binding.
#[test]
fn same_scope_redeclaration_is_rejected() {
    let lets = lower("main() {\n    let int32 x = 1;\n    mut int32 x = 2;\n}\n");
    assert!(
        diags(&lets).iter().any(
            |e| matches!(e, HirError::Resolve(ResolveError::DuplicateLocal { name }) if name == "x")
        ),
        "same-scope let/mut redeclaration must be rejected; got: {:?}",
        lets.diagnostics
    );
    let const_let = lower("main() {\n    const int32 K = 1;\n    let int32 K = 2;\n}\n");
    assert!(
        diags(&const_let).iter().any(
            |e| matches!(e, HirError::Resolve(ResolveError::DuplicateLocal { name }) if name == "K")
        ),
        "a let rebinding a same-scope const must be rejected; got: {:?}",
        const_let.diagnostics
    );
    // a nested block scope shadows legally.
    let nested = lower(
        "main() {\n    let int32 x = 1;\n    if true {\n        let int32 x = 2;\n    }\n}\n",
    );
    assert!(
        !diags(&nested)
            .iter()
            .any(|e| matches!(e, HirError::Resolve(ResolveError::DuplicateLocal { .. }))),
        "nested-scope shadowing must stay legal; got: {:?}",
        nested.diagnostics
    );
}

/// `&` requires a place (T036, ruled 2026-06-12): `&(a + b)` would spill the
/// value to a MIR temp and silently take the temp's address. places (a
/// variable, field, index, deref) stay legal.
#[test]
fn ref_of_non_place_is_rejected() {
    let non_place = lower(
        "main() {\n    let int32 a = 1;\n    let int32 b = 2;\n    let &int32 p = &(a + b);\n}\n",
    );
    assert!(
        diags(&non_place)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::RefOfNonPlace))),
        "`&(a + b)` must be rejected; got: {:?}",
        non_place.diagnostics
    );
    let place = lower("main() {\n    let int32 a = 1;\n    let &int32 p = &a;\n}\n");
    assert!(
        !diags(&place)
            .iter()
            .any(|e| matches!(e, HirError::Type(TypeError::RefOfNonPlace))),
        "`&a` must stay legal; got: {:?}",
        place.diagnostics
    );
}

/// a zero-length array `[T; 0]` lowers to a nonstandard c zero-length array, so
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

/// arrays as struct/union fields are accepted now that codegen orders type
/// declarations by dependency (the wrapper typedef is emitted before the struct
/// that embeds it). no diagnostic.
#[test]
fn array_struct_field_is_accepted() {
    let hir = lower("structure Buf { [int32; 4] data, };\nmain() {}\n");
    assert!(
        diags(&hir).is_empty(),
        "an array struct field must lower clean; got: {:?}",
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

/// `main` is the entry point; the c backend wraps it in `int main(void)` and
/// calls it with no arguments, so a parameterized `main` is rejected (it would
/// otherwise emit c that clang rejects).
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

/// `main` may return any type - the c entry shim adapts it to the exit code -
/// so a non-integer return is accepted, not rejected. (only declaring
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

/// a bare function name in value position is a function pointer of the
/// function's signature, not an error (the old `FnAsValue` rejection is gone). a
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
        "value-position ternary match must lower clean, got: {:?}",
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

/// track 2 cutover (I2): every reachable name in value position that does not
/// denote a value is rejected in HIR, so MIR's lowering of a `Path` is
/// `unreachable!` for the non-value resolutions. covers the full `Resolution`
/// set: an undeclared name (call callee, bare value, struct-literal shorthand),
/// a struct type name, a function name, and the `print`/`len` intrinsics outside
/// callee position. values (a local, an enum variant) and valid callees (a
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

    // undeclared name: call callee, bare value, and struct-literal shorthand.
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
    // a struct type name as a value (and so a struct as a callee, `P()`).
    assert!(
        has(
            "structure P { int32 x, };\nmain() {\n    let int32 y = P;\n}\n",
            |e| matches!(e, StructNameAsValue { .. })
        ),
        "struct name in value position must be rejected"
    );
    // the `print`/`len` intrinsics outside callee position are undeclared.
    assert!(
        has("main() {\n    let int32 p = print;\n}\n", |e| matches!(
            e,
            UnresolvedName { .. }
        )),
        "bare `print` value must be rejected"
    );

    // controls: real values and valid callees stay clean.
    assert!(
        resolve_err("f() -> int32 { 1 }\nmain() {\n    println(\"{}\", f());\n}\n").is_empty(),
        "a function call and `println(...)` are valid, not errors"
    );
    assert!(
        resolve_err("enum E = A | B;\nmain() {\n    let E y = A;\n}\n").is_empty(),
        "an enum variant is a value, not an error"
    );
}

/// c seam: a variadic extern signature sets `Function::variadic`; an opaque
/// `type Name;` lands in the opaque arena + namespace. a defined fn is never
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

/// an opaque type name collides with the nominal-type namespaces: redeclaring
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
/// rejected instead of emitting `(Foo){..}` into c.
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
/// cast - instead of emitting the name verbatim into c. a forward reference
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
        // globals are collected (pass 1b) before items (pass 1), so `gee`
        // is recorded first.
        ["gee", "off", "wat", "huh", "blah", "zap"],
        "expected exactly the undeclared type names (and not `Late`): {:?}",
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
/// both must lower clean (the decay cast satisfies the declared type).
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

/// `sizeof(T)` folds to a `usize` constant for every named type.
#[test]
fn sizeof_for_primitives_and_structs() {
    let hir = lower(
        "\
structure Point {
    int32 x,
    int32 y,
};
main() {
    let usize a = sizeof(int32);
    let usize b = sizeof(int64);
    let usize c = sizeof(float64);
    let usize d = sizeof(Point);
    let usize e = sizeof(uint8);
    let usize f = sizeof(char);
    println(\"{} {} {} {} {} {}\", a, b, c, d, e, f);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "sizeof for named types must lower clean: {:?}",
        diags(&hir)
    );
}

/// `as` casts between numeric types lower clean (widening, narrowing, int↔float).
#[test]
fn numeric_as_casts_lower_clean() {
    let hir = lower(
        "\
main() {
    let int32 x = 42;
    let int64 y = x as int64;
    let uint8 z = x as uint8;
    let float64 w = x as float64;
    let int32 a = w as int32;
    let float32 b = x as float32;
    println(\"{} {} {} {} {} {}\", y, z, w, a, b, 0);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "numeric as casts must lower clean: {:?}",
        diags(&hir)
    );
}

/// `as` casts between `ptr` and typed pointers lower clean.
#[test]
fn pointer_as_casts_lower_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let ptr p = malloc(8);
    let uint8* bp = p as uint8*;
    let ptr q = bp as ptr;
    println(\"{}\", q == p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "pointer as casts must lower clean: {:?}",
        diags(&hir)
    );
}

/// typed pointer arithmetic lowers clean.
#[test]
fn typed_pointer_arithmetic_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(8) as int32*;
    let int32* q = p + 1;
    println(\"{}\", q == p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "typed pointer arithmetic must lower clean: {:?}",
        diags(&hir)
    );
}

/// typed pointer dereference lowers clean.
#[test]
fn typed_pointer_deref_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    *p = 42;
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "typed pointer dereference must lower clean: {:?}",
        diags(&hir)
    );
}

/// `&` reference on a variable lowers clean.
#[test]
fn ref_on_var_lowers_clean() {
    let hir = lower(
        "\
main() {
    let int32 a = 1;
    let &int32 r = &a;
    let int32 b = *r;
    println(\"{}\", b);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "ref on var must lower clean: {:?}",
        diags(&hir)
    );
}

/// multiple pointer indirection (`int32** pp`) lowers clean.
#[test]
fn multiple_pointer_indirection_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    let int32** pp = &p;
    let int32 v = **pp;
    println(\"{}\", v);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "multiple pointer indirection must lower clean: {:?}",
        diags(&hir)
    );
}

/// array of pointers lowers clean.
#[test]
fn array_of_pointers_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let [ptr; 3] ps = [malloc(4), malloc(4), malloc(4)];
    println(\"{}\", ps[0] == ps[1]);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "array of pointers must lower clean: {:?}",
        diags(&hir)
    );
}

/// `*(&x)` round-trip (deref of ref to a place) lowers clean.
#[test]
fn deref_of_ref_roundtrip_lowers_clean() {
    let hir = lower(
        "\
main() {
    let int32 x = 42;
    let int32 y = *(&x);
    println(\"{}\", y);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "deref of ref round-trip must lower clean: {:?}",
        diags(&hir)
    );
}

/// `sizeof` in a nested expression (e.g. `sizeof(int32) + 1`) lowers clean.
#[test]
fn sizeof_in_expression_lowers_clean() {
    let hir = lower(
        "\
main() {
    let usize n = sizeof(int32) + sizeof(int64);
    println(\"{}\", n);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "sizeof in expression must lower clean: {:?}",
        diags(&hir)
    );
}

/// compound assignment with pointer arithmetic lowers clean.
#[test]
fn pointer_compound_assignment_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(16) as int32*;
    let int32* q = p + 1;
    println(\"{}\", 0);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "pointer compound assignment must lower clean: {:?}",
        diags(&hir)
    );
}

/// deref assignment (`*p = val`) through a typed pointer lowers clean.
#[test]
fn deref_assignment_lowers_clean() {
    let hir = lower(
        "\
extern {
    malloc(usize n) -> ptr;
}
main() {
    let int32* p = malloc(4) as int32*;
    *p = 42;
    println(\"{}\", *p);
}
",
    );
    assert!(
        diags(&hir).is_empty(),
        "deref assignment must lower clean: {:?}",
        diags(&hir)
    );
}
