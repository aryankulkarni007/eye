//! Value-recursion check (pass 1.5).
//!
//! Rejects a struct or union that contains itself by value - directly
//! (`structure A { A a }`), mutually (`A { B b }; B { A a }`), or through an
//! array (`structure A { [A; 4] xs }`). Such a type has infinite size and is a
//! hard clang error; this catches it with a clear diagnostic first. The cycle
//! must be broken with a pointer (`Node* next`), which is a soft edge.
//!
//! Edge classification (value vs. pointer) comes from the shared
//! [`cyclic_nodes`] over the [`typegraph`](crate::core), so this check and
//! codegen's type-declaration ordering agree on exactly which programs are
//! orderable - a mismatch would let an unorderable program reach clang.

use diagnostics::Span;
use syntax::SyntaxToken;

use crate::core::{HIR, HirError, Text, TypeError, TypeNode, compute_scc};

pub(super) fn check_value_recursion(hir: &mut HIR, file: &ast::SourceFile) {
    let scc = compute_scc(hir);
    if scc.is_empty() {
        return;
    }
    // Gather offenders with only `&hir` (cyclic set + AST), then emit: emitting
    // borrows `hir.diagnostics` mutably.
    let mut offenders: Vec<(Span, Text)> = Vec::new();
    for item in file.items() {
        let name_tok: Option<SyntaxToken> = match &item {
            ast::Item::StructDef(s) => s.name(),
            ast::Item::UnionDef(u) => u.name(),
            _ => None,
        };
        let Some(tok) = name_tok else { continue };
        let name = Text::from(tok.text());
        if scc.contains(&TypeNode::Nominal(name.clone())) {
            offenders.push((Span::from(tok.text_range()), name));
        }
    }
    // One diagnostic per cycle. Mutual recursion (`A{B b}; B{A a}`) flags both
    // `A` and `B`, but it is a single infinite-size cycle, so report it once on
    // its first-declared member and skip any later member of the same cycle.
    // (Still uses `&hir` only - the emit loop is split out below.)
    let mut to_emit: Vec<(Span, Text)> = Vec::new();
    let mut reps: Vec<TypeNode> = Vec::new();
    for (span, name) in offenders {
        let node = TypeNode::Nominal(name.clone());
        if reps.iter().any(|r| scc.same_cycle(r, &node)) {
            continue;
        }
        reps.push(node);
        to_emit.push((span, name));
    }
    for (span, name) in to_emit {
        hir.diagnostics
            .emit(span, HirError::Type(TypeError::RecursiveValueType { name }));
    }
}
