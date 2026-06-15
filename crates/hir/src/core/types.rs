//! type references as they exist at HIR time.
//!
//! stays *unresolved* at HIR time: just a name. type inference / resolution
//! runs in a later pass and produces real `Ty` ids. builtins (`int32`, `bool`)
//! are still recognized here as a convenience.
//!
//! types are interned: `TypeRef` is a `Copy` handle (a `u32` index) into a
//! [`TypeInterner`] that stores the structural [`TypeKind`] data. this makes
//! `Clone`, `PartialEq`, `Eq`, and `Hash` all o(1) operations on the handle,
//! and deduplicates structurally identical types.

use std::fmt;
use std::ops::Index;

use rustc_hash::{FxBuildHasher, FxHashMap};

use super::*;

/// a handle to an interned type representation.
///
/// o(1) `Clone`, `PartialEq`, `Eq`, `Hash`. resolve the structural
/// [`TypeKind`] through a [`TypeInterner`] to inspect or pattern-match.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct TypeRef(u32);

/// the structural shape of a type, stored in a [`TypeInterner`].
///
/// recursive children are referenced by [`TypeRef`] handle rather than
/// `Rc<TypeRef>`, so `Clone` on a `TypeKind` clones `Vec<TypeRef>` only
/// (each element is a `Copy` handle).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// a named type (e.g. `int32`, `Point`).
    Path(Text),
    /// `&T` -- a shared-reference type.
    Ref(TypeRef),
    /// `T*` -- a typed raw-pointer type.
    Ptr(TypeRef),
    /// `ptr` -- the untyped raw pointer (c `void*`). a real variant rather
    /// than a magic `Path("ptr")` so every type judgment dispatches on
    /// structure, not on a name (typeck S0 prep).
    RawPtr,
    /// `[T; N]` -- a fixed-size array.
    Array { elem: TypeRef, len: u64 },
    /// `(A, B) -> R` -- a function-pointer type.
    Fn {
        params: Vec<TypeRef>,
        ret: Option<TypeRef>,
        /// `true` when this is the type of a variadic extern
        /// (`printf(string, ...)`), so indirect calls through a pointer to
        /// it can be arity-checked as a minimum, not an exact count.
        variadic: bool,
    },
    /// the error sentinel -- produced when a prior diagnostic already fired.
    Error,
}

/// an interner that deduplicates [`TypeKind`] values and assigns each a unique
/// [`TypeRef`] handle.
///
/// all well-known primitive types (`int32`, `bool`, ...) are pre-injected at
/// construction.
#[derive(Debug, Clone)]
pub struct TypeInterner {
    arena: Vec<TypeKind>,
    map: FxHashMap<TypeKind, TypeRef>,
    /// pre-cached read-only handles for the most-requested builtin types.
    /// avoids needing `&mut self` just to look up `int32`, `uint8`, `usize`,
    /// or the error sentinel.
    error_ty: TypeRef,
    int32_ty: TypeRef,
    uint8_ty: TypeRef,
    usize_ty: TypeRef,
}

impl TypeInterner {
    /// create a new interner with all primitive types pre-injected.
    pub fn new() -> Self {
        let mut this = TypeInterner {
            arena: Vec::new(),
            map: FxHashMap::with_capacity_and_hasher(32, FxBuildHasher),
            error_ty: TypeRef(0),
            int32_ty: TypeRef(0),
            uint8_ty: TypeRef(0),
            usize_ty: TypeRef(0),
        };
        this.inject_builtins();
        this
    }

    fn inject_builtins(&mut self) {
        self.error_ty = self.intern(TypeKind::Error);
        for name in &[
            "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32",
            "float64", "bool", "char", "string", "usize", "isize", "void",
        ] {
            self.intern(TypeKind::Path(Text::from(*name)));
        }
        // `ptr` is structural (`TypeKind::RawPtr`), not a named path.
        self.intern(TypeKind::RawPtr);
        // pre-cached handles for the most-requested builtins (read-only
        // lookups that avoid needing `&mut self`).
        self.int32_ty = self.intern(TypeKind::Path(Text::from("int32")));
        self.uint8_ty = self.intern(TypeKind::Path(Text::from("uint8")));
        self.usize_ty = self.intern(TypeKind::Path(Text::from("usize")));
    }

    /// convenience: retrieve the canonical error sentinel type (read-only).
    pub fn error_type(&self) -> TypeRef {
        self.error_ty
    }

    /// retrieve the pre-injected `int32` handle (read-only).
    pub fn int32_ty(&self) -> TypeRef {
        self.int32_ty
    }

    /// retrieve the pre-injected `uint8` handle (read-only).
    pub fn uint8_ty(&self) -> TypeRef {
        self.uint8_ty
    }

    /// retrieve the pre-injected `usize` handle (read-only).
    pub fn usize_ty(&self) -> TypeRef {
        self.usize_ty
    }

    /// intern a [`TypeKind`], returning its canonical [`TypeRef`] handle.
    ///
    /// recursive child handles in `kind` must already be interned (i.e.
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

    /// look up the [`TypeKind`] for a [`TypeRef`] handle.
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
    /// the inner type of `Ref` or `Ptr`.
    pub fn inner_ref_ptr(self, interner: &TypeInterner) -> Option<TypeRef> {
        match interner.lookup(self) {
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => Some(*inner),
            _ => None,
        }
    }

    /// the element type and length of `Array`.
    pub fn as_array(self, interner: &TypeInterner) -> Option<(TypeRef, u64)> {
        match interner.lookup(self) {
            TypeKind::Array { elem, len } => Some((*elem, *len)),
            _ => None,
        }
    }

    /// the `Fn` pointer's parameter types and optional return type.
    pub fn as_fn(self, interner: &TypeInterner) -> Option<(&[TypeRef], Option<TypeRef>)> {
        match interner.lookup(self) {
            TypeKind::Fn { params, ret, .. } => Some((params, *ret)),
            _ => None,
        }
    }

    /// whether this is the error sentinel type.
    pub fn is_error(self, interner: &TypeInterner) -> bool {
        matches!(interner.lookup(self), TypeKind::Error)
    }
}

/// a helper returned by [`TypeInterner::display`] that formats a [`TypeRef`]
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
            TypeKind::RawPtr => write!(f, "ptr"),
            TypeKind::Array { elem, len } => {
                write!(f, "[{}; {len}]", self.interner.display(*elem))
            }
            TypeKind::Fn {
                params,
                ret,
                variadic,
            } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", self.interner.display(*p))?;
                }
                if *variadic {
                    if !params.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "...")?;
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
    /// return a [`Display`] wrapper that formats `ty` by resolving through
    /// this interner.
    pub fn display(&self, ty: TypeRef) -> TypeRefDisplay<'_> {
        TypeRefDisplay { interner: self, ty }
    }

    /// walk a [`TypeRef`] tree, calling [`VisitTypeRef::visit_ty`] for each
    /// node in pre-order (before children) and [`VisitTypeRef::visit_ty_post`]
    /// in post-order (after children). the pre-order call can return `false` to
    /// prune a subtree; the post-order call is skipped when the subtree is
    /// pruned.
    pub fn walk(&self, ty: TypeRef, visitor: &mut impl VisitTypeRef) {
        if !visitor.visit_ty(ty, self) {
            return;
        }
        match self.lookup(ty) {
            TypeKind::Ref(inner) | TypeKind::Ptr(inner) => self.walk(*inner, visitor),
            TypeKind::Array { elem, .. } => self.walk(*elem, visitor),
            TypeKind::Fn { params, ret, .. } => {
                for &p in params {
                    self.walk(p, visitor);
                }
                if let Some(r) = ret {
                    self.walk(*r, visitor);
                }
            }
            TypeKind::Path(_) | TypeKind::RawPtr | TypeKind::Error => {}
        }
        visitor.visit_ty_post(ty, self);
    }
}

/// trait for walking a [`TypeRef`] tree.
///
/// implement [`visit_ty`](VisitTypeRef::visit_ty) to run logic in pre-order
/// (before children), and optionally [`visit_ty_post`](VisitTypeRef::visit_ty_post)
/// for post-order (after children). return `false` from `visit_ty` to prune
/// further recursion into the current node's children (the post-order callback
/// is also skipped when pruned).
///
/// # example
///
/// ```
/// # use hir::core::{TypeInterner, TypeKind, TypeRef, VisitTypeRef};
/// # let mut types = TypeInterner::new();
/// # let int32 = types.intern(TypeKind::Path("int32".into()));
/// # let arr = types.intern(TypeKind::Array { elem: int32, len: 4 });
/// struct CountRefs(usize);
/// impl VisitTypeRef for CountRefs {
/// fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
/// if matches!(types.lookup(ty), TypeKind::Ref(_)) {
/// self.0 += 1;
/// }
/// true
/// }
/// }
/// let mut v = CountRefs(0);
/// types.walk(arr, &mut v);
/// ```
pub trait VisitTypeRef {
    /// called for each [`TypeRef`] node during a [`TypeInterner::walk`].
    /// return `true` to continue walking into children, `false` to prune.
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool;

    /// called for each [`TypeRef`] node in post-order (after all children have
    /// been visited). not called when [`visit_ty`](visittyperef::visit_ty)
    /// returned `false` for the same node (i.e. the subtree was pruned).
    fn visit_ty_post(&mut self, _ty: TypeRef, _types: &TypeInterner) {}
}
