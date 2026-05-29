//! LSP request handlers.

use lsp_server::{Connection, Message, Response};
use lsp_types::SemanticTokensParams;

use crate::documents::DocumentStore;
use crate::highlight::compute_semantic_tokens;

pub const METHOD_NOT_FOUND: i32 = -32601;

pub fn handle_request(
    connection: &Connection,
    req: lsp_server::Request,
    documents: &DocumentStore,
) -> anyhow::Result<bool> {
    if connection.handle_shutdown(&req)? {
        return Ok(true);
    }

    match req.method.as_str() {
        "textDocument/semanticTokens/full" => {
            let params: SemanticTokensParams = serde_json::from_value(req.params)?;
            let uri = params.text_document.uri.to_string();
            let text = documents.get(&uri).unwrap_or("");

            let response = match compute_semantic_tokens(text) {
                Ok(tokens) => Response::new_ok(req.id, serde_json::to_value(tokens)?),
                Err(e) => Response::new_err(req.id, -1, e.to_string()),
            };
            connection.sender.send(Message::Response(response))?;
        }
        _ => {
            let response = Response::new_err(
                req.id,
                METHOD_NOT_FOUND,
                format!("Method not found: {}", req.method),
            );
            connection.sender.send(Message::Response(response))?;
        }
    }

    Ok(false)
}
