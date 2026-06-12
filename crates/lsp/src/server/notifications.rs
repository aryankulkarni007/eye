//! EXPERIMENTAL: LSP notification handlers backed by the salsa [`Database`].
//!
//! On every `didOpen` / `didChange` the handler mutates the salsa input and
//! re-queries. Diagnostics phase-gate exactly like the CLI driver: lexer,
//! then parser, then HIR. The HIR phase goes through the *per-function*
//! query path (`database::hir_diagnostics`), so an edit that re-parses but
//! leaves a function's body node intact re-checks only the changed bodies.

use database::Database;
use lexer::SourceText;
use lsp_server::{Connection, Message};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Url,
};

use crate::diagnostics::{diags_to_lsp, publish_diagnostics_notification};
use crate::documents::DocumentStore;

pub fn handle_notification(
    connection: &Connection,
    not: lsp_server::Notification,
    db: &mut Database,
    documents: &mut DocumentStore,
) -> anyhow::Result<()> {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            let params: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri;
            let text = params.text_document.text;
            documents.open(db, uri.as_str(), uri.path().to_owned(), text.clone());
            publish_diagnostics(connection, &uri, db, documents)?;
        }
        "textDocument/didChange" => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri;
            if let Some(change) = params.content_changes.into_iter().last() {
                documents.change(db, uri.as_str(), change.text.clone());
                publish_diagnostics(connection, &uri, db, documents)?;
            }
        }
        "textDocument/didClose" => {
            let params: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri;
            documents.close(uri.as_str());
            let notification = publish_diagnostics_notification(&uri, vec![]);
            connection
                .sender
                .send(Message::Notification(notification))?;
        }
        _ => {}
    }
    Ok(())
}

/// Query the database and publish every diagnostic (lexer, parser, HIR),
/// short-circuiting exactly as the CLI driver does.
fn publish_diagnostics(
    connection: &Connection,
    uri: &Url,
    db: &Database,
    documents: &DocumentStore,
) -> anyhow::Result<()> {
    let Some(input) = documents.get(uri.as_str()) else {
        return Ok(());
    };

    let source = SourceText::new(input.text(db).to_owned());
    let input = *input;

    let diags = compute_diagnostics(db, input, source, uri);
    let notification = publish_diagnostics_notification(uri, diags);
    connection
        .sender
        .send(Message::Notification(notification))?;
    Ok(())
}

fn compute_diagnostics(
    db: &Database,
    input: database::SourceFileInput,
    source: SourceText,
    uri: &Url,
) -> Vec<lsp_types::Diagnostic> {
    // Phase 1 -- lexer: no tree exists, so root is None.
    let lexed = database::lex(db, input);
    if !lexed.diags.is_empty() {
        return diags_to_lsp(
            &source,
            uri,
            lexed.diags.clone().into_diags(),
            None,
            "eye-lexer",
        );
    }

    // Phase 2 -- parser: tree exists.
    let parse = database::parse(db, input);
    let root = parse.syntax();
    if !parse.diagnostics.is_empty() {
        return diags_to_lsp(
            &source,
            uri,
            parse.diagnostics.clone().into_diags(),
            Some(&root),
            "eye-parser",
        );
    }

    // Phase 3 -- HIR: item-scope + per-function body diagnostics.
    diags_to_lsp(
        &source,
        uri,
        database::hir_diagnostics(db, input).into_diags(),
        Some(&root),
        "eye-hir",
    )
}
