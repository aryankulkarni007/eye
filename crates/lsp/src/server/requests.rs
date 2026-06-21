//! LSP request handlers backed by the salsa [`Database`].
//!
//! every handler receives an immutable `&Database` for query access and an
//! immutable `&DocumentStore` for URI-to-input mapping. the queries are
//! salsa-memoized, so repeated requests within one revision reuse cached
//! results without re-execution.

use ast::SourceFile;
use database::{CheckedFile, Database};
use hir::core::{BodyId, Expr, ExprId, FnId, Function, HIR, Pat, PatId, Resolution};
use lexer::SourceText;
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    Hover, HoverContents, HoverParams, MarkedString, SemanticTokensParams,
};
use text_size::{TextRange, TextSize};

use crate::documents::DocumentStore;
use crate::highlight::compute_semantic_tokens;

pub const METHOD_NOT_FOUND: i32 = -32601;

/// the hover tooltip for whatever the cursor sits on. three element kinds carry
/// information worth surfacing, checked most-specific first:
///
/// - a **function name** (at its definition or any reference) renders its
///   signature prefixed with the inferred effect set (`io foo(int32 n) -> int32`);
/// - a **variable binding** renders its type;
/// - any other **expression** renders its type.
///
/// `None` when the cursor is not over any of these.
fn hover_info(checked: &CheckedFile, ast: &SourceFile, offset: TextSize) -> Option<String> {
    // a function name at its definition is a bare CST token with no body
    // expression, so it is found from the parse tree, not the source maps.
    if let Some(rendered) = hover_fn_def(checked, ast, offset) {
        return Some(rendered);
    }
    hover_in_body(checked, offset)
}

/// hover over the name token of a top-level `fn` definition: its signature plus
/// the inferred effect set.
fn hover_fn_def(checked: &CheckedFile, ast: &SourceFile, offset: TextSize) -> Option<String> {
    for item in ast.items() {
        let ast::Item::FnDef(f) = item else { continue };
        let Some(name) = f.name() else { continue };
        if !name.text_range().contains_inclusive(offset) {
            continue;
        }
        let target = name.text();
        let (fn_id, function) = checked
            .hir
            .functions
            .iter()
            .find(|(_, function)| function.name == target)?;
        return Some(render_fn(
            &checked.hir,
            function,
            &checked.effects.effect_of(fn_id).to_string(),
        ));
    }
    None
}

/// hover over an element inside a function body: the smallest source range
/// covering `offset` wins. a binding renders its type, a function reference its
/// signature + effect, any other expression its type.
fn hover_in_body(checked: &CheckedFile, offset: TextSize) -> Option<String> {
    let hir = &checked.hir;
    let mut best: Option<(TextRange, FnId, BodyId, Hit)> = None;
    for (fn_id, function) in hir.functions.iter() {
        let Some(body_id) = function.body else { continue };
        let body = &hir.bodies[body_id];
        for (expr_id, ptr) in body.source_map.expr.iter() {
            consider(&mut best, offset, ptr.text_range(), fn_id, body_id, Hit::Expr(expr_id.into()));
        }
        for (pat_id, ptr) in body.source_map.pat.iter() {
            consider(&mut best, offset, ptr.text_range(), fn_id, body_id, Hit::Pat(pat_id.into()));
        }
    }

    let (_, fn_id, body_id, hit) = best?;
    let body = &hir.bodies[body_id];
    match hit {
        Hit::Expr(expr_id) => {
            // a value-position reference to a function shows its signature and
            // effect, not just the bare function-pointer type.
            if let Expr::Path(Resolution::Fn(target)) = &body.exprs[expr_id] {
                return Some(render_fn(
                    hir,
                    &hir.functions[*target],
                    &checked.effects.effect_of(*target).to_string(),
                ));
            }
            let ty = checked.typeck.get(&fn_id)?.expr_types.get(expr_id.into())?;
            Some(hir.types.display(*ty).to_string())
        }
        Hit::Pat(pat_id) => {
            let Pat::Bind(local) = &body.pats[pat_id] else {
                return None;
            };
            // a binding's type is its declared annotation, or - when lowering
            // left it untyped - the type inference recorded in `local_types`.
            let ty = body.locals[*local].ty.or_else(|| {
                checked
                    .typeck
                    .get(&fn_id)
                    .and_then(|r| r.local_types.get(local).copied())
            })?;
            Some(hir.types.display(ty).to_string())
        }
    }
}

/// the kind of body element under the cursor, paired with its arena id.
enum Hit {
    Expr(ExprId),
    Pat(PatId),
}

/// keep `hit` if its `range` covers `offset` and is tighter than the current
/// best (smaller range = more specific element).
fn consider(
    best: &mut Option<(TextRange, FnId, BodyId, Hit)>,
    offset: TextSize,
    range: TextRange,
    fn_id: FnId,
    body_id: BodyId,
    hit: Hit,
) {
    if range.contains_inclusive(offset)
        && best.as_ref().is_none_or(|(r, ..)| range.len() < r.len())
    {
        *best = Some((range, fn_id, body_id, hit));
    }
}

/// render a function signature prefixed with its inferred effect set, in the
/// language's own surface syntax: `io foo(int32 a, &uint8 b) -> int32`.
fn render_fn(hir: &HIR, function: &Function, effect: &str) -> String {
    let mut out = format!("{effect} {}(", function.name);
    for (i, param) in function.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("{} {}", hir.types.display(param.ty), param.name));
    }
    out.push(')');
    if let Some(ret) = function.ret {
        out.push_str(&format!(" -> {}", hir.types.display(ret)));
    }
    out
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
                let parse = database::parse(db, input);
                let checked = database::lowered_file(db, input);
                hover_info(&checked, &parse.ast(), offset).map(|info| Hover {
                    contents: HoverContents::Scalar(MarkedString::String(info)),
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

    fn hover_at(db: &Database, input: database::SourceFileInput, line: u32, col: u32) -> Option<String> {
        let source = SourceText::new(input.text(db).to_owned());
        let offset = source.offset_utf16(line, col);
        let parse = database::parse(db, input);
        let checked = database::lowered_file(db, input);
        hover_info(&checked, &parse.ast(), offset)
    }

    #[test]
    fn hover_reports_expression_type() {
        let src = "main() {\n    let int32 x = 5 + 2;\n}\n";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        // cursor on the `5` literal (line 1, col 18, zero-based UTF-16).
        assert_eq!(hover_at(&db, input, 1, 18).as_deref(), Some("int32"));
        // cursor on the `{` of the body - no element there.
        assert_eq!(hover_at(&db, input, 0, 7), None);
    }

    #[test]
    fn hover_reports_variable_binding_type() {
        let src = "main() {\n    let int32 x = 5 + 2;\n}\n";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        // cursor on the binding name `x` (line 1, col 14).
        assert_eq!(hover_at(&db, input, 1, 14).as_deref(), Some("int32"));
    }

    #[test]
    fn hover_reports_function_signature_and_effect() {
        let src = "pure square(int32 n) -> int32 {\n    n * n\n}\n";
        let db = Database::default();
        let input = database::SourceFileInput::new(&db, "test.eye".into(), src.into());
        // cursor on the function name `square` (line 0, col 5 inside `square`).
        assert_eq!(
            hover_at(&db, input, 0, 7).as_deref(),
            Some("pure square(int32 n) -> int32"),
        );
    }
}
