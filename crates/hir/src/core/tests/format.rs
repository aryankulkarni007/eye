use super::*;

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
