//! Eye language server: semantic highlighting and compiler diagnostics.
//!
//! Split by concern:
//! - [`legend`]: semantic token legend and server capabilities
//! - [`documents`]: open buffer store
//! - [`highlight`]: semantic token computation (CST + lexer)
//! - [`diagnostics`]: lexer, parser and HIR diagnostics as LSP diagnostics
//! - [`server`]: JSON-RPC loop

pub mod diagnostics;
pub mod documents;
pub mod highlight;
pub mod legend;
pub mod server;

pub use highlight::compute_semantic_tokens;
pub use legend::server_capabilities;

/// Run initialize + the main message loop. Caller must join I/O threads afterward.
pub fn run(connection: &lsp_server::Connection) -> anyhow::Result<()> {
    let caps = server_capabilities();
    connection.initialize(serde_json::to_value(caps)?)?;
    server::main_loop(connection)
}

#[cfg(test)]
mod tests {
    use super::legend;

    #[test]
    fn public_api_constants_stable() {
        assert_eq!(legend::STRUCT, 2);
        assert_eq!(legend::FUNCTION, 7);
    }
}
