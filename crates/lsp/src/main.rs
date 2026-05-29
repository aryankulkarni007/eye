//! lsp implementation simply to provide syntax highlighting for eye files

use lsp_server::Connection;
use lsp_types::{
    SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend, SemanticTokensOptions,
    ServerCapabilities,
};

mod semantic;
mod server;

fn main() -> anyhow::Result<()> {
    // println!("eylsp starting");
    let (connection, io_threads) = Connection::stdio();

    let legend = SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::TYPE,
            SemanticTokenType::ENUM,
            SemanticTokenType::STRUCT,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::METHOD,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::COMMENT,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::new("fallback"),
        ],
        token_modifiers: vec![SemanticTokenModifier::READONLY],
    };

    let capabilities = ServerCapabilities {
        text_document_sync: Some(lsp_types::TextDocumentSyncCapability::Kind(
            lsp_types::TextDocumentSyncKind::FULL,
        )),
        semantic_tokens_provider: Some(
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    legend,
                    range: None,
                    full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                    ..Default::default()
                },
            ),
        ),
        ..Default::default()
    };

    connection.initialize(serde_json::to_value(&capabilities).unwrap())?;
    server::main_loop(&connection)?;
    io_threads.join()?;
    Ok(())
}
