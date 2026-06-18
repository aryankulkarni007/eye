use super::*;

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
/// correctly-typed binding lowers without error.
#[test]
fn function_name_as_value_is_accepted() {
    let hir = lower(
        "add(int32 a, int32 b) -> int32 { a + b }\n\
         main() {\n    let (int32, int32) -> int32 op = add;\n    println(\"{}\", op(1, 2));\n}\n",
    );
    assert!(
        diags(&hir).is_empty(),
        "a function name as a value must succeed; got: {:?}",
        hir.diagnostics
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
