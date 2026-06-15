//! HIR -> MIR lowering.
//!
//! a builder over a finished HIR [`Body`]. it linearizes value-producing
//! expressions into three-address form and (in later segments) flattens control
//! flow. the output is total: every well-typed HIR body lowers to valid MIR
//! without rejecting or emitting diagnostics (REDESIGN I2).
//!
//! status: incremental. covers straight-line bodies (segment 1), statement-
//! position control flow (segment 2: `if`/`loop`/`break`/`continue`/`return`/
//! statement-`match`/assign), value-position control flow plus general calls
//! (segment 3: a value `if`/`match` lowered in place via a typed temp - the I3
//! acid test - and a direct `Call`), and the full expression surface (segment 4:
//! `Unary`, `Index`, `Field`, `ArrayLit`, `StructLit`, `Ref`/`Deref`, `Cast`,
//! place projections, and the `&&`/`||` short-circuit rewrite). a bare
//! value-position block is lowered via a typed temp. a
//! name in value position that does not denote a value (an undeclared name, a
//! struct/function name) is rejected in HIR before MIR runs, so its `Path` is
//! `unreachable!` here, not lowered - see `docs/planning/DEFER.md`.

use ast::{AssignOp, BinOp, UnaryOp};
use hir::core::TypedArena;
use hir::core::{
    BlockId, Body, ConstId, ConstValue, Expr, ExprId, HIR, Literal, LocalId as HirLocalId,
    MatchArm, Pat, PatId, Resolution, Stmt, Text, TypeKind,
};
use smallvec::SmallVec;
use thin_vec::ThinVec;

use crate::core::*;

/// lower one function body to a [`MirBody`]. `param_count` is the number of
/// leading [`Body`] locals that are parameters (HIR allocates them first, before
/// any block local); they are pre-created as MIR locals so references resolve
/// and so the emitter can skip declaring them (the signature already does).
pub fn lower_function(
    hir: &HIR,
    types: &hir::core::TypeInterner,
    body: &Body,
    typeck: &typeck::TypeckResults,
    param_count: usize,
    ret: Option<Type>,
) -> MirBody {
    let mut cx = Lower::new(hir, types, body, typeck, ret);
    cx.lower_params(param_count);
    let block = cx.lower_top_block();
    MirBody {
        locals: cx.locals,
        params: cx.params,
        body: block,
    }
}

struct Lower<'a> {
    /// the lowered module, read for const values (a const reference inlines its
    /// folded scalar; `docs/design/HORIZON0.md` component 1).
    hir: &'a HIR,
    /// the interner this body's `TypeRef` handles resolve through. for the
    /// whole-file path this is `hir.types`; for the per-fn query path it is
    /// the body's own interner (scope clone + body-local types).
    types: &'a hir::core::TypeInterner,
    body: &'a Body,
    /// the type side table for this body (`typeck::check_body`); since the
    /// S2 cutover this is the only source of expression types.
    typeck: &'a typeck::TypeckResults,
    /// the function's declared return type, used by [`Lower::lower_tail`] to
    /// decide whether the body tail is a returned value or a discarded effect.
    /// `None` for a void function (and for `main`, where the caller passes
    /// `None` so the tail is discarded and the emitter supplies `return 0`).
    ret: Option<Type>,
    locals: TypedArena<MirLocal, LocalId>,
    params: ThinVec<LocalId>,
    /// maps HIR local index -> MIR local. indexed by `hid.raw_idx().into_u32() as usize`.
    /// `None` means the HIR local hasn't been lowered yet (lazy fallback).
    local_map: Vec<Option<LocalId>>,
}

/// how a match arm pattern dispatches, lifted off the borrowed [`Body`] before
/// lowering each arm body (which mutably borrows `self`).
enum ArmKind {
    Variant(VariantRef),
    /// an int / char / bool literal arm (`1 -> ..`).
    Const(Literal),
    /// a bare-ident binding arm (`x -> ..`) over a primitive scrutinee: bind the
    /// scrutinee to the local, then run the body. irrefutable, so it lowers as
    /// the default with the binding prepended.
    Bind(HirLocalId),
    Default,
    /// `Missing`/struct-in-match in an arm: broken or not-yet-supported; dropped.
    Skip,
}

impl<'a> Lower<'a> {
    fn new(
        hir: &'a HIR,
        types: &'a hir::core::TypeInterner,
        body: &'a Body,
        typeck: &'a typeck::TypeckResults,
        ret: Option<Type>,
    ) -> Self {
        Self {
            hir,
            types,
            body,
            typeck,
            ret,
            locals: TypedArena::new(),
            params: ThinVec::new(),
            local_map: vec![None; body.locals.len()],
        }
    }

    fn lower_params(&mut self, param_count: usize) {
        let body = self.body;
        for (hid, l) in body.locals.iter().take(param_count) {
            let ty = l.ty.unwrap_or_else(|| self.types.error_type());
            let name = Some(l.name.clone());
            let mutable = l.mutable;
            let mid = self.locals.alloc(MirLocal { ty, name, mutable });
            self.local_map[hid.raw_idx().into_u32() as usize] = Some(mid);
            self.params.push(mid);
        }
    }

    fn collect_operands(
        &mut self,
        args: &[ExprId],
        buf: &mut ThinVec<MirStmt>,
    ) -> ThinVec<Operand> {
        let mut lowered = ThinVec::with_capacity(args.len());
        lowered.extend(args.iter().copied().map(|a| self.lower_operand(a, buf)));
        lowered
    }

    /// whether `buf` ends in an unconditional jump (`return`/`break`/`continue`).
    /// statements after such a terminator are unreachable, so the block builders
    /// stop appending once it is present - straight-line dead-code elimination,
    /// which keeps the emitted c free of dead stores after an early return.
    fn terminated(buf: &[MirStmt]) -> bool {
        matches!(
            buf.last(),
            Some(MirStmt::Return(_) | MirStmt::Break | MirStmt::Continue)
        )
    }

    fn lower_top_block(&mut self) -> MirBlock {
        let body = self.body;
        let mut buf = ThinVec::with_capacity(body.block.len() + usize::from(body.tail.is_some()));
        for &sid in &body.block {
            self.lower_stmt(&body.stmts[sid], &mut buf);
            if Self::terminated(&buf) {
                return MirBlock { stmts: buf };
            }
        }
        if let Some(tail) = body.tail {
            self.lower_tail(tail, &mut buf);
        }
        MirBlock { stmts: buf }
    }

    /// lower a function body's tail. with a declared return type the tail is the
    /// implicit return value; otherwise (void fn / `main`) its value is
    /// discarded and it lowers for effect.
    fn lower_tail(&mut self, tail: ExprId, buf: &mut ThinVec<MirStmt>) {
        // a diverging tail (`... return e`, `break`, `continue` as the final
        // expression) is its own terminator: lower it as a statement, never
        // wrapped in a synthesized `return`, so a typed function whose tail is
        // `return e` emits just `return e;` and not a dead `return <poison>;`.
        if matches!(
            self.body.exprs[tail],
            Expr::Return(_) | Expr::Break | Expr::Continue
        ) {
            self.lower_expr_stmt(tail, buf);
            return;
        }
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
                let lty = ty
                    .as_ref()
                    .or(local.ty.as_ref())
                    .cloned()
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

    /// get a [`Place`] holding the value of `e`, spilling to a temp when needed.
    /// a value-position control-flow expression (`if`/`match`) is not handled by
    /// [`Lower::lower_place`] (it is not an rvalue), so route it through
    /// [`Lower::lower_operand`], which spills it into a temp and yields a place.
    fn place_for_value(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Place {
        if self.is_value_control_flow(e) {
            match self.lower_operand(e, buf) {
                Operand::Copy(p) => p,
                // a control-flow value is never a constant, but stay total: park a
                // constant in a temp so the caller still gets a place.
                op @ Operand::Const(_) => {
                    let ty = self.mir_type_of(e);
                    let mid = self.locals.alloc(MirLocal {
                        ty,
                        name: None,
                        mutable: false,
                    });
                    buf.push(MirStmt::Let {
                        local: mid,
                        init: Some(RValue::Use(op)),
                    });
                    Place::Local(mid)
                }
            }
        } else {
            self.lower_place(e, buf)
        }
    }

    /// lower an expression in statement (discarded-value) position. a
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
    fn lower_block(&mut self, block_id: BlockId) -> MirBlock {
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
    fn lower_binding_arm_body_into(
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

    /// lower a place expression (`a`, `a[i]`, `s.f`, `*p`) to a [`Place`],
    /// emitting any prerequisite temps into `buf`. used for an assign target,
    /// the operand of `&`, and reading a projection as a trivial operand. a
    /// non-trivial index spills to a temp; a base that is not itself a place (a
    /// call or literal returning an aggregate) is evaluated into a temp and
    /// projected from that local, preserving evaluation order.
    fn lower_place(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Place {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Path(Resolution::Local(hid)) => Place::Local(self.map_local(*hid)),
            Expr::Path(Resolution::Global(gid)) => {
                Place::Global(self.hir.globals[*gid].name.clone())
            }
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
            // the base is not a place (e.g. a call returning an aggregate):
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

    /// lower an expression to an [`RValue`], applying any typeck read
    /// adjustment first. an array-reference decay (`typeck::Adjustment::Decay`,
    /// S2C C4) reads the underlying `&[T; N]` value and casts it to the target
    /// pointer type - the cast lowering once injected as an HIR `Cast` node,
    /// emitted here from the side table instead so the generated c is identical.
    fn lower_rvalue(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> RValue {
        if let Some(typeck::Adjustment::Decay(target)) =
            self.typeck.adjustments.get(e.into()).copied()
        {
            let inner = self.lower_operand_raw(e, buf);
            return RValue::Cast(inner, target);
        }
        self.lower_rvalue_raw(e, buf)
    }

    /// lower an expression to an [`RValue`], emitting any prerequisite temps
    /// into `buf`. used where an rvalue is wanted directly (a `let` init, a
    /// discarded effect). the adjustment-aware entry point is [`Lower::lower_rvalue`];
    /// this core is also the undecayed inner read a decay adjustment casts.
    fn lower_rvalue_raw(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> RValue {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Literal(lit) => RValue::Use(Operand::Const(lit.clone())),
            // a `Path` in value position. HIR rejects every non-value resolution
            // (a type name, an unresolved name) before MIR runs, so only the
            // value resolutions are reachable; the rest are checked-`unreachable!`
            // (I2). a bare function name is a value here - its address - and
            // lowers to `RValue::Func`. exhaustive so a new `Resolution` variant
            // must declare its value-ness.
            Expr::Path(res) => match res {
                Resolution::Local(hid) => {
                    RValue::Use(Operand::Copy(Place::Local(self.map_local(*hid))))
                }
                Resolution::Variant { enum_id, idx } => RValue::Variant(VariantRef {
                    enum_id: *enum_id,
                    idx: *idx,
                }),
                Resolution::Fn(fid) => RValue::Func(*fid),
                // a const inlines its folded scalar value (HORIZON0 C1): a value
                // with no address, so it is substituted, not read from a symbol.
                // block-scope consts inline identically; only the lookup differs.
                Resolution::Const(cid) => self.const_rvalue(*cid),
                Resolution::LocalConst(lcid) => {
                    const_value_rvalue(self.body.local_consts[*lcid].value.as_ref())
                }
                // a global is addressable storage (HORIZON0 C3): read its named
                // c symbol as a place, unlike the inlined const.
                Resolution::Global(gid) => RValue::Use(Operand::Copy(Place::Global(
                    self.hir.globals[*gid].name.clone(),
                ))),
                Resolution::Enum(_) | Resolution::Struct(_) | Resolution::Unresolved(_) => {
                    unreachable!("non-value Path in rvalue position (rejected in HIR)")
                }
            },
            Expr::Binary { op, lhs, rhs } => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    // discarded `a && b;` / `a || b;` in statement position (the
                    // primary path is `lower_expr_stmt`; this arm is a
                    // defense-in-depth). lower both sub-expressions with
                    // short-circuit control flow; the temp result is discarded.
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
                    return RValue::Use(Operand::Const(Literal::Int(0)));
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
            // reading a place projection in value position: `Use` of the place.
            Expr::Index { .. } | Expr::Field { .. } | Expr::Deref { .. } => {
                RValue::Use(Operand::Copy(self.lower_place(e, buf)))
            }
            Expr::Ref { operand } => RValue::Ref(self.lower_place(*operand, buf)),
            Expr::Cast { operand, ty } => {
                let (operand, ty) = (*operand, *ty);
                let o = self.lower_operand(operand, buf);
                RValue::Cast(o, ty)
            }
            // `sizeof(T)`: carry the type through to codegen, which emits c
            // `sizeof(ctype)`. eye does not compute layout (HORIZON0 C2).
            Expr::SizeOf(ty) => RValue::SizeOf(*ty),
            // `len(arr)`: fold to `(usize)N`, the operand's element count read
            // from its type. reproduces exactly the MIR a pre-cutover `len`
            // fold emitted (a usize-cast int const), so codegen is unchanged -
            // the count just moves here, where the types live after the cutover.
            Expr::Len(operand) => {
                let arg_ty = self.mir_type_of(*operand);
                let n = match self.types.lookup(arg_ty) {
                    &TypeKind::Array { len, .. } => len,
                    &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => {
                        match self.types.lookup(inner) {
                            &TypeKind::Array { len, .. } => len,
                            _ => 0,
                        }
                    }
                    _ => 0,
                };
                RValue::Cast(Operand::Const(Literal::Int(n as u128)), self.mir_type_of(e))
            }
            Expr::ArrayLit(elems) => {
                let ty = self.mir_type_of(e);
                RValue::ArrayLit {
                    ty,
                    elems: self.collect_operands(elems, buf),
                }
            }
            Expr::ArrayRepeat { value, count } => {
                let ty = self.mir_type_of(e);
                // `lower_operand` spills a non-trivial value to a temp, so the
                // element is evaluated exactly once even though it is copied
                // `count` times.
                RValue::ArrayRepeat {
                    ty,
                    value: self.lower_operand(*value, buf),
                    count: *count,
                }
            }
            Expr::StructLit { ty, fields } => {
                let ty = *ty;
                let mut lowered = ThinVec::with_capacity(fields.len());
                lowered.extend(
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), self.lower_operand(f.value, buf))),
                );
                RValue::StructLit {
                    ty,
                    fields: lowered,
                }
            }
            Expr::Call { callee, args } => {
                let callee = *callee;
                match &body.exprs[callee] {
                    // the `println` intrinsic is sniffed by its unresolved callee
                    // name and carried as a dedicated node.
                    Expr::Path(Resolution::Unresolved(name)) if name == "println" => {
                        RValue::Println {
                            args: self.collect_operands(args, buf),
                        }
                    }
                    // a direct call to a named function (defined or `extern`).
                    Expr::Path(Resolution::Fn(fid)) => {
                        let func = *fid;
                        RValue::Call {
                            func,
                            args: self.collect_operands(args, buf),
                        }
                    }
                    // an indirect call through a function-pointer value (a local,
                    // field, index, or call result of function type). a callee
                    // that is neither a function nor a function pointer (an
                    // undeclared name, a non-function value) is rejected in HIR
                    // before MIR runs (`UnresolvedName` / `CallNonFunction`), so
                    // the callee here is always a real function-pointer value
                    // (I2: the emitter trusts upstream rejection).
                    _ => {
                        let callee_op = self.lower_operand(callee, buf);
                        RValue::CallIndirect {
                            callee: callee_op,
                            args: self.collect_operands(args, buf),
                        }
                    }
                }
            }
            // diverging control flow in value position: `let x = return v;`,
            // `f(break)`, `let y = continue;`. these produce no value. lower the
            // jump as a statement, then return a poison rvalue that the
            // consuming `Let`/`Assign` emits as dead code *after* the jump - it
            // never executes, so its value is irrelevant. without these arms a
            // direct value-position jump would fall to the `_ => unreachable!`
            // below and panic the compiler. (`Break`/`Continue` shared this gap;
            // both are fixed here.) matches rust, where `let x = return;` is
            // legal with `x: !`. a jump wrapped in an `if`/`match` takes the
            // `lower_into` path instead and never reaches here.
            Expr::Return(value) => {
                self.lower_return(*value, buf);
                RValue::Use(Operand::Const(Literal::Int(0)))
            }
            Expr::Break => {
                buf.push(MirStmt::Break);
                RValue::Use(Operand::Const(Literal::Int(0)))
            }
            Expr::Continue => {
                buf.push(MirStmt::Continue);
                RValue::Use(Operand::Const(Literal::Int(0)))
            }
            // a `loop` in value position (`let x = loop {...}`, a value-returning
            // fn tail, a `loop` argument). it has no value today: `break` is
            // valueless (break-with-value is fork d), so a loop either diverges
            // (the poison below is unreachable dead code) or exits with no value
            // (the poison `0` stands in, consistent with `break` dropping its
            // value). lower the loop as a statement, then return the poison; this
            // replaces a former `unreachable!` panic on valid-parsing syntax.
            Expr::Loop { body } => {
                let body_block = self.lower_block(*body);
                buf.push(MirStmt::Loop { body: body_block });
                RValue::Use(Operand::Const(Literal::Int(0)))
            }
            // bare value-position blocks (`let x = { ...; tail };`). a temp local
            // is declared (uninit), the block's statements and tail assignment are
            // emitted inline, and the temp is returned as `Use(Copy)`. tail-less
            // blocks in value position leave the temp unassigned -- a latent gap
            // shared with else-less value `if`.
            Expr::Block(block_id) => {
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
                let block = self.lower_block_into(*block_id, &place);
                buf.extend(block.stmts);
                RValue::Use(Operand::Copy(place))
            }
            // anything else here is not a value-producing expression in
            // well-typed HIR: a value `if`/`match` is intercepted upstream
            // (`is_value_control_flow`), and a diagnosed expression lowers to
            // `Expr::Missing` and halts compilation before MIR. so a value is
            // always expected here (I2).
            _ => unreachable!("MIR lowering: non-value expression in rvalue position"),
        }
    }

    /// lower an expression to a trivial [`Operand`], applying any typeck read
    /// adjustment first. an array-reference decay (S2C C4) spills
    /// `target _t = (target)<value>` to a temp and yields it - exactly the temp
    /// lowering's former injected cast node produced in operand position.
    fn lower_operand(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Operand {
        if let Some(typeck::Adjustment::Decay(target)) =
            self.typeck.adjustments.get(e.into()).copied()
        {
            let inner = self.lower_operand_raw(e, buf);
            let mid = self.locals.alloc(MirLocal {
                ty: target,
                name: None,
                mutable: true,
            });
            buf.push(MirStmt::Let {
                local: mid,
                init: Some(RValue::Cast(inner, target)),
            });
            return Operand::Copy(Place::Local(mid));
        }
        self.lower_operand_raw(e, buf)
    }

    /// lower an expression to a trivial [`Operand`]. a non-trivial expression is
    /// evaluated into a fresh temp and the temp is returned, preserving
    /// left-to-right evaluation order via the order of emitted statements. the
    /// adjustment-aware entry point is [`Lower::lower_operand`]; this core is
    /// also the undecayed inner read a decay adjustment casts.
    fn lower_operand_raw(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Operand {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Literal(lit) => Operand::Const(lit.clone()),
            Expr::Path(Resolution::Local(hid)) => Operand::Copy(Place::Local(self.map_local(*hid))),
            Expr::Path(Resolution::Global(gid)) => {
                Operand::Copy(Place::Global(self.hir.globals[*gid].name.clone()))
            }
            // a const inlines to a trivial constant operand. a negative integer
            // has no unsigned-literal form, so it spills its unary-negation
            // rvalue to a temp (preserving the trivial-operand invariant).
            // block-scope consts inline identically; only the lookup differs.
            Expr::Path(Resolution::Const(cid)) => {
                let value = self.hir.consts[*cid].value.as_ref();
                self.const_operand_or_spill(value, e, buf)
            }
            Expr::Path(Resolution::LocalConst(lcid)) => {
                let value = body.local_consts[*lcid].value.as_ref();
                self.const_operand_or_spill(value, e, buf)
            }
            // a place projection (`a[i]`, `s.f`, `*p`) is already a trivial
            // operand: it reads as `Copy(place)` with no spill, exactly as the
            // old codegen rendered it inline. any non-trivial sub-part (a
            // side-effecting index, a non-place base) is spilled to a temp by
            // `lower_place`, preserving evaluation order.
            Expr::Index { .. } | Expr::Field { .. } | Expr::Deref { .. } => {
                Operand::Copy(self.lower_place(e, buf))
            }
            // a value-position `if`/`match`, or a short-circuit `&&`/`||`, is
            // control flow, not an rvalue: declare the temp first
            // (uninitialized, hence mutable), then lower the construct so each
            // branch assigns the temp. this is the in-place lowering that
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
                let rv = self.lower_rvalue_raw(e, buf);
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

    /// whether `e` is a value-producing control-flow expression (a value
    /// `if`/`match`, or a short-circuit `&&`/`||`). these are not [`RValue`]s;
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

    /// lower `e` so its value is stored into `target`. a value-position
    /// `if`/`match` becomes the matching control-flow statement whose every
    /// branch assigns `target`; this is the in-place lowering that supersedes
    /// codegen's hoist and unbans nested value-matches (REDESIGN I3). anything
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
                // a value-position `if` should have an `else`, since both
                // branches must produce the value. the front end does NOT
                // enforce this today (verified: `let x = if c { 1 };` compiles on
                // both paths), so a missing `else` leaves `target` assigned only
                // in the `then` branch and read uninitialized when the condition
                // is false. this MIR matches the HIR-walk path byte-for-byte on
                // that shape (same latent gap, parity preserved); a proper fix -
                // rejecting an else-less value `if` - belongs to the type-check
                // track, not this lowering. lower what is present.
                let else_block = else_branch.map(|b| self.lower_block_into(b, target));
                buf.push(MirStmt::If {
                    cond,
                    then_block,
                    else_block,
                });
            }
            Expr::Match { scrut, arms } => {
                let scrut_expr = *scrut;
                let scrut = self.lower_operand(scrut_expr, buf);
                let (arms_out, default) = self.lower_match_arms(
                    &scrut,
                    arms,
                    |s, e| s.lower_arm_body_into(e, target),
                    |s, hid, scrut, e| s.lower_binding_arm_body_into(hid, scrut, e, target),
                );
                buf.push(MirStmt::Switch {
                    scrut,
                    arms: arms_out,
                    default,
                });
            }
            // short-circuit `&&`/`||`, lowered to control flow so the rhs runs
            // only when the lhs does not already decide the result (REDESIGN
            // I5). shape, with `target` the result temp:
            // `&&`: target = lhs; if (target) { target = rhs }
            // `||`: target = lhs; if (target) {} else { target = rhs }
            // the rhs lowers into the branch block's OWN buffer
            // (`lower_into_block`), never `buf`: emitting its prerequisite temps
            // into `buf` would run them before the `if`, eager-evaluating the
            // rhs and defeating the short-circuit. no negation is needed because
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
            // a `return` in value position (a branch tail or match-arm body,
            // e.g. `let x = match c { A -> return 1, _ -> 2 };`). it diverges,
            // so `target` is intentionally left unassigned on this path: the
            // code that reads `target` never runs when the return is taken, and
            // the other branches assign it. same uninitialized-temp shape as the
            // else-less value `if` documented above. without this arm the `_`
            // case below would route a value-position return through
            // `lower_rvalue` and hit its `unreachable!`.
            Expr::Return(value) => self.lower_return(*value, buf),
            _ => {
                let rv = self.lower_rvalue(e, buf);
                buf.push(MirStmt::Assign {
                    place: target.clone(),
                    value: rv,
                });
            }
        }
    }

    /// lower a `return expr?;` to a [`MirStmt::Return`]. shared by the statement
    /// and value positions; in value position the enclosing target temp is left
    /// unassigned because a return diverges.
    fn lower_return(&mut self, value: Option<ExprId>, buf: &mut ThinVec<MirStmt>) {
        let op = value.map(|v| self.lower_operand(v, buf));
        buf.push(MirStmt::Return(op));
    }

    /// lower `e` into its own [`MirBlock`], assigning its value into `target`.
    /// used for a short-circuit branch body, where the contents must run only
    /// when the branch is taken (REDESIGN I5).
    fn lower_into_block(&mut self, e: ExprId, target: &Place) -> MirBlock {
        let mut buf = ThinVec::new();
        self.lower_into(e, target, &mut buf);
        MirBlock { stmts: buf }
    }

    /// lower a block in value position: its statements run, then its tail value
    /// is assigned into `target`. a tail-less (void) block leaves `target`
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

    /// lower a value-position match arm body, assigning its value into `target`.
    fn lower_arm_body_into(&mut self, body_expr: ExprId, target: &Place) -> MirBlock {
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

    fn lower_match_arms(
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

    fn map_local(&mut self, hid: HirLocalId) -> LocalId {
        let idx = hid.raw_idx().into_u32() as usize;
        if let Some(mid) = self.local_map[idx] {
            return mid;
        }
        // a reference to a local not yet lowered (a parameter outside the
        // pre-created range, or any local seen before its `let`): materialize
        // it from the HIR local so the place resolves.
        let local = &self.body.locals[hid];
        let mid = self.locals.alloc(MirLocal {
            ty: local.ty.unwrap_or_else(|| self.types.error_type()),
            name: Some(local.name.clone()),
            mutable: local.mutable,
        });
        self.local_map[idx] = Some(mid);
        mid
    }

    /// a temp's type comes from the typeck `expr_types` side table, the sole
    /// type source since the S2C cutover. the walker is total over every
    /// expression MIR consumes (proven by the `corpus_generates_no_error_type`
    /// e2e test), so a miss here is a compiler bug, not bad user input: it ices
    /// (S2C C5), since codegen only runs on a program with no diagnostics, where
    /// inference is complete by construction.
    fn mir_type_of(&self, e: ExprId) -> Type {
        self.typeck.expr_types.get(e.into()).copied().unwrap_or_else(|| {
            panic!(
                "MIR: typeck left {e:?} untyped - the walker must be total over \
                 every expression MIR lowers (S2C C5)"
            )
        })
    }

    /// inline a const reference as an [`RValue`]. a non-negative scalar is a
    /// `Use` of a trivial constant; a negative integer becomes a unary negation
    /// of its magnitude (literals are unsigned). a poisoned const (fold failed,
    /// already diagnosed) inlines `0` - dead code the front end never let reach
    /// here without a prior error.
    fn const_rvalue(&self, cid: ConstId) -> RValue {
        const_value_rvalue(self.hir.consts[cid].value.as_ref())
    }

    /// a const value as an operand: trivially when it has a literal form,
    /// otherwise (a negative integer, or poison) spilled through its rvalue to
    /// a temp, preserving the trivial-operand invariant.
    fn const_operand_or_spill(
        &mut self,
        value: Option<&ConstValue>,
        e: ExprId,
        buf: &mut ThinVec<MirStmt>,
    ) -> Operand {
        match value.and_then(const_operand) {
            Some(op) => op,
            None => {
                let rv = const_value_rvalue(value);
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
}

/// a folded const value as an rvalue: a trivial constant operand, or a unary
/// negation for a negative integer (which has no unsigned-literal form). a
/// poisoned const (`None` - the fold failed and was diagnosed) reads as 0.
/// shared by top-level ([`Lower::const_rvalue`]) and block-scope consts.
fn const_value_rvalue(value: Option<&ConstValue>) -> RValue {
    match value {
        Some(ConstValue::Int(n)) if *n < 0 => {
            RValue::Unary(UnaryOp::Neg, Operand::Const(Literal::Int(n.unsigned_abs())))
        }
        Some(v) => RValue::Use(const_operand(v).expect("non-negative const is a trivial operand")),
        None => RValue::Use(Operand::Const(Literal::Int(0))),
    }
}

/// a const value as a trivial constant operand, when it has one. a negative
/// integer has no unsigned-literal form, so it returns `None`; the caller
/// spills it through [`Lower::const_rvalue`] (a unary negation) into a temp.
fn const_operand(v: &ConstValue) -> Option<Operand> {
    Some(Operand::Const(match v {
        ConstValue::Int(n) if *n >= 0 => Literal::Int(*n as u128),
        ConstValue::Int(_) => return None,
        ConstValue::Float(f) => Literal::Float(float_to_text(*f)),
        ConstValue::Bool(b) => Literal::Bool(*b),
        ConstValue::Char(c) => Literal::Char(*c),
    }))
}

/// render a folded `f64` as a c floating literal. a value with no decimal point
/// or exponent (`6.0` formats as `6`) would be an `int` literal in c, so a `.0`
/// is appended to keep it a `double` - notably so `printf("%f", ...)` is not
/// handed an `int`.
fn float_to_text(f: f64) -> Text {
    let mut s = format!("{f}");
    if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        s.push_str(".0");
    }
    Text::from(s)
}
