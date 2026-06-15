//! open-document state backed by salsa [`SourceFileInput`] handles.
//!
//! each open file gets a salsa input handle; mutating the handle's text
//! bumps the database revision so cached queries are automatically
//! invalidated.

use database::{Database, SourceFileInput};
use rustc_hash::FxHashMap;
use salsa::Setter as _;

/// maps uris to their salsa input handles.
///
/// single-file only (current limitation). a multi-file database would hold
/// `FxHashMap<FileId, SourceFileInput>` plus a URI-to-fileid index.
#[derive(Debug, Default)]
pub struct DocumentStore {
    files: FxHashMap<String, SourceFileInput>,
}

impl DocumentStore {
    /// register a newly opened document, creating a salsa input handle.
    pub fn open(&mut self, db: &mut Database, uri: &str, path: String, text: String) {
        let input = SourceFileInput::new(db, path, text);
        self.files.insert(uri.to_string(), input);
    }

    /// update a document's text. bumps the database revision, which
    /// automatically invalidates any cached query results.
    pub fn change(&mut self, db: &mut Database, uri: &str, text: String) {
        if let Some(input) = self.files.get_mut(uri) {
            input.set_text(db).to(text);
        }
    }

    /// remove a closed document.
    pub fn close(&mut self, uri: &str) {
        self.files.remove(uri);
    }

    /// the salsa input handle for an open document, if any.
    pub fn get(&self, uri: &str) -> Option<&SourceFileInput> {
        self.files.get(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::Database;

    #[test]
    fn open_change_close() {
        let mut db = Database::default();
        let mut store = DocumentStore::default();
        store.open(
            &mut db,
            "file:///a.eye",
            "a.eye".into(),
            "let x = 1;".into(),
        );
        assert!(store.get("file:///a.eye").is_some());
        store.change(&mut db, "file:///a.eye", "let y = 2;".into());
        assert!(store.get("file:///a.eye").is_some());
        store.close("file:///a.eye");
        assert!(store.get("file:///a.eye").is_none());
    }
}
