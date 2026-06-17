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

use rustc_hash::FxBuildHasher;

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
    /// `()` -- the unit type. the type of an expression that completes but
    /// yields no value: a statement-position `if`/`loop`/`block`, a call to a
    /// function with no return type, an assignment. spellable in source as `()`.
    /// binding or operating on a unit value is a `VoidValueInValuePosition`
    /// error - the kernel has no use for a discardable value in value position.
    Unit,
    /// `!` -- the never type (the bottom type). the type of an expression that
    /// diverges and never yields control: `return`, `break`, `continue`, a
    /// `loop` with no reachable `break`, or an `if`/`match` whose every branch
    /// is `Never`. coerces to any type, so a diverging branch never forces a
    /// branch-consistency mismatch (`if c { 5 } else { return }` types as the
    /// `5`). synthesized by inference only - not spellable in source.
    Never,
    /// the error sentinel -- produced when a prior diagnostic already fired.
    Error,
}

/// an interner that deduplicates [`TypeKind`] values and assigns each a unique
/// [`TypeRef`] handle.
///
/// all well-known primitive types (`int32`, `bool`, ...) are pre-injected at
/// construction.
/// the canonical type store. lock-free by design (S6): `intern` takes `&self`,
/// so every body in a file can intern into one shared interner concurrently
/// without the per-body clone the old `&mut self` model forced.
///
/// - `arena` is a [`boxcar::Vec`]: a lock-free append-only vector with *stable*
///   element addresses, so [`lookup`](Self::lookup) hands out `&TypeKind`
///   directly (no guard, no clone) - the property a `Mutex`/`RwLock` interner
///   cannot give.
/// - `map` is a [`papaya::HashMap`]: a lock-free dedup index from structural
///   `TypeKind` to its canonical arena slot.
///
/// the one tolerated race: two threads interning the *same* new `TypeKind`
/// concurrently may each append a slot, but `get_or_insert` elects a single
/// canonical index and both return it - the losing slot is dead (referenced by
/// nothing), never a second handle for one type. structural equality stays a
/// handle compare.
#[derive(Clone)]
pub struct TypeInterner {
    arena: boxcar::Vec<TypeKind>,
    map: papaya::HashMap<TypeKind, u32, FxBuildHasher>,
    /// pre-cached read-only handles for the most-requested builtin types.
    error_ty: TypeRef,
    int32_ty: TypeRef,
    uint8_ty: TypeRef,
    usize_ty: TypeRef,
    unit_ty: TypeRef,
    never_ty: TypeRef,
}

impl TypeInterner {
    /// create a new interner with all primitive types pre-injected.
    pub fn new() -> Self {
        let this = TypeInterner {
            arena: boxcar::Vec::new(),
            map: papaya::HashMap::with_hasher(FxBuildHasher),
            error_ty: TypeRef(0),
            int32_ty: TypeRef(0),
            uint8_ty: TypeRef(0),
            usize_ty: TypeRef(0),
            unit_ty: TypeRef(0),
            never_ty: TypeRef(0),
        };
        // `&self` interning lets this run on the freshly built (not-yet-shared)
        // interner; the cached handles are captured before it is handed out.
        let error_ty = this.intern(TypeKind::Error);
        for name in &[
            "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32",
            "float64", "bool", "char", "string", "usize", "isize", "void",
        ] {
            this.intern(TypeKind::Path(Text::from(*name)));
        }
        // `ptr` is structural (`TypeKind::RawPtr`), not a named path; `()` and
        // `!` likewise have their own variants.
        this.intern(TypeKind::RawPtr);
        let unit_ty = this.intern(TypeKind::Unit);
        let never_ty = this.intern(TypeKind::Never);
        let int32_ty = this.intern(TypeKind::Path(Text::from("int32")));
        let uint8_ty = this.intern(TypeKind::Path(Text::from("uint8")));
        let usize_ty = this.intern(TypeKind::Path(Text::from("usize")));
        TypeInterner {
            error_ty,
            int32_ty,
            uint8_ty,
            usize_ty,
            unit_ty,
            never_ty,
            ..this
        }
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

    /// retrieve the pre-injected unit (`()`) handle (read-only).
    pub fn unit_ty(&self) -> TypeRef {
        self.unit_ty
    }

    /// retrieve the pre-injected never (`!`) handle (read-only).
    pub fn never_ty(&self) -> TypeRef {
        self.never_ty
    }

    /// intern a [`TypeKind`], returning its canonical [`TypeRef`] handle.
    /// `&self` (not `&mut self`): the lock-free arena and dedup map let many
    /// bodies intern into one shared interner at once (S6).
    ///
    /// recursive child handles in `kind` must already be interned (i.e.
    /// obtained from this interner) so that structural equality is correctly
    /// detected.
    pub fn intern(&self, kind: TypeKind) -> TypeRef {
        let map = self.map.pin();
        if let Some(&id) = map.get(&kind) {
            return TypeRef(id);
        }
        // append first to reserve a slot, then elect the canonical index. if a
        // racing thread interned the same kind, `get_or_insert` returns its
        // index and this thread's freshly appended slot is left dead.
        let idx = self.arena.push(kind.clone()) as u32;
        TypeRef(*map.get_or_insert(kind, idx))
    }

    /// look up the [`TypeKind`] for a [`TypeRef`] handle. the arena's stable
    /// addresses let this borrow directly out of `&self`.
    pub fn lookup(&self, id: TypeRef) -> &TypeKind {
        &self.arena[id.0 as usize]
    }
}

impl Default for TypeInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TypeInterner {
    /// the canonical arena in handle order. the dedup `map` is a derived index
    /// whose iteration order is non-deterministic (lock-free hashing), so it is
    /// deliberately not part of the debug surface - this keeps dumps stable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut m = f.debug_map();
        for (i, kind) in self.arena.iter() {
            m.entry(&TypeRef(i as u32), kind);
        }
        m.finish()
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
            TypeKind::Unit => write!(f, "()"),
            TypeKind::Never => write!(f, "!"),
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
            TypeKind::Path(_)
            | TypeKind::RawPtr
            | TypeKind::Unit
            | TypeKind::Never
            | TypeKind::Error => {}
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
