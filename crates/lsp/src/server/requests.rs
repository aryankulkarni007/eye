//! EXPERIMENTAL: LSP request handlers backed by the salsa [`Database`].
//!
//! Every handler receives an immutable `&Database` for query access and an
//! immutable `&DocumentStore` for URI-to-input mapping. The queries are
//! salsa-memoized, so repeated requests within one revision reuse cached
//! results without re-execution.

use database::Database;
use lexer::SourceText;
use lsp_server::{Connection, Message, Response};
use lsp_types::SemanticTokensParams;

use crate::documents::DocumentStore;
use crate::highlight::compute_semantic_tokens;

pub const METHOD_NOT_FOUND: i32 = -32601;

pub fn handle_request(
    connection: &Connection,
    req: lsp_server::Request,
    db: &Database,
    documents: &DocumentStore,
) -> anyhow::Result<bool> {
    if connection.handle_shutdown(&req)? {
        return Ok(true);
    }

    match req.method.as_str() {
        "textDocument/semanticTokens/full" => {
            let params: SemanticTokensParams = serde_json::from_value(req.params)?;
            let tokens = documents
                .get(params.text_document.uri.as_str())
                .map(|input| {
                    let input = *input;
                    let source = SourceText::new(input.text(db).to_owned());
                    let lexed = database::lex(db, input);
                    let parse = database::parse(db, input);
                    let hir = database::lowered_file(db, input);
                    compute_semantic_tokens(&source, &lexed, &parse, &hir)
                        .unwrap_or(lsp_types::SemanticTokens {
                            result_id: None,
                            data: vec![],
                        })
                })
                .unwrap_or(lsp_types::SemanticTokens {
                    result_id: None,
                    data: vec![],
                });
            let response = Response::new_ok(req.id, serde_json::to_value(tokens)?);
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
