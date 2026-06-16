//! the bidirectional walker (tier 1) - the sole type authority since the S2C
//! cutover. lowering no longer stamps any expression; this pass interns every
//! `expr_types` entry codegen and mir read.
//!
//! rules still carrying a `PARITY(S3)` marker were faithful ports of lowering's
//! old stamping, kept bug-compatible only while the shadow oracle ran. the
//! oracle retired at C5, so these are now fixed in place, each with its own
//! test (M2 - mixed-width arithmetic - is the first).

use ast::{BinOp, UnaryOp};
use hir::core::{
    BlockId, Body, EnumId, Expr, ExprId, HIR, HirError, Literal, Pat, Resolution, Stmt, StmtId,
    Text, TypeInterner, TypeKind, TypeRef, VisitTypeRef,
};
use syntax::SyntaxNodePtr;

use crate::{Adjustment, InferObserver, ObserverCx, TypeckResults};

pub(crate) struct InferCtx<'a, O> {
    scope: &'a HIR,
    body: &'a Body,
    types: &'a TypeInterner,
    fn_ret: Option<TypeRef>,
    results: TypeckResults,
    obs: &'a mut O,
}

impl<'a, O: InferObserver> InferCtx<'a, O> {
    pub(crate) fn new(
        scope: &'a HIR,
        body: &'a Body,
        fn_ret: Option<TypeRef>,
        types: &'a TypeInterner,
        obs: &'a mut O,
    ) -> Self {
        Self {
            scope,
            body,
            types,
            fn_ret,
            results: TypeckResults::default(),
            obs,
        }
    }

    pub(crate) fn run(mut self) -> TypeckResults {
        for &stmt in &self.body.block {
            self.infer_stmt(stmt);
        }
        if let Some(tail) = self.body.tail {
            self.infer_expr(tail);
            // the tail goes through the coercion point against the declared
            // return type, then the declared type is re-recorded onto a
            // value-position tail match (stamping; mirrors `fn_body.rs`'s
            // tail coercion + match re-record). the return-type *diagnostics*
            // live in `enforce_return_type` below.
            if let Some(ret) = self.fn_ret {
                self.site_coerce(ret, tail);
                if matches!(self.body.exprs[tail], Expr::Match { .. }) {
                    self.record(tail, ret);
                }
            }
        }
        self.enforce_return_type();
        self.check_int_literal_ranges();
        self.check_match_arm_consistency();
        self.check_if_branch_consistency();
        self.check_matches();
        self.check_value_position_assignments();
        self.results
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
    fn check_value_position_assignments(&mut self) {
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
    fn check_if_branch_consistency(&mut self) {
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
        // collect first (immutable interner reads), then emit, so the borrow
        // does not overlap `emit_at`'s `&mut self` (same pattern as the match
        // consistency check).
        let mut mismatches: Vec<(ExprId, String, String)> = Vec::new();
        for (id, then_b, else_b) in ifs {
            if discarded.contains(&id) {
                continue;
            }
            let (Some(tt), Some(et)) = (self.block_type(then_b), self.block_type(else_b)) else {
                continue;
            };
            if !types_compatible(tt, et, self.types) {
                let expected = self.types.display(tt).to_string();
                let found = self.types.display(et).to_string();
                mismatches.push((id, expected, found));
            }
        }
        for (id, expected, found) in mismatches {
            self.emit_at(
                id,
                None,
                hir::core::TypeError::IfBranchTypeMismatch { expected, found },
            );
        }
    }

    /// a block's value type: the recorded type of its tail expression, or
    /// `None` for a tail-less (statement-only) block.
    fn block_type(&self, id: BlockId) -> Option<TypeRef> {
        self.body.blocks[id].tail.and_then(|t| self.ty_of(t))
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
    fn check_match_arm_consistency(&mut self) {
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
    fn check_matches(&mut self) {
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

    /// return-type enforcement for the body tail (moved from lowering, S2
    /// step b): the implicit-return tail must produce the declared return
    /// type. a body with neither a tail nor any explicit `return val;` never
    /// produces a value (`ReturnMissingValue`, anchored on the whole fn
    /// block). a tail that yields no value (a void-branch `if`) is rejected;
    /// a value-position tail match defers to the per-arm consistency check;
    /// otherwise the tail type must be `types_compatible` with the return.
    /// the tail-match re-record stays in `run`/lowering (it stamps the
    /// codegen hoist temp); this is diagnostics only.
    fn enforce_return_type(&mut self) {
        let Some(ret) = self.fn_ret else { return };
        let Some(tail) = self.body.tail else {
            // no tail and no explicit `return val;`: the body never produces a
            // value despite its declaration.
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
            return;
        };
        // a tail else-less / void-branch `if` yields no value for the return.
        if self.yields_no_value(tail) {
            self.emit_at(tail, None, hir::core::TypeError::VoidValueInValuePosition);
            return;
        }
        // a value-position tail match: `check_match_arm_consistency` owns the
        // per-arm reporting against the declared return type.
        if matches!(self.body.exprs[tail], Expr::Match { .. }) {
            return;
        }
        // an array-literal tail was already re-typed onto the declared return
        // type at the coercion site, so a matching length compares equal and a
        // wrong length falls through to the mismatch below.
        let Some(actual) = self.ty_of(tail) else {
            return;
        };
        if !types_compatible(actual, ret, self.types)
            && !array_ref_decays_to(ret, actual, self.types)
        {
            let expected = self.types.display(ret).to_string();
            let found = self.types.display(actual).to_string();
            self.emit_at(
                tail,
                None,
                hir::core::TypeError::ReturnTypeMismatch { expected, found },
            );
        }
    }

    /// explicit-return arity/type check (moved from lowering, S2 step b):
    /// `return expr?;` against the enclosing function's declared return type
    /// (`self.fn_ret`). covers the three returns clang would reject - a value
    /// in a void fn, a missing value in a typed fn, a wrong-typed value - plus
    /// the void-branch `if` leak. arity diagnostics anchor on the whole
    /// `return`; a type mismatch anchors on the returned value. lenient via
    /// `types_compatible`, matching the tail check, pending real inference.
    fn check_explicit_return(&mut self, id: ExprId, value: Option<ExprId>) {
        let ret_ptr = self.body.source_map.expr.get(id.into()).cloned();
        match (self.fn_ret, value) {
            (None, None) => {}
            (None, Some(_)) => self.emit_ptr(ret_ptr, hir::core::TypeError::ReturnValueInVoid),
            (Some(expected), None) => {
                let expected = self.types.display(expected).to_string();
                self.emit_ptr(
                    ret_ptr,
                    hir::core::TypeError::ReturnMissingValue { expected },
                );
            }
            (Some(ret), Some(val)) => {
                // a returned else-less / void-branch `if` yields no value.
                if self.yields_no_value(val) {
                    self.emit_at(val, ret_ptr, hir::core::TypeError::VoidValueInValuePosition);
                    return;
                }
                let Some(actual) = self.ty_of(val) else {
                    self.emit_at(val, ret_ptr, hir::core::TypeError::VoidValueInValuePosition);
                    return;
                };
                if !types_compatible(actual, ret, self.types)
                    && !array_ref_decays_to(ret, actual, self.types)
                {
                    let expected = self.types.display(ret).to_string();
                    let found = self.types.display(actual).to_string();
                    self.emit_at(
                        val,
                        ret_ptr,
                        hir::core::TypeError::ReturnTypeMismatch { expected, found },
                    );
                }
            }
        }
    }

    /// array-init-length check (moved from lowering, S2 step b): a typed array
    /// binding must initialize exactly the declared element count. c accepts a
    /// short initializer and zero-fills; eye reports the mismatch.
    fn check_array_init_len(
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

    /// explicit-let initializer type check (moved from lowering, S2 step b): an
    /// `if` initializer must yield a value on every path, and a call
    /// initializer's result type must match the declared type. other
    /// initializers are not yet checked (no full inference). lenient on `Error`
    /// and on array length (the latter is `check_array_init_len`'s job).
    fn check_explicit_let_init_type(
        &mut self,
        expected: TypeRef,
        init: ExprId,
        stmt_ptr: Option<SyntaxNodePtr>,
    ) {
        // an else-less / void-branch `if` leaves the binding uninitialized.
        if self.yields_no_value(init) {
            self.emit_at(
                init,
                stmt_ptr,
                hir::core::TypeError::VoidValueInValuePosition,
            );
            return;
        }
        if !matches!(self.body.exprs[init], Expr::Call { .. }) {
            return;
        }
        let Some(actual) = self.ty_of(init) else {
            self.emit_at(
                init,
                stmt_ptr,
                hir::core::TypeError::VoidValueInValuePosition,
            );
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

    /// true when an expression provably yields no value on some control path
    /// (a value-consuming-position error). ported from lowering's
    /// `yields_no_value`; the only proven case is an else-less / void-branch
    /// `if`. conservative: anything unproven yields `false`.
    fn yields_no_value(&self, id: ExprId) -> bool {
        let (then_block, else_block) = match &self.body.exprs[id] {
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => (*then_branch, *else_branch),
            _ => return false,
        };
        match else_block {
            None => true,
            Some(eb) => self.block_yields_no_value(then_block) || self.block_yields_no_value(eb),
        }
    }

    /// a block provably yields no value only when its tail does (a nested
    /// else-less `if`); a tail-less block may diverge, which is legal in value
    /// position, so it returns `false`.
    fn block_yields_no_value(&self, block: BlockId) -> bool {
        match self.body.blocks[block].tail {
            None => false,
            Some(tail) => self.yields_no_value(tail),
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
    fn check_int_literal_ranges(&mut self) {
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

    fn record(&mut self, id: ExprId, ty: TypeRef) {
        self.results.expr_types.insert(id.into(), ty);
    }

    /// emit a type-judgment diagnostic anchored at an expression's syntax
    /// pointer (the body's source map; same anchoring lowering used).
    /// falls back to the enclosing expression when the target has none.
    fn emit_at(&mut self, id: ExprId, fallback: Option<SyntaxNodePtr>, err: impl Into<HirError>) {
        let ptr = self
            .body
            .source_map
            .expr
            .get(id.into())
            .cloned()
            .or(fallback);
        // a missing pointer means a programming error -- every expression id
        // the walker visits was allocated via `alloc_expr` which always inserts
        // a source-map entry. ICE, don't silently drop.
        let ptr = ptr.unwrap_or_else(|| {
            panic!("emit_at: no source-map entry for ExprId({id:?}) and no fallback")
        });
        self.results.diagnostics.emit(ptr, err.into());
    }

    /// emit a diagnostic at an explicit pointer (no expr key): the return
    /// arity diagnostics anchor on the whole `return` or the fn block, which
    /// have no expression id of their own.
    fn emit_ptr(&mut self, ptr: Option<SyntaxNodePtr>, err: impl Into<HirError>) {
        // a missing pointer means a programming error -- callers always compute
        // one from a live syntax node or a source-map lookup. ICE, don't
        // silently drop.
        let ptr = ptr.unwrap_or_else(|| panic!("emit_ptr: missing syntax pointer"));
        self.results.diagnostics.emit(ptr, err.into());
    }

    fn ty_of(&self, id: ExprId) -> Option<TypeRef> {
        self.results.expr_types.get(id.into()).copied()
    }

    fn infer_stmt(&mut self, id: StmtId) {
        match &self.body.stmts[id] {
            Stmt::Let { ty, init, .. } => {
                let (ty, init) = (*ty, *init);
                if let Some(init) = init {
                    self.infer_expr(init);
                    if let Some(declared) = ty {
                        self.site_coerce(declared, init);
                        // mirrors `record_match_result_override`: an
                        // explicitly typed `let` is authoritative for a
                        // value-position match initializer.
                        if matches!(self.body.exprs[init], Expr::Match { .. }) {
                            self.record(init, declared);
                        }
                        // let-initializer judgments (moved from lowering, S2
                        // step b), against the explicit declared type.
                        let stmt_ptr = self.body.source_map.stmt.get(id.into()).cloned();
                        self.check_array_init_len(declared, init, stmt_ptr);
                        self.check_explicit_let_init_type(declared, init, stmt_ptr);
                    }
                }
            }
            Stmt::Expr(e) => {
                self.infer_expr(*e);
            }
            // purely compile-time: the value is folded into
            // `body.local_consts`, no expressions to type.
            Stmt::Const(_) => {}
        }
    }

    fn infer_block(&mut self, id: BlockId) -> Option<TypeRef> {
        // same lifetime decouple as `infer_expr`: a `&Body` copy lets the
        // block's stmts be iterated while calling `&mut self`, no `ThinVec`
        // clone.
        let body = self.body;
        let tail = body.blocks[id].tail;
        for &stmt in &body.blocks[id].stmts {
            self.infer_stmt(stmt);
        }
        tail.and_then(|t| {
            self.infer_expr(t);
            self.ty_of(t)
        })
    }

    /// type one expression bottom-up, mirroring `lower_expr`'s stamping.
    /// returns the recorded type (`None` = unstamped, S1 partial contract).
    fn infer_expr(&mut self, id: ExprId) -> Option<TypeRef> {
        self.results.visited.insert(id);
        // `self.body` is a shared `&Body`; copying it into a local decouples
        // the expression tree's lifetime from `self`, so every arm can borrow
        // the tree while still calling the `&mut self` walk methods - no
        // per-expression clone / `to_vec` to dodge the borrow checker.
        let body = self.body;
        let ty = match &body.exprs[id] {
            Expr::Missing => None,
            Expr::Literal(lit) => Some(self.literal_type(lit)),
            Expr::Path(res) => self.path_type(res, false),
            Expr::Binary { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                self.infer_expr(lhs);
                self.infer_expr(rhs);
                self.binary_judgments(id, op, lhs, rhs)
            }
            Expr::Unary { op, operand } => {
                let (op, operand) = (*op, *operand);
                self.infer_expr(operand);
                // opaque enums (T035): `-`/`~` on an enum value is arithmetic
                // and rejected, like the binary operators. PARITY(S3): emit the
                // diagnostic but keep lowering's type (the operand's); the
                // poison flip lands at the S2 cutover.
                if matches!(op, UnaryOp::Neg | UnaryOp::BitNot)
                    && let Some(enum_name) = self.expr_enum_name(operand)
                {
                    let op_str = if matches!(op, UnaryOp::Neg) { "-" } else { "~" };
                    self.emit_at(
                        id,
                        None,
                        hir::core::TypeError::ArithmeticOnEnum {
                            op: op_str,
                            enum_name,
                        },
                    );
                }
                // F2 (S3): unary `-` on an unsigned integer wraps modulo 2^N in
                // C; reject it (Rust parity). `~` stays legal (well-defined
                // complement). a negated literal is exempt - it is a single
                // signed constant the range sweep already bounds, not a runtime
                // negation of an unsigned value.
                if matches!(op, UnaryOp::Neg)
                    && !matches!(self.body.exprs[operand], Expr::Literal(Literal::Int(_)))
                    && let Some(ty) = self.ty_of(operand)
                    && let Some(name) = unsigned_int_name(ty, self.types)
                {
                    self.emit_at(
                        id,
                        None,
                        hir::core::TypeError::NegationOnUnsigned { ty: name },
                    );
                }
                match op {
                    UnaryOp::Not => Some(self.types.intern(TypeKind::Path(Text::from("bool")))),
                    UnaryOp::Neg | UnaryOp::BitNot => self.ty_of(operand),
                }
            }
            Expr::Call { callee, args } => self.infer_call(*callee, args),
            Expr::ArrayLit(elems) => {
                for &e in elems {
                    self.infer_expr(e);
                }
                // typed as [first-elem; n] when the first element's type is
                // known; a declared array type re-types it at the coercion
                // site (`coerce_array_literal` mirror in `site_coerce`).
                let len = elems.len() as u64;
                elems
                    .first()
                    .and_then(|&first| self.ty_of(first))
                    .map(|elem| self.types.intern(TypeKind::Array { elem, len }))
            }
            Expr::ArrayRepeat { value, count } => {
                let (value, count) = (*value, *count);
                self.infer_expr(value);
                // `count == 0` is the inert placeholder of a failed const
                // length (a real 0 is rejected upstream): lowering left the
                // repeat untyped in that case.
                if count > 0 {
                    self.ty_of(value)
                        .map(|elem| self.types.intern(TypeKind::Array { elem, len: count }))
                } else {
                    None
                }
            }
            Expr::Index { base, index } => {
                let (base, index) = (*base, *index);
                self.infer_expr(base);
                self.infer_expr(index);
                self.index_judgments(id, base, index);
                // element type: the base's element/pointee, peeling one
                // ref/ptr so `r[i]` on `&[T; N]` yields `T`.
                self.ty_of(base).and_then(|ty| match self.types.lookup(ty) {
                    &TypeKind::Array { elem, .. } => Some(elem),
                    &TypeKind::Ptr(inner) | &TypeKind::Ref(inner) => {
                        match self.types.lookup(inner) {
                            &TypeKind::Array { elem, .. } => Some(elem),
                            _ => Some(inner),
                        }
                    }
                    _ => None,
                })
            }
            Expr::StructLit { ty, fields } => {
                let lit_ty = *ty;
                // one owned copy of the struct name (released the `self.types`
                // borrow); the field list is read in place off the body.
                let struct_name = match self.types.lookup(lit_ty) {
                    TypeKind::Path(n) => Some(n.clone()),
                    _ => None,
                };
                for f in fields {
                    let value = f.value;
                    self.infer_expr(value);
                    // field values with a known declared field type go
                    // through the coercion site (the L1 fix's 5th site).
                    let field_ty = struct_name
                        .as_ref()
                        .and_then(|sname| self.field_decl_type(sname, &f.name));
                    if let Some(ft) = field_ty {
                        self.site_coerce(ft, value);
                        // field value type judgment (S3): `P { x: "hi" }` with
                        // `int32 x` reached clang before (only missing/unknown
                        // fields were caught). length-mismatched array fields
                        // surface here too (no field-specific length check).
                        if let Some(found) = self.ty_of(value)
                            && !site_assignable(ft, found, self.types)
                        {
                            let expected = self.types.display(ft).to_string();
                            let got = self.types.display(found).to_string();
                            self.emit_at(
                                value,
                                None,
                                hir::core::TypeError::StructFieldTypeMismatch {
                                    field: f.name.clone(),
                                    expected,
                                    found: got,
                                },
                            );
                        }
                    }
                }
                Some(lit_ty)
            }
            Expr::Field { base, name } => {
                let base = *base;
                self.infer_expr(base);
                let base_ty = self.ty_of(base).unwrap_or_else(|| self.types.error_type());
                // `.len` on an array is reserved for a future `.len()` method;
                // steer to the `len(x)` intrinsic (lenfieldonarray, relocated
                // from lowering at S2C C5).
                if name == "len" && peeled_array(base_ty, self.types) {
                    self.emit_at(id, None, hir::core::TypeError::LenFieldOnArray);
                }
                Some(self.lookup_field_type(base_ty, name))
            }
            Expr::Assign { lhs, rhs, .. } => {
                let (lhs, rhs) = (*lhs, *rhs);
                self.infer_expr(lhs);
                self.infer_expr(rhs);
                // assignment is ruled non-value (S3); a value-position use is
                // rejected post-walk by `check_value_position_assignments`. the
                // synthesized type is the rhs's - unused either way (a
                // statement-position assign is discarded, a value-position one
                // is a rejected program), but kept so the walker stays total.
                self.ty_of(rhs)
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                self.infer_expr(cond);
                let then_ty = self.infer_block(then_branch);
                let else_ty = else_branch.and_then(|b| self.infer_block(b));
                then_ty.or(else_ty)
            }
            Expr::Loop { body: loop_body } => {
                let loop_body = *loop_body;
                self.infer_block(loop_body);
                None
            }
            Expr::Break | Expr::Continue => None,
            Expr::Return(value) => {
                let value = *value;
                if let Some(v) = value {
                    self.infer_expr(v);
                    if let Some(ret) = self.fn_ret {
                        self.site_coerce(ret, v);
                    }
                }
                self.check_explicit_return(id, value);
                None
            }
            Expr::Ref { operand } => {
                let operand = *operand;
                self.infer_expr(operand);
                let inner = self
                    .ty_of(operand)
                    .unwrap_or_else(|| self.types.error_type());
                Some(self.types.intern(TypeKind::Ref(inner)))
            }
            Expr::Deref { operand } => {
                let operand = *operand;
                self.infer_expr(operand);
                let op_ty = self
                    .ty_of(operand)
                    .unwrap_or_else(|| self.types.error_type());
                // classify with a copy result so the `&self.types` borrow ends
                // before the `&mut self` emit / interner write (no typekind clone).
                let (inner, is_raw_ptr) = match self.types.lookup(op_ty) {
                    &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => (Some(inner), false),
                    // the untyped `ptr` has no pointee type to deref (L7/P1).
                    TypeKind::RawPtr => (None, true),
                    // other non-pointers poison silently (diagnosed upstream).
                    _ => (None, false),
                };
                if is_raw_ptr {
                    self.emit_at(id, None, hir::core::TypeError::DerefOfPtr);
                }
                Some(inner.unwrap_or_else(|| self.types.error_type()))
            }
            Expr::Cast { operand, ty } => {
                let (operand, ty) = (*operand, *ty);
                self.infer_expr(operand);
                // cast-lattice judgment (S3): `as` is no longer any-to-any.
                // scalar<->scalar, pointer<->pointer, and pointer<->integer
                // convert; an aggregate (array/struct/union) on either side has
                // no value-level conversion and is rejected. an unresolved /
                // error operand stays lenient (no cascade).
                if let Some(from) = self.ty_of(operand)
                    && !cast_allowed(from, ty, self.scope, self.types)
                {
                    let from_s = self.types.display(from).to_string();
                    let to_s = self.types.display(ty).to_string();
                    self.emit_at(
                        id,
                        None,
                        hir::core::TypeError::CastNotAllowed {
                            from: from_s,
                            to: to_s,
                        },
                    );
                }
                Some(ty)
            }
            Expr::Match { scrut, arms } => {
                let scrut = *scrut;
                self.infer_expr(scrut);
                // a bare-ident binding arm (`x -> ..`) takes the scrutinee's
                // type. lowering left these locals untyped (it no longer knows
                // the scrutinee type, S2C C2); record the type before the arm
                // bodies are walked so a body reference to the binding resolves.
                if let Some(scrut_ty) = self.ty_of(scrut) {
                    for arm in arms {
                        if let Pat::Bind(local) = self.body.pats[arm.pat] {
                            self.results.local_types.insert(local, scrut_ty);
                        }
                    }
                }
                // type of the whole match mirrors `if`: the first arm whose
                // body type is known. a `let`/return override re-records it
                // afterwards (see `infer_stmt` / `run`).
                let mut arm_type: Option<TypeRef> = None;
                for arm in arms {
                    if let Some(g) = arm.guard {
                        self.infer_expr(g);
                    }
                    self.infer_expr(arm.body);
                    if arm_type.is_none() {
                        arm_type = self.ty_of(arm.body);
                    }
                }
                arm_type
            }
            Expr::SizeOf(_) => Some(self.types.usize_ty()),
            Expr::Len(operand) => {
                let operand = *operand;
                self.infer_expr(operand);
                // `len(x)` requires an array operand (lennotarray, relocated
                // from lowering at S2C C5). only checked on a place operand, so
                // a non-place already flagged `LenNotAPlace` in lowering is not
                // double-reported. `len(arr)` is a compile-time `usize`; MIR
                // folds the count from the operand's array type.
                let is_place = matches!(
                    self.body.exprs[operand],
                    Expr::Path(Resolution::Local(_))
                        | Expr::Field { .. }
                        | Expr::Index { .. }
                        | Expr::Deref { .. }
                );
                if is_place
                    && let Some(ty) = self.ty_of(operand)
                    && !peeled_array(ty, self.types)
                {
                    self.emit_at(operand, None, hir::core::TypeError::LenNotArray);
                }
                Some(self.types.usize_ty())
            }
            Expr::Block(b) => {
                let b = *b;
                self.infer_block(b)
            }
        };
        if let Some(ty) = ty {
            self.record(id, ty);
        }
        let cx = ObserverCx {
            scope: self.scope,
            body,
            types: self.types,
            expr_types: &self.results.expr_types,
        };
        self.obs.visit(id, &body.exprs[id], ty, &cx);
        ty
    }

    fn infer_call(&mut self, callee: ExprId, args: &[ExprId]) -> Option<TypeRef> {
        match &self.body.exprs[callee] {
            // a direct callee is deliberately left untyped (recording its
            // fn-pointer type would force a typedef for every called fn).
            Expr::Path(Resolution::Fn(fn_id)) => {
                let fn_id = *fn_id;
                self.results.visited.insert(callee);
                {
                    let cx = ObserverCx {
                        scope: self.scope,
                        body: self.body,
                        types: self.types,
                        expr_types: &self.results.expr_types,
                    };
                    self.obs.visit(callee, &self.body.exprs[callee], None, &cx);
                }
                for &a in args {
                    self.infer_expr(a);
                }
                // coerce each argument against its parameter's declared type.
                // `self.scope` is a shared `&HIR`; a copy reads the params at
                // the scope's lifetime while `site_coerce` takes `&mut self` -
                // no `param_tys` vec to dodge the borrow. extra args (variadic)
                // have no parameter and are left uncoerced.
                let scope = self.scope;
                for (i, &a) in args.iter().enumerate() {
                    let Some(param) = scope.functions[fn_id].params.get(i) else {
                        continue;
                    };
                    let param_ty = param.ty;
                    self.site_coerce(param_ty, a);
                    // argument type judgment (S3): the coerced argument must be
                    // assignable to the parameter. swapped or wrong-type args
                    // (`generate_lang` FIXME) were previously accepted - only
                    // arity was checked.
                    if let Some(found) = self.ty_of(a)
                        && !site_assignable(param_ty, found, self.types)
                    {
                        let expected = self.types.display(param_ty).to_string();
                        let found = self.types.display(found).to_string();
                        self.emit_at(
                            a,
                            None,
                            hir::core::TypeError::ArgTypeMismatch {
                                index: i + 1,
                                expected,
                                found,
                            },
                        );
                    }
                }
                self.scope.functions[fn_id].ret
            }
            // the `println` intrinsic (the only unresolved callee that
            // survives lowering as a call): not a typed value.
            Expr::Path(Resolution::Unresolved(_)) => {
                self.results.visited.insert(callee);
                {
                    let cx = ObserverCx {
                        scope: self.scope,
                        body: self.body,
                        types: self.types,
                        expr_types: &self.results.expr_types,
                    };
                    self.obs.visit(callee, &self.body.exprs[callee], None, &cx);
                }
                for &a in args {
                    self.infer_expr(a);
                }
                // printcannotformat (relocated from lowering, S2C C5): an
                // array/struct/union argument has no `{}` rendering. the first
                // argument is the format string and is exempt. collected before
                // emitting so the interner read does not overlap `&mut self`.
                let bad: Vec<(ExprId, &'static str)> = args
                    .iter()
                    .skip(1)
                    .filter_map(|&a| self.unformattable(a).map(|kind| (a, kind)))
                    .collect();
                for (a, kind) in bad {
                    self.emit_at(a, None, hir::core::TypeError::PrintCannotFormat { kind });
                }
                None
            }
            // indirect call through a function-pointer value. a callee that is
            // neither a function pointer nor `Error` is not callable
            // (callnonfunction, relocated from lowering at S2C C5); the result
            // is poison.
            _ => {
                let callee_ty = self.infer_expr(callee);
                for &a in args {
                    self.infer_expr(a);
                }
                match callee_ty {
                    Some(ty) => match self.types.lookup(ty) {
                        TypeKind::Fn { ret, .. } => *ret,
                        TypeKind::Error => None,
                        _ => {
                            let found = self.types.display(ty).to_string();
                            self.emit_at(callee, None, hir::core::TypeError::CallNonFunction { found });
                            None
                        }
                    },
                    None => None,
                }
            }
        }
    }

    /// the binary-operator judgments (moved from lowering, S2 step b):
    /// whole-array operands, `ptr` arithmetic, opaque-enum arithmetic, and
    /// float modulo each emit a diagnostic. a comparison yields `bool`;
    /// arithmetic/bitwise yields the operands' common integer type via
    /// [`Self::arith_result_type`] (M2: literal adoption, fixed at S3).
    fn binary_judgments(
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
        None
    }

    /// index judgments (moved from lowering): indexing the untyped `ptr`
    /// (L7), and a compile-time index past or below a known fixed length
    /// (A4). dynamic indices stay unchecked (runtime safety is deferred).
    fn index_judgments(&mut self, id: ExprId, base: ExprId, index: ExprId) {
        let base_ty = self.ty_of(base);
        if let Some(ty) = base_ty
            && matches!(self.types.lookup(ty), TypeKind::RawPtr)
        {
            self.emit_at(id, None, hir::core::TypeError::IndexOnPtr);
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

    /// the enum name of an expression's inferred type, when that type is a
    /// declared enum (drives the opaque-enum rejections, T035).
    fn expr_enum_name(&self, id: ExprId) -> Option<Text> {
        let ty = self.ty_of(id)?;
        match self.types.lookup(ty) {
            TypeKind::Path(name) if self.scope.items.enums.contains_key(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// a value-position name's type, mirroring the `NameRef` arm.
    fn path_type(&mut self, res: &Resolution, is_callee: bool) -> Option<TypeRef> {
        match res {
            // a match-arm binding is untyped in the arena (lowering no longer
            // knows the scrutinee type, S2C C2); its type is filled in
            // `local_types` when the enclosing match is walked.
            Resolution::Local(id) => self
                .body
                .locals[*id]
                .ty
                .or_else(|| self.results.local_types.get(id).copied()),
            Resolution::Const(cid) => Some(self.scope.consts[*cid].ty),
            Resolution::LocalConst(lcid) => Some(self.body.local_consts[*lcid].ty),
            Resolution::Global(gid) => Some(self.scope.globals[*gid].ty),
            Resolution::Variant { enum_id, .. } => Some(
                self.types
                    .intern(TypeKind::Path(self.scope.enums[*enum_id].name.clone())),
            ),
            Resolution::Fn(fn_id) if !is_callee => self.scope.functions[*fn_id].fn_type,
            _ => None,
        }
    }

    // -----------------------------------------------------------------
    // the coercion-site mirror (`coerce.rs`). each adjustment lowering once
    // performed by mutating the tree is reproduced here: literal re-typing as
    // a stamp, and decay as an entry in the `adjustments` table that MIR reads
    // (S2C C4 - lowering no longer injects the cast node).
    // -----------------------------------------------------------------

    fn site_coerce(&mut self, expected: TypeRef, id: ExprId) {
        self.coerce_array_literal(expected, id);
        self.adopt_int_literal(expected, id);
        self.adopt_float_literal(expected, id);
        self.adopt_divergent(expected, id);
        self.record_decay(expected, id);
    }

    /// record an array-reference *decay* (`coerce.rs` `maybe_decay` mirror): a
    /// `&[T; N]` value meeting a `&T` / `string` expectation is read through a
    /// pointer cast. the decay site keeps its own `&[T; N]` type; only the read
    /// is adjusted, so MIR emits `(target)<value>` exactly as lowering's former
    /// cast node did. recorded after the literal mirrors so `ty_of` reads the
    /// pre-coercion type (a string literal stays `&[uint8; N]`, never re-typed).
    fn record_decay(&mut self, expected: TypeRef, id: ExprId) {
        if let Some(found) = self.ty_of(id)
            && array_ref_decays_to(expected, found, self.types)
        {
            self.results
                .adjustments
                .insert(id.into(), Adjustment::Decay(expected));
        }
    }

    /// a value-position divergent expression (`loop` with no `break value`, or
    /// `return`/`break`/`continue`) never produces a value: MIR lowers it as a
    /// statement and yields poison `0` in its place. it has no synthesized
    /// type, so adopt the expected type at the coercion site. this keeps
    /// `expr_types` complete - MIR types the poison temp from it, and without
    /// the stamp it falls back (A3: `void* /* ERROR TY */`, an invalid c
    /// return). the whole-corpus shadow oracle permits this walker-extra stamp;
    /// lowering left these untyped and leaned on the fallback.
    fn adopt_divergent(&mut self, expected: TypeRef, id: ExprId) {
        if self.ty_of(id).is_none()
            && matches!(
                self.body.exprs[id],
                Expr::Loop { .. } | Expr::Return(_) | Expr::Break | Expr::Continue
            )
        {
            self.record(id, expected);
        }
    }

    fn coerce_array_literal(&mut self, declared: TypeRef, id: ExprId) {
        let &TypeKind::Array {
            elem,
            len: declared_len,
        } = self.types.lookup(declared)
        else {
            return;
        };
        if !matches!(
            self.body.exprs[id],
            Expr::ArrayLit(_) | Expr::ArrayRepeat { .. }
        ) {
            return;
        }
        let Some(lit_len) = self.ty_of(id).and_then(|t| match self.types.lookup(t) {
            &TypeKind::Array { len, .. } => Some(len),
            _ => None,
        }) else {
            return;
        };
        if lit_len != declared_len {
            return;
        }
        self.record(id, declared);
        let children: Vec<ExprId> = match &self.body.exprs[id] {
            Expr::ArrayLit(elems) => elems.to_vec(),
            Expr::ArrayRepeat { value, .. } => vec![*value],
            _ => return,
        };
        for (i, child) in children.into_iter().enumerate() {
            self.site_coerce(elem, child);
            // L4 (S3): per-element value judgment against the declared element
            // type (`[1, true, "x"]` against `[int32; 3]`). runs after the
            // coercion so an adopted literal / decayed ref is not falsely
            // flagged; the same `site_assignable` the arg/field judgments use.
            if let Some(found) = self.ty_of(child)
                && !site_assignable(elem, found, self.types)
            {
                let expected = self.types.display(elem).to_string();
                let got = self.types.display(found).to_string();
                self.emit_at(
                    child,
                    None,
                    hir::core::TypeError::ArrayElementTypeMismatch {
                        index: i,
                        expected,
                        found: got,
                    },
                );
            }
        }
    }

    fn adopt_int_literal(&mut self, expected: TypeRef, id: ExprId) {
        match self.types.lookup(expected) {
            TypeKind::Path(name) if is_int_type_name(name) => {}
            _ => return,
        }
        match self.body.exprs[id] {
            Expr::Literal(Literal::Int(_)) => self.record(id, expected),
            Expr::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                if matches!(self.body.exprs[operand], Expr::Literal(Literal::Int(_))) {
                    self.record(operand, expected);
                    self.record(id, expected);
                }
            }
            _ => {}
        }
    }

    /// F3 (S3): a bare float literal defaults to `float64` (`literal_type`);
    /// against a `float32` expectation it adopts the narrower width, so the
    /// expression type agrees with its binding/slot instead of carrying a
    /// latent `float64` the C assignment silently narrows. the int-literal
    /// analogue of `adopt_int_literal`.
    fn adopt_float_literal(&mut self, expected: TypeRef, id: ExprId) {
        match self.types.lookup(expected) {
            TypeKind::Path(name) if is_float_type_name(name) => {}
            _ => return,
        }
        if matches!(self.body.exprs[id], Expr::Literal(Literal::Float(_))) {
            self.record(id, expected);
        }
    }

    // -----------------------------------------------------------------
    // type lookups (mirrors of `ctx.rs` / `lower/types.rs` helpers).
    // -----------------------------------------------------------------

    fn literal_type(&mut self, lit: &Literal) -> TypeRef {
        match lit {
            Literal::Int(_) => self.types.int32_ty(),
            Literal::Float(_) => self.types.intern(TypeKind::Path(Text::from("float64"))),
            Literal::String(s) => {
                let uint8 = self.types.uint8_ty();
                let arr = self.types.intern(TypeKind::Array {
                    elem: uint8,
                    len: hir::core::decode_string_literal(s).len() as u64,
                });
                self.types.intern(TypeKind::Ref(arr))
            }
            Literal::Bool(_) => self.types.intern(TypeKind::Path(Text::from("bool"))),
            Literal::Char(_) => self.types.intern(TypeKind::Path(Text::from("char"))),
        }
    }

    /// the human description of a `println` argument that has no `{}` rendering
    /// (an array, struct, or union), or `None` when it is formattable. drives
    /// printcannotformat (relocated from lowering, S2C C5).
    fn unformattable(&self, id: ExprId) -> Option<&'static str> {
        let ty = self.ty_of(id)?;
        match self.types.lookup(ty) {
            TypeKind::Array { .. } => Some("an array"),
            TypeKind::Path(name) if self.scope.items.structs.contains_key(name) => Some("a struct"),
            TypeKind::Path(name) if self.scope.items.unions.contains_key(name) => Some("a union"),
            _ => None,
        }
    }

    /// the declared type of `name`'s field `field`, struct or union.
    fn field_decl_type(&self, name: &Text, field: &Text) -> Option<TypeRef> {
        self.scope
            .items
            .structs
            .get(name)
            .and_then(|&sid| self.scope.structs[sid].field_index.get(field).copied())
            .or_else(|| {
                self.scope
                    .items
                    .unions
                    .get(name)
                    .and_then(|&uid| self.scope.unions[uid].field_index.get(field).copied())
            })
            .map(|fid| self.scope.fields[fid].ty)
    }

    /// field access type: struct/union member through auto-deref, error
    /// sentinel otherwise (mirrors `lookup_field_type`).
    fn lookup_field_type(&self, base_ty: TypeRef, field: &Text) -> TypeRef {
        // all arms are `&self` reads, so the `&TypeKind` borrow can stay live
        // across the recursive / `field_decl_type` calls - no `TypeKind` clone.
        match self.types.lookup(base_ty) {
            TypeKind::Path(name) => self
                .field_decl_type(name, field)
                .unwrap_or_else(|| self.types.error_type()),
            &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => self.lookup_field_type(inner, field),
            _ => self.types.error_type(),
        }
    }
}

/// the source spelling of a binary operator, for diagnostics.
fn bin_op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        Eq => "==",
        Neq => "!=",
        Lt => "<",
        Gt => ">",
        Leq => "<=",
        Geq => ">=",
        And => "&&",
        Or => "||",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

fn is_comparison(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq
            | BinOp::Neq
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::Leq
            | BinOp::Geq
            | BinOp::And
            | BinOp::Or
    )
}

fn is_int_type_name(n: &str) -> bool {
    int_type_range(n).is_some()
}

/// whether `ty` is an array, peeling one ref/ptr (so `&[T; N]` counts) - the
/// `len` / `.len` array-ness test, ported from lowering's `peeled_array_len`
/// (S2C C5).
fn peeled_array(ty: TypeRef, types: &TypeInterner) -> bool {
    match types.lookup(ty) {
        TypeKind::Array { .. } => true,
        &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => {
            matches!(types.lookup(inner), TypeKind::Array { .. })
        }
        _ => false,
    }
}

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

/// display text for a literal pattern, used in a domain-mismatch diagnostic.
/// moved with the match judgments from lowering (S2C C2).
fn literal_pat_text(lit: &Literal) -> Text {
    match lit {
        Literal::Int(v) => Text::from(v.to_string()),
        Literal::Char(c) => Text::from(format!("'{c}'")),
        Literal::Bool(b) => Text::from(if *b { "true" } else { "false" }),
        // float / string never reach a pattern (the parser excludes them).
        Literal::Float(s) | Literal::String(s) => Text::from(s.as_str()),
    }
}

/// the value range of a primitive integer type, as `(negative magnitude
/// bound, positive bound)`: a literal `n` must satisfy `n <= max`, a negated
/// literal `-N` must satisfy `N <= neg`. `usize`/`isize` use 64-bit ranges
/// (LP64 targets; a 32-bit target needs a target description). `None` for
/// any non-integer name. moved from lowering's coerce module (S2 step b).
fn int_type_range(name: &str) -> Option<(u128, u128)> {
    Some(match name {
        "int8" => (1 << 7, (1 << 7) - 1),
        "int16" => (1 << 15, (1 << 15) - 1),
        "int32" => (1 << 31, (1 << 31) - 1),
        "int64" | "isize" => (1 << 63, (1 << 63) - 1),
        "uint8" => (0, (1 << 8) - 1),
        "uint16" => (0, (1 << 16) - 1),
        "uint32" => (0, (1 << 32) - 1),
        "uint64" | "usize" => (0, u64::MAX as u128),
        _ => return None,
    })
}

/// a type tree containing an `Error` anywhere (poison absorber for
/// `types_compatible`).
struct ContainsError(bool);

impl VisitTypeRef for ContainsError {
    fn visit_ty(&mut self, ty: TypeRef, types: &TypeInterner) -> bool {
        let is_error = matches!(types.lookup(ty), TypeKind::Error);
        if is_error {
            self.0 = true;
        }
        !is_error
    }
}

fn type_ref_contains_error(ty: TypeRef, types: &TypeInterner) -> bool {
    let mut v = ContainsError(false);
    types.walk(ty, &mut v);
    v.0
}

/// compatibility test for value-position match-arm types (ported from
/// lowering, S2 step b). compatible when equal, when either side carries an
/// `Error` (no follow-on cascade), or when both are integer-family scalars.
/// PARITY(S3): the integer leniency exists because integer literals are all
/// `int32` today, so a wider explicit binding would otherwise reject literal
/// arms; it dies with literal adoption at the cutover.
fn types_compatible(a: TypeRef, b: TypeRef, types: &TypeInterner) -> bool {
    if type_ref_contains_error(a, types) || type_ref_contains_error(b, types) {
        return true;
    }
    if is_integer_path(a, types) && is_integer_path(b, types) {
        return true;
    }
    a == b
}

/// whether a `&[T; N]` (`found`) decays to `declared`: `declared` is `&T` / `T*`
/// with the same element type, or `string` (the byte-pointer view of a
/// `&[uint8; N]`). mirrors `coerce.rs::array_ref_decays_to`; the coercion sites
/// accept this pairing without a mismatch, and `record_decay` files the cast
/// MIR applies. directional (found -> declared) so it never relaxes a symmetric
/// equality and mask a real mismatch.
fn array_ref_decays_to(declared: TypeRef, found: TypeRef, types: &TypeInterner) -> bool {
    let &TypeKind::Ref(arr) = types.lookup(found) else {
        return false;
    };
    let &TypeKind::Array { elem, .. } = types.lookup(arr) else {
        return false;
    };
    match types.lookup(declared) {
        TypeKind::Path(n) if n == "string" => {
            matches!(types.lookup(elem), TypeKind::Path(e) if e == "uint8")
        }
        TypeKind::Ref(t) | TypeKind::Ptr(t) => *t == elem,
        _ => false,
    }
}

/// whether a value of type `found` is acceptable at a site expecting
/// `expected` - a call argument or a struct/union-literal field - after the
/// coercion-site adjustments (`site_coerce`) have run. accepts an
/// equal/integer-family-compatible type, the `&[T; N] -> &T` / `string` decay
/// (`record_decay` files the cast MIR applies), and any pointer-shaped value
/// widening into the untyped `ptr` (`void*` absorbs any pointer - the FFI
/// escape). `Error` on either side is silent via `types_compatible`. the
/// integer-family leniency defers the strict-width rule (M2b) until a corpus
/// program needs it, matching every other coercion site.
fn site_assignable(expected: TypeRef, found: TypeRef, types: &TypeInterner) -> bool {
    types_compatible(found, expected, types)
        || array_ref_decays_to(expected, found, types)
        || (matches!(types.lookup(expected), TypeKind::RawPtr) && is_pointer_shaped(found, types))
}

/// whether `ty` is a pointer-shaped value (a typed reference/pointer, or the
/// untyped `ptr`): the values that widen into `ptr` without an explicit cast.
fn is_pointer_shaped(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::RawPtr
    )
}

/// the name of an unsigned integer type, or `None` for anything else - the F2
/// test for `-` rejection.
fn unsigned_int_name(ty: TypeRef, types: &TypeInterner) -> Option<Text> {
    match types.lookup(ty) {
        TypeKind::Path(name)
            if matches!(
                name.as_str(),
                "uint8" | "uint16" | "uint32" | "uint64" | "usize"
            ) =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

/// whether `name` is a float type (the F3 adoption test).
fn is_float_type_name(n: &str) -> bool {
    matches!(n, "float32" | "float64")
}

/// the cast-lattice class of a type (S3, CAST.md). the ratified `as` ruling is
/// directional - `char`/`bool`/`enum` widen OUT to an integer but cannot be
/// fabricated IN - so each scalar keeps its own class rather than collapsing to
/// one "scalar". `Unknown` (an `Error` or an unresolved type name - a type
/// parameter the floor cannot resolve) stays lenient.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CastClass {
    Int,
    Float,
    Bool,
    Char,
    Enum,
    Pointer,
    Aggregate,
    Fn,
    Unknown,
}

fn cast_class(ty: TypeRef, scope: &HIR, types: &TypeInterner) -> CastClass {
    match types.lookup(ty) {
        TypeKind::Error => CastClass::Unknown,
        TypeKind::Array { .. } => CastClass::Aggregate,
        TypeKind::Fn { .. } => CastClass::Fn,
        TypeKind::Ref(_) | TypeKind::Ptr(_) | TypeKind::RawPtr => CastClass::Pointer,
        TypeKind::Path(name) => {
            if is_int_type_name(name) {
                CastClass::Int
            } else if is_float_type_name(name) {
                CastClass::Float
            } else if name == "bool" {
                CastClass::Bool
            } else if name == "char" {
                CastClass::Char
            } else if scope.items.enums.contains_key(name) {
                CastClass::Enum
            } else if scope.items.structs.contains_key(name)
                || scope.items.unions.contains_key(name)
            {
                CastClass::Aggregate
            } else {
                CastClass::Unknown
            }
        }
    }
}

/// whether an `as` cast from `from` to `to` is in the cast lattice (CAST.md).
/// the allowed directed pairs are listed explicitly; everything else rejects.
/// an `Unknown` side is lenient (no cascade).
fn cast_allowed(from: TypeRef, to: TypeRef, scope: &HIR, types: &TypeInterner) -> bool {
    use CastClass::*;
    match (cast_class(from, scope, types), cast_class(to, scope, types)) {
        (Unknown, _) | (_, Unknown) => true,
        // numeric <-> numeric.
        (Int, Int) | (Int, Float) | (Float, Int) | (Float, Float) => true,
        // pointer puns and the integer<->pointer bridge.
        (Pointer, Pointer) | (Int, Pointer) | (Pointer, Int) => true,
        // the tagged scalars widen OUT to an integer, never the reverse.
        (Char, Int) | (Bool, Int) | (Enum, Int) => true,
        // everything else - `_ -> bool`/`_ -> char`, `int -> enum`,
        // float<->pointer, any aggregate/fn - is rejected.
        _ => false,
    }
}

fn is_integer_path(ty: TypeRef, types: &TypeInterner) -> bool {
    matches!(
        types.lookup(ty),
        TypeKind::Path(name)
            if matches!(
                name.as_str(),
                "int8" | "int16" | "int32" | "int64"
                    | "uint8" | "uint16" | "uint32" | "uint64"
                    | "usize" | "isize"
            )
    )
}
