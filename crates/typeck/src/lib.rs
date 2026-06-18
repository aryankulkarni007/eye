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

/// why an expectation exists (the tier-2 spine, TYPECK.md). every
/// `Expectation::HasType` carries one so the funnel (`infer`'s `expect`)
/// knows which coercion site imposed the type: it selects the matching
/// `TypeError` variant when a value cannot satisfy the expectation, and that
/// site's assignability policy.
///
/// three variants name an *atomic* site whose funnel owns the mismatch
/// diagnostic (`Arg`/`Field`/`Return`); the rest are *adopt-only* markers - the
/// expectation flows down for literal-width adoption but the mismatch, when one
/// exists, is reported by a separate judgment (the let-init check for
/// `LetDecl`, the branch/arm consistency checks for `IfBranch`/`MatchArm`).
/// the imposing-site causes (`Arg`/`Field`/`Return`) carry the declaration's
/// span (`decl`) for the secondary "declared here" label on the two-span
/// diagnostic the funnel emits.
#[derive(Debug, Clone)]
pub enum Cause {
    /// `let T x = init`: adoption only. the Call-init mismatch stays in
    /// `check_explicit_let_init_type` pending the let-init width ruling.
    LetDecl,
    /// `f(arg)` against parameter `index` (1-based) -> `ArgTypeMismatch`. `decl`
    /// = the callee parameter's declaration span.
    Arg {
        index: usize,
        decl: Option<diagnostics::Span>,
    },
    /// the fn tail or an explicit `return e` -> `ReturnTypeMismatch`. `decl` =
    /// the return-type annotation span.
    Return { decl: Option<diagnostics::Span> },
    /// `S { field: v }` against the declared field type ->
    /// `StructFieldTypeMismatch`. `decl` = the field's declaration span.
    Field {
        name: hir::core::Text,
        decl: Option<diagnostics::Span>,
    },
    /// an `if` branch tail; adoption only. the mismatch is `check_if_branch_consistency`.
    IfBranch,
    /// a `match` arm body; adoption only. the mismatch is `check_match_arm_consistency`.
    MatchArm,
}

/// a downward-flowing expected type plus its provenance. the spine threads this
/// through [`infer::InferCtx::infer_expr`]; `None` synthesizes bottom-up.
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
    fn_ret_span: Option<diagnostics::Span>,
    types: &TypeInterner,
) -> TypeckResults {
    check_body_with(scope, body, fn_ret, fn_ret_span, types, &mut ())
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
        let results = check_body(
            hir,
            &hir.bodies[body_id],
            function.ret,
            function.ret_span.clone(),
            &hir.types,
        );
        out.insert(fn_id, results);
    }
    out
}

/// check one body with a fused observer (the dual-inference entry point).
pub fn check_body_with<O: InferObserver>(
    scope: &HIR,
    body: &Body,
    fn_ret: Option<TypeRef>,
    fn_ret_span: Option<diagnostics::Span>,
    types: &TypeInterner,
    obs: &mut O,
) -> TypeckResults {
    infer::InferCtx::new(scope, body, fn_ret, fn_ret_span, types, obs).run()
}
