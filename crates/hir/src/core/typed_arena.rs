//! type-safe arena wrapper.
//!
//! [`TypedArena<T, Id>`] wraps an [`Arena<T>`] and returns `Id` (a newtype
//! around [`Idx<T>`]) from [`alloc`](typedarena::alloc) instead of a raw
//! `Idx<T>`. together with the newtype indices in [`ids`](super::ids) this
//! makes every arena index carry its element type at the type level, so a
//! `StructId` and a `FnId` are distinct types that the compiler refuses to
//! mix up.
//!
//! the wrapper is transparent to everything that reads from arenas (`Index`,
//! `IndexMut`, `iter`, `len`) so the vast majority of call sites need no
//! changes beyond the struct-field type.
//!
//! # `ArenaMap` compatibility
//!
//! [`ArenaMap`](la_arena::arenamap) in `la_arena` v0.3 is concretely typed to
//! key on [`Idx<T>`], so `ArenaMap` fields in [`Body`](super::body) still
//! store `Idx<T>` keys. use `.into()` to convert a newtype `Id` to `Idx<T>`
//! when calling `ArenaMap::insert` / `get`.

use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use la_arena::{Arena, Idx};

/// an arena that returns a newtype `Id` from
/// [`alloc`](typedarena::alloc) instead of a raw [`Idx<T>`].
///
/// `Id` must implement [`From<Idx<T>>`] (for [`alloc`](typedarena::alloc))
/// and [`Into<Idx<T>>`] (for [`Index`] / [`IndexMut`]).
/// `PhantomData<fn() -> Id>` rather than `PhantomData<*const Id>`: the arena
/// neither stores nor points at an `Id`, and the `fn` spelling keeps the
/// wrapper `Send + Sync` whenever `T` is (a raw-pointer phantom poisons both,
/// which would bar `HIR` from being a salsa query result).
#[derive(Debug)]
pub struct TypedArena<T, Id> {
    inner: Arena<T>,
    _phantom: PhantomData<fn() -> Id>,
}

// ---- construction ----------------------------------------------------------

impl<T, Id> TypedArena<T, Id> {
    pub fn new() -> Self {
        Self {
            inner: Arena::new(),
            _phantom: PhantomData,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arena::with_capacity(capacity),
            _phantom: PhantomData,
        }
    }
}

impl<T, Id> Default for TypedArena<T, Id> {
    fn default() -> Self {
        Self::new()
    }
}

// ---- allocation (requires id: from<idx<t>>) --------------------------------

impl<T, Id: From<Idx<T>>> TypedArena<T, Id> {
    /// allocate a new element, returning its typed index.
    pub fn alloc(&mut self, value: T) -> Id {
        Id::from(self.inner.alloc(value))
    }

    /// borrow the inner [`Arena<T>`] for low-level `ArenaMap` operations.
    pub fn inner(&self) -> &Arena<T> {
        &self.inner
    }
}

// ---- read / write (requires id: into<idx<t>>) ------------------------------

impl<T, Id: Into<Idx<T>> + Copy> Index<Id> for TypedArena<T, Id> {
    type Output = T;

    fn index(&self, id: Id) -> &T {
        &self.inner[id.into()]
    }
}

impl<T, Id: Into<Idx<T>> + Copy> IndexMut<Id> for TypedArena<T, Id> {
    fn index_mut(&mut self, id: Id) -> &mut T {
        &mut self.inner[id.into()]
    }
}

// ---- iteration (requires id: from<idx<t>>) ---------------------------------

impl<T, Id: From<Idx<T>>> TypedArena<T, Id> {
    /// iterate over `(Id, &T)` pairs in insertion order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (Id, &T)> + DoubleEndedIterator + Clone {
        self.inner.iter().map(|(idx, value)| (Id::from(idx), value))
    }

    /// the number of elements in the arena.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// whether the arena is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// allocate a new element and return the raw [`Idx<T>`] (for callers that
    /// need to pass the index to an [`ArenaMap`](la_arena::arenamap)).
    pub fn alloc_raw(&mut self, value: T) -> Idx<T> {
        self.inner.alloc(value)
    }
}

/// macro to define a type-safe arena index newtype.
///
/// `$name` becomes a struct newtype around [`Idx<$inner>`] that implements
/// [`From<Idx<$inner>>`] and [`Into<Idx<$inner>>`].
///
/// # example
///
/// ```ignore
/// arena_id!(structid, struct);
/// arena_id!(fnid, function);
/// ```
#[macro_export]
macro_rules! arena_id {
    ($name:ident, $inner:ty) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) la_arena::Idx<$inner>);

        impl From<la_arena::Idx<$inner>> for $name {
            fn from(idx: la_arena::Idx<$inner>) -> Self {
                $name(idx)
            }
        }

        impl From<$name> for la_arena::Idx<$inner> {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl $name {
            pub fn raw_idx(self) -> la_arena::RawIdx {
                self.0.into_raw()
            }
        }
    };
}
