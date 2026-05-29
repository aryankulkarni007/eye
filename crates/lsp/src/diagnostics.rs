//! Parser diagnostics as LSP `publishDiagnostics` payloads.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url, notification::Notification};
use parser::Parse;
use text_size::TextRange;

use lexer::SourceText;

pub fn parser_diagnostics(source: &str, parse: &Parse) -> Vec<Diagnostic> {
    let text = SourceText::new(source.to_string());
    parse
        .diagnostics
        .iter()
        .map(|err| Diagnostic {
            range: text_range_to_lsp(&text, err.range),
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("eye-parser".into()),
            message: err.msg.to_string(),
            related_information: None,
            tags: None,
            data: None,
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
    let start = source.line_col(range.start());
    let end = source.line_col(range.end());
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
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let diags = parser_diagnostics(src, &parse);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }
}
