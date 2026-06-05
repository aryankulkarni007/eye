//! LSP notification handlers.

use lsp_server::{Connection, Message};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Url,
};

use crate::diagnostics::{diags_to_lsp, publish_diagnostics_notification};
use crate::documents::DocumentStore;
use ast::AstNode;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};
use parser::parse;

pub fn handle_notification(
    connection: &Connection,
    not: lsp_server::Notification,
    documents: &mut DocumentStore,
) -> anyhow::Result<()> {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri.to_string();
            let text = params.text_document.text;
            documents.open(&uri, text.clone());
            publish_diagnostics(connection, &params.text_document.uri, &text)?;
        }
        "textDocument/didChange" => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri.to_string();
            if let Some(change) = params.content_changes.into_iter().last() {
                documents.change(&uri, change.text.clone());
                publish_diagnostics(connection, &params.text_document.uri, &change.text)?;
            }
        }
        "textDocument/didClose" => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri.to_string();
            documents.close(&uri);
            let notification = publish_diagnostics_notification(&params.text_document.uri, vec![]);
            connection
                .sender
                .send(Message::Notification(notification))?;
        }
        _ => {}
    }
    Ok(())
}

/// Run the compiler pipeline far enough to collect every diagnostic, then
/// publish them. The phases short-circuit exactly as the `eye` driver does
/// (`src/main.rs`): a lexer error blocks parsing, a parse error blocks HIR
/// lowering - a downstream phase fed a broken tree only emits noise. The first
/// phase to report wins; when all phases are clean we publish an empty list,
/// which clears any stale diagnostics in the editor.
fn publish_diagnostics(connection: &Connection, uri: &Url, text: &str) -> anyhow::Result<()> {
    let source = SourceText::new(text.to_string());

    let diags = compute_diagnostics(&source, uri);
    let notification = publish_diagnostics_notification(uri, diags);
    connection
        .sender
        .send(Message::Notification(notification))?;
    Ok(())
}

fn compute_diagnostics(source: &SourceText, uri: &Url) -> Vec<lsp_types::Diagnostic> {
    // 1. lexer: no tree exists yet, so spans are tight byte ranges (root None).
    let lexed = Lexer::new(source).tokenize();
    if !lexed.diags.is_empty() {
        return diags_to_lsp(source, uri, lexed.diags.into_diags(), None, "eye-lexer");
    }

    // 2. parser: a tree now exists; pass it so pointer spans can be trimmed.
    let parse = parse(&lexed.tokens, source);
    if !parse.diagnostics.is_empty() {
        return diags_to_lsp(
            source,
            uri,
            parse.diagnostics.into_diags(),
            Some(&parse.green),
            "eye-parser",
        );
    }

    // 3. HIR: semantic diagnostics (name resolution, types, const-eval, ...) -
    // the bulk of useful editor feedback, invisible to lexer and parser alike.
    let Some(file) = ast::SourceFile::cast(parse.green.clone()) else {
        return Vec::new();
    };
    let hir = lower_source_file(file);
    diags_to_lsp(
        source,
        uri,
        hir.diagnostics.into_diags(),
        Some(&parse.green),
        "eye-hir",
    )
}
