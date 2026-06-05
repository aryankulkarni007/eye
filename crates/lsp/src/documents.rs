//! Open document buffer keyed by URI.

use rustc_hash::FxHashMap;

#[derive(Debug, Default)]
pub struct DocumentStore {
    by_uri: FxHashMap<String, String>,
}

impl DocumentStore {
    pub fn open(&mut self, uri: &str, text: String) {
        self.by_uri.insert(uri.to_string(), text);
    }

    pub fn change(&mut self, uri: &str, text: String) {
        self.by_uri.insert(uri.to_string(), text);
    }

    pub fn close(&mut self, uri: &str) {
        self.by_uri.remove(uri);
    }

    pub fn get(&self, uri: &str) -> Option<&str> {
        self.by_uri.get(uri).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_change_close() {
        let mut store = DocumentStore::default();
        store.open("file:///a.eye", "let x = 1;".into());
        assert_eq!(store.get("file:///a.eye"), Some("let x = 1;"));
        store.change("file:///a.eye", "let y = 2;".into());
        assert_eq!(store.get("file:///a.eye"), Some("let y = 2;"));
        store.close("file:///a.eye");
        assert_eq!(store.get("file:///a.eye"), None);
    }
}
