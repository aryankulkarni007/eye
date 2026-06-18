//! LSP request handlers backed by the salsa [`Database`].
//!
//! every handler receives an immutable `&Database` for query access and an
//! immutable `&DocumentStore` for URI-to-input mapping. the queries are
//! salsa-memoized, so repeated requests within one revision reuse cached
//! results without re-execution.

use database::{CheckedFile, Database};
use lexer::SourceText;
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    Hover, HoverContents, HoverParams, MarkedString, SemanticTokensParams,
};
use text_size::{TextRange, TextSize};

use crate::documents::DocumentStore;
use crate::highlight::compute_semantic_tokens;

pub const METHOD_NOT_FOUND: i32 = -32601;

/// the type of the innermost expression whose source range covers `offset`,
/// rendered for a hover tooltip. scans every checked body's expr source-map for
/// the smallest range containing the cursor, then reads that expr's type from
/// the body's [`TypeckResults`]. `None` when the cursor is not over a typed
/// expression.
fn hover_type(checked: &CheckedFile, offset: TextSize) -> Option<String> {
    let hir = &checked.hir;
    let mut best: Option<(TextRange, hir::core::TypeRef)> = None;
    for (fn_id, function) in hir.functions.iter() {
        let (Some(body_id), Some(results)) = (function.body, checked.typeck.get(&fn_id)) else {
            continue;
        };
        for (expr_id, ptr) in hir.bodies[body_id].source_map.expr.iter() {
            let range = ptr.text_range();
            if range.contains_inclusive(offset)
                && let Some(&ty) = results.expr_types.get(expr_id)
                && best.is_none_or(|(r, _)| range.len() < r.len())
            {
                best = Some((range, ty));
            }
        }
    }
    best.map(|(_, ty)| hir.types.display(ty).to_string())
}

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
                    let checked = database::lowered_file(db, input);
                    let hir = &checked.hir;
                    compute_semantic_tokens(&source, &lexed, &parse, hir).unwrap_or(
                        lsp_types::SemanticTokens {
                            result_id: None,
                            data: vec![],
                        },
                    )
                })
                .unwrap_or(lsp_types::SemanticTokens {
                    result_id: None,
                    data: vec![],
                });
            let response = Response::new_ok(req.id, serde_json::to_value(tokens)?);
            connection.sender.send(Message::Response(response))?;
        }
        "textDocument/hover" => {
            let params: HoverParams = serde_json::from_value(req.params)?;
            let pos = params.text_document_position_params.position;
            let uri = params.text_document_position_params.text_document.uri;
            let hover = documents.get(uri.as_str()).and_then(|input| {
                let input = *input;
                let source = SourceText::new(input.text(db).to_owned());
                let offset = source.offset_utf16(pos.line, pos.character);
                let checked = database::lowered_file(db, input);
                hover_type(&checked, offset).map(|ty| Hover {
                    contents: HoverContents::Scalar(MarkedString::String(ty)),
                    range: None,
                })
            });
            let response = Response::new_ok(req.id, serde_json::to_value(hover)?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use database::Database;

    #[test]
    fn hover_reports_expression_type() {
        let src = "main() {\n    let int32 x = 5 + 2;\n}\n";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        let source = SourceText::new(input.text(&db).to_owned());
        let checked = database::lowered_file(&db, input);
        // cursor on the `5` literal (line 1, col 18, zero-based UTF-16).
        let on_lit = source.offset_utf16(1, 18);
        assert_eq!(hover_type(&checked, on_lit).as_deref(), Some("int32"));
        // cursor on the `{` of the body - no expression there.
        let off_expr = source.offset_utf16(0, 7);
        assert_eq!(hover_type(&checked, off_expr), None);
    }
}
