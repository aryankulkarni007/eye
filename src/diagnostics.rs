//! diagnostic rendering at the binary edge.
//!
//! the core crates emit typed [`Diagnostic`](diagnostics::diagnostic) kinds and
//! accumulate them in a [`Sink`](diagnostics::sink). here, and only here, those
//! typed diagnostics are turned into human output with `ariadne`. no core crate
//! depends on a renderer, so this can be swapped or rethemed without touching
//! them.

use std::ops::Range;
use std::path::Path;

use ariadne::{Label, Report, ReportKind, Source};
use diagnostics::{Diag, Severity, Span};
use lexer::SourceText;
use syntax::SyntaxNode;

/// the byte range `ariadne` underlines for a span: [`Span::trimmed_range`] does
/// the resolve-and-trim against the parse `root` (shared with the language
/// server); this only adapts the result to `ariadne`'s `usize` range.
fn resolve(span: &Span, root: Option<&SyntaxNode>) -> Range<usize> {
    let r = span.trimmed_range(root);
    usize::from(r.start())..usize::from(r.end())
}

/// render every diagnostic in `diags` against `source`. each phase produces a
/// `Sink<K>`; the caller boxes it with `Sink::into_diags()` and passes the
/// `Vec<Diag>` here. `root` is the parse tree when one exists, used to trim
/// trivia off pointer spans; pass `None` for the pre-parse lexer phase.
/// `source_path` is the source file path used in diagnostic headers (instead
/// of `<unknown>`).
pub fn render(
    source: &SourceText,
    diags: Vec<Diag>,
    root: Option<&SyntaxNode>,
    source_path: Option<&Path>,
) {
    let text = source.as_str();
    let src_id: String = source_path
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    for d in &diags {
        let span = resolve(&d.span, root);
        let kind = match d.kind.severity() {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
        };
        let message = d.kind.to_string();
        let mut builder = Report::build(kind, (&src_id, span.clone()))
            .with_code(d.kind.code())
            .with_message(&message)
            .with_label(Label::new((&src_id, span)).with_message(&message));
        for (sp, label) in d.kind.secondary_labels() {
            builder =
                builder.with_label(Label::new((&src_id, resolve(&sp, root))).with_message(label));
        }
        for note in d.kind.notes() {
            builder = builder.with_note(note);
        }
        if let Some(help) = d.kind.help() {
            builder = builder.with_help(help);
        }
        let _ = builder.finish().eprint((&src_id, Source::from(text)));
    }
}
