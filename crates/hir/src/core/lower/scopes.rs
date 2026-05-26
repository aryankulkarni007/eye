//! Lexical scope stack used while lowering a function body.

use rustc_hash::FxHashMap;

use crate::core::{LocalId, Text};

#[derive(Debug, Default)]
pub struct Scopes {
    stack: Vec<FxHashMap<Text, LocalId>>,
}

impl Scopes {
    pub fn new() -> Self {
        Self {
            stack: vec![FxHashMap::default()],
        }
    }

    pub fn push(&mut self) {
        self.stack.push(FxHashMap::default());
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    pub fn define(&mut self, name: Text, id: LocalId) {
        self.stack
            .last_mut()
            .expect("at least one scope frame")
            .insert(name, id);
    }

    pub fn lookup(&self, name: &Text) -> Option<LocalId> {
        self.stack.iter().rev().find_map(|f| f.get(name).copied())
    }
}
