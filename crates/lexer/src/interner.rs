//! the string interner: [`Symbol`] handles and the [`Interner`] table that maps
//! every identifier / string-literal text to one canonical handle, plus the
//! `syntax::StringTable` impl that exposes it to HIR lowering.

use rustc_hash::{FxBuildHasher, FxHashMap};
use smol_str::SmolStr;
use syntax::StringTable;

/// a interned string handle - an index into an [`Interner`]'s table. `Copy`
/// and pointer-free, so name comparison downstream is a `u32` equality check
/// instead of a `str` compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Symbol(pub u32);

/// the canonical string table. every identifier and string literal the lexer
/// sees is interned here, so the same text always maps to the same [`Symbol`].
///
/// strings are stored as [`SmolStr`]: identifiers - almost always short -
/// stay inline with no heap allocation, and a cache-hit clone is `O(1)`.
///
/// the lexer pre-populates this during tokenizing; later stages (HIR name
/// resolution) re-intern identifier text against the *same* table - a cache
/// hit yields the original `Symbol`. the table outlives the lexer: it is
/// handed off in [`Lexed`].
#[derive(Debug)]
/// a string interner backed by a hash map. every distinct string is stored once
/// and identified by a lightweight [`Symbol`] handle.
///
/// ```
/// # use lexer::Interner;
/// let mut interner = Interner::new();
/// let a = interner.intern("hello");
/// let b = interner.intern("world");
/// let c = interner.intern("hello"); // same as `a`
///
/// assert_eq!(interner.lookup(a), "hello");
/// assert_eq!(interner.lookup(b), "world");
/// assert_eq!(a, c);
/// assert_eq!(interner.len(), 2);
/// ```
pub struct Interner {
    map: FxHashMap<SmolStr, Symbol>,
    vec: Vec<SmolStr>,
}

impl Interner {
    pub fn new() -> Self {
        Interner {
            map: FxHashMap::with_capacity_and_hasher(256, FxBuildHasher),
            vec: Vec::new(),
        }
    }

    /// intern `s`, returning its handle. idempotent: equal strings always map
    /// to the same [`Symbol`].
    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = Symbol(self.vec.len() as u32);
        let owned = SmolStr::new(s);
        self.map.insert(owned.clone(), id);
        self.vec.push(owned);
        id
    }

    /// the text behind a [`Symbol`]. panics if `id` came from another table.
    pub fn lookup(&self, id: Symbol) -> &str {
        debug_assert!(
            (id.0 as usize) < self.vec.len(),
            "Symbol({}) out of range for this Interner (len {}); it likely came from a different table",
            id.0,
            self.vec.len()
        );
        &self.vec[id.0 as usize]
    }

    /// number of distinct strings interned.
    /// retrieve the canonical [`SmolStr`] for `s` if it was already interned.
    /// returns `None` if `s` is not in the table. the clone is o(1) - short
    /// strings (≤22 bytes) are inline; long strings bump an `Arc` refcount.
    pub fn get(&self, s: &str) -> Option<SmolStr> {
        self.map.get(s).map(|&sym| self.vec[sym.0 as usize].clone())
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl StringTable for Interner {
    fn get(&self, s: &str) -> Option<SmolStr> {
        self.map.get(s).map(|&sym| self.vec[sym.0 as usize].clone())
    }
}
