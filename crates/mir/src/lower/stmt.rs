//! statement-position lowering: `lower_stmt` and its `let` destructure, the
//! statement-position expression form (`lower_expr_stmt`), block lowering, and
//! `return`. also the match-arm machinery (`arm_kind` / `lower_match_arms` /
//! `lower_guard` / the arm-body + binding-arm helpers), shared by the statement
//! position here and the value position in [`super::expr`].

use ast::{AssignOp, BinOp};
use hir::core::{BlockId, Expr, ExprId, LocalId as HirLocalId, MatchArm, Pat, PatId, Stmt, Text};
use smallvec::SmallVec;
use thin_vec::ThinVec;

use super::{ArmKind, Lower};
use crate::core::*;

impl<'a> Lower<'a> {
    pub(crate) fn lower_stmt(&mut self, stmt: &Stmt, buf: &mut ThinVec<MirStmt>) {
        match stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                mutable,
            } => {
                // struct destructure (`let Point { x, y } = p`, HORIZON0 C4 / S2):
                // expand into one field-projection `Let` per binding. the source
                // struct value is spilled to a place; each binding reads
                // `base.field`.
                if let Pat::Struct { fields, .. } = &self.body.pats[*pat] {
                    let fields: Vec<(Text, HirLocalId)> =
                        fields.iter().map(|b| (b.field.clone(), b.local)).collect();
                    self.lower_let_destructure(fields, *init, *mutable, buf);
                    return;
                }
                let body = self.body;
                let hid = match &body.pats[*pat] {
                    Pat::Bind(id) => *id,
                    // only bind comes from let-pat lowering; anything else is
                    // broken syntax already diagnosed upstream.
                    _ => return,
                };
                // a directly diverging initializer (`let x = return e;`,
                // `= break`, `= continue`) produces no value: lower the jump and
                // drop the binding. the jump terminates the block, so the block
                // builder skips the (now dead) binding and everything after it -
                // including any later reference to `x`, which is unreachable.
                if let Some(e) = init
                    && matches!(
                        body.exprs[*e],
                        Expr::Return(_) | Expr::Break | Expr::Continue
                    )
                {
                    self.lower_expr_stmt(*e, buf);
                    return;
                }
                let local = &body.locals[hid];
                // an untyped `let x = init` carries no type in the HIR arena;
                // typeck inferred it from the initializer (let-from-init) and
                // recorded it in `local_types`. fall back to it, as `bind_local_to`
                // does for match-arm bindings.
                let lty = ty
                    .as_ref()
                    .or(local.ty.as_ref())
                    .cloned()
                    .or_else(|| self.typeck.local_types.get(&hid).copied())
                    .unwrap_or_else(|| self.types.error_type());
                let name = Some(local.name.clone());
                // lower the initializer before the binding is in scope: a `let`
                // cannot reference itself, and any temps the init needs must be
                // emitted ahead of the declaration. a value-position `if`/`match`
                // is not an rvalue (it is control flow); route it through
                // `lower_operand`, which spills it into its own temp and assigns
                // that temp in each branch, so the binding stays a plain
                // (`const`-able) copy of the result.
                let init_rv = init.map(|e| {
                    if self.is_value_control_flow(e) {
                        RValue::Use(self.lower_operand(e, buf))
                    } else {
                        self.lower_rvalue(e, buf)
                    }
                });
                let mid = self.locals.alloc(MirLocal {
                    ty: lty,
                    name,
                    mutable: *mutable,
                });
                self.local_map[hid.raw_idx().into_u32() as usize] = Some(mid);
                buf.push(MirStmt::Let {
                    local: mid,
                    init: init_rv,
                });
            }
            Stmt::Expr(e) => self.lower_expr_stmt(*e, buf),
            // a block-scope `const` is purely compile-time: its folded value is
            // inlined at every reference, so the declaration emits nothing.
            Stmt::Const(_) => {}
        }
    }

    /// expand a `let Point { x, y } = init` destructure: spill `init` to a place,
    /// then emit one `Let` per field binding reading `base.field`. `fields` is
    /// `(source field name, HIR binding local)` in source order.
    fn lower_let_destructure(
        &mut self,
        fields: Vec<(Text, HirLocalId)>,
        init: Option<ExprId>,
        mutable: bool,
        buf: &mut ThinVec<MirStmt>,
    ) {
        // the grammar requires `= value`, so init is always present; a missing
        // one means broken syntax already diagnosed - drop the bindings.
        let Some(init) = init else { return };
        let base = self.place_for_value(init, buf);
        for (field, hid) in fields {
            let local = &self.body.locals[hid];
            let ty = local.ty.unwrap_or_else(|| self.types.error_type());
            let name = Some(local.name.clone());
            let mid = self.locals.alloc(MirLocal { ty, name, mutable });
            self.local_map[hid.raw_idx().into_u32() as usize] = Some(mid);
            let proj = Place::Field(Box::new(base.clone()), field);
            buf.push(MirStmt::Let {
                local: mid,
                init: Some(RValue::Use(Operand::Copy(proj))),
            });
        }
    }

    /// lower an expression in statement (discarded-value) position. a
    /// control-flow expression becomes its MIR statement form with no temp;
    /// everything else is evaluated for effect.
    pub(crate) fn lower_expr_stmt(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) {
        let body = self.body;
        match &body.exprs[e] {
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                let cond = self.lower_operand(cond, buf);
                let then_block = self.lower_block(then_branch);
                let else_block = else_branch.map(|b| self.lower_block(b));
                buf.push(MirStmt::If {
                    cond,
                    then_block,
                    else_block,
                });
            }
            Expr::Loop { body: block } => {
                let block = *block;
                let body_block = self.lower_block(block);
                buf.push(MirStmt::Loop { body: body_block });
            }
            Expr::Match { scrut, arms } => {
                let scrut_expr = *scrut;
                let scrut = self.lower_operand(scrut_expr, buf);
                let (arms_out, default) = self.lower_match_arms(
                    &scrut,
                    arms,
                    Lower::lower_arm_body,
                    Lower::lower_binding_arm_body,
                );
                buf.push(MirStmt::Switch {
                    scrut,
                    arms: arms_out,
                    default,
                });
            }
            Expr::Break => buf.push(MirStmt::Break),
            Expr::Continue => buf.push(MirStmt::Continue),
            Expr::Return(value) => self.lower_return(*value, buf),
            // discarded `a && b;` / `a || b;` in statement position. lower both
            // sub-expressions with short-circuit control flow (same shape as the
            // value-position lowering in `lower_into`). the result is written to
            // an unread temp and discarded.
            Expr::Binary {
                op: op @ (BinOp::And | BinOp::Or),
                lhs,
                rhs,
            } => {
                let (is_and, lhs, rhs) = (matches!(op, BinOp::And), *lhs, *rhs);
                let ty = self.mir_type_of(e);
                let mid = self.locals.alloc(MirLocal {
                    ty,
                    name: None,
                    mutable: true,
                });
                let place = Place::Local(mid);
                buf.push(MirStmt::Let {
                    local: mid,
                    init: None,
                });
                self.lower_into(lhs, &place, buf);
                let cond = Operand::Copy(place.clone());
                let rhs_block = self.lower_into_block(rhs, &place);
                let (then_block, else_block) = if is_and {
                    (rhs_block, None)
                } else {
                    (MirBlock::default(), Some(rhs_block))
                };
                buf.push(MirStmt::If {
                    cond,
                    then_block,
                    else_block,
                });
            }
            Expr::Assign { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                let place = self.lower_place(lhs, buf);
                match op {
                    // a plain `place = <value if/match/&&/||>`: the rhs is
                    // control flow, not an rvalue. lower it directly into the
                    // target so each branch assigns `place` (same in-place
                    // lowering as a value `let`, REDESIGN I3); no temp needed.
                    AssignOp::Assign if self.is_value_control_flow(rhs) => {
                        self.lower_into(rhs, &place, buf);
                    }
                    AssignOp::Assign => {
                        let value = self.lower_rvalue(rhs, buf);
                        buf.push(MirStmt::Assign { place, value });
                    }
                    // a compound assignment (`a += b`, `a <<= b`, ...) desugars
                    // to `a = a <op> b`. the place is re-read as the left
                    // operand; it is a local today, so the re-read is
                    // side-effect-free. `to_bin_op` is `Some` for every arm here
                    // (the plain `=` is handled above).
                    op => {
                        let bin = op.to_bin_op().expect("compound assignment has a binary op");
                        let rhs_op = self.lower_operand(rhs, buf);
                        let value = RValue::Binary(bin, Operand::Copy(place.clone()), rhs_op);
                        buf.push(MirStmt::Assign { place, value });
                    }
                }
            }
            _ => {
                let rv = self.lower_rvalue(e, buf);
                buf.push(MirStmt::Eval(rv));
            }
        }
    }

    /// classify a match arm pattern. reads only the borrowed body so the result
    /// can outlive the borrow while arm bodies are lowered.
    fn arm_kind(&self, pat: PatId) -> ArmKind {
        match &self.body.pats[pat] {
            Pat::Variant { enum_id, idx } => ArmKind::Variant(VariantRef {
                enum_id: *enum_id,
                idx: *idx,
            }),
            Pat::Literal(lit) => ArmKind::Const(lit.clone()),
            Pat::Wildcard => ArmKind::Default,
            Pat::Bind(id) => ArmKind::Bind(*id),
            // struct patterns in match arms are S3 (with guards); the parser
            // rejects them (`GrammarError::StructPatInMatchArm`) so a `Pat::Struct`
            // never reaches arm classification here. `Pat::Missing` (a failed or
            // rejected arm pattern) produces no MIR.
            Pat::Missing | Pat::Struct { .. } => ArmKind::Skip,
        }
    }

    /// lower a HIR block in statement position into its own [`MirBlock`]. its
    /// tail value is discarded (lowered for effect); a value-producing block is
    /// later-segment work.
    pub(crate) fn lower_block(&mut self, block_id: BlockId) -> MirBlock {
        let body = self.body;
        let block = &body.blocks[block_id];
        let mut buf = ThinVec::with_capacity(block.stmts.len() + usize::from(block.tail.is_some()));
        for &sid in &block.stmts {
            self.lower_stmt(&body.stmts[sid], &mut buf);
            if Self::terminated(&buf) {
                return MirBlock { stmts: buf };
            }
        }
        if let Some(tail) = block.tail {
            self.lower_expr_stmt(tail, &mut buf);
        }
        MirBlock { stmts: buf }
    }

    /// lower a match arm body. statement-position match: the arm value is
    /// discarded, so the body lowers for effect.
    fn lower_arm_body(&mut self, body_expr: ExprId) -> MirBlock {
        self.lower_arm_body_impl(body_expr, Lower::lower_expr_stmt)
    }

    /// bind the local `hid` to the scrutinee, then lower the arm body (statement
    /// position). the binding `let` is prepended so the body sees it - the
    /// statement form of the arm-binding seam.
    fn lower_binding_arm_body(
        &mut self,
        hid: HirLocalId,
        scrut: &Operand,
        body_expr: ExprId,
    ) -> MirBlock {
        self.lower_binding_arm_body_impl(hid, scrut, body_expr, Lower::lower_expr_stmt)
    }

    /// value-position variant of [`Lower::lower_binding_arm_body`]: bind, then
    /// lower the arm body into `target`.
    pub(crate) fn lower_binding_arm_body_into(
        &mut self,
        hid: HirLocalId,
        scrut: &Operand,
        body_expr: ExprId,
        target: &Place,
    ) -> MirBlock {
        self.lower_binding_arm_body_impl(hid, scrut, body_expr, |s, e, buf| {
            s.lower_into(e, target, buf)
        })
    }

    fn lower_binding_arm_body_impl(
        &mut self,
        hid: HirLocalId,
        scrut: &Operand,
        body_expr: ExprId,
        lower: impl FnOnce(&mut Self, ExprId, &mut ThinVec<MirStmt>),
    ) -> MirBlock {
        let mut buf = ThinVec::new();
        self.bind_local_to(hid, scrut, &mut buf);
        lower(self, body_expr, &mut buf);
        MirBlock { stmts: buf }
    }

    /// emit `let <hid> = <scrut>`, allocating the MIR local for the HIR binding
    /// and recording the mapping so later references resolve.
    fn bind_local_to(&mut self, hid: HirLocalId, scrut: &Operand, buf: &mut ThinVec<MirStmt>) {
        let local = &self.body.locals[hid];
        let declared = local.ty;
        let name = Some(local.name.clone());
        // a match-arm binding (`x -> ..`) is untyped in the HIR arena: lowering
        // no longer knows the scrutinee type (S2C C2). typeck recorded it (the
        // scrutinee's type) in `local_types`; fall back to it here.
        let ty = declared
            .or_else(|| self.typeck.local_types.get(&hid).copied())
            .unwrap_or_else(|| self.types.error_type());
        let mid = self.locals.alloc(MirLocal {
            ty,
            name,
            mutable: false,
        });
        self.local_map[hid.raw_idx().into_u32() as usize] = Some(mid);
        buf.push(MirStmt::Let {
            local: mid,
            init: Some(RValue::Use(scrut.clone())),
        });
    }

    /// lower a `return expr?;` to a [`MirStmt::Return`]. shared by the statement
    /// and value positions; in value position the enclosing target temp is left
    /// unassigned because a return diverges.
    pub(crate) fn lower_return(&mut self, value: Option<ExprId>, buf: &mut ThinVec<MirStmt>) {
        let op = value.map(|v| self.lower_operand(v, buf));
        buf.push(MirStmt::Return(op));
    }

    /// lower a value-position match arm body, assigning its value into `target`.
    pub(crate) fn lower_arm_body_into(&mut self, body_expr: ExprId, target: &Place) -> MirBlock {
        self.lower_arm_body_impl(body_expr, |s, e, buf| s.lower_into(e, target, buf))
    }

    fn lower_arm_body_impl(
        &mut self,
        body_expr: ExprId,
        lower: impl FnOnce(&mut Self, ExprId, &mut ThinVec<MirStmt>),
    ) -> MirBlock {
        let mut buf = ThinVec::new();
        lower(self, body_expr, &mut buf);
        MirBlock { stmts: buf }
    }

    pub(crate) fn lower_match_arms(
        &mut self,
        scrut: &Operand,
        arms: &[MatchArm],
        lower_arm: impl Fn(&mut Self, ExprId) -> MirBlock,
        lower_binding_arm: impl Fn(&mut Self, HirLocalId, &Operand, ExprId) -> MirBlock,
    ) -> (ThinVec<SwitchArm>, Option<MirBlock>) {
        // include guard exprid in arm data.
        let arm_data: SmallVec<[(ArmKind, Option<ExprId>, ExprId); 4]> = arms
            .iter()
            .map(|arm| (self.arm_kind(arm.pat), arm.guard, arm.body))
            .collect();
        let mut arms_out = ThinVec::with_capacity(arms.len());
        let mut default = None;
        for (kind, guard, arm_body) in arm_data {
            match kind {
                ArmKind::Variant(variant) => arms_out.push(SwitchArm {
                    test: ArmTest::Variant(variant),
                    guard: self.lower_guard(guard),
                    body: lower_arm(self, arm_body),
                }),
                ArmKind::Const(lit) => arms_out.push(SwitchArm {
                    test: ArmTest::Const(lit),
                    guard: self.lower_guard(guard),
                    body: lower_arm(self, arm_body),
                }),
                // a guarded catch-all (`x if c` / `_ if c`) cannot use the
                // `default` slot: a false guard must fall through. it becomes an
                // ordered `Always` arm so the flag chain re-checks the next arm.
                // for a binding the local is bound as the FIRST guard statement so
                // both the guard cond and the body see it - and crucially before
                // `lower_operand(guard)`, so the guard's reference resolves to this
                // local instead of materializing a fresh one. an UNGUARDED
                // catch-all stays the unconditional `default`.
                ArmKind::Bind(hid) => match guard {
                    Some(g) => {
                        let mut stmts = ThinVec::new();
                        self.bind_local_to(hid, scrut, &mut stmts);
                        let cond = self.lower_operand(g, &mut stmts);
                        arms_out.push(SwitchArm {
                            test: ArmTest::Always,
                            guard: Some(Guard { stmts, cond }),
                            body: lower_arm(self, arm_body),
                        });
                    }
                    None => default = Some(lower_binding_arm(self, hid, scrut, arm_body)),
                },
                ArmKind::Default => match self.lower_guard(guard) {
                    Some(guard) => arms_out.push(SwitchArm {
                        test: ArmTest::Always,
                        guard: Some(guard),
                        body: lower_arm(self, arm_body),
                    }),
                    None => default = Some(lower_arm(self, arm_body)),
                },
                ArmKind::Skip => {}
            }
        }
        (arms_out, default)
    }

    /// lower an optional guard into its prerequisite temps + a final boolean. the
    /// temps are kept (not anded into the test, not folded into the body), so
    /// codegen can place them inside the matched block and fall through to the
    /// next arm when the guard is false (`gen_switch` flag chain). for a binding
    /// catch-all the local must be bound before this runs, so that path builds the
    /// `Guard` directly rather than calling here.
    fn lower_guard(&mut self, guard: Option<ExprId>) -> Option<Guard> {
        guard.map(|g| {
            let mut stmts = ThinVec::new();
            let cond = self.lower_operand(g, &mut stmts);
            Guard { stmts, cond }
        })
    }
}
