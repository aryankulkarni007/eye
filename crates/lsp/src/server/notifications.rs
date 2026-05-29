//! LSP notification handlers.

use lsp_server::{Connection, Message};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Url,
};

use crate::diagnostics::{parser_diagnostics, publish_diagnostics_notification};
use crate::documents::DocumentStore;
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
            publish_parse_diagnostics(connection, &params.text_document.uri, &text)?;
        }
        "textDocument/didChange" => {
            let params: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
            let uri = params.text_document.uri.to_string();
            if let Some(change) = params.content_changes.into_iter().last() {
                documents.change(&uri, change.text.clone());
                publish_parse_diagnostics(connection, &params.text_document.uri, &change.text)?;
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

fn publish_parse_diagnostics(connection: &Connection, uri: &Url, text: &str) -> anyhow::Result<()> {
    let source = SourceText::new(text.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parse(&lexed.tokens, &source);
    let diags = parser_diagnostics(text, &parse);
    let notification = publish_diagnostics_notification(uri, diags);
    connection
        .sender
        .send(Message::Notification(notification))?;
    Ok(())
}
