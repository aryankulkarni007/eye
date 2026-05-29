//! Semantic token legend. Indices must match the order in [`legend`] exactly.

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
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_indices_match_declaration_order() {
        let leg = legend();
        assert_eq!(leg.token_types.len(), 15);
        assert_eq!(TYPE, 0);
        assert_eq!(ENUM, 1);
        assert_eq!(STRUCT, 2);
        assert_eq!(PARAMETER, 3);
        assert_eq!(VARIABLE, 4);
        assert_eq!(PROPERTY, 5);
        assert_eq!(ENUM_MEMBER, 6);
        assert_eq!(FUNCTION, 7);
        assert_eq!(METHOD, 8);
        assert_eq!(KEYWORD, 9);
        assert_eq!(COMMENT, 10);
        assert_eq!(STRING, 11);
        assert_eq!(NUMBER, 12);
        assert_eq!(OPERATOR, 13);
        assert_eq!(FALLBACK, 14);
    }
}
