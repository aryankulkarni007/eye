//! Compiler diagnostics as LSP `publishDiagnostics` payloads.
//!
//! Every phase (lexer, parser, HIR) accumulates typed diagnostics in a
//! `Sink<K>` and boxes them into the cross-layer [`Diag`] carrier. This module
//! turns a `Vec<Diag>` from any phase into LSP [`Diagnostic`]s, so the server
//! surfaces the *same* diagnostics the `eye` binary renders - not just parse
//! errors. The mapping has one definition here; the phase orchestration (which
//! sink to publish, and when) lives in [`crate::server::notifications`].

use std::fmt::Write as _;

use diagnostics::{Diag, Severity};
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
    Position, Range, Url, notification::Notification,
};
use syntax::SyntaxNode;
use text_size::TextRange;

use lexer::SourceText;

/// Map one phase's boxed diagnostics into LSP diagnostics.
///
/// `root` is the parse tree when one exists; HIR spans are [`syntax::SyntaxNodePtr`]s
/// that need it to resolve, while lexer/parser spans are tight byte ranges and
/// ignore it (pass `None` before a tree exists). `source` labels the producing
/// phase (`"eye-lexer"` / `"eye-parser"` / `"eye-hir"`). `uri` anchors the
/// secondary labels' related-information locations.
pub fn diags_to_lsp(
    source: &SourceText,
    uri: &Url,
    diags: Vec<Diag>,
    root: Option<&SyntaxNode>,
    label: &str,
) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .map(|d| {
            let severity = match d.kind.severity() {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
            };

            // The primary message; notes and the help hint have no dedicated
            // LSP field, so fold them into the message body the editor shows on
            // hover - the same text the CLI renderer prints below the span.
            let mut message = d.kind.to_string();
            for note in d.kind.notes() {
                let _ = write!(message, "\nnote: {note}");
            }
            if let Some(help) = d.kind.help() {
                let _ = write!(message, "\nhelp: {help}");
            }

            // Secondary labels become related-information so the editor can jump
            // to the supporting spans (e.g. a conflicting earlier definition).
            let related: Vec<DiagnosticRelatedInformation> = d
                .kind
                .secondary_labels()
                .into_iter()
                .map(|(span, msg)| DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri.clone(),
                        range: text_range_to_lsp(source, span.trimmed_range(root)),
                    },
                    message: msg.into_owned(),
                })
                .collect();

            Diagnostic {
                range: text_range_to_lsp(source, d.span.trimmed_range(root)),
                severity: Some(severity),
                code: Some(NumberOrString::String(d.kind.code().to_string())),
                code_description: None,
                source: Some(label.to_owned()),
                message,
                related_information: (!related.is_empty()).then_some(related),
                tags: None,
                data: None,
            }
        })
        .collect()
}

pub fn publish_diagnostics_notification(
    uri: &Url,
    diags: Vec<Diagnostic>,
) -> lsp_server::Notification {
    let params = lsp_types::PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: diags,
        version: None,
    };
    lsp_server::Notification {
        method: lsp_types::notification::PublishDiagnostics::METHOD.to_owned(),
        params: serde_json::to_value(params).expect("PublishDiagnosticsParams serializes"),
    }
}

fn text_range_to_lsp(source: &SourceText, range: TextRange) -> Range {
    // LSP positions are UTF-16 code units (the default encoding; the server
    // does not negotiate another), not bytes.
    let start = source.line_col_utf16(range.start());
    let end = source.line_col_utf16(range.end());
    Range {
        start: Position {
            line: start.line.saturating_sub(1),
            character: start.col.saturating_sub(1),
        },
        end: Position {
            line: end.line.saturating_sub(1),
            character: end.col.saturating_sub(1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexer::Lexer;

    #[test]
    fn parse_error_becomes_diagnostic() {
        let src = "let ;";
        let uri = Url::parse("file:///t.eye").unwrap();
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let diags = diags_to_lsp(
            &source,
            &uri,
            parse.diagnostics.into_diags(),
            Some(&parse.green),
            "eye-parser",
        );
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        // the stable error code is threaded through (e.g. `S###`).
        assert!(matches!(diags[0].code, Some(NumberOrString::String(_))));
    }

    #[test]
    fn lexer_error_becomes_diagnostic() {
        // an unterminated string is a lexer-phase error: it produces no tree, so
        // this is the only path mapping spans with `root = None`.
        let src = "main() { let s = \"oops }";
        let uri = Url::parse("file:///t.eye").unwrap();
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        assert!(
            !lexed.diags.is_empty(),
            "unterminated string should lex-error"
        );
        let diags = diags_to_lsp(&source, &uri, lexed.diags.into_diags(), None, "eye-lexer");
        assert!(!diags.is_empty());
        assert_eq!(diags[0].source.as_deref(), Some("eye-lexer"));
    }

    #[test]
    fn semantic_error_becomes_diagnostic() {
        // a clean parse with a name that resolves to nothing: lexer + parser are
        // silent, so the diagnostic can only come from the HIR phase.
        use ast::AstNode;
        let src = "main() -> int32 { undefined_name }";
        let uri = Url::parse("file:///t.eye").unwrap();
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        assert!(lexed.diags.is_empty());
        let parse = parser::parse(&lexed.tokens, &source);
        assert!(parse.diagnostics.is_empty());
        let file = ast::SourceFile::cast(parse.green.clone()).unwrap();
        let hir = hir::core::lower_source_file(file, &lexed.interner);
        let diags = diags_to_lsp(
            &source,
            &uri,
            hir.diagnostics.into_diags(),
            Some(&parse.green),
            "eye-hir",
        );
        assert!(!diags.is_empty(), "HIR should report the undefined name");
    }
}
