//! value-position lowering: the rvalue and operand cores (`lower_rvalue` /
//! `lower_operand` and their adjustment-aware `_raw` entry points), the
//! `lower_into` family that lowers a value-position `if`/`match`/`&&`/`||` in
//! place against a target, and const inlining. these produce the [`RValue`] /
//! [`Operand`] forms MIR is built from.

use ast::{BinOp, UnaryOp};
use hir::core::{
    BlockId, ConstId, ConstValue, Expr, ExprId, Literal, Resolution, Text, TypeKind,
};
use thin_vec::ThinVec;

use super::Lower;
use crate::core::*;

impl<'a> Lower<'a> {
    /// lower an expression to an [`RValue`], applying any typeck read
    /// adjustment first. an array-reference decay (`typeck::Adjustment::Decay`,
    /// S2C C4) reads the underlying `&[T; N]` value and casts it to the target
    /// pointer type - the cast lowering once injected as an HIR `Cast` node,
    /// emitted here from the side table instead so the generated c is identical.
    pub(crate) fn lower_rvalue(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> RValue {
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
    pub(crate) fn lower_operand(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Operand {
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
    pub(crate) fn is_value_control_flow(&self, e: ExprId) -> bool {
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
    pub(crate) fn lower_into(&mut self, e: ExprId, target: &Place, buf: &mut ThinVec<MirStmt>) {
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

    /// lower `e` into its own [`MirBlock`], assigning its value into `target`.
    /// used for a short-circuit branch body, where the contents must run only
    /// when the branch is taken (REDESIGN I5).
    pub(crate) fn lower_into_block(&mut self, e: ExprId, target: &Place) -> MirBlock {
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
