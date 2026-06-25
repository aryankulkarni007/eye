//! machine-EFFECT inference (EFFECT.md) - the second lattice of the fused
//! dual-inference walk. effects ride the `typeck::InferObserver` seam: as the
//! bidirectional type walk visits each expression, the [`EffectJudge`]
//! classifies its machine effect, so types and effects are inferred on one
//! traversal (one walk per body, two lattices).
//!
//! status: S4 foundational slice. per-body atom collection + call-edge
//! collection are built and tested here. NOT YET built: the whole-program SCC
//! condensation fixpoint (a fn's effects = its own atoms ∪ its callees'),
//! annotations + the exact-match contract, the `EffectError` (e) diagnostic
//! class, and salsa/LSP wiring. see EFFECT.md "build pieces (segment S4)" and
//! docs/planning/ledger.md for the path forward.

use diagnostics::Sink;
use hir::core::{EffectError, FnId, HIR, HirError, Text};
use rustc_hash::FxHashMap;
use typeck::TypeckResults;

mod judge;
mod lattice;

pub use judge::{EffectJudge, EffectResult, infer_body_effects};
pub use lattice::{Atom, EffectSet};

use judge::WitnessKind;
use lattice::{LIVE_ATOMS, atom_index, describe, parse_effect_name};

/// the whole-program effect verdict: every function's *total* effect set, with
/// callees' effects propagated in. produced by [`infer_effects`] / [`infer_file`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectMap {
    effects: FxHashMap<FnId, EffectSet>,
}

impl EffectMap {
    /// the total effect of `fid` (its own atoms unioned with everything it
    /// transitively calls). a fn absent from the map (none was inferred) is
    /// `pure`.
    pub fn effect_of(&self, fid: FnId) -> EffectSet {
        self.effects.get(&fid).copied().unwrap_or_default()
    }

    /// every `(fn, total effect)` pair, in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (FnId, EffectSet)> + '_ {
        self.effects.iter().map(|(&k, &v)| (k, v))
    }
}

/// walk every defined body **once**, fused, producing both the per-fn type side
/// tables and the per-fn local effect results - the dual-inference whole-file
/// driver (EFFECT.md "fused per-body walk"). every body interns into the shared
/// `hir.types` (`&self` interning - no take/restore), so every type handle
/// resolves through it. a bodyless fn (an extern signature) has no body to walk
/// and synthesizes its verdict: calling it is `ffi`.
fn collect_results(hir: &HIR) -> (FxHashMap<FnId, TypeckResults>, Vec<(FnId, EffectResult)>) {
    use rayon::prelude::*;

    // wave 1 (S6): one task per body, the fused type+effect walk, fanned out
    // across rayon. each task owns its `EffectJudge` and interns into the one
    // shared lock-free interner (`&self`), so there is no shared mutable state
    // and no interner clone. `&HIR` is `Send + Sync`.
    //
    // determinism law #1: results are collected in arena order (an indexed
    // parallel `collect` preserves input order), never completion order, so the
    // serial and parallel runs produce byte-identical diagnostics and effect
    // maps. (law #2 - no observable dependence on `TypeRef` numeric values - is
    // a codegen-ordering contract checked by the corpus-diff-twice gate.)
    let per_fn: Vec<(FnId, Option<TypeckResults>, EffectResult)> = hir
        .functions
        .iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(fid, function)| match function.body {
            Some(body_id) => {
                let mut judge = EffectJudge::default();
                let results = typeck::check_body_with(
                    hir,
                    &hir.bodies[body_id],
                    function.ret,
                    function.ret_span.clone(),
                    &hir.types,
                    &mut judge,
                );
                (fid, Some(results), judge.into_result())
            }
            None => {
                let mut s = EffectSet::pure();
                let mut local_witness: [Option<WitnessKind>; LIVE_ATOMS] = Default::default();
                if function.is_extern {
                    s.insert(Atom::Ffi);
                    // the extern signature itself is the ffi primitive (a trail
                    // recursing into it lands here).
                    local_witness[1] = Some(WitnessKind::Extern(function.name.clone()));
                }
                let result = EffectResult {
                    set: s,
                    callees: Vec::new(),
                    indirect: false,
                    local_witness,
                };
                (fid, None, result)
            }
        })
        .collect();

    let mut typeck = FxHashMap::default();
    let mut effects: Vec<(FnId, EffectResult)> = Vec::with_capacity(per_fn.len());
    for (fid, results, effect) in per_fn {
        if let Some(results) = results {
            typeck.insert(fid, results);
        }
        effects.push((fid, effect));
    }
    (typeck, effects)
}

/// the whole-program effect map alone (the type results are discarded). runs the
/// full per-body walk; prefer [`infer_file`] when the type results are also
/// needed (the pipeline path), so the walk runs once.
pub fn infer_effects(hir: &HIR) -> EffectMap {
    let (_typeck, effects) = collect_results(hir);
    run_fixpoint(&effects)
}

/// the fused dual-inference whole-file driver for the pipeline: one walk per
/// body yields both the type side tables and the whole-program effect map, then
/// the annotation contract is checked against the inferred map. the database's
/// `lowered_file` calls this so types and effects share a single traversal and
/// are memoized together (EFFECT.md "salsa wiring"). the returned `Sink` carries
/// the effect-contract diagnostics (unknown effect names, declared/inferred
/// mismatches).
pub fn infer_file(hir: &HIR) -> (FxHashMap<FnId, TypeckResults>, EffectMap, Sink<HirError>) {
    let (typeck, results) = collect_results(hir);
    let map = run_fixpoint(&results);
    let diagnostics = check_contracts(hir, &results, &map);
    (typeck, map, diagnostics)
}

/// check every annotated fn's declared effect set against the inferred set (the
/// exact-match contract, EFFECT.md). unannotated fns are skipped - inference is
/// total, annotations are optional. emits `EffectError::UnknownEffect` for a
/// name outside the atom set and `EffectError::EffectMismatch` when the declared
/// set is not equal to the inferred set.
fn check_contracts(
    hir: &HIR,
    results: &[(FnId, EffectResult)],
    effects: &EffectMap,
) -> Sink<HirError> {
    let index: FxHashMap<FnId, usize> = results
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (*f, i))
        .collect();
    let mut sink = Sink::default();
    for (fid, function) in hir.functions.iter() {
        if function.declared_effects.is_empty() {
            continue; // unannotated: no contract
        }
        let mut declared = EffectSet::pure();
        let mut had_unknown = false;
        for (name, span) in &function.declared_effects {
            match parse_effect_name(name) {
                Ok(Some(atom)) => declared.insert(atom),
                Ok(None) => {} // `pure` contributes nothing
                Err(()) => {
                    sink.emit(
                        span.clone(),
                        HirError::Effect(EffectError::UnknownEffect { name: name.clone() }),
                    );
                    had_unknown = true;
                }
            }
        }
        // a bad annotation already errored; a derived mismatch would be noise.
        if had_unknown {
            continue;
        }
        let inferred = effects.effect_of(fid);
        if declared != inferred {
            // anchor on the first annotation (declared_effects is non-empty here).
            let anchor = function.declared_effects[0].1.clone();
            // explain each *unexpected* atom (inferred but not declared) by
            // walking the call graph to the primitive that produced it.
            let witness =
                witness_for_surprises(fid, declared, inferred, hir, results, &index, effects);
            sink.emit(
                anchor,
                HirError::Effect(EffectError::EffectMismatch {
                    function: function.name.clone(),
                    declared: describe(declared),
                    inferred: describe(inferred),
                    witness,
                }),
            );
        }
    }
    sink
}

/// build the witness note for a mismatch: one trail per atom that inference
/// found but the annotation omitted. `None` when no atom is surprising (the
/// annotation over-declared - the set message alone is the explanation).
fn witness_for_surprises(
    fid: FnId,
    declared: EffectSet,
    inferred: EffectSet,
    hir: &HIR,
    results: &[(FnId, EffectResult)],
    index: &FxHashMap<FnId, usize>,
    effects: &EffectMap,
) -> Option<String> {
    let mut trails = Vec::new();
    for atom in [Atom::Io, Atom::Ffi, Atom::State] {
        if inferred.contains(atom) && !declared.contains(atom) {
            let (chain, leaf) = witness_trail(fid, atom, hir, results, index, effects)?;
            let mut one = EffectSet::pure();
            one.insert(atom);
            let atom_name = describe(one);
            let trail = if chain.is_empty() {
                format!("the `{atom_name}` effect comes from {leaf}")
            } else {
                let via = chain
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                format!("the `{atom_name}` effect comes from {leaf} (via {via})")
            };
            trails.push(trail);
        }
    }
    if trails.is_empty() {
        None
    } else {
        Some(trails.join("; "))
    }
}

/// read-only context for a witness-trail walk.
struct TrailCx<'a> {
    hir: &'a HIR,
    results: &'a [(FnId, EffectResult)],
    index: &'a FxHashMap<FnId, usize>,
    effects: &'a EffectMap,
}

/// walk the call graph from `fid` to the body that produces `atom` directly,
/// returning the chain of callee names traversed (empty when `fid` itself
/// produces it) and the leaf primitive's label. a single witness per atom
/// (EFFECT.md): the first path found in call order.
fn witness_trail(
    fid: FnId,
    atom: Atom,
    hir: &HIR,
    results: &[(FnId, EffectResult)],
    index: &FxHashMap<FnId, usize>,
    effects: &EffectMap,
) -> Option<(Vec<Text>, String)> {
    let ai = atom_index(atom)?;
    let cx = TrailCx {
        hir,
        results,
        index,
        effects,
    };
    let mut visited = rustc_hash::FxHashSet::default();
    dfs_trail(fid, ai, atom, &cx, &mut visited)
}

/// one step of [`witness_trail`]'s DFS (recursion factored out so the context is
/// borrowed once rather than threaded as separate args).
fn dfs_trail(
    cur: FnId,
    ai: usize,
    atom: Atom,
    cx: &TrailCx<'_>,
    visited: &mut rustc_hash::FxHashSet<FnId>,
) -> Option<(Vec<Text>, String)> {
    if !visited.insert(cur) {
        return None;
    }
    let r = &cx.results[*cx.index.get(&cur)?].1;
    if let Some(w) = &r.local_witness[ai] {
        return Some((Vec::new(), w.label()));
    }
    // recurse into a callee whose total effect carries the atom.
    for &callee in &r.callees {
        if cx.effects.effect_of(callee).contains(atom)
            && cx.index.contains_key(&callee)
            && let Some((mut chain, leaf)) = dfs_trail(callee, ai, atom, cx, visited)
        {
            chain.insert(0, cx.hir.functions[callee].name.clone());
            return Some((chain, leaf));
        }
    }
    None
}

/// propagate per-body effects up the call graph (EFFECT.md "fixpoint").
/// recursion needs no iteration - tarjan's SCC condensation *is* the fixpoint:
/// an SCC's effect is the union over its members (a recursive cycle's members
/// share one verdict), and the condensation is a DAG processed callee-first.
/// o(v + e) over byte-sized sets.
fn run_fixpoint(results: &[(FnId, EffectResult)]) -> EffectMap {
    let n = results.len();
    let index: FxHashMap<FnId, usize> = results
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (*f, i))
        .collect();
    // adjacency caller -> callee (deduped, every callee is a known fn id).
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, (_, r)) in results.iter().enumerate() {
        let mut seen = rustc_hash::FxHashSet::default();
        for c in &r.callees {
            if let Some(&j) = index.get(c)
                && seen.insert(j)
            {
                deps[i].push(j);
            }
        }
    }

    let (scc_id, scc_count) = tarjan_scc(&deps);

    // tarjan numbers sccs in reverse topological order: for a caller->callee
    // edge across sccs, scc_id[caller] > scc_id[callee]. so seeding each SCC
    // with its members' own atoms and then processing sccs in increasing id
    // unions every callee SCC's *final* effect before its callers read it.
    let mut scc_effect = vec![EffectSet::pure(); scc_count];
    for (i, (_, r)) in results.iter().enumerate() {
        let s = scc_id[i];
        scc_effect[s] = scc_effect[s].union(r.set);
        if r.indirect {
            scc_effect[s] = scc_effect[s].union(EffectSet::live());
        }
    }
    // cross-SCC callee effects, callee-first (guaranteed scc_id[callee] < s).
    for s in 0..scc_count {
        for i in (0..n).filter(|&i| scc_id[i] == s) {
            for &j in &deps[i] {
                let t = scc_id[j];
                if t != s {
                    let e = scc_effect[t];
                    scc_effect[s] = scc_effect[s].union(e);
                }
            }
        }
    }

    let effects = results
        .iter()
        .enumerate()
        .map(|(i, (f, _))| (*f, scc_effect[scc_id[i]]))
        .collect();
    EffectMap { effects }
}

/// tarjan's SCC over the call-graph adjacency list (o(v + e)); returns each
/// node's SCC id and the SCC count. mirrors `hir::core::typegraph`'s component
/// machinery. SCC ids are assigned in reverse topological order of the
/// condensation (a sink SCC gets a lower id than the callers reaching it).
fn tarjan_scc(deps: &[Vec<usize>]) -> (Vec<usize>, usize) {
    const UNVISITED: u32 = u32::MAX;
    let n = deps.len();
    let mut tarjan_idx = 0u32;
    let mut indices = vec![UNVISITED; n];
    let mut lowlink = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut scc_id = vec![0usize; n];
    let mut scc_count = 0;

    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        v: usize,
        deps: &[Vec<usize>],
        tarjan_idx: &mut u32,
        indices: &mut [u32],
        lowlink: &mut [u32],
        on_stack: &mut [bool],
        stack: &mut Vec<usize>,
        scc_id: &mut [usize],
        scc_count: &mut usize,
    ) {
        indices[v] = *tarjan_idx;
        lowlink[v] = *tarjan_idx;
        *tarjan_idx += 1;
        stack.push(v);
        on_stack[v] = true;

        for &w in &deps[v] {
            if indices[w] == UNVISITED {
                strongconnect(
                    w, deps, tarjan_idx, indices, lowlink, on_stack, stack, scc_id, scc_count,
                );
                lowlink[v] = lowlink[v].min(lowlink[w]);
            } else if on_stack[w] {
                lowlink[v] = lowlink[v].min(indices[w]);
            }
        }

        if lowlink[v] == indices[v] {
            loop {
                let w = stack.pop().unwrap();
                on_stack[w] = false;
                scc_id[w] = *scc_count;
                if w == v {
                    break;
                }
            }
            *scc_count += 1;
        }
    }

    for v in 0..n {
        if indices[v] == UNVISITED {
            strongconnect(
                v,
                deps,
                &mut tarjan_idx,
                &mut indices,
                &mut lowlink,
                &mut on_stack,
                &mut stack,
                &mut scc_id,
                &mut scc_count,
            );
        }
    }

    (scc_id, scc_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{AstNode, SourceFile};
    use lexer::{Lexer, SourceText};

    /// lower `src` and infer the local effects of fn `name`.
    fn effects_of(src: &str, name: &str) -> EffectResult {
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = SourceFile::cast(parse.green).expect("root is SourceFile");
        let hir = hir::core::lower_source_file(file, &lexed.interner);
        let fn_id = *hir.items.functions.get(name).expect("fn exists");
        let body_id = hir.functions[fn_id].body.expect("fn has a body");
        let ret = hir.functions[fn_id].ret;
        infer_body_effects(&hir, &hir.bodies[body_id], ret, &hir.types)
    }

    #[test]
    fn println_is_io() {
        let r = effects_of("main() {\n    println(\"{}\", 1);\n}\n", "main");
        assert!(r.set.contains(Atom::Io), "println must produce io: {r:?}");
        assert!(!r.set.contains(Atom::Ffi));
        assert!(!r.set.contains(Atom::State));
    }

    #[test]
    fn extern_call_is_ffi_and_an_edge() {
        let r = effects_of(
            "extern {\n    malloc(usize n) -> ptr;\n}\nmain() {\n    let ptr p = malloc(8);\n}\n",
            "main",
        );
        assert!(
            r.set.contains(Atom::Ffi),
            "extern call must produce ffi: {r:?}"
        );
        assert_eq!(r.callees.len(), 1, "the extern call is a call edge: {r:?}");
    }

    #[test]
    fn plain_fn_is_pure() {
        let r = effects_of("add(int32 a, int32 b) -> int32 {\n    a + b\n}\n", "add");
        assert!(r.set.is_pure(), "a pure computation has no atoms: {r:?}");
        assert!(r.callees.is_empty());
    }

    #[test]
    fn mut_global_access_is_state() {
        let r = effects_of(
            "mut int32 counter = 0;\nbump() {\n    counter = counter + 1;\n}\n",
            "bump",
        );
        assert!(
            r.set.contains(Atom::State),
            "writing a mut global must produce state: {r:?}"
        );
    }

    #[test]
    fn fn_call_edge_is_collected_without_atoms() {
        let r = effects_of(
            "helper() -> int32 {\n    1\n}\nmain() {\n    let int32 x = helper();\n}\n",
            "main",
        );
        assert_eq!(r.callees.len(), 1, "the call to helper is an edge: {r:?}");
        assert!(
            r.set.is_pure(),
            "main's own atoms are empty (helper is pure; the fixpoint that \
             would union helper's effects is not built yet): {r:?}"
        );
    }

    /// lower `src` and run the whole-program effect fixpoint.
    fn whole_program(src: &str) -> (hir::core::HIR, EffectMap) {
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = SourceFile::cast(parse.green).expect("root is SourceFile");
        let hir = hir::core::lower_source_file(file, &lexed.interner);
        let map = infer_effects(&hir);
        (hir, map)
    }

    /// the total (fixpoint) effect of fn `name`.
    fn total(hir: &hir::core::HIR, map: &EffectMap, name: &str) -> EffectSet {
        let fid = *hir.items.functions.get(name).expect("fn exists");
        map.effect_of(fid)
    }

    #[test]
    fn io_propagates_transitively() {
        // main -> reporter -> println. the fixpoint must lift io two edges up.
        let (hir, map) = whole_program(
            "reporter(int32 n) {\n    println(\"{}\", n);\n}\n\
             main() {\n    reporter(7);\n}\n",
        );
        assert!(
            total(&hir, &map, "reporter").contains(Atom::Io),
            "reporter calls println directly"
        );
        assert!(
            total(&hir, &map, "main").contains(Atom::Io),
            "main inherits io through reporter (transitive propagation)"
        );
    }

    #[test]
    fn recursion_unions_the_cycle() {
        // ping <-> pong are mutually recursive; only pong does io. both share
        // one SCC verdict, so the condensation gives ping io with no iteration.
        let (hir, map) = whole_program(
            "ping(int32 n) {\n    if n > 0 {\n        pong(n - 1);\n    }\n}\n\
             pong(int32 n) {\n    println(\"{}\", n);\n    if n > 0 {\n        ping(n - 1);\n    }\n}\n",
        );
        assert!(total(&hir, &map, "pong").contains(Atom::Io));
        assert!(
            total(&hir, &map, "ping").contains(Atom::Io),
            "the recursive cycle shares one effect verdict"
        );
    }

    #[test]
    fn extern_callee_is_ffi_and_propagates() {
        // the extern fn's own entry is ffi; a caller inherits it.
        let (hir, map) = whole_program(
            "extern {\n    malloc(usize n) -> ptr;\n}\n\
             alloc8() -> ptr {\n    return malloc(8);\n}\n",
        );
        assert!(
            total(&hir, &map, "malloc").contains(Atom::Ffi),
            "an extern signature is ffi"
        );
        assert!(
            total(&hir, &map, "alloc8").contains(Atom::Ffi),
            "calling an extern propagates ffi to the caller"
        );
    }

    #[test]
    fn fn_pointer_call_is_conservatively_full_live() {
        // `operation`/`callback` are fn-pointer *parameters*: their targets are
        // unknown, so the fixpoint assumes the full live set (io | ffi | state).
        let (hir, map) = whole_program(
            "execute(int32 x, (int32) -> int32 operation, (int32) callback) {\n    let int32 r = operation(x);\n    callback(r);\n}\n",
        );
        let e = total(&hir, &map, "execute");
        assert!(
            e.contains(Atom::Io),
            "indirect call -> conservative io: {e:?}"
        );
        assert!(
            e.contains(Atom::Ffi),
            "indirect call -> conservative ffi: {e:?}"
        );
        assert!(
            e.contains(Atom::State),
            "indirect call -> conservative state: {e:?}"
        );
    }

    /// lower `src`, run the full file inference, and return the effect-contract
    /// diagnostics (unwrapped from `HirError::Effect`).
    fn contracts(src: &str) -> Vec<EffectError> {
        let source = SourceText::new(src.to_string());
        let lexed = Lexer::new(&source).tokenize();
        let parse = parser::parse(&lexed.tokens, &source);
        let file = SourceFile::cast(parse.green).expect("root is SourceFile");
        let hir = hir::core::lower_source_file(file, &lexed.interner);
        let (_typeck, _map, sink) = infer_file(&hir);
        sink.into_iter()
            .map(|(_span, e)| match e {
                HirError::Effect(ee) => ee,
                other => panic!("effect sink carried a non-effect error: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn matching_annotation_is_accepted() {
        // declared == inferred, both directions of the live set.
        assert!(
            contracts("pure add(int32 a, int32 b) -> int32 {\n    a + b\n}\n").is_empty(),
            "pure annotation on a pure fn is clean"
        );
        assert!(
            contracts("io report(int32 n) {\n    println(\"{}\", n);\n}\n").is_empty(),
            "io annotation on an io fn is clean"
        );
    }

    #[test]
    fn pure_annotation_on_effectful_fn_is_rejected() {
        let ds = contracts("pure report(int32 n) {\n    println(\"{}\", n);\n}\n");
        assert!(
            matches!(
                &ds[..],
                [EffectError::EffectMismatch { declared, inferred, witness: Some(w), .. }]
                    if declared == "pure" && inferred == "io" && w.contains("`println`")
            ),
            "declared pure, inferred io, witness names println: {ds:?}"
        );
    }

    #[test]
    fn effect_annotation_on_pure_fn_is_rejected() {
        // the reverse direction: declaring io on an inference-pure fn.
        let ds = contracts("io add(int32 a, int32 b) -> int32 {\n    a + b\n}\n");
        assert!(
            matches!(
                ds.as_slice(),
                [EffectError::EffectMismatch { declared, inferred, .. }]
                    if declared == "io" && inferred == "pure"
            ),
            "declared io, inferred pure: {ds:?}"
        );
    }

    #[test]
    fn unknown_effect_name_is_rejected() {
        let ds = contracts("bogus add(int32 a, int32 b) -> int32 {\n    a + b\n}\n");
        assert!(
            matches!(ds.as_slice(), [EffectError::UnknownEffect { name }] if name == "bogus"),
            "unknown effect name: {ds:?}"
        );
    }

    #[test]
    fn unannotated_fn_has_no_contract() {
        // an effectful fn with no annotation is silent - inference is total,
        // annotations optional.
        assert!(
            contracts("report(int32 n) {\n    println(\"{}\", n);\n}\n").is_empty(),
            "no annotation, no contract"
        );
    }

    #[test]
    fn transitive_effect_is_checked_against_the_annotation() {
        // main declares pure but reaches io two calls deep -> mismatch via the
        // fixpoint, proving the contract reads the propagated set.
        let ds = contracts(
            "reporter(int32 n) {\n    println(\"{}\", n);\n}\n\
             pure main() {\n    reporter(7);\n}\n",
        );
        assert!(
            matches!(
                &ds[..],
                [EffectError::EffectMismatch { function, inferred, witness: Some(w), .. }]
                    if function == "main" && inferred == "io"
                        && w.contains("`println`") && w.contains("via `reporter`")
            ),
            "main declared pure, witness trails through reporter to println: {ds:?}"
        );
    }

    #[test]
    fn pure_fn_stays_pure_through_the_fixpoint() {
        // a pure leaf and its pure caller keep the empty set after propagation.
        let (hir, map) = whole_program(
            "square(int32 n) -> int32 {\n    n * n\n}\n\
             main() {\n    let int32 x = square(4);\n}\n",
        );
        assert!(total(&hir, &map, "square").is_pure());
        assert!(
            total(&hir, &map, "main").is_pure(),
            "propagating a pure callee adds nothing"
        );
    }
}
