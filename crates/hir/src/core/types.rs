//! Type references as they exist at HIR time.
//!
//! Stays *unresolved* at HIR time: just a name. Type inference / resolution
//! runs in a later pass and produces real `Ty` ids. Builtins (`int32`, `bool`)
//! are still recognized here as a convenience.
//!
//! Types are interned: `TypeRef` is a `Copy` handle (a `u32` index) into a
//! [`TypeInterner`] that stores the structural [`TypeKind`] data. This makes
//! `Clone`, `PartialEq`, `Eq`, and `Hash` all O(1) operations on the handle,
//! and deduplicates structurally identical types.

use std::fmt;
use std::ops::Index;

use rustc_hash::{FxBuildHasher, FxHashMap};

use super::*;

/// A handle to an interned type representation.
///
/// O(1) `Clone`, `PartialEq`, `Eq`, `Hash`. Resolve the structural
/// [`TypeKind`] through a [`TypeInterner`] to inspect or pattern-match.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct TypeRef(u32);

/// The structural shape of a type, stored in a [`TypeInterner`].
///
/// Recursive children are referenced by [`TypeRef`] handle rather than
/// `Rc<TypeRef>`, so `Clone` on a `TypeKind` clones `Vec<TypeRef>` only
/// (each element is a `Copy` handle).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// A named type (e.g. `int32`, `Point`).
    Path(Text),
    /// `&T` — a shared-reference type.
    Ref(TypeRef),
    /// `*T` — a raw-pointer type.
    Ptr(TypeRef),
    /// `[T; N]` — a fixed-size array.
    Array {
        elem: TypeRef,
        len: u64,
    },
    /// `(A, B) -> R` — a function-pointer type.
    Fn {
        params: Vec<TypeRef>,
        ret: Option<TypeRef>,
    },
    /// The error sentinel — produced when a prior diagnostic already fired.
    Error,
}

/// An interner that deduplicates [`TypeKind`] values and assigns each a unique
/// [`TypeRef`] handle.
///
/// All well-known primitive types (`int32`, `bool`, ...) are pre-injected at
/// construction.
#[derive(Debug)]
pub struct TypeInterner {
    arena: Vec<TypeKind>,
    map: FxHashMap<TypeKind, TypeRef>,
}

impl TypeInterner {
    /// Create a new interner with all primitive types pre-injected.
    pub fn new() -> Self {
        let mut this = TypeInterner {
            arena: Vec::new(),
            map: FxHashMap::with_capacity_and_hasher(32, FxBuildHasher),
        };
        this.inject_builtins();
        this
    }

    fn inject_builtins(&mut self) {
        self.intern(TypeKind::Error);
        for name in &[
            "int8", "int16", "int32", "int64",
            "uint8", "uint16", "uint32", "uint64",
            "float32", "float64",
            "bool", "char", "string",
            "usize", "isize", "ptr", "void",
        ] {
            self.intern(TypeKind::Path(Text::from(*name)));
        }
    }

    /// Convenience: intern or retrieve the canonical error sentinel type.
    pub fn error_type(&mut self) -> TypeRef {
        // Pre-injected, so always returns the same handle
        self.intern(TypeKind::Error)
    }

    /// Intern a [`TypeKind`], returning its canonical [`TypeRef`] handle.
    ///
    /// Recursive child handles in `kind` must already be interned (i.e.
    /// obtained from this interner) so that structural equality is correctly
    /// detected.
    pub fn intern(&mut self, kind: TypeKind) -> TypeRef {
        if let Some(&id) = self.map.get(&kind) {
            return id;
        }
        let id = TypeRef(self.arena.len() as u32);
        self.arena.push(kind.clone());
        self.map.insert(kind, id);
        id
    }

    /// Look up the [`TypeKind`] for a [`TypeRef`] handle.
    pub fn lookup(&self, id: TypeRef) -> &TypeKind {
        &self.arena[id.0 as usize]
    }
}

impl Default for TypeInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl Index<TypeRef> for TypeInterner {
    type Output = TypeKind;
    fn index(&self, id: TypeRef) -> &TypeKind {
        self.lookup(id)
    }
}

impl TypeRef {
    /// The inner type of `Ref` or `Ptr`.
    pub fn inner_ref_ptr(self, interner: &TypeInterner) -> Option<TypeRef> {
        match interner.lookup(self) {
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The element type and length of `Array`.
    pub fn as_array(self, interner: &TypeInterner) -> Option<(TypeRef, u64)> {
        match interner.lookup(self) {
            TypeKind::Array { elem, len } => Some((*elem, *len)),
            _ => None,
        }
    }

    /// The `Fn` pointer's parameter types and optional return type.
    pub fn as_fn(self, interner: &TypeInterner) -> Option<(&[TypeRef], Option<TypeRef>)> {
        match interner.lookup(self) {
            TypeKind::Fn { params, ret } => Some((params, *ret)),
            _ => None,
        }
    }

    /// Whether this is the error sentinel type.
    pub fn is_error(self, interner: &TypeInterner) -> bool {
        matches!(interner.lookup(self), TypeKind::Error)
    }
}

/// A helper returned by [`TypeInterner::display`] that formats a [`TypeRef`]
/// handle by resolving it through the interner.
pub struct TypeRefDisplay<'a> {
    interner: &'a TypeInterner,
    ty: TypeRef,
}

impl<'a> fmt::Display for TypeRefDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.interner[self.ty] {
            TypeKind::Path(name) => write!(f, "{name}"),
            TypeKind::Ref(inner) => write!(f, "&{}", self.interner.display(*inner)),
            TypeKind::Ptr(inner) => write!(f, "{}*", self.interner.display(*inner)),
            TypeKind::Array { elem, len } => {
                write!(f, "[{}; {len}]", self.interner.display(*elem))
            }
            TypeKind::Fn { params, ret } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", self.interner.display(*p))?;
                }
                write!(f, ")")?;
                if let Some(ret) = ret {
                    write!(f, " -> {}", self.interner.display(*ret))?;
                }
                Ok(())
            }
            TypeKind::Error => write!(f, "<error>"),
        }
    }
}

impl TypeInterner {
    /// Return a [`Display`] wrapper that formats `ty` by resolving through
    /// this interner.
    pub fn display(&self, ty: TypeRef) -> TypeRefDisplay<'_> {
        TypeRefDisplay { interner: self, ty }
    }

    /// Walk a [`TypeRef`] tree, calling [`VisitTypeRef::visit_ty`] for each
    /// node in pre-order (before children) and [`VisitTypeRef::visit_ty_post`]
    /// in post-order (after children). The pre-order call can return `false` to
    /// prune a subtree; the post-order call is skipped when the subtree is
    /// pruned.
    pub fn walk(&self, ty: TypeRef, visitor: &mut impl VisitTypeRef) {
        if !visitor.visit_ty(ty, self) {
            return;
        }
        match self.lookup(ty) {
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => self.walk(*inner, visitor),
            TypeKind::Array { elem, .. } => self.walk(*elem, visitor),
            TypeKind::Fn { params, ret } => {
                for &p in params {
                    self.walk(p, visitor);
                }
                if let Some(r) = ret {
                    self.walk(*r, visitor);
                }
            }
            TypeKind::Path(_) | TypeKind::Error => {}
        }
        visitor.visit_ty_post(ty, self);
    }
}

/// Trait for walking a [`TypeRef`] tree.
///
/// Implement [`visit_ty`](VisitTypeRef::visit_ty) to run logic in pre-order
/// (before children), and optionally [`visit_ty_post`](VisitTypeRef::visit_ty_post)
/// for post-order (after children). Return `false` from `visit_ty` to prune
/// further recursion into the current node's children (the post-order callback
/// is also skipped when pruned).
///
/// # Example
///
/// ```
/// # use hir::core::{TypeInterner, TypeKind, TypeRef, VisitTypeRef};
/// # let mut types = TypeInterner::new();
/// # let int32 = types.intern(TypeKind::Path("int32".into()));
/// # let arr = types.intern(TypeKind::Array { elem: int32, len: 4 });
/// struct CountRefs(usize);
/// impl VisitTypeRef for CountRefs {
///     fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
///         if matches!(types.lookup(ty), TypeKind::Ref(_)) {
///             self.0 += 1;
///         }
///         true
///     }
/// }
/// let mut v = CountRefs(0);
/// types.walk(arr, &mut v);
/// ```
pub trait VisitTypeRef {
    /// Called for each [`TypeRef`] node during a [`TypeInterner::walk`].
    /// Return `true` to continue walking into children, `false` to prune.
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool;

    /// Called for each [`TypeRef`] node in post-order (after all children have
    /// been visited). Not called when [`visit_ty`](VisitTypeRef::visit_ty)
    /// returned `false` for the same node (i.e. the subtree was pruned).
    fn visit_ty_post(&mut self, _ty: TypeRef, _types: &TypeInterner) {}
}


