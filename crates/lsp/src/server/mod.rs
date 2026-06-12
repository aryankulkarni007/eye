//! EXPERIMENTAL: LSP message loop and dispatch backed by a salsa [`Database`].
//!
//! The database owns all incremental compilation state. Every request or
//! notification handler gets either `&Database` (queries) or `&mut Database`
//! (input mutation, e.g. `set_text` on `didChange`). The document store maps
//! URIs to salsa input handles — never raw strings.
//!
//! Future: when `compile_file` is decomposed into per-function queries,
//! handlers will call fine-grained methods like `db.lower_fn(fn_id)` instead
//! of the whole-file result.

mod notifications;
mod requests;

use database::Database;
use lsp_server::{Connection, Message};

use crate::documents::DocumentStore;

pub fn main_loop(connection: &Connection) -> anyhow::Result<()> {
    let mut db = Database::default();
    let mut documents = DocumentStore::default();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if requests::handle_request(connection, req, &db, &documents)? {
                    return Ok(());
                }
            }
            Message::Notification(not) => {
                notifications::handle_notification(connection, not, &mut db, &mut documents)?;
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}
