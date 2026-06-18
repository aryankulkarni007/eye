//! semantic token legend and server capabilities.
//!
//! token type indices must match the order in [`legend`] exactly.

use lsp_types::{
    SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensServerCapabilities, ServerCapabilities,
};

pub const TYPE: u32 = 0;
pub const ENUM: u32 = 1;
pub const STRUCT: u32 = 2;
pub const PARAMETER: u32 = 3;
pub const VARIABLE: u32 = 4;
pub const PROPERTY: u32 = 5;
pub const ENUM_MEMBER: u32 = 6;
pub const FUNCTION: u32 = 7;
pub const METHOD: u32 = 8;
pub const KEYWORD: u32 = 9;
pub const COMMENT: u32 = 10;
pub const STRING: u32 = 11;
pub const NUMBER: u32 = 12;
pub const OPERATOR: u32 = 13;
pub const FALLBACK: u32 = 14;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
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
    }
}

pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(lsp_types::TextDocumentSyncCapability::Kind(
            lsp_types::TextDocumentSyncKind::FULL,
        )),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: legend(),
                range: None,
                full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        ..Default::default()
    }
}
