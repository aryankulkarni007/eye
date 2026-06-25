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
//!
//! split by concern (same layout as `typeck::infer` and `codegen::core`):
//! - this module: the [`lower_function`] entry, the [`Lower`] builder + its
//!   [`ArmKind`] helper, the body/tail drivers, and the shared infra
//!   (`collect_operands`, `terminated`, `map_local`, `mir_type_of`).
//! - [`stmt`]: statement lowering (`lower_stmt` / `lower_expr_stmt` / blocks /
//!   `return`) and the match-arm machinery shared by both positions.
//! - [`expr`]: value-position lowering - the rvalue / operand cores, the
//!   `lower_into` in-place lowering, and const inlining.
//! - [`place`]: place lowering (`lower_place` / `place_for_value`).

use hir::core::TypedArena;
use hir::core::{Body, Expr, ExprId, HIR, Literal, LocalId as HirLocalId};
use thin_vec::ThinVec;

use crate::core::*;

mod expr;
mod place;
mod stmt;

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
        self.typeck
            .expr_types
            .get(e.into())
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "MIR: typeck left {e:?} untyped - the walker must be total over \
                 every expression MIR lowers (S2C C5)"
                )
            })
    }
}
