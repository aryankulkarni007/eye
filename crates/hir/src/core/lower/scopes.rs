//! Lexical scope stack used while lowering a function body.

use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::core::{LocalConstId, LocalId, Text};

/// What a name in a lexical scope frame binds to: a runtime local (`let`/`mut`
/// or a pattern binding) or a block-scope `const`.
#[derive(Debug, Clone, Copy)]
pub enum Binding {
    Local(LocalId),
    Const(LocalConstId),
}

#[derive(Debug, Default)]
pub struct Scopes {
    stack: Vec<FxHashMap<Text, Binding>>,
}

impl Scopes {
    pub fn new() -> Self {
        Self {
            stack: vec![FxHashMap::with_capacity_and_hasher(16, FxBuildHasher)],
        }
    }

    pub fn push(&mut self) {
        self.stack
            .push(FxHashMap::with_capacity_and_hasher(4, FxBuildHasher));
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    pub fn define(&mut self, name: Text, id: LocalId) {
        self.insert(name, Binding::Local(id));
    }

    pub fn define_const(&mut self, name: Text, id: LocalConstId) {
        self.insert(name, Binding::Const(id));
    }

    fn insert(&mut self, name: Text, binding: Binding) {
        self.stack
            .last_mut()
            .expect("at least one scope frame")
            .insert(name, binding);
    }

    pub fn lookup(&self, name: &Text) -> Option<Binding> {
        self.stack.iter().rev().find_map(|f| f.get(name).copied())
    }
}
