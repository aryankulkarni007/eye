//! the standalone type judgments: the `check_*` value-position / consistency /
//! range sweeps, the binary and index operator judgments, and match analysis.
//! these read the walker's `expr_types` (during or after the spine walk) and
//! emit diagnostics; they impose no expectation of their own. driven from
//! [`super::InferCtx::run`] and from the spine.

use ast::{BinOp, UnaryOp};
use hir::core::{BlockId, EnumId, Expr, ExprId, Literal, Pat, Stmt, Text, TypeKind, TypeRef};
use syntax::SyntaxNodePtr;

use crate::InferObserver;

use super::ty::*;
use super::InferCtx;

/// a match scrutinee's discriminant domain (see `check_matches`). only an enum
/// or a primitive scalar is matchable; everything else is `Other`.
#[derive(Clone, Copy)]
enum MatchDomain {
    Enum(EnumId),
    Bool,
    Int,
    Char,
    Other,
}

impl<'a, O: InferObserver> InferCtx<'a, O> {
    /// completeness sweep - the walker's totality guarantee, now expressed
    /// through the unit type. every expression in value position (one not in
    /// `discarded_set`) that yields *no* value has type `()`; binding or
    /// operating on it is rejected (`VoidValueInValuePosition`). this is what
    /// makes the walker *total*: such an expression would otherwise reach MIR as
    /// `void` and miscompile. the let-initializer / return-value / tail sites
    /// catch what they can see; this sweep generalizes to *every* value position
    /// (a `Binary` operand, an index, a nested argument), so a void value buried
    /// in an expression is caught, not silently miscompiled. a diverging
    /// expression has type `!` (`Never`), not `()`, so it is never swept - its
    /// value is never read. an assignment is `()` too but owns its own
    /// `AssignInValuePosition` message, so it is skipped here.
    pub(crate) fn check_value_position_voids(&mut self) {
        let unit = self.types.unit_ty();
        let discarded = self.discarded_set();
        let voids: Vec<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, expr)| {
                if !self.results.visited.contains(&id)
                    || discarded.contains(&id)
                    || matches!(expr, Expr::Assign { .. })
                {
                    return None;
                }
                (self.ty_of(id) == Some(unit)).then_some(id)
            })
            .collect();
        for id in voids {
            self.emit_at(id, None, hir::core::TypeError::VoidValueInValuePosition);
        }
    }

    /// whether an expression has the never type (`!`) - it diverges and never
    /// yields control. used to detect a diverging tail-less block.
    pub(crate) fn is_never(&self, id: ExprId) -> bool {
        self.ty_of(id) == Some(self.types.never_ty())
    }

    /// whether an expression has the unit type (`()`) - it completes but yields
    /// no value. the value-position void check; a coercion site skips its
    /// type-mismatch comparison for a unit value (the sweep owns the diagnostic).
    fn is_unit(&self, id: ExprId) -> bool {
        self.ty_of(id) == Some(self.types.unit_ty())
    }

    /// the never-absorbing join of two branch types: a `Never` branch takes the
    /// other's type, so `if c { 5 } else { return }` joins to the `5`. when both
    /// are non-`Never` and differ, the first wins and the branch-consistency
    /// check reports the mismatch (it stays bug-compatible with the old
    /// first-arm-wins rule for the non-divergent case).
    fn join(&self, a: TypeRef, b: TypeRef) -> TypeRef {
        if a == self.types.never_ty() { b } else { a }
    }

    /// [`Self::join`] lifted over optional branch types (an untyped branch
    /// contributes nothing).
    pub(crate) fn join_opt(&self, a: Option<TypeRef>, b: Option<TypeRef>) -> Option<TypeRef> {
        match (a, b) {
            (Some(x), Some(y)) => Some(self.join(x, y)),
            (Some(x), None) => Some(x),
            (None, y) => y,
        }
    }

    /// whether a `loop` body contains a `break` that targets *this* loop (a
    /// `break` inside a nested loop belongs to the inner loop, so the scan does
    /// not descend into nested `Loop` bodies). a loop with such a break completes
    /// with unit; one without never returns control and is `Never`.
    pub(crate) fn loop_has_break(&self, block: BlockId) -> bool {
        self.block_has_break(block)
    }

    fn block_has_break(&self, block: BlockId) -> bool {
        let b = &self.body.blocks[block];
        b.stmts.iter().any(|&s| {
            matches!(&self.body.stmts[s], Stmt::Expr(e) if self.expr_has_break(*e))
        }) || b.tail.is_some_and(|t| self.expr_has_break(t))
    }

    fn expr_has_break(&self, id: ExprId) -> bool {
        match &self.body.exprs[id] {
            Expr::Break => true,
            // a `break` inside a nested loop targets the inner loop.
            Expr::Loop { .. } => false,
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.block_has_break(*then_branch)
                    || else_branch.is_some_and(|b| self.block_has_break(b))
            }
            Expr::Block(b) => self.block_has_break(*b),
            Expr::Match { arms, .. } => arms.iter().any(|a| self.expr_has_break(a.body)),
            _ => false,
        }
    }

    /// assignment-non-value judgment (S3): an `Expr::Assign` is legal only in
    /// statement position (the direct expr of a `Stmt::Expr`, anywhere in the
    /// body) or as a discarded tail (a void function's body tail, or a tail
    /// reached from one). every other assignment - a `let` initializer, an
    /// argument, a condition, an operand, a value-producing branch tail - is in
    /// value position and rejected (`if x = y` is the canonical footgun). the
    /// discarded set is seeded from the statement arena and propagated through
    /// the tails of discarded `if`/`block`/`match` expressions; an assignment
    /// not in it is a value-position use.
    pub(crate) fn check_value_position_assignments(&mut self) {
        let assigns: Vec<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| matches!(e, Expr::Assign { .. }).then_some(id))
            .collect();
        if assigns.is_empty() {
            return;
        }
        let discarded = self.discarded_set();
        for a in assigns {
            if !discarded.contains(&a) {
                self.emit_at(a, None, hir::core::TypeError::AssignInValuePosition);
            }
        }
    }

    /// the set of expressions whose value is discarded - statement position
    /// (a `Stmt::Expr` anywhere in the body, nested blocks included) plus a
    /// void function's body tail, propagated through the tails of discarded
    /// `if`/`block`/`match` expressions. an expression *not* in this set is in
    /// value position. shared by the assignment-non-value (S3) and value-`if`
    /// branch-consistency (F1) judgments.
    fn discarded_set(&self) -> rustc_hash::FxHashSet<ExprId> {
        let mut discarded: rustc_hash::FxHashSet<ExprId> = rustc_hash::FxHashSet::default();
        for (_, stmt) in self.body.stmts.iter() {
            if let Stmt::Expr(e) = stmt {
                self.mark_discarded(*e, &mut discarded);
            }
        }
        if self.fn_ret.is_none()
            && let Some(tail) = self.body.tail
        {
            self.mark_discarded(tail, &mut discarded);
        }
        discarded
    }

    /// value-position `if` branch-type consistency (F1, S3). a value-position
    /// `if` (one not in `discarded_set`) with both branches present and typed
    /// must have `types_compatible` branch types - the `if` analogue of
    /// `check_match_arm_consistency`. `let int32 x = if c { 1 } else { true }`
    /// was accepted (it typed as the `then` branch, the `else` converting
    /// silently in C); now rejected. a statement-position or discarded `if`
    /// runs its branches for effect, so a type difference there stays legal.
    pub(crate) fn check_if_branch_consistency(&mut self) {
        let ifs: Vec<(ExprId, BlockId, BlockId)> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| match e {
                Expr::If {
                    then_branch,
                    else_branch: Some(eb),
                    ..
                } => Some((id, *then_branch, *eb)),
                _ => None,
            })
            .collect();
        if ifs.is_empty() {
            return;
        }
        let discarded = self.discarded_set();
        // each present branch tail must agree with the `if`'s settled type - the
        // type the funnel restamped from a downward expectation, or the
        // bottom-up join when there is none. comparing each branch against the
        // node type (not then-vs-else) also catches a branch that disagrees with
        // the *expected* type even when the branches agree with each other
        // (`fn -> int32 { if c { 1.0 } else { 2.0 } }`) - the role the per-branch
        // `expect_branch_type` played before the spine folded it here. collect
        // first (immutable interner reads), then emit, so the borrow does not
        // overlap `emit_at`'s `&mut self` (same pattern as the match check).
        let mut mismatches: Vec<(ExprId, String, String)> = Vec::new();
        for (id, then_b, else_b) in ifs {
            if discarded.contains(&id) {
                continue;
            }
            let Some(node_ty) = self.ty_of(id) else {
                continue;
            };
            for branch in [then_b, else_b] {
                let Some(tail) = self.body.blocks[branch].tail else {
                    continue;
                };
                let Some(branch_ty) = self.ty_of(tail) else {
                    continue;
                };
                // a `()` (void) branch tail is the completeness sweep's
                // diagnostic, not a branch-type mismatch.
                if matches!(self.types.lookup(branch_ty), TypeKind::Unit) {
                    continue;
                }
                if !types_compatible(branch_ty, node_ty, self.types)
                    && !array_ref_decays_to(node_ty, branch_ty, self.types)
                {
                    let expected = self.types.display(node_ty).to_string();
                    let found = self.types.display(branch_ty).to_string();
                    mismatches.push((tail, expected, found));
                }
            }
        }
        for (tail, expected, found) in mismatches {
            self.emit_at(
                tail,
                None,
                hir::core::TypeError::IfBranchTypeMismatch { expected, found },
            );
        }
    }

    /// mark `e` and the tails it discards as value-discarded positions. a
    /// discarded `if`/`block`/`match` discards each of its branch/tail
    /// sub-expressions in turn (so an `if c { x = 1 } else { y = 2 }` *statement*
    /// keeps its branch-tail assignments legal). read-only; fills `set`.
    fn mark_discarded(&self, e: ExprId, set: &mut rustc_hash::FxHashSet<ExprId>) {
        if !set.insert(e) {
            return;
        }
        match &self.body.exprs[e] {
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                if let Some(t) = self.body.blocks[*then_branch].tail {
                    self.mark_discarded(t, set);
                }
                if let Some(eb) = else_branch
                    && let Some(t) = self.body.blocks[*eb].tail
                {
                    self.mark_discarded(t, set);
                }
            }
            Expr::Block(b) => {
                if let Some(t) = self.body.blocks[*b].tail {
                    self.mark_discarded(t, set);
                }
            }
            // a `loop` never yields its body's value, so the body block's tail
            // is discarded - a trailing else-less `if` (`loop { ..; if c { f(); } }`)
            // runs for effect, exactly like a statement.
            Expr::Loop { body } => {
                if let Some(t) = self.body.blocks[*body].tail {
                    self.mark_discarded(t, set);
                }
            }
            Expr::Match { arms, .. } => {
                for arm in arms {
                    self.mark_discarded(arm.body, set);
                }
            }
            _ => {}
        }
    }

    /// value-position match-arm result-type consistency (moved from lowering,
    /// S2 step b). a value-position match is any `Expr::Match` that is not in
    /// statement position - the direct expr of a `Stmt::Expr`, or a tail whose
    /// value is discarded (a void body, `fn_ret == None`). every arm whose body
    /// type is known must be `types_compatible` with the match's result type (a
    /// let/return override when present, else the provisional first-known-arm
    /// type recorded during the walk). reads the walker's own `expr_types`,
    /// which mirror lowering's stamps under the shadow contract.
    pub(crate) fn check_match_arm_consistency(&mut self) {
        // statement-position matches: the direct expr of a `Stmt::Expr`.
        let mut stmt_pos: rustc_hash::FxHashSet<ExprId> = self
            .body
            .stmts
            .iter()
            .filter_map(|(_, stmt)| match stmt {
                Stmt::Expr(id) if matches!(self.body.exprs[*id], Expr::Match { .. }) => Some(*id),
                _ => None,
            })
            .collect();
        // a tail match whose value is discarded runs for effect like a
        // statement-position match (codegen emits a bare `switch`).
        if self.fn_ret.is_none()
            && let Some(tail) = self.body.tail
            && matches!(self.body.exprs[tail], Expr::Match { .. })
        {
            stmt_pos.insert(tail);
        }
        let value_matches: Vec<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, expr)| matches!(expr, Expr::Match { .. }).then_some(id))
            .filter(|id| !stmt_pos.contains(id))
            .collect();
        for match_id in value_matches {
            let Some(result_ty) = self.ty_of(match_id) else {
                continue;
            };
            let arm_bodies: Vec<ExprId> = match &self.body.exprs[match_id] {
                Expr::Match { arms, .. } => arms.iter().map(|a| a.body).collect(),
                _ => continue,
            };
            // collect first (immutable interner reads), then emit, so the
            // borrow does not overlap `emit_at`'s `&mut self`.
            let mismatches: Vec<(ExprId, String, String)> = arm_bodies
                .iter()
                .filter_map(|&body_id| {
                    let arm_ty = self.ty_of(body_id)?;
                    if types_compatible(arm_ty, result_ty, self.types) {
                        return None;
                    }
                    let expected = self.types.display(result_ty).to_string();
                    let found = self.types.display(arm_ty).to_string();
                    Some((body_id, expected, found))
                })
                .collect();
            for (body_id, expected, found) in mismatches {
                self.emit_at(
                    body_id,
                    None,
                    hir::core::TypeError::MatchArmTypeMismatch { expected, found },
                );
            }
        }
    }

    /// match type-judgments (relocated from lowering, S2C C2). lowering now
    /// lowers match arms purely structurally - a bare-ident pattern is a variant
    /// or a binding by NAME, with no scrutinee type read - so every judgment that
    /// needs the type lives here: the scrutinee must be a matchable domain (enum
    /// / int / char / bool); an enum-variant pattern must belong to the
    /// scrutinee's enum and a literal/variant must match its domain; the arms
    /// must be exhaustive; and no arm may duplicate a discriminant or sit after
    /// an irrefutable one. anchors mirror lowering exactly - the arm span for
    /// per-arm errors (`MatchArm::ptr`), the pattern span for a wrong-enum
    /// variant, the scrutinee for a non-matchable domain, the whole match for
    /// exhaustiveness.
    pub(crate) fn check_matches(&mut self) {
        let match_ids: Vec<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| matches!(e, Expr::Match { .. }).then_some(id))
            .collect();
        for match_id in match_ids {
            self.check_one_match(match_id);
        }
    }

    fn check_one_match(&mut self, match_id: ExprId) {
        // `body` is a copy of the shared `&Body`, so the arm/pattern reads below
        // are independent of `self` and the `&mut self` emits can interleave.
        let body = self.body;
        let (scrut, arms) = match &body.exprs[match_id] {
            Expr::Match { scrut, arms } => (*scrut, arms.as_slice()),
            _ => return,
        };
        let match_ptr = body.source_map.expr.get(match_id.into()).cloned();
        let scrut_ty = self.ty_of(scrut);
        let domain = self.match_domain(scrut_ty);
        let scrut_ty_name = self.type_name(scrut_ty);
        // a non-matchable scrutinee: report once, then skip the domain-specific
        // per-arm and exhaustiveness checks (but still flag unreachable arms).
        if matches!(domain, MatchDomain::Other) {
            self.emit_at(scrut, match_ptr, hir::core::TypeError::MatchScrutineeNotEnum);
        }
        let mut covered: Vec<bool> = match domain {
            MatchDomain::Enum(eid) => vec![false; self.scope.enums[eid].variants.len()],
            _ => Vec::new(),
        };
        let mut saw_true = false;
        let mut saw_false = false;
        let mut saw_wildcard = false;
        for arm in arms {
            let after_wildcard = saw_wildcard;
            // a guarded arm never discharges coverage (its guard may be false),
            // and a guarded catch-all is not the totalizing wildcard.
            let has_guard = arm.guard.is_some();
            if after_wildcard {
                self.emit_ptr(
                    Some(arm.ptr),
                    hir::core::PatternError::UnreachableAfterWildcard,
                );
            }
            match &body.pats[arm.pat] {
                // irrefutable: a wildcard or a binding (a named wildcard) makes
                // the match total and shadows any following arm.
                Pat::Wildcard | Pat::Bind(_) => {
                    if !has_guard {
                        saw_wildcard = true;
                    }
                }
                Pat::Variant { enum_id, idx } => {
                    let (enum_id, idx) = (*enum_id, *idx);
                    match domain {
                        MatchDomain::Enum(eid) => {
                            if enum_id != eid {
                                // a variant of the wrong enum (a bare or
                                // qualified name resolved by `lower_match_pat`
                                // without the scrutinee type); anchor on the
                                // pattern, as lowering did for the qualified form.
                                let pattern_enum = self.scope.enums[enum_id].name.clone();
                                let scrutinee_enum = self.scope.enums[eid].name.clone();
                                let pat_ptr = body.source_map.pat.get(arm.pat.into()).cloned();
                                self.emit_ptr(
                                    pat_ptr,
                                    hir::core::ResolveError::PatternEnumMismatch {
                                        pattern_enum,
                                        scrutinee_enum,
                                    },
                                );
                            } else {
                                let i = idx as usize;
                                if covered.get(i).copied() == Some(true) {
                                    let vname = self.scope.enums[eid].variants[i].name.clone();
                                    self.emit_ptr(
                                        Some(arm.ptr),
                                        hir::core::PatternError::DuplicateArm { variant: vname },
                                    );
                                }
                                if !has_guard && let Some(slot) = covered.get_mut(i) {
                                    *slot = true;
                                }
                            }
                        }
                        MatchDomain::Bool | MatchDomain::Int | MatchDomain::Char => {
                            let vname = self.scope.enums[enum_id].variants[idx as usize].name.clone();
                            self.emit_ptr(
                                Some(arm.ptr),
                                hir::core::PatternError::PatternDomainMismatch {
                                    scrutinee: scrut_ty_name.clone(),
                                    pattern: vname,
                                },
                            );
                        }
                        MatchDomain::Other => {}
                    }
                }
                Pat::Literal(lit) => match domain {
                    // int and char are the same integer comparison in c, so a
                    // char literal against an int scrutinee (and vice versa) is
                    // allowed; bool/enum stay strict.
                    MatchDomain::Int | MatchDomain::Char
                        if matches!(lit, Literal::Int(_) | Literal::Char(_)) => {}
                    MatchDomain::Bool if matches!(lit, Literal::Bool(_)) => {
                        if !has_guard {
                            if matches!(lit, Literal::Bool(true)) {
                                saw_true = true;
                            } else {
                                saw_false = true;
                            }
                        }
                    }
                    MatchDomain::Other => {}
                    _ => {
                        let pattern = literal_pat_text(lit);
                        self.emit_ptr(
                            Some(arm.ptr),
                            hir::core::PatternError::PatternDomainMismatch {
                                scrutinee: scrut_ty_name.clone(),
                                pattern,
                            },
                        );
                    }
                },
                // a `Missing` (failed resolution) or struct pattern discharges
                // nothing; the resolution failure was already diagnosed.
                Pat::Missing | Pat::Struct { .. } => {}
            }
        }
        // exhaustiveness, by domain. enum and bool have finite known universes;
        // int and char need an explicit `_`.
        if !saw_wildcard {
            match domain {
                MatchDomain::Enum(eid) => {
                    let missing: Vec<Text> = self.scope.enums[eid]
                        .variants
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !covered[*i])
                        .map(|(_, v)| v.name.clone())
                        .collect();
                    if !missing.is_empty() {
                        let enum_name = self.scope.enums[eid].name.clone();
                        self.emit_ptr(
                            match_ptr,
                            hir::core::PatternError::NonExhaustive { enum_name, missing },
                        );
                    }
                }
                MatchDomain::Bool => {
                    let mut missing: Vec<Text> = Vec::new();
                    if !saw_false {
                        missing.push(Text::from("false"));
                    }
                    if !saw_true {
                        missing.push(Text::from("true"));
                    }
                    if !missing.is_empty() {
                        self.emit_ptr(
                            match_ptr,
                            hir::core::PatternError::NonExhaustivePrimitive {
                                ty: Text::from("bool"),
                                missing,
                            },
                        );
                    }
                }
                MatchDomain::Int | MatchDomain::Char => self.emit_ptr(
                    match_ptr,
                    hir::core::PatternError::NonExhaustivePrimitive {
                        ty: scrut_ty_name,
                        missing: Vec::new(),
                    },
                ),
                MatchDomain::Other => {}
            }
        }
    }

    /// classify a match scrutinee into its discriminant domain. only a `Path`
    /// naming an enum or a primitive scalar is matchable.
    fn match_domain(&self, scrut_ty: Option<TypeRef>) -> MatchDomain {
        let Some(ty) = scrut_ty else {
            return MatchDomain::Other;
        };
        match self.types.lookup(ty) {
            TypeKind::Path(name) => {
                if let Some(&eid) = self.scope.items.enums.get(name) {
                    MatchDomain::Enum(eid)
                } else if name == "bool" {
                    MatchDomain::Bool
                } else if name == "char" {
                    MatchDomain::Char
                } else if is_int_type_name(name) {
                    MatchDomain::Int
                } else {
                    MatchDomain::Other
                }
            }
            _ => MatchDomain::Other,
        }
    }

    /// the scrutinee type's path name (for the domain-mismatch and
    /// non-exhaustive-primitive diagnostics), or `<unknown>` when it is not a
    /// named type.
    fn type_name(&self, scrut_ty: Option<TypeRef>) -> Text {
        match scrut_ty.map(|t| self.types.lookup(t)) {
            Some(TypeKind::Path(name)) => name.clone(),
            _ => Text::from("<unknown>"),
        }
    }

    /// return-type *arity* for the body tail (the type judgment moved to the
    /// funnel with the spine - the tail is checked against the declared return
    /// by its `Return` expectation in `run`). this catches only the structural
    /// case the funnel cannot see: a body with no tail *and* no explicit
    /// `return val;` produces no value despite its declaration
    /// (`ReturnMissingValue`, anchored on the whole fn block). a tail present
    /// (any kind) is the funnel's / consistency checks' responsibility.
    pub(crate) fn enforce_return_type(&mut self) {
        let Some(ret) = self.fn_ret else { return };
        if self.body.tail.is_some() {
            return;
        }
        let has_return = self.body.stmts.iter().any(|(_, s)| match s {
            Stmt::Expr(e) => matches!(self.body.exprs[*e], Expr::Return(Some(_))),
            _ => false,
        });
        if !has_return {
            let block_ptr = self.body.fn_block_ptr;
            let expected = self.types.display(ret).to_string();
            self.emit_ptr(
                block_ptr,
                hir::core::TypeError::ReturnMissingValue { expected },
            );
        }
    }

    /// explicit-return *arity* check (the type judgment moved to the funnel with
    /// the spine - the returned value is checked against the declared return by
    /// its `Return` expectation, threaded in the `Return` arm of `infer_expr`).
    /// this owns only the two arity cases clang would reject: a value in a void
    /// fn, and a missing value in a typed fn. both anchor on the whole `return`.
    pub(crate) fn check_explicit_return(&mut self, id: ExprId, value: Option<ExprId>) {
        let ret_ptr = self.body.source_map.expr.get(id.into()).cloned();
        match (self.fn_ret, value) {
            (None, None) | (Some(_), Some(_)) => {}
            (None, Some(_)) => self.emit_ptr(ret_ptr, hir::core::TypeError::ReturnValueInVoid),
            (Some(expected), None) => {
                let expected = self.types.display(expected).to_string();
                self.emit_ptr(
                    ret_ptr,
                    hir::core::TypeError::ReturnMissingValue { expected },
                );
            }
        }
    }

    /// array-init-length check (moved from lowering, S2 step b): a typed array
    /// binding must initialize exactly the declared element count. c accepts a
    /// short initializer and zero-fills; eye reports the mismatch.
    pub(crate) fn check_array_init_len(
        &mut self,
        declared: TypeRef,
        init: ExprId,
        stmt_ptr: Option<SyntaxNodePtr>,
    ) {
        let declared_len = match self.types.lookup(declared) {
            &TypeKind::Array { len, .. } => len,
            _ => return,
        };
        let Some(init_len) = self.ty_of(init).and_then(|t| match self.types.lookup(t) {
            &TypeKind::Array { len, .. } => Some(len),
            _ => None,
        }) else {
            return;
        };
        if declared_len != init_len {
            self.emit_at(
                init,
                stmt_ptr,
                hir::core::TypeError::ArrayInitLenMismatch {
                    declared: declared_len,
                    found: init_len,
                },
            );
        }
    }

    /// explicit-let initializer type check (moved from lowering, S2 step b): a
    /// call initializer's result type must match the declared type. a void
    /// initializer (an else-less `if`, a tail-less block, a void call) is the
    /// completeness sweep's job (`check_value_position_voids`); here we only
    /// compare a *known* initializer type. other initializers are not yet checked
    /// (no full inference). lenient on `Error` and on array length (the latter is
    /// `check_array_init_len`'s job).
    pub(crate) fn check_explicit_let_init_type(
        &mut self,
        expected: TypeRef,
        init: ExprId,
        stmt_ptr: Option<SyntaxNodePtr>,
    ) {
        if self.is_unit(init) {
            return;
        }
        if !matches!(self.body.exprs[init], Expr::Call { .. }) {
            return;
        }
        let Some(actual) = self.ty_of(init) else {
            return;
        };
        if type_ref_contains_error(expected, self.types)
            || type_ref_contains_error(actual, self.types)
        {
            return;
        }
        // a length mismatch between arrays is reported by check_array_init_len.
        if let (TypeKind::Array { len: exp_len, .. }, TypeKind::Array { len: act_len, .. }) =
            (self.types.lookup(expected), self.types.lookup(actual))
            && exp_len != act_len
        {
            return;
        }
        if actual != expected && !array_ref_decays_to(expected, actual, self.types) {
            let expected = self.types.display(expected).to_string();
            let got = self.types.display(actual).to_string();
            self.emit_at(
                init,
                stmt_ptr,
                hir::core::TypeError::LetTypeMismatch { expected, got },
            );
        }
    }

    /// M1, moved here from lowering (S2 step b): every integer literal's
    /// value must fit the integer type it ended up with - a coercion site's
    /// expected type, or the `int32` default. runs once per body after every
    /// coercion site has typed its literals. visited expressions only, in
    /// arena order (deterministic, and poison children of rejected
    /// expressions are excluded by construction). the literal synthesized by
    /// the `len` fold is skipped by its tell: it shares its `Cast` wrapper's
    /// syntax pointer (a user-written cast operand has its own).
    pub(crate) fn check_int_literal_ranges(&mut self) {
        // collect the integer literals first and bail when there are none, so a
        // literal-free body builds neither auxiliary set.
        let lits: Vec<(ExprId, u128)> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| match e {
                Expr::Literal(Literal::Int(v)) => Some((id, *v)),
                _ => None,
            })
            .collect();
        if lits.is_empty() {
            return;
        }
        let neg_operands: rustc_hash::FxHashSet<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(_, e)| match e {
                &Expr::Unary {
                    op: UnaryOp::Neg,
                    operand,
                } => Some(operand),
                _ => None,
            })
            .collect();
        let synthesized: rustc_hash::FxHashSet<ExprId> = self
            .body
            .exprs
            .iter()
            .filter_map(|(id, e)| match e {
                &Expr::Cast { operand, .. } => {
                    let cast_ptr = self.body.source_map.expr.get(id.into())?;
                    let op_ptr = self.body.source_map.expr.get(operand.into())?;
                    (cast_ptr == op_ptr).then_some(operand)
                }
                _ => None,
            })
            .collect();
        for (id, v) in lits {
            if !self.results.visited.contains(&id) || synthesized.contains(&id) {
                continue;
            }
            let Some(ty) = self.ty_of(id) else { continue };
            let name: Text = match self.types.lookup(ty) {
                TypeKind::Path(n) => n.clone(),
                _ => continue,
            };
            let Some((neg_mag, max)) = int_type_range(&name) else {
                continue;
            };
            let negated = neg_operands.contains(&id);
            let limit = if negated { neg_mag } else { max };
            if v <= limit {
                continue;
            }
            let value = if negated {
                format!("-{v}")
            } else {
                v.to_string()
            };
            let min = if neg_mag == 0 {
                "0".to_string()
            } else {
                format!("-{neg_mag}")
            };
            self.emit_at(
                id,
                None,
                hir::core::TypeError::IntLiteralOutOfRange {
                    value,
                    ty: name,
                    min,
                    max: max.to_string(),
                },
            );
        }
    }
    /// the binary-operator judgments (moved from lowering, S2 step b):
    /// whole-array operands, `ptr` arithmetic, opaque-enum arithmetic, and
    /// float modulo each emit a diagnostic. a comparison yields `bool`;
    /// arithmetic/bitwise yields the operands' common integer type via
    /// [`Self::arith_result_type`] (M2: literal adoption, fixed at S3).
    pub(crate) fn binary_judgments(
        &mut self,
        id: ExprId,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    ) -> Option<TypeRef> {
        if let Some(err) = self.binary_judgment_error(op, lhs, rhs) {
            self.emit_at(id, None, err);
        }
        if is_comparison(op) {
            Some(self.types.intern(TypeKind::Path(Text::from("bool"))))
        } else {
            self.arith_result_type(lhs, rhs)
        }
    }

    /// result type of an arithmetic/bitwise binary: the operands' common
    /// integer type. an integer-literal operand adopts the other operand's
    /// concrete integer type (rust-style literal inference), fixing M2 where
    /// `7 - usize` previously took the literal's `int32` and the MIR temp
    /// truncated the wider c result. equal operands, a non-integer pair, or two
    /// literals keep the left type (lowering's prior rule); a mismatched pair of
    /// distinct concrete widths is left to the operand-mismatch judgment.
    fn arith_result_type(&self, lhs: ExprId, rhs: ExprId) -> Option<TypeRef> {
        match (self.ty_of(lhs), self.ty_of(rhs)) {
            (Some(l), Some(r))
                if l != r
                    && is_integer_path(l, self.types)
                    && is_integer_path(r, self.types)
                    && self.is_int_literal(lhs)
                    && !self.is_int_literal(rhs) =>
            {
                Some(r)
            }
            (Some(l), _) => Some(l),
            (None, r) => r,
        }
    }

    /// whether an expression is a bare integer literal (the adopting side of
    /// [`Self::arith_result_type`]).
    fn is_int_literal(&self, e: ExprId) -> bool {
        matches!(self.body.exprs[e], Expr::Literal(Literal::Int(_)))
    }

    /// the first applicable operator-judgment error for a binary expression,
    /// or `None` when the operands are legal. read-only and clone-free: each
    /// operand's `TypeKind` is matched by reference, never copied out.
    fn binary_judgment_error(
        &self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    ) -> Option<hir::core::TypeError> {
        let is_array = |e: ExprId| {
            matches!(
                self.ty_of(e).map(|t| self.types.lookup(t)),
                Some(TypeKind::Array { .. })
            )
        };
        // a whole array is a struct in the c backend; any binary operator on it
        // emits invalid c (a reference compares as a pointer, so only value
        // arrays are caught).
        if is_array(lhs) || is_array(rhs) {
            return Some(hir::core::TypeError::OpOnArray { op: bin_op_str(op) });
        }
        if !is_comparison(op) {
            let is_raw_ptr = |e: ExprId| {
                matches!(
                    self.ty_of(e).map(|t| self.types.lookup(t)),
                    Some(TypeKind::RawPtr)
                )
            };
            // P1: arithmetic/bitwise on `ptr` would emit c `void*` arithmetic
            // (a GNU extension, no element size). comparisons stay legal.
            if is_raw_ptr(lhs) || is_raw_ptr(rhs) {
                return Some(hir::core::TypeError::ArithmeticOnPtr { op: bin_op_str(op) });
            }
            // enums are opaque (T035): arithmetic/bitwise rejected, comparisons
            // allowed, `as` is the explicit escape.
            if let Some(enum_name) = self
                .expr_enum_name(lhs)
                .or_else(|| self.expr_enum_name(rhs))
            {
                return Some(hir::core::TypeError::ArithmeticOnEnum {
                    op: bin_op_str(op),
                    enum_name,
                });
            }
        }
        // `%` is integer-only: on a float it would lower to `double % double`
        // (invalid c).
        if matches!(op, BinOp::Rem) {
            let is_float = |e: ExprId| {
                matches!(
                    self.ty_of(e).map(|t| self.types.lookup(t)),
                    Some(TypeKind::Path(p)) if p == "float32" || p == "float64"
                )
            };
            if is_float(lhs) || is_float(rhs) {
                return Some(hir::core::TypeError::ModuloOnFloat);
            }
        }
        // M2b: two operands of *distinct concrete* integer widths (neither a
        // literal that would adopt the other's width, M2) silently narrow - the
        // C result takes one operand's width and truncates the wider value.
        // reject; the user casts explicitly (no-footgun, Rust's strict-width
        // rule). applies to arithmetic, bitwise, and comparison alike.
        if let (Some(l), Some(r)) = (self.ty_of(lhs), self.ty_of(rhs))
            && l != r
            && is_integer_path(l, self.types)
            && is_integer_path(r, self.types)
            && !self.is_int_literal(lhs)
            && !self.is_int_literal(rhs)
        {
            return Some(hir::core::TypeError::MixedIntegerWidths {
                left: self.types.display(l).to_string(),
                right: self.types.display(r).to_string(),
            });
        }
        None
    }

    /// index judgments (moved from lowering): indexing the untyped `ptr`
    /// (L7), and a compile-time index past or below a known fixed length
    /// (A4). dynamic indices stay unchecked (runtime safety is deferred).
    pub(crate) fn index_judgments(&mut self, id: ExprId, base: ExprId, index: ExprId) {
        let base_ty = self.ty_of(base);
        if let Some(ty) = base_ty {
            match self.types.lookup(ty) {
                // the untyped `ptr` has no element type (L7).
                TypeKind::RawPtr => self.emit_at(id, None, hir::core::TypeError::IndexOnPtr),
                // arrays index (bounds-checked below); a typed pointer/reference
                // indexes as in c (`p[i]`), no bounds known here.
                TypeKind::Array { .. } | TypeKind::Ref(_) | TypeKind::Ptr(_) => {}
                TypeKind::Path(n) if n == "string" => {}
                // an error operand is already diagnosed - stay silent.
                TypeKind::Error => {}
                // a scalar / struct / union / enum / `()` / `!` has no elements;
                // `x[i]` on it would emit invalid c.
                _ => {
                    let found = self.types.display(ty).to_string();
                    self.emit_at(id, None, hir::core::TypeError::IndexOfNonIndexable { found });
                }
            }
        }
        let arr_len = base_ty.and_then(|ty| match self.types.lookup(ty) {
            &TypeKind::Array { len, .. } => Some(len),
            &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => match self.types.lookup(inner) {
                &TypeKind::Array { len, .. } => Some(len),
                _ => None,
            },
            _ => None,
        });
        let Some(len) = arr_len else { return };
        if let Some(v) = self.const_uint_index(index) {
            if v >= len as u128 {
                self.emit_at(
                    id,
                    None,
                    hir::core::ConstError::IndexOutOfBounds { index: v, len },
                );
            }
        } else if let Expr::Unary {
            op: UnaryOp::Neg,
            operand,
        } = &self.body.exprs[index]
        {
            // a negative literal lowers to `-(int)`; out of bounds for any
            // length.
            if matches!(&self.body.exprs[*operand], Expr::Literal(Literal::Int(v)) if *v > 0) {
                self.emit_at(id, None, hir::core::ConstError::NegativeIndex);
            }
        }
    }
    /// a statically-known non-negative index: a bare integer literal, or one
    /// behind a cast (notably the `(usize)N` a `len(x)` fold lowers to).
    fn const_uint_index(&self, idx: ExprId) -> Option<u128> {
        match &self.body.exprs[idx] {
            Expr::Literal(Literal::Int(v)) => Some(*v),
            Expr::Cast { operand, .. } => match &self.body.exprs[*operand] {
                Expr::Literal(Literal::Int(v)) => Some(*v),
                _ => None,
            },
            // `len(arr)` folds to the operand's element count: `a[len(a)]` is a
            // static off-by-one, so peel the operand's array type here too.
            &Expr::Len(operand) => {
                let arg_ty = self.ty_of(operand)?;
                match self.types.lookup(arg_ty) {
                    &TypeKind::Array { len, .. } => Some(len as u128),
                    &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => {
                        match self.types.lookup(inner) {
                            &TypeKind::Array { len, .. } => Some(len as u128),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
