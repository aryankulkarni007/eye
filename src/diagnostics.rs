use hir::core::HirDiagnostic;
use lexer::{Diagnostic as LexerDiagnostic, SourceText};
use parser::ParseDiagnostic;

macro_rules! report_diagnostics {
    ($source:expr, $diagnostics:expr, $prefix:expr, $label:literal, |$diag:ident| $range:expr, $msg:expr) => {{
        let diagnostics = $diagnostics;
        eprintln!("{}{} {} diagnostic(s):", $prefix, diagnostics.len(), $label);
        for $diag in diagnostics {
            let lc = $source.line_col($range.start());
            eprintln!("  {}:{}: {}", lc.line, lc.col, $msg);
        }
    }};
}

pub fn report_lexer_diagnostics(source: &SourceText, diagnostics: &[LexerDiagnostic]) {
    report_diagnostics!(
        source,
        diagnostics,
        "",
        "lexer",
        |diag| diag.range,
        diag.msg
    );
}

pub fn report_parse_diagnostics(source: &SourceText, diagnostics: &[ParseDiagnostic]) {
    report_diagnostics!(
        source,
        diagnostics,
        "\n",
        "parse",
        |diag| diag.range,
        diag.msg
    );
}

pub fn report_hir_diagnostics(source: &SourceText, diagnostics: &[HirDiagnostic]) {
    report_diagnostics!(
        source,
        diagnostics,
        "\n",
        "hir",
        |diag| diag.ptr.text_range(),
        diag.msg
    );
}
