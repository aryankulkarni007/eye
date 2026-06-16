//! sealed-body type inference (TYPECK.md).
//!
//! this pass re-derives every expression type over a frozen, already-lowered
//! [`Body`] and is the sole source of expression types since the S2C cutover:
//! MIR/codegen read [`TypeckResults`], and lowering no longer stamps types.
//!
//! some rules are still ruled wrong and carry `// PARITY(S3):` markers - they
//! reproduce the pre-cutover behavior (e.g. a binary expression takes its left
//! operand's type, the M2 narrowing). the shadow oracle that pinned this parity
//! retired with the cutover (S2C C5); the S3 judgment pass fixes these rules,
//! each with its own test.

use diagnostics::Sink;
use hir::core::{Body, Expr, ExprId, HIR, HirError, TypeInterner, TypeRef};
use la_arena::ArenaMap;
use rustc_hash::FxHashSet;
use syntax::SyntaxNodePtr;

mod infer;

/// the side table the pass produces. S1 mirrors lowering's partial stamping
/// (an unstamped expression is simply absent); the completeness contract
/// (every expression typed or `Error` + diagnostic) starts at S2.
#[derive(Debug, Default)]
pub struct TypeckResults {
    pub expr_types: ArenaMap<la_arena::Idx<Expr>, TypeRef>,
    /// context-directed read adjustments, keyed by the adjusted expression.
    /// currently only array-reference decay (`Adjustment::Decay`): a `&[T; N]`
    /// value read against a `&T` / `string` expectation. lowering once injected
    /// a cast node for this (S2C C4 moved it here); MIR reads the table and
    /// emits the cast, so the decaying expression keeps its own `&[T; N]` type
    /// and only its *read* is adjusted.
    pub adjustments: ArenaMap<la_arena::Idx<Expr>, Adjustment>,
    /// types for locals lowering left untyped because it no longer knows the
    /// scrutinee type: a match-arm binding (`x -> ..`, S2C C2) takes the
    /// scrutinee's type, recorded here during the walk. `path_type` and MIR's
    /// `bind_local_to` read this as the binding's type.
    pub local_types: rustc_hash::FxHashMap<hir::core::LocalId, TypeRef>,
    /// every expression the walk visited (reachable from the body's block
    /// structure). lowering's arena also holds typed *orphans* - children of
    /// rejected expressions - which are deliberately outside this set.
    pub visited: FxHashSet<ExprId>,
    /// type-judgment diagnostics, anchored through the body's source map.
    /// filling up cluster by cluster as S2 step b moves the `check_*` fns
    /// out of lowering; shape/place/mutability checks stay there (boundary
    /// rulings, TYPECK.md).
    pub diagnostics: Sink<HirError>,
}

/// context-directed adjustment MIR applies when reading an expression.
/// load-bearing since S2C C4: lowering no longer injects a decay cast node, so
/// MIR emits the pointer cast from this table instead. the generated c is
/// unchanged - the cast just moves from a synthesized HIR node to an
/// adjustment the backend applies on read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjustment {
    /// `&[T; N]` value meeting a `&T` / `T*` / `string` expectation.
    Decay(TypeRef),
}

/// why an expectation exists (tier 2, TYPECK.md). carried by every
/// expectation so a mismatch diagnostic is natively two-span: the mismatch
/// site plus the declaration that imposed the type. rendered from S2 (when
/// this pass owns user-facing diagnostics); the horizon 2 extension point is
/// an `Expansion` frame wrapping an inner cause.
#[derive(Debug, Clone)]
pub enum Cause {
    LetDecl(SyntaxNodePtr),
    Param {
        callee: hir::core::Text,
        idx: u32,
        decl: SyntaxNodePtr,
    },
    ReturnDecl(SyntaxNodePtr),
    FieldDecl {
        strukt: hir::core::Text,
        field: hir::core::Text,
    },
    ElemType,
    /// S1 placeholder: the imposing declaration's pointer is not yet
    /// threaded from item collection. replaced site-by-site in S2.
    Unknown,
}

/// a downward-flowing expected type plus its provenance.
#[derive(Debug, Clone)]
pub enum Expectation {
    None,
    HasType(TypeRef, Cause),
}

/// read-only context handed to an [`InferObserver`] at each visit: the item
/// scope, the body, the interner, and the types computed so far this body.
/// an observer reads these to classify without re-walking - effect inference
/// resolves a `Call`'s callee (`cx.body` -> `cx.scope`), a global's mutability
/// (`cx.scope`), and whether a `*p` operand is a raw pointer (`cx.expr_types`
/// + `cx.types`), the one type-dependent atom (EFFECT.md).
pub struct ObserverCx<'a> {
    pub scope: &'a HIR,
    pub body: &'a Body,
    pub types: &'a TypeInterner,
    pub expr_types: &'a ArenaMap<la_arena::Idx<Expr>, TypeRef>,
}

/// the fusion seam (EFFECT.md "crate boundary"): called once per expression
/// on the same traversal, with the type the walk just computed (`None` =
/// unstamped under S1's partial contract; tightens to `TypeRef` with the S2
/// completeness contract). `crates/effect` implements this; `()` is the
/// type-only no-op.
pub trait InferObserver {
    fn visit(&mut self, id: ExprId, expr: &Expr, ty: Option<TypeRef>, cx: &ObserverCx<'_>);
}

impl InferObserver for () {
    fn visit(&mut self, _id: ExprId, _expr: &Expr, _ty: Option<TypeRef>, _cx: &ObserverCx<'_>) {}
}

/// every expression type the walk produced across the whole file, as a flat
/// seed for codegen's type-declaration topology. since the S2C cutover lowering
/// no longer stamps expression types, so the array/fn-pointer wrapper typedefs
/// for intermediate values (an array literal passed as an argument, a string
/// literal's `&[uint8; N]`) are discovered here rather than from the HIR body.
pub fn expr_type_seed(
    typeck: &rustc_hash::FxHashMap<hir::core::FnId, TypeckResults>,
) -> Vec<TypeRef> {
    typeck
        .values()
        .flat_map(|r| r.expr_types.iter().map(|(_, &ty)| ty))
        .collect()
}

/// check one body, type lattice only.
pub fn check_body(
    scope: &HIR,
    body: &Body,
    fn_ret: Option<TypeRef>,
    types: &TypeInterner,
) -> TypeckResults {
    check_body_with(scope, body, fn_ret, types, &mut ())
}

/// check every defined function in a lowered file, interning any new types
/// into the file's shared interner (`hir.types`, `&self` interning - no take/
/// restore), so every handle in the results resolves through `hir.types` and
/// MIR/codegen can consume them directly. this is the whole-file pipeline
/// driver; the per-fn query path arrives with the `typeck_fn` salsa query.
pub fn check_file(hir: &HIR) -> rustc_hash::FxHashMap<hir::core::FnId, TypeckResults> {
    let mut out = rustc_hash::FxHashMap::default();
    for (fn_id, function) in hir.functions.iter() {
        let Some(body_id) = function.body else {
            continue;
        };
        let results = check_body(hir, &hir.bodies[body_id], function.ret, &hir.types);
        out.insert(fn_id, results);
    }
    out
}

/// check one body with a fused observer (the dual-inference entry point).
pub fn check_body_with<O: InferObserver>(
    scope: &HIR,
    body: &Body,
    fn_ret: Option<TypeRef>,
    types: &TypeInterner,
    obs: &mut O,
) -> TypeckResults {
    infer::InferCtx::new(scope, body, fn_ret, types, obs).run()
}
