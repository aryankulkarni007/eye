//! LSP message loop and dispatch.

mod notifications;
mod requests;

use lsp_server::{Connection, Message};

use crate::documents::DocumentStore;

pub fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut documents = DocumentStore::default();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if requests::handle_request(connection, req, &documents)? {
                    return Ok(());
                }
            }
            Message::Notification(not) => {
                notifications::handle_notification(connection, not, &mut documents)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}
