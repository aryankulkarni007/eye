use lsp_server::{Connection, Message};
use lsp_types::{DidChangeTextDocumentParams, DidOpenTextDocumentParams, SemanticTokensParams};
use std::collections::HashMap;

use crate::semantic::compute_semantic_tokens;

pub fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut document_state: HashMap<String, String> = HashMap::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                match req.method.as_str() {
                    "textDocument/semanticTokens/full" => {
                        let params: SemanticTokensParams = serde_json::from_value(req.params)?;
                        let uri_str = params.text_document.uri.to_string();
                        let text = document_state.get(&uri_str).cloned().unwrap_or_default();

                        let response = match compute_semantic_tokens(&text) {
                            Ok(tokens) => lsp_server::Response::new_ok(
                                req.id,
                                serde_json::to_value(&tokens).unwrap(),
                            ),
                            Err(e) => lsp_server::Response::new_err(req.id, -1, e.to_string()),
                        };
                        connection.sender.send(Message::Response(response))?;
                    }
                    _ => {
                        eprintln!("Unknown request: {}", req.method);
                    }
                }
            }
            Message::Notification(not) => {
                match not.method.as_str() {
                    "textDocument/didOpen" => {
                        let params: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
                        document_state.insert(
                            params.text_document.uri.to_string(),
                            params.text_document.text,
                        );
                    }
                    "textDocument/didChange" => {
                        let params: DidChangeTextDocumentParams =
                            serde_json::from_value(not.params)?;
                        if let Some(change) = params.content_changes.into_iter().last() {
                            document_state
                                .insert(params.text_document.uri.to_string(), change.text);
                        }
                    }
                    // Handle 'exit' or other notifications if needed
                    _ => {}
                }
            }
            Message::Response(_resp) => {}
        }
    }
    Ok(())
}
