use super::*;

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
