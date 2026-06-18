//! the per-body effect collector: [`EffectJudge`] rides the
//! `typeck::InferObserver` seam, classifying each visited expression's machine
//! effect into an [`EffectResult`] (atoms + call edges + witness leaves).
//! [`WitnessKind`] names the primitive that produced an atom, for the
//! contract-violation trail.

use hir::core::{Body, Expr, ExprId, FnId, HIR, Resolution, Text, TypeInterner, TypeKind, TypeRef};
use typeck::{InferObserver, ObserverCx};

use crate::lattice::{Atom, EffectSet, LIVE_ATOMS, atom_index};

/// why a fn has a given atom *locally* - the primitive that produced it, for the
/// witness trail in a contract-violation diagnostic (EFFECT.md witness edges).
#[derive(Debug, Clone)]
pub(crate) enum WitnessKind {
    /// a `println` / `print` call (`io`).
    Println,
    /// a call to an `extern` fn, by name (`ffi`).
    Extern(Text),
    /// a raw-pointer dereference (`ffi`).
    RawDeref,
    /// read/write of a `mut` global, by name (`state`).
    MutGlobal(Text),
    /// a call through a fn-pointer value, whose target (and effect) is unknown.
    Indirect,
}

impl WitnessKind {
    /// the phrase naming this primitive, e.g. "a call to `println`".
    pub(crate) fn label(&self) -> String {
        match self {
            WitnessKind::Println => "a call to `println`".to_string(),
            WitnessKind::Extern(name) => format!("a call to extern `{name}`"),
            WitnessKind::RawDeref => "a raw-pointer dereference".to_string(),
            WitnessKind::MutGlobal(name) => format!("access to mutable global `{name}`"),
            WitnessKind::Indirect => {
                "a call through a fn-pointer (its effect is assumed live)".to_string()
            }
        }
    }
}

/// one body's inferred local effects plus its direct call edges. the atoms are
/// what the body produces *itself*; the whole-program fixpoint (EFFECT.md, not
/// yet built) unions in the callees' effects over the call graph's SCC
/// condensation.
#[derive(Debug, Clone, Default)]
pub struct EffectResult {
    pub set: EffectSet,
    pub callees: Vec<FnId>,
    /// true when the body calls through a fn-pointer *value* (a callee that is
    /// not a statically-known fn). the whole-program fixpoint cannot name the
    /// target, so it unions in the full live set ([`EffectSet::live`]) - sound,
    /// tightenable when EFFECT rows land on fn types (EFFECT.md).
    pub indirect: bool,
    /// per live atom (`io`/`ffi`/`state`, indexed by [`atom_index`]), the
    /// primitive in *this* body that first produced it - the leaf of a witness
    /// trail. `None` = the atom is not produced locally (it arrives through a
    /// callee, found by walking the call graph at diagnostic time).
    pub(crate) local_witness: [Option<WitnessKind>; LIVE_ATOMS],
}

/// the per-body effect collector. implements the typeck observer seam so it is
/// driven by the type walk: each visited expression is classified by
/// resolution (the type-dependent `*p`-is-`ffi` case reads the operand type
/// the walk just computed, EFFECT.md).
#[derive(Debug, Default)]
pub struct EffectJudge {
    set: EffectSet,
    callees: Vec<FnId>,
    indirect: bool,
    local_witness: [Option<WitnessKind>; LIVE_ATOMS],
}

impl EffectJudge {
    /// add `atom` to the set and record `w` as its local witness if none yet
    /// (the first, most-specific producer wins).
    fn record(&mut self, atom: Atom, w: WitnessKind) {
        self.set.insert(atom);
        if let Some(i) = atom_index(atom)
            && self.local_witness[i].is_none()
        {
            self.local_witness[i] = Some(w);
        }
    }
}

impl InferObserver for EffectJudge {
    fn visit(&mut self, _id: ExprId, expr: &Expr, _ty: Option<TypeRef>, cx: &ObserverCx<'_>) {
        match expr {
            // a call: `io` for the `println` intrinsic, `ffi` for an `extern`
            // fn, plus the call edge for the fixpoint. a direct fn callee is a
            // `Path` child resolved at lowering.
            Expr::Call { callee, .. } => match &cx.body.exprs[*callee] {
                Expr::Path(Resolution::Unresolved(name))
                    if name == "println" || name == "print" =>
                {
                    self.record(Atom::Io, WitnessKind::Println);
                }
                Expr::Path(Resolution::Fn(fid)) => {
                    self.callees.push(*fid);
                    if cx.scope.functions[*fid].is_extern {
                        self.record(
                            Atom::Ffi,
                            WitnessKind::Extern(cx.scope.functions[*fid].name.clone()),
                        );
                    }
                }
                // a callee that is not a statically-known fn nor the println
                // intrinsic is a call through a fn-pointer value (the callee
                // resolves to a local/param/field of fn type). its target is
                // unknown, so the fixpoint must assume the full live set.
                // (unresolved non-intrinsic names are rejected upstream, so in
                // an accepted program this is exactly the indirect-call case.)
                _ => {
                    self.indirect = true;
                    // an indirect call can produce any live atom; record it as
                    // each atom's witness only where nothing more specific is.
                    for atom in [Atom::Io, Atom::Ffi, Atom::State] {
                        if let Some(i) = atom_index(atom)
                            && self.local_witness[i].is_none()
                        {
                            self.local_witness[i] = Some(WitnessKind::Indirect);
                        }
                    }
                }
            },
            // dereferencing a raw pointer (`T*` / `ptr`) is `ffi` - its
            // provenance is outside eye's model. a `&T` deref is a checked eye
            // reference and is NOT an effect. the one type-dependent atom:
            // classify by the operand's already-computed type.
            Expr::Deref { operand } => {
                if let Some(t) = cx.expr_types.get((*operand).into())
                    && matches!(cx.types.lookup(*t), TypeKind::Ptr(_) | TypeKind::RawPtr)
                {
                    self.record(Atom::Ffi, WitnessKind::RawDeref);
                }
            }
            // reading or writing a `mut` global is `state` (the assignment LHS
            // is itself a visited `Path(Global)`, so this catches both).
            Expr::Path(Resolution::Global(gid)) if cx.scope.globals[*gid].mutable => {
                self.record(
                    Atom::State,
                    WitnessKind::MutGlobal(cx.scope.globals[*gid].name.clone()),
                );
            }
            _ => {}
        }
    }
}

impl EffectJudge {
    /// consume the judge into its per-body result (atoms + call edges + the
    /// indirect-call flag).
    pub(crate) fn into_result(self) -> EffectResult {
        EffectResult {
            set: self.set,
            callees: self.callees,
            indirect: self.indirect,
            local_witness: self.local_witness,
        }
    }
}

/// infer one body's local effects (the atoms it produces directly plus its
/// callees) by running the type walk with the effect judge fused in. the
/// whole-program verdict needs the fixpoint over all bodies' results
/// ([`infer_effects`] / [`infer_file`]).
pub fn infer_body_effects(
    scope: &HIR,
    body: &Body,
    fn_ret: Option<TypeRef>,
    types: &TypeInterner,
) -> EffectResult {
    let mut judge = EffectJudge::default();
    // effect-only path: the type results (and so the return-mismatch secondary
    // span) are discarded, so the decl span is irrelevant here.
    typeck::check_body_with(scope, body, fn_ret, None, types, &mut judge);
    judge.into_result()
}
