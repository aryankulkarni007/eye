//! HIR -> MIR lowering.
//!
//! A builder over a finished HIR [`Body`]. It linearizes value-producing
//! expressions into three-address form and (in later segments) flattens control
//! flow. The output is total: every well-typed HIR body lowers to valid MIR
//! without rejecting or emitting diagnostics (REDESIGN I2).
//!
//! Status: incremental. Covers straight-line bodies (Segment 1), statement-
//! position control flow (Segment 2: `if`/`loop`/`break`/`continue`/`return`/
//! statement-`match`/assign), value-position control flow plus general calls
//! (Segment 3: a value `if`/`match` lowered in place via a typed temp - the I3
//! acid test - and a direct `Call`), and the full expression surface (Segment 4:
//! `Unary`, `Index`, `Field`, `ArrayLit`, `StructLit`, `Ref`/`Deref`, `Cast`,
//! place projections, and the `&&`/`||` short-circuit rewrite). A bare
//! value-position block still hits a `todo!()` (it needs scope-flattening). A
//! name in value position that does not denote a value (an undeclared name, a
//! struct/function name) is rejected in HIR before MIR runs, so its `Path` is
//! `unreachable!` here, not lowered - see `docs/DEFER.md`.

use ast::{AssignOp, BinOp};
use hir::core::{
    BlockId, Body, Expr, ExprId, HIR, LocalId as HirLocalId, Pat, PatId, Resolution, Stmt, Text,
};
use la_arena::Arena;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use thin_vec::ThinVec;

use crate::core::*;

/// Lower one function body to a [`MirBody`]. `param_count` is the number of
/// leading [`Body`] locals that are parameters (HIR allocates them first, before
/// any block local); they are pre-created as MIR locals so references resolve
/// and so the emitter can skip declaring them (the signature already does).
pub fn lower_function(hir: &HIR, body: &Body, param_count: usize, ret: Option<Type>) -> MirBody {
    let mut cx = Lower::new(hir, body, ret);
    cx.lower_params(param_count);
    let block = cx.lower_top_block();
    MirBody {
        locals: cx.locals,
        params: cx.params,
        body: block,
    }
}

struct Lower<'a> {
    body: &'a Body,
    /// The function's declared return type, used by [`Lower::lower_tail`] to
    /// decide whether the body tail is a returned value or a discarded effect.
    /// `None` for a void function (and for `main`, where the caller passes
    /// `None` so the tail is discarded and the emitter supplies `return 0`).
    ret: Option<Type>,
    locals: Arena<MirLocal>,
    params: ThinVec<LocalId>,
    local_map: FxHashMap<HirLocalId, LocalId>,
}

/// How a match arm pattern dispatches, lifted off the borrowed [`Body`] before
/// lowering each arm body (which mutably borrows `self`).
enum ArmKind {
    Variant(VariantRef),
    Default,
    /// `Bind`/`Missing` in an arm: broken lowering already diagnosed upstream;
    /// the arm is dropped.
    Skip,
}

impl<'a> Lower<'a> {
    fn new(_hir: &'a HIR, body: &'a Body, ret: Option<Type>) -> Self {
        Self {
            body,
            ret,
            locals: Arena::new(),
            params: ThinVec::new(),
            local_map: FxHashMap::default(),
        }
    }

    fn lower_params(&mut self, param_count: usize) {
        let body = self.body;
        for (hid, l) in body.locals.iter().take(param_count) {
            let ty = l.ty.clone().unwrap_or(Type::Error);
            let name = Some(l.name.clone());
            let mutable = l.mutable;
            let mid = self.locals.alloc(MirLocal { ty, name, mutable });
            self.local_map.insert(hid, mid);
            self.params.push(mid);
        }
    }

    fn lower_top_block(&mut self) -> MirBlock {
        let body = self.body;
        let mut buf = ThinVec::with_capacity(body.block.len() + usize::from(body.tail.is_some()));
        for &sid in &body.block {
            self.lower_stmt(&body.stmts[sid], &mut buf);
        }
        if let Some(tail) = body.tail {
            self.lower_tail(tail, &mut buf);
        }
        MirBlock { stmts: buf }
    }

    /// Lower a function body's tail. With a declared return type the tail is the
    /// implicit return value; otherwise (void fn / `main`) its value is
    /// discarded and it lowers for effect.
    fn lower_tail(&mut self, tail: ExprId, buf: &mut ThinVec<MirStmt>) {
        if self.ret.is_some() {
            let op = self.lower_operand(tail, buf);
            buf.push(MirStmt::Return(Some(op)));
        } else {
            self.lower_expr_stmt(tail, buf);
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt, buf: &mut ThinVec<MirStmt>) {
        match stmt {
            Stmt::Let {
                pat,
                ty,
                init,
                mutable,
            } => {
                let body = self.body;
                let hid = match &body.pats[*pat] {
                    Pat::Bind(id) => *id,
                    // Only Bind comes from let-pat lowering; anything else is
                    // broken syntax already diagnosed upstream.
                    _ => return,
                };
                let local = &body.locals[hid];
                let lty = ty
                    .clone()
                    .or_else(|| local.ty.clone())
                    .unwrap_or(Type::Error);
                let name = Some(local.name.clone());
                // Lower the initializer before the binding is in scope: a `let`
                // cannot reference itself, and any temps the init needs must be
                // emitted ahead of the declaration. A value-position `if`/`match`
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
                self.local_map.insert(hid, mid);
                buf.push(MirStmt::Let {
                    local: mid,
                    init: init_rv,
                });
            }
            Stmt::Expr(e) => self.lower_expr_stmt(*e, buf),
        }
    }

    /// Lower an expression in statement (discarded-value) position. A
    /// control-flow expression becomes its MIR statement form with no temp;
    /// everything else is evaluated for effect.
    fn lower_expr_stmt(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) {
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
                let scrut = *scrut;
                // Lift arm dispatch info off the borrowed body first; lowering an
                // arm body mutably borrows `self`.
                let arm_data: SmallVec<[(ArmKind, ExprId); 4]> = arms
                    .iter()
                    .map(|arm| (self.arm_kind(arm.pat), arm.body))
                    .collect();
                let scrut = self.lower_operand(scrut, buf);
                let mut arms_out = ThinVec::with_capacity(arms.len());
                let mut default = None;
                for (kind, arm_body) in arm_data {
                    match kind {
                        ArmKind::Variant(variant) => arms_out.push(SwitchArm {
                            variant,
                            body: self.lower_arm_body(arm_body),
                        }),
                        ArmKind::Default => default = Some(self.lower_arm_body(arm_body)),
                        ArmKind::Skip => {}
                    }
                }
                buf.push(MirStmt::Switch {
                    scrut,
                    arms: arms_out,
                    default,
                });
            }
            Expr::Break => buf.push(MirStmt::Break),
            Expr::Continue => buf.push(MirStmt::Continue),
            Expr::Assign { op, lhs, rhs } => {
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                let place = self.lower_place(lhs, buf);
                match op {
                    // A plain `place = <value if/match/&&/||>`: the rhs is
                    // control flow, not an rvalue. Lower it directly into the
                    // target so each branch assigns `place` (same in-place
                    // lowering as a value `let`, REDESIGN I3); no temp needed.
                    AssignOp::Assign if self.is_value_control_flow(rhs) => {
                        self.lower_into(rhs, &place, buf);
                    }
                    AssignOp::Assign => {
                        let value = self.lower_rvalue(rhs, buf);
                        buf.push(MirStmt::Assign { place, value });
                    }
                    // `a += b` / `a -= b` desugar to `a = a <op> b`. The place is
                    // re-read as the left operand; it is a local today, so the
                    // re-read is side-effect-free.
                    AssignOp::AddAssign | AssignOp::SubAssign => {
                        let bin = if matches!(op, AssignOp::AddAssign) {
                            BinOp::Add
                        } else {
                            BinOp::Sub
                        };
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

    /// Classify a match arm pattern. Reads only the borrowed body so the result
    /// can outlive the borrow while arm bodies are lowered.
    fn arm_kind(&self, pat: PatId) -> ArmKind {
        match &self.body.pats[pat] {
            Pat::Variant { enum_id, idx } => ArmKind::Variant(VariantRef {
                enum_id: *enum_id,
                idx: *idx,
            }),
            Pat::Wildcard => ArmKind::Default,
            Pat::Bind(_) | Pat::Missing => ArmKind::Skip,
        }
    }

    /// Lower a HIR block in statement position into its own [`MirBlock`]. Its
    /// tail value is discarded (lowered for effect); a value-producing block is
    /// later-segment work.
    fn lower_block(&mut self, block_id: BlockId) -> MirBlock {
        let body = self.body;
        let block = &body.blocks[block_id];
        let mut buf = ThinVec::with_capacity(block.stmts.len() + usize::from(block.tail.is_some()));
        for &sid in &block.stmts {
            self.lower_stmt(&body.stmts[sid], &mut buf);
        }
        if let Some(tail) = block.tail {
            self.lower_expr_stmt(tail, &mut buf);
        }
        MirBlock { stmts: buf }
    }

    /// Lower a match arm body. Statement-position match: the arm value is
    /// discarded, so the body lowers for effect.
    fn lower_arm_body(&mut self, body_expr: ExprId) -> MirBlock {
        let mut buf = ThinVec::new();
        self.lower_expr_stmt(body_expr, &mut buf);
        MirBlock { stmts: buf }
    }

    /// Lower a place expression (`a`, `a[i]`, `s.f`, `*p`) to a [`Place`],
    /// emitting any prerequisite temps into `buf`. Used for an assign target,
    /// the operand of `&`, and reading a projection as a trivial operand. A
    /// non-trivial index spills to a temp; a base that is not itself a place (a
    /// call or literal returning an aggregate) is evaluated into a temp and
    /// projected from that local, preserving evaluation order.
    fn lower_place(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Place {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Path(Resolution::Local(hid)) => Place::Local(self.map_local(*hid)),
            Expr::Index { base, index } => {
                let (base, index) = (*base, *index);
                let base_place = self.lower_place(base, buf);
                let idx = self.lower_operand(index, buf);
                Place::Index(Box::new(base_place), Box::new(idx))
            }
            Expr::Field { base, name } => {
                let (base, name) = (*base, name.clone());
                let base_place = self.lower_place(base, buf);
                Place::Field(Box::new(base_place), name)
            }
            Expr::Deref { operand } => {
                let base_place = self.lower_place(*operand, buf);
                Place::Deref(Box::new(base_place))
            }
            // The base is not a place (e.g. a call returning an aggregate):
            // evaluate it into a temp and treat that local as the place.
            _ => {
                let rv = self.lower_rvalue(e, buf);
                let ty = self.mir_type_of(e);
                let mid = self.locals.alloc(MirLocal {
                    ty,
                    name: None,
                    mutable: true,
                });
                buf.push(MirStmt::Let {
                    local: mid,
                    init: Some(rv),
                });
                Place::Local(mid)
            }
        }
    }

    /// Lower an expression to an [`RValue`], emitting any prerequisite temps
    /// into `buf`. Used where an rvalue is wanted directly (a `let` init, a
    /// discarded effect).
    fn lower_rvalue(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> RValue {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Literal(lit) => RValue::Use(Operand::Const(lit.clone())),
            // A `Path` in value position. HIR rejects every non-value resolution
            // (a type name, a bare function, an unresolved name) before MIR runs,
            // so only the two value resolutions are reachable; the rest are
            // checked-`unreachable!` (I2). Exhaustive so a new `Resolution`
            // variant must declare its value-ness.
            Expr::Path(res) => match res {
                Resolution::Local(hid) => {
                    RValue::Use(Operand::Copy(Place::Local(self.map_local(*hid))))
                }
                Resolution::Variant { enum_id, idx } => RValue::Variant(VariantRef {
                    enum_id: *enum_id,
                    idx: *idx,
                }),
                Resolution::Enum(_)
                | Resolution::Struct(_)
                | Resolution::Fn(_)
                | Resolution::Unresolved(_) => {
                    unreachable!("non-value Path in rvalue position (rejected in HIR)")
                }
            },
            Expr::Binary { op, lhs, rhs } => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    // I5: a value-position `&&`/`||` is intercepted upstream
                    // (`is_value_control_flow` -> `lower_operand`/`lower_into`)
                    // and lowered to control flow, so it never reaches here.
                    // Only a discarded statement-position `a && b;` would, which
                    // no program writes; it is left unbuilt rather than lowered
                    // eagerly (which would defeat short-circuiting).
                    // FIXME: HIR accepts discarded `&&`/`||`; lower it as
                    // short-circuit control flow instead of panicking.
                    todo!("MIR lowering: discarded statement-position && / ||");
                }
                let (op, lhs, rhs) = (*op, *lhs, *rhs);
                let l = self.lower_operand(lhs, buf);
                let r = self.lower_operand(rhs, buf);
                RValue::Binary(op, l, r)
            }
            Expr::Unary { op, operand } => {
                let (op, operand) = (*op, *operand);
                let o = self.lower_operand(operand, buf);
                RValue::Unary(op, o)
            }
            // Reading a place projection in value position: `Use` of the place.
            Expr::Index { .. } | Expr::Field { .. } | Expr::Deref { .. } => {
                RValue::Use(Operand::Copy(self.lower_place(e, buf)))
            }
            Expr::Ref { operand } => RValue::Ref(self.lower_place(*operand, buf)),
            Expr::Cast { operand, ty } => {
                let (operand, ty) = (*operand, ty.clone());
                let o = self.lower_operand(operand, buf);
                RValue::Cast(o, ty)
            }
            Expr::ArrayLit(elems) => {
                let elem_ids: ThinVec<ExprId> = elems.clone();
                let ty = self.mir_type_of(e);
                let mut lowered = ThinVec::with_capacity(elem_ids.len());
                lowered.extend(elem_ids.into_iter().map(|el| self.lower_operand(el, buf)));
                RValue::ArrayLit { ty, elems: lowered }
            }
            Expr::StructLit { ty, fields } => {
                let ty = ty.clone();
                let field_data: SmallVec<[(Text, ExprId); 4]> =
                    fields.iter().map(|f| (f.name.clone(), f.value)).collect();
                let mut lowered = ThinVec::with_capacity(field_data.len());
                lowered.extend(
                    field_data
                        .into_iter()
                        .map(|(name, value)| (name, self.lower_operand(value, buf))),
                );
                RValue::StructLit {
                    ty,
                    fields: lowered,
                }
            }
            Expr::Call { callee, args } => {
                let callee = *callee;
                let arg_ids: ThinVec<ExprId> = args.clone();
                match &body.exprs[callee] {
                    // The `print` intrinsic is sniffed by its unresolved callee
                    // name and carried as a dedicated node.
                    Expr::Path(Resolution::Unresolved(name)) if name == "print" => {
                        let mut lowered = ThinVec::with_capacity(arg_ids.len());
                        lowered.extend(arg_ids.into_iter().map(|a| self.lower_operand(a, buf)));
                        RValue::Print { args: lowered }
                    }
                    // A direct call to a named function (defined or `extern`).
                    Expr::Path(Resolution::Fn(fid)) => {
                        let func = *fid;
                        let mut lowered = ThinVec::with_capacity(arg_ids.len());
                        lowered.extend(arg_ids.into_iter().map(|a| self.lower_operand(a, buf)));
                        RValue::Call {
                            func,
                            args: lowered,
                        }
                    }
                    // An unresolved callee - a call to an undeclared name. HIR
                    // lowering rejects this as `ResolveError::UnresolvedName`
                    // (the callee path lowers to `Expr::Missing`), so a body
                    // reaching MIR never carries one (I2: the emitter trusts
                    // upstream rejection).
                    _ => {
                        unreachable!("MIR lowering: call to an unresolved callee (rejected in HIR)")
                    }
                }
            }
            // A bare value-position block (`let x = { ...; tail };`). Inlining it
            // flattens its scope into the enclosing one; correct only once every
            // local is suffixed by its id (the emitter now does this), but no
            // built program exercises it, so it stays a loud `todo!()` rather
            // than speculative code.
            // FIXME: Value-position blocks are accepted HIR; route them through
            // `lower_block_into` so MIR-only codegen stays total.
            Expr::Block(_) => todo!("MIR lowering: bare value-position block"),
            // Anything else here is not a value-producing expression in
            // well-typed HIR: a value `if`/`match` is intercepted upstream
            // (`is_value_control_flow`), and a diagnosed expression lowers to
            // `Expr::Missing` and halts compilation before MIR. So a value is
            // always expected here (I2).
            _ => unreachable!("MIR lowering: non-value expression in rvalue position"),
        }
    }

    /// Lower an expression to a trivial [`Operand`]. A non-trivial expression is
    /// evaluated into a fresh temp and the temp is returned, preserving
    /// left-to-right evaluation order via the order of emitted statements.
    fn lower_operand(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Operand {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Literal(lit) => Operand::Const(lit.clone()),
            Expr::Path(Resolution::Local(hid)) => Operand::Copy(Place::Local(self.map_local(*hid))),
            // A place projection (`a[i]`, `s.f`, `*p`) is already a trivial
            // operand: it reads as `Copy(place)` with no spill, exactly as the
            // old codegen rendered it inline. Any non-trivial sub-part (a
            // side-effecting index, a non-place base) is spilled to a temp by
            // `lower_place`, preserving evaluation order.
            Expr::Index { .. } | Expr::Field { .. } | Expr::Deref { .. } => {
                Operand::Copy(self.lower_place(e, buf))
            }
            // A value-position `if`/`match`, or a short-circuit `&&`/`||`, is
            // control flow, not an rvalue: declare the temp first
            // (uninitialized, hence mutable), then lower the construct so each
            // branch assigns the temp. This is the in-place lowering that
            // replaces codegen's hoist (REDESIGN I3) and keeps `&&`/`||` from
            // evaluating eagerly (REDESIGN I5).
            Expr::If { .. }
            | Expr::Match { .. }
            | Expr::Binary {
                op: BinOp::And | BinOp::Or,
                ..
            } => {
                let ty = self.mir_type_of(e);
                let mid = self.locals.alloc(MirLocal {
                    ty,
                    name: None,
                    mutable: true,
                });
                buf.push(MirStmt::Let {
                    local: mid,
                    init: None,
                });
                let place = Place::Local(mid);
                self.lower_into(e, &place, buf);
                Operand::Copy(place)
            }
            _ => {
                let rv = self.lower_rvalue(e, buf);
                let ty = self.mir_type_of(e);
                let mid = self.locals.alloc(MirLocal {
                    ty,
                    name: None,
                    mutable: true,
                });
                buf.push(MirStmt::Let {
                    local: mid,
                    init: Some(rv),
                });
                Operand::Copy(Place::Local(mid))
            }
        }
    }

    /// Whether `e` is a value-producing control-flow expression (a value
    /// `if`/`match`, or a short-circuit `&&`/`||`). These are not [`RValue`]s;
    /// they lower in place against a temp via [`Lower::lower_into`] rather than
    /// nesting inside an rvalue. `&&`/`||` are here, not in [`RValue::Binary`],
    /// because flattening their operands to temps would evaluate the right-hand
    /// side eagerly and silently break short-circuiting (REDESIGN I5).
    fn is_value_control_flow(&self, e: ExprId) -> bool {
        matches!(
            self.body.exprs[e],
            Expr::If { .. }
                | Expr::Match { .. }
                | Expr::Binary {
                    op: BinOp::And | BinOp::Or,
                    ..
                }
        )
    }

    /// Lower `e` so its value is stored into `target`. A value-position
    /// `if`/`match` becomes the matching control-flow statement whose every
    /// branch assigns `target`; this is the in-place lowering that supersedes
    /// codegen's hoist and unbans nested value-matches (REDESIGN I3). Anything
    /// else is an rvalue assigned directly.
    fn lower_into(&mut self, e: ExprId, target: &Place, buf: &mut ThinVec<MirStmt>) {
        let body = self.body;
        match &body.exprs[e] {
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                let cond = self.lower_operand(cond, buf);
                let then_block = self.lower_block_into(then_branch, target);
                // A value-position `if` should have an `else`, since both
                // branches must produce the value. The front end does NOT
                // enforce this today (verified: `let x = if c { 1 };` compiles on
                // both paths), so a missing `else` leaves `target` assigned only
                // in the `then` branch and read uninitialized when the condition
                // is false. This MIR matches the HIR-walk path byte-for-byte on
                // that shape (same latent gap, parity preserved); a proper fix -
                // rejecting an else-less value `if` - belongs to the type-check
                // track, not this lowering. Lower what is present.
                let else_block = else_branch.map(|b| self.lower_block_into(b, target));
                buf.push(MirStmt::If {
                    cond,
                    then_block,
                    else_block,
                });
            }
            Expr::Match { scrut, arms } => {
                let scrut = *scrut;
                let arm_data: SmallVec<[(ArmKind, ExprId); 4]> = arms
                    .iter()
                    .map(|arm| (self.arm_kind(arm.pat), arm.body))
                    .collect();
                let scrut = self.lower_operand(scrut, buf);
                let mut arms_out = ThinVec::with_capacity(arms.len());
                let mut default = None;
                for (kind, arm_body) in arm_data {
                    match kind {
                        ArmKind::Variant(variant) => arms_out.push(SwitchArm {
                            variant,
                            body: self.lower_arm_body_into(arm_body, target),
                        }),
                        ArmKind::Default => {
                            default = Some(self.lower_arm_body_into(arm_body, target))
                        }
                        ArmKind::Skip => {}
                    }
                }
                buf.push(MirStmt::Switch {
                    scrut,
                    arms: arms_out,
                    default,
                });
            }
            // Short-circuit `&&`/`||`, lowered to control flow so the rhs runs
            // only when the lhs does not already decide the result (REDESIGN
            // I5). Shape, with `target` the result temp:
            //   `&&`:  target = lhs;  if (target) { target = rhs }
            //   `||`:  target = lhs;  if (target) {} else { target = rhs }
            // The rhs lowers into the branch block's OWN buffer
            // (`lower_into_block`), never `buf`: emitting its prerequisite temps
            // into `buf` would run them before the `if`, eager-evaluating the
            // rhs and defeating the short-circuit. No negation is needed because
            // `||` puts the rhs in the `else`.
            Expr::Binary {
                op: op @ (BinOp::And | BinOp::Or),
                lhs,
                rhs,
            } => {
                let (is_and, lhs, rhs) = (matches!(op, BinOp::And), *lhs, *rhs);
                self.lower_into(lhs, target, buf);
                let cond = Operand::Copy(target.clone());
                let rhs_block = self.lower_into_block(rhs, target);
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
            _ => {
                let rv = self.lower_rvalue(e, buf);
                buf.push(MirStmt::Assign {
                    place: target.clone(),
                    value: rv,
                });
            }
        }
    }

    /// Lower `e` into its own [`MirBlock`], assigning its value into `target`.
    /// Used for a short-circuit branch body, where the contents must run only
    /// when the branch is taken (REDESIGN I5).
    fn lower_into_block(&mut self, e: ExprId, target: &Place) -> MirBlock {
        let mut buf = ThinVec::new();
        self.lower_into(e, target, &mut buf);
        MirBlock { stmts: buf }
    }

    /// Lower a block in value position: its statements run, then its tail value
    /// is assigned into `target`. A tail-less (void) block leaves `target`
    /// unassigned; see the else-less-`if` note in [`Lower::lower_into`] for the
    /// shared latent gap when a value position lacks a value.
    fn lower_block_into(&mut self, block_id: BlockId, target: &Place) -> MirBlock {
        let body = self.body;
        let block = &body.blocks[block_id];
        let mut buf = ThinVec::with_capacity(block.stmts.len() + usize::from(block.tail.is_some()));
        for &sid in &block.stmts {
            self.lower_stmt(&body.stmts[sid], &mut buf);
        }
        if let Some(tail) = block.tail {
            self.lower_into(tail, target, &mut buf);
        }
        MirBlock { stmts: buf }
    }

    /// Lower a value-position match arm body, assigning its value into `target`.
    fn lower_arm_body_into(&mut self, body_expr: ExprId, target: &Place) -> MirBlock {
        let mut buf = ThinVec::new();
        self.lower_into(body_expr, target, &mut buf);
        MirBlock { stmts: buf }
    }

    fn map_local(&mut self, hid: HirLocalId) -> LocalId {
        if let Some(&mid) = self.local_map.get(&hid) {
            return mid;
        }
        // A reference to a local not yet lowered (a parameter outside the
        // pre-created range, or any local seen before its `let`): materialize
        // it from the HIR local so the place resolves.
        let local = &self.body.locals[hid];
        let mid = self.locals.alloc(MirLocal {
            ty: local.ty.clone().unwrap_or(Type::Error),
            name: Some(local.name.clone()),
            mutable: local.mutable,
        });
        self.local_map.insert(hid, mid);
        mid
    }

    /// The single quarantined type-fallback site. A temp's type comes from the
    /// HIR `expr_types` side table; when absent it defaults to `int32`. Measured
    /// to never fire on the current corpus (`docs/MIR.md`); isolating it here
    /// makes the Track 3 flip to a hard `Type` diagnostic a one-line change.
    fn mir_type_of(&self, e: ExprId) -> Type {
        self.body
            .expr_types
            .get(e)
            .cloned()
            .unwrap_or_else(|| Type::Path("int32".into()))
    }
}
