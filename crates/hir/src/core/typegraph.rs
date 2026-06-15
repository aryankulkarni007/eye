//! type-declaration dependency graph: the shared edge model behind both the
//! value-recursion check (this crate) and the c type-declaration ordering
//! (codegen). both must classify edges identically - a divergence would either
//! reject a valid program or emit unorderable c (a raw clang error) - so this
//! module is the single source of that classification.
//!
//! a node is a nominal type (struct/union, by name), a fixed-array value wrapper
//! (by element type + length), or a function-pointer typedef (by params + ret).
//! node x has a **hard edge** to node y when y's c definition must be emitted
//! before x's: x embeds y by value. a pointer or reference to a nominal/array y
//! is a **soft edge** - y is forward-declared first (every struct, union, and
//! array wrapper gets a named-tag forward declaration) - and yields no node,
//! which is what lets a struct hold a pointer to itself (`Node* next`) or a
//! pointer to an array of itself (`&[Node; 4]`). a function-pointer typedef has
//! no forward-declared form, so naming it is always a hard edge; but its own
//! param/return types are soft (a function type may name incomplete types), so a
//! function-pointer typedef has no hard dependencies and never forms a cycle.
//!
//! cycle detection uses tarjan's SCC algorithm (o(v + e)) instead of the
//! earlier per-node DFS walk (o(v²)). the closed-form SCC membership also
//! makes [`SccInfo::same_cycle`] a constant-time lookup.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::core::{Expr, HIR, Stmt, Text, TypeInterner, TypeKind, TypeRef, VisitTypeRef, fx_set};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeNode {
    /// a struct or union, by name.
    Nominal(Text),
    /// the value wrapper for `[elem; len]`.
    Array { elem: TypeRef, len: u64 },
    /// a function-pointer typedef `(params) -> ret`.
    Fn {
        params: Vec<TypeRef>,
        ret: Option<TypeRef>,
        /// carried so a variadic and a non-variadic signature with the same
        /// params/ret dedup as *distinct* typedefs (their c differs by `...`).
        variadic: bool,
    },
}

/// true when `name` is a user struct or union. enums are excluded: they have no
/// dependencies and are emitted before every struct/union definition, so a field
/// of enum type never needs an ordering edge.
fn is_nominal(hir: &HIR, name: &Text) -> bool {
    hir.items.structs.contains_key(name) || hir.items.unions.contains_key(name)
}

struct HardDepsVisitor<'a> {
    hir: &'a HIR,
    out: &'a mut Vec<TypeNode>,
    under_pointer: bool,
    pointer_stack: Vec<bool>,
}

impl VisitTypeRef for HardDepsVisitor<'_> {
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
        match types.lookup(ty) {
            &TypeKind::Ref(_) | &TypeKind::Ptr(_) => {
                self.pointer_stack.push(self.under_pointer);
                self.under_pointer = true;
            }
            TypeKind::Path(name) if !self.under_pointer && is_nominal(self.hir, name) => {
                self.out.push(TypeNode::Nominal(name.clone()));
            }
            &TypeKind::Array { elem, len } if !self.under_pointer => {
                self.out.push(TypeNode::Array { elem, len });
            }
            &TypeKind::Fn {
                ref params,
                ret,
                variadic,
            } => {
                self.out.push(TypeNode::Fn {
                    params: params.clone(),
                    ret,
                    variadic,
                });
            }
            _ => {}
        }
        true
    }

    fn visit_ty_post(&mut self, ty: TypeRef, types: &TypeInterner) {
        if matches!(types.lookup(ty), TypeKind::Ref(_) | TypeKind::Ptr(_)) {
            self.under_pointer = self.pointer_stack.pop().unwrap_or(false);
        }
    }
}

/// append the nodes whose c definition must precede a definition that embeds
/// `ty` by value. see the module docs for the edge rules.
pub fn hard_deps(hir: &HIR, ty: TypeRef, out: &mut Vec<TypeNode>) {
    let types = &hir.types;
    let mut visitor = HardDepsVisitor {
        hir,
        out,
        under_pointer: false,
        pointer_stack: Vec::new(),
    };
    types.walk(ty, &mut visitor);
}

/// like [`hard_deps`] but treats every node as if it is behind a pointer
/// (soft edges). used for function-pointer param/return types and recursive
/// pointer fields.
fn soft_deps(hir: &HIR, ty: TypeRef, out: &mut Vec<TypeNode>) {
    let types = &hir.types;
    let mut visitor = HardDepsVisitor {
        hir,
        out,
        under_pointer: true,
        pointer_stack: Vec::new(),
    };
    types.walk(ty, &mut visitor);
}

/// the dependency nodes of a node itself: a nominal's fields (each embedded by
/// value), or an array wrapper's element (`elem data[N]`, embedded by value).
fn node_deps(hir: &HIR, node: &TypeNode, out: &mut Vec<TypeNode>) {
    match node {
        TypeNode::Nominal(name) => {
            for ty in nominal_field_types(hir, name) {
                hard_deps(hir, ty, out);
            }
        }
        TypeNode::Array { elem, .. } => hard_deps(hir, *elem, out),
        TypeNode::Fn { params, ret, .. } => {
            // a function-pointer typedef may reference incomplete param/return
            // types (a forward declaration suffices), so each is a soft edge:
            // compute as if behind a pointer. a nested function type is still a
            // hard edge (its typedef has no forward form).
            for &p in params {
                soft_deps(hir, p, out);
            }
            if let Some(r) = ret {
                soft_deps(hir, *r, out);
            }
        }
    }
}

/// the field types of a nominal type (struct or union) by name.
fn nominal_field_types(hir: &HIR, name: &Text) -> Vec<TypeRef> {
    if let Some(&id) = hir.items.structs.get(name) {
        return hir.structs[id]
            .fields
            .iter()
            .map(|&f| hir.fields[f].ty)
            .collect();
    }
    if let Some(&id) = hir.items.unions.get(name) {
        return hir.unions[id]
            .fields
            .iter()
            .map(|&f| hir.fields[f].ty)
            .collect();
    }
    Vec::new()
}

struct WrapperNodesVisitor<'a> {
    seen: &'a mut FxHashSet<TypeNode>,
    out: &'a mut Vec<TypeNode>,
}

impl VisitTypeRef for WrapperNodesVisitor<'_> {
    fn visit_ty(&mut self, _ty: TypeRef, _types: &TypeInterner) -> bool {
        true
    }

    fn visit_ty_post(&mut self, ty: TypeRef, types: &TypeInterner) {
        match *types.lookup(ty) {
            TypeKind::Array { elem, len } => {
                let node = TypeNode::Array { elem, len };
                if self.seen.insert(node.clone()) {
                    self.out.push(node);
                }
            }
            TypeKind::Fn {
                ref params,
                ret,
                variadic,
            } => {
                let node = TypeNode::Fn {
                    params: params.clone(),
                    ret,
                    variadic,
                };
                if self.seen.insert(node.clone()) {
                    self.out.push(node);
                }
            }
            _ => {}
        }
    }
}

/// register every array wrapper and function-pointer typedef inside `ty`,
/// innermost first (post-order), so a nested wrapper/typedef is a node before
/// the one that embeds it. a function-pointer typedef is a discoverable node
/// even when it appears only as a bare local or parameter, so its typedef is
/// always emitted (the advisor's discovery touch-point).
fn collect_wrapper_nodes(
    ty: TypeRef,
    types: &TypeInterner,
    seen: &mut FxHashSet<TypeNode>,
    out: &mut Vec<TypeNode>,
) {
    let mut visitor = WrapperNodesVisitor { seen, out };
    types.walk(ty, &mut visitor);
}

/// every type-declaration node in the program, in a deterministic order: nominal
/// types in arena (declaration) order, then every distinct array wrapper found
/// by walking the program, innermost wrapper first. this order only fixes
/// tie-breaks; the emission order is [`topo_order`]. (when run before bodies are
/// lowered - the value-recursion check - body-local wrappers are simply absent,
/// which is fine: a cycle can only form through a struct/union field.)
pub fn collect_type_nodes(hir: &HIR) -> Vec<TypeNode> {
    let mut nodes = Vec::new();
    let estimate = hir.structs.len() + hir.unions.len() + hir.fields.len() + hir.functions.len();
    let mut seen = fx_set(estimate);
    let types = &hir.types;

    for (_, s) in hir.structs.iter() {
        if seen.insert(TypeNode::Nominal(s.name.clone())) {
            nodes.push(TypeNode::Nominal(s.name.clone()));
        }
    }
    for (_, u) in hir.unions.iter() {
        if seen.insert(TypeNode::Nominal(u.name.clone())) {
            nodes.push(TypeNode::Nominal(u.name.clone()));
        }
    }

    for (_, field) in hir.fields.iter() {
        collect_wrapper_nodes(field.ty, types, &mut seen, &mut nodes);
    }
    // EXPERIMENTAL(U1): walk const and global type annotations so array
    // wrapper typedefs appear for types used only in const/global decls.
    for (_, c) in hir.consts.iter() {
        collect_wrapper_nodes(c.ty, types, &mut seen, &mut nodes);
    }
    for (_, g) in hir.globals.iter() {
        collect_wrapper_nodes(g.ty, types, &mut seen, &mut nodes);
    }
    for (_, f) in hir.functions.iter() {
        for p in &f.params {
            collect_wrapper_nodes(p.ty, types, &mut seen, &mut nodes);
        }
        if let Some(ret) = &f.ret {
            collect_wrapper_nodes(*ret, types, &mut seen, &mut nodes);
        }
    }
    for (_, body) in hir.bodies.iter() {
        for (_, local) in body.locals.iter() {
            if let Some(ty) = local.ty {
                collect_wrapper_nodes(ty, types, &mut seen, &mut nodes);
            }
        }
        // array/fn-ptr wrappers that surface only as an expression's type (an
        // intermediate value with no declared local) are seeded into
        // `topo_order` from the typeck results, since lowering no longer stamps
        // expression types (S2C C5).
        for (_, stmt) in body.stmts.iter() {
            if let Stmt::Let { ty: Some(ty), .. } = stmt {
                collect_wrapper_nodes(*ty, types, &mut seen, &mut nodes);
            }
        }
        for (_, expr) in body.exprs.iter() {
            if let Expr::Cast { ty, .. } | Expr::StructLit { ty, .. } = expr {
                collect_wrapper_nodes(*ty, types, &mut seen, &mut nodes);
            }
            if let Expr::SizeOf(ty) = expr {
                collect_wrapper_nodes(*ty, types, &mut seen, &mut nodes);
            }
        }
    }
    nodes
}

/// the type-declaration definitions in dependency order: every node appears
/// after all nodes it embeds by value. kahn's algorithm, seeded and tie-broken
/// in node (arena/discovery) order so the generated c is deterministic. a
/// residual value cycle (rejected upstream by [`cyclic_nodes`]) leaves nodes
/// unorderable; they are appended in node order rather than dropped, so this is
/// total and never panics.
pub fn topo_order(hir: &HIR, extra: &[TypeRef]) -> Vec<TypeNode> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut nodes = collect_type_nodes(hir);
    // seed wrapper typedefs that only surface as expression/temp types - codegen
    // passes the whole-file typeck expr types (S2C C5). declaration-derived nodes
    // already in `nodes` are de-duplicated via `seen`.
    if !extra.is_empty() {
        let mut seen: FxHashSet<TypeNode> = nodes.iter().cloned().collect();
        for &ty in extra {
            collect_wrapper_nodes(ty, &hir.types, &mut seen, &mut nodes);
        }
    }
    let index: FxHashMap<TypeNode, usize> = nodes
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, n)| (n, i))
        .collect();
    let n = nodes.len();

    let mut indeg = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, node) in nodes.iter().enumerate() {
        let mut raw = Vec::new();
        node_deps(hir, node, &mut raw);
        let mut seen = fx_set(4);
        for d in raw {
            // a self-edge (a direct value cycle) is ignored so kahn does not
            // deadlock on it; `cyclic_nodes` rejects it upstream.
            if let Some(&j) = index.get(&d)
                && j != i
                && seen.insert(j)
            {
                indeg[i] += 1;
                dependents[j].push(i);
            }
        }
    }

    let mut ready: BinaryHeap<Reverse<usize>> =
        (0..n).filter(|&i| indeg[i] == 0).map(Reverse).collect();
    let mut order = Vec::with_capacity(n);
    let mut emitted = vec![false; n];
    while let Some(Reverse(i)) = ready.pop() {
        order.push(nodes[i].clone());
        emitted[i] = true;
        for &y in &dependents[i] {
            indeg[y] -= 1;
            if indeg[y] == 0 {
                ready.push(Reverse(y));
            }
        }
    }
    for (i, done) in emitted.iter().enumerate() {
        if !done {
            order.push(nodes[i].clone());
        }
    }
    order
}

/// computed SCC (strongly connected component) information over the
/// type-declaration graph, built with tarjan's algorithm (o(v + e)).
pub struct SccInfo {
    nodes: Vec<TypeNode>,
    index: FxHashMap<TypeNode, usize>,
    scc_id: Vec<usize>,
    cyclic_scc: Vec<bool>,
}

impl SccInfo {
    /// whether `node` lies on a value-dependency cycle.
    pub fn contains(&self, node: &TypeNode) -> bool {
        self.index
            .get(node)
            .is_some_and(|&i| self.cyclic_scc[self.scc_id[i]])
    }

    /// whether any cycle exists.
    pub fn is_empty(&self) -> bool {
        !self.cyclic_scc.iter().any(|&c| c)
    }

    /// whether two nodes belong to the same cyclic SCC (same cycle).
    pub fn same_cycle(&self, a: &TypeNode, b: &TypeNode) -> bool {
        let Some(&ia) = self.index.get(a) else {
            return false;
        };
        let Some(&ib) = self.index.get(b) else {
            return false;
        };
        let sa = self.scc_id[ia];
        let sb = self.scc_id[ib];
        sa == sb && self.cyclic_scc[sa]
    }
}

/// build the type-declaration graph and compute its sccs.
/// returns the [`SccInfo`] with cycle information.
pub fn compute_scc(hir: &HIR) -> SccInfo {
    let nodes = collect_type_nodes(hir);
    let n = nodes.len();
    let index: FxHashMap<TypeNode, usize> = nodes
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, n)| (n, i))
        .collect();

    // build adjacency list from hard deps. self-loops are included so
    // tarjan's SCC can detect single-node cycles (e.g. `structure A { A a }`).
    let mut deps = vec![Vec::new(); n];
    let mut edge_seen: FxHashSet<(usize, usize)> = fx_set(n.max(1) * 2);
    for (i, node) in nodes.iter().enumerate() {
        let mut raw = Vec::new();
        node_deps(hir, node, &mut raw);
        for d in raw {
            if let Some(&j) = index.get(&d)
                && edge_seen.insert((i, j))
            {
                deps[i].push(j);
            }
        }
    }

    // tarjan's SCC algorithm -- o(v + e)
    const UNVISITED: u32 = u32::MAX;
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
                &deps,
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

    // classify: an SCC is cyclic if it has >1 node or has a self-loop
    let mut scc_size = vec![0usize; scc_count];
    let mut has_self_loop = vec![false; scc_count];
    for v in 0..n {
        scc_size[scc_id[v]] += 1;
        for &w in &deps[v] {
            if v == w {
                has_self_loop[scc_id[v]] = true;
            }
        }
    }
    let mut cyclic_scc = vec![false; scc_count];
    for i in 0..scc_count {
        cyclic_scc[i] = scc_size[i] > 1 || has_self_loop[i];
    }

    SccInfo {
        nodes,
        index,
        scc_id,
        cyclic_scc,
    }
}

/// convenience wrapper: the set of all cyclic type nodes.
/// prefer [`compute_scc`] when you also need [`SccInfo::same_cycle`].
pub fn cyclic_nodes(hir: &HIR) -> FxHashSet<TypeNode> {
    let scc = compute_scc(hir);
    scc.nodes
        .iter()
        .filter(|n| scc.contains(n))
        .cloned()
        .collect()
}

/// whether two nodes lie on the same value-dependency cycle.
/// prefer [`SccInfo::same_cycle`] when the SCC info is already computed.
pub fn same_value_cycle(hir: &HIR, a: &TypeNode, b: &TypeNode) -> bool {
    let scc = compute_scc(hir);
    scc.same_cycle(a, b)
}
