//! the bidirectional walker (tier 1) - the sole type authority since the S2C
//! cutover. lowering no longer stamps any expression; this pass interns every
//! `expr_types` entry codegen and mir read.
//!
//! split by concern (same layout as `lower/` and `codegen::core`):
//! - this module: the [`InferCtx`] context, the driver ([`InferCtx::run`]), the
//!   bidirectional spine (`infer_stmt` / `infer_block` / `infer_expr` /
//!   `infer_call`), the diagnostic infra, and the type-lookup helpers.
//! - [`coerce`]: the tier-2 funnel (`expect`) and its coercion / adoption mirror.
//! - [`judgments`]: the standalone judgments - the `check_*` value-position /
//!   consistency / range sweeps, the binary / index operator judgments, and
//!   match analysis.
//! - [`ty`]: the free type predicates and the `as` cast lattice (CAST.md).
//!
//! rules still carrying a `PARITY(S3)` marker were faithful ports of lowering's
//! old stamping, kept bug-compatible only while the shadow oracle ran. the
//! oracle retired at C5, so these are now fixed in place, each with its own
//! test (M2 - mixed-width arithmetic - is the first).

use ast::UnaryOp;
use hir::core::{
    BlockId, Body, Expr, ExprId, HIR, HirError, Literal, Pat, Resolution, Stmt, StmtId, Text,
    TypeInterner, TypeKind, TypeRef,
};
use syntax::SyntaxNodePtr;

use crate::{Cause, Expectation, InferObserver, ObserverCx, TypeckResults};

mod coerce;
mod judgments;
mod ty;

use ty::*;

pub(crate) struct InferCtx<'a, O> {
    pub(crate) scope: &'a HIR,
    pub(crate) body: &'a Body,
    pub(crate) types: &'a TypeInterner,
    pub(crate) fn_ret: Option<TypeRef>,
    pub(crate) results: TypeckResults,
    pub(crate) obs: &'a mut O,
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
            // a `-> ()` return is the void return: the body completes with unit,
            // needs no explicit `return`, and discards its tail. normalizing it
            // to `None` here makes the explicit unit return behave identically to
            // the implicit void one across every return/tail judgment.
            fn_ret: fn_ret.filter(|&t| t != types.unit_ty()),
            results: TypeckResults::default(),
            obs,
        }
    }

    pub(crate) fn run(mut self) -> TypeckResults {
        for &stmt in &self.body.block {
            self.infer_stmt(stmt);
        }
        if let Some(tail) = self.body.tail {
            // the tail is checked against the declared return type by the spine:
            // the `Return` expectation flows down so a branch/arm literal adopts
            // the return width and the funnel coerces the tail node onto it (a
            // value-position match/`if` is restamped, MIR reading its hoist temp
            // from the result). the per-branch/arm mismatches surface through the
            // consistency checks; the tail-vs-return mismatch through the funnel;
            // the *arity* (a body that yields no value) through `enforce_return_type`.
            let expected = match self.fn_ret {
                Some(ret) => Expectation::HasType(ret, Cause::Return),
                None => Expectation::None,
            };
            self.infer_expr(tail, expected);
        }
        self.enforce_return_type();
        self.check_int_literal_ranges();
        self.check_match_arm_consistency();
        self.check_if_branch_consistency();
        self.check_matches();
        self.check_value_position_assignments();
        self.check_value_position_voids();
        self.results
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
                    // the declared type flows down the spine as a `LetDecl`
                    // expectation: literals adopt its width and a value-position
                    // match/`if` initializer is restamped onto it (the funnel's
                    // container restamp, formerly `record_match_result_override`).
                    // `LetDecl` is adopt-only - the let-init mismatch stays in
                    // `check_explicit_let_init_type` (Call-init, pending the
                    // let-init width ruling).
                    let expected = match ty {
                        Some(declared) => Expectation::HasType(declared, Cause::LetDecl),
                        None => Expectation::None,
                    };
                    self.infer_expr(init, expected);
                    if let Some(declared) = ty {
                        // let-initializer judgments (moved from lowering, S2
                        // step b), against the explicit declared type.
                        let stmt_ptr = self.body.source_map.stmt.get(id.into()).cloned();
                        self.check_array_init_len(declared, init, stmt_ptr);
                        self.check_explicit_let_init_type(declared, init, stmt_ptr);
                    }
                }
            }
            Stmt::Expr(e) => {
                self.infer_expr(*e, Expectation::None);
            }
            // purely compile-time: the value is folded into
            // `body.local_consts`, no expressions to type.
            Stmt::Const(_) => {}
        }
    }

    fn infer_block(&mut self, id: BlockId, expected: Expectation) -> Option<TypeRef> {
        // same lifetime decouple as `infer_expr`: a `&Body` copy lets the
        // block's stmts be iterated while calling `&mut self`, no `ThinVec`
        // clone.
        let body = self.body;
        let tail = body.blocks[id].tail;
        for &stmt in &body.blocks[id].stmts {
            self.infer_stmt(stmt);
        }
        match tail {
            // a block is transparent: its value is its tail's, so the
            // expectation flows straight through (the tail adopts/funnels it).
            Some(t) => {
                self.infer_expr(t, expected);
                self.ty_of(t)
            }
            // a tail-less block ran its statements for effect and completes with
            // unit (`()`) - unless its last statement diverges (`{ ..; return; }`),
            // in which case control never reaches the end and the block is
            // `Never`. before the unit type this was `None`, which left a
            // value-position block untyped and ICEd MIR.
            None => {
                let diverges = body.blocks[id].stmts.last().is_some_and(|&s| {
                    matches!(&body.stmts[s], Stmt::Expr(e) if self.is_never(*e))
                });
                Some(if diverges {
                    self.types.never_ty()
                } else {
                    self.types.unit_ty()
                })
            }
        }
    }

    /// the bidirectional spine: type one expression, with `expected` flowing
    /// *down* from the site that uses its value (tier 2, TYPECK.md). a
    /// transparent node (block, `if`, `match`) forwards the expectation to its
    /// value-producing children; an imposing site (a call argument, a struct
    /// field) starts a fresh one; an operand position passes `None`. the
    /// bottom-up type is then funneled through [`Self::expect`], which adopts a
    /// literal to the expected width, records array decay, and reports a
    /// site-specific mismatch. returns the type the expression settles on
    /// (`None` = unstamped).
    fn infer_expr(&mut self, id: ExprId, expected: Expectation) -> Option<TypeRef> {
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
                // operands synthesize bottom-up: the binary's expectation does
                // not flow into them (M2 makes the operands determine the result
                // type, not the reverse).
                self.infer_expr(lhs, Expectation::None);
                self.infer_expr(rhs, Expectation::None);
                self.binary_judgments(id, op, lhs, rhs)
            }
            Expr::Unary { op, operand } => {
                let (op, operand) = (*op, *operand);
                self.infer_expr(operand, Expectation::None);
                // opaque enums (T035): `-`/`~` on an enum value is arithmetic
                // and rejected, like the binary operators. the expr keeps the
                // operand's type below - unused, since a rejected program never
                // reaches codegen.
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
                // elements synthesize bottom-up; a declared element type adopts
                // them (and runs the L4 per-element judgment) at the funnel, via
                // `coerce_array_literal` on this node.
                for &e in elems {
                    self.infer_expr(e, Expectation::None);
                }
                let len = elems.len() as u64;
                elems
                    .first()
                    .and_then(|&first| self.ty_of(first))
                    .map(|elem| self.types.intern(TypeKind::Array { elem, len }))
            }
            Expr::ArrayRepeat { value, count } => {
                let (value, count) = (*value, *count);
                self.infer_expr(value, Expectation::None);
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
                self.infer_expr(base, Expectation::None);
                self.infer_expr(index, Expectation::None);
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
                    // each field value is checked against its declared type by
                    // the funnel (the `Field` expectation): it adopts a literal
                    // to the field width and reports `StructFieldTypeMismatch`
                    // (`P { x: "hi" }` with `int32 x`). a field with no declared
                    // type (an unknown field, diagnosed elsewhere) synthesizes.
                    let expected = struct_name
                        .as_ref()
                        .and_then(|sname| self.field_decl_type(sname, &f.name))
                        .map_or(Expectation::None, |ft| {
                            Expectation::HasType(ft, Cause::Field { name: f.name.clone() })
                        });
                    self.infer_expr(f.value, expected);
                }
                Some(lit_ty)
            }
            Expr::Field { base, name } => {
                let base = *base;
                self.infer_expr(base, Expectation::None);
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
                self.infer_expr(lhs, Expectation::None);
                self.infer_expr(rhs, Expectation::None);
                // assignment yields unit (`()`), Rust's rule; it is ruled
                // non-value (S3), so a value-position use is rejected by
                // `check_value_position_assignments` with its own message.
                Some(self.types.unit_ty())
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let (cond, then_branch, else_branch) = (*cond, *then_branch, *else_branch);
                self.infer_expr(cond, Expectation::None);
                // the expectation flows into both branch tails (re-tagged
                // `IfBranch`, so a branch literal adopts the expected width); the
                // per-branch mismatch is `check_if_branch_consistency` against the
                // `if`'s settled type, not the funnel.
                let branch_expected = rebind(&expected, Cause::IfBranch);
                let then_ty = self.infer_block(then_branch, branch_expected.clone());
                match else_branch {
                    // an else-less `if` completes with unit: the false path
                    // yields no value, so the whole `if` is `()`. a value-yielding
                    // then branch is simply discarded (legal as a statement);
                    // binding the `()` result in value position is rejected by
                    // the completeness sweep.
                    None => Some(self.types.unit_ty()),
                    // both branches present: the never-absorbing join, so a
                    // diverging branch (`else { return }`) takes the other's type.
                    Some(b) => {
                        let else_ty = self.infer_block(b, branch_expected);
                        self.join_opt(then_ty, else_ty)
                    }
                }
            }
            Expr::Loop { body: loop_body } => {
                let loop_body = *loop_body;
                self.infer_block(loop_body, Expectation::None);
                // a loop that can `break` out completes with unit; one with no
                // reachable break never returns control, so it is `Never`.
                if self.loop_has_break(loop_body) {
                    Some(self.types.unit_ty())
                } else {
                    Some(self.types.never_ty())
                }
            }
            // a `break`/`continue` transfers control away and never yields a
            // value here: the never type, which coerces to any expectation.
            Expr::Break | Expr::Continue => Some(self.types.never_ty()),
            Expr::Return(value) => {
                let value = *value;
                if let Some(v) = value {
                    // the returned value is checked against the declared return
                    // by the funnel (the `Return` expectation: literal adoption +
                    // `ReturnTypeMismatch`); `check_explicit_return` keeps only
                    // the *arity* judgments (a value in a void fn, a missing value).
                    let expected = match self.fn_ret {
                        Some(ret) => Expectation::HasType(ret, Cause::Return),
                        None => Expectation::None,
                    };
                    self.infer_expr(v, expected);
                }
                self.check_explicit_return(id, value);
                Some(self.types.never_ty())
            }
            Expr::Ref { operand } => {
                let operand = *operand;
                self.infer_expr(operand, Expectation::None);
                let inner = self
                    .ty_of(operand)
                    .unwrap_or_else(|| self.types.error_type());
                Some(self.types.intern(TypeKind::Ref(inner)))
            }
            Expr::Deref { operand } => {
                let operand = *operand;
                self.infer_expr(operand, Expectation::None);
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
                self.infer_expr(operand, Expectation::None);
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
                self.infer_expr(scrut, Expectation::None);
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
                // type of the whole match mirrors `if`: the join of the arm
                // body types, with `Never` arms absorbed so the first
                // value-yielding arm wins. the expectation flows into each arm
                // body (re-tagged `MatchArm`, so an arm literal adopts the
                // expected width); the per-arm mismatch is
                // `check_match_arm_consistency` against the match's settled type.
                let arm_expected = rebind(&expected, Cause::MatchArm);
                let mut arm_type: Option<TypeRef> = None;
                for arm in arms {
                    if let Some(g) = arm.guard {
                        self.infer_expr(g, Expectation::None);
                    }
                    self.infer_expr(arm.body, arm_expected.clone());
                    arm_type = self.join_opt(arm_type, self.ty_of(arm.body));
                }
                arm_type
            }
            Expr::SizeOf(_) => Some(self.types.usize_ty()),
            Expr::Len(operand) => {
                let operand = *operand;
                self.infer_expr(operand, Expectation::None);
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
                self.infer_block(b, expected.clone())
            }
        };
        // record the bottom-up type provisionally, then let the observer see it
        // (effects read the pre-funnel type, unchanged by the spine) before the
        // funnel adopts/coerces it against the expectation.
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
        self.expect(id, ty, expected)
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
                // each argument flows down with its parameter's declared type as
                // an `Arg` expectation: the funnel adopts a literal to the
                // parameter width and reports `ArgTypeMismatch` (swapped or
                // wrong-typed args). `self.scope` is a shared `&HIR`; reading the
                // param type before `infer_expr` (which takes `&mut self`) keeps
                // the borrow short. extra args (variadic) have no parameter and
                // synthesize uncoerced.
                let scope = self.scope;
                for (i, &a) in args.iter().enumerate() {
                    let expected = match scope.functions[fn_id].params.get(i) {
                        Some(param) => {
                            Expectation::HasType(param.ty, Cause::Arg { index: i + 1 })
                        }
                        None => Expectation::None,
                    };
                    self.infer_expr(a, expected);
                }
                // a call to a function with no return type yields unit (`()`).
                self.scope.functions[fn_id]
                    .ret
                    .or_else(|| Some(self.types.unit_ty()))
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
                    self.infer_expr(a, Expectation::None);
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
                // an unresolved callee (`println`, or a genuinely undeclared
                // name already diagnosed) has an unknown result: poison, so a
                // value-position use never cascades into a spurious mismatch.
                Some(self.types.error_type())
            }
            // indirect call through a function-pointer value. a callee that is
            // neither a function pointer nor `Error` is not callable
            // (callnonfunction, relocated from lowering at S2C C5); the result
            // is poison.
            _ => {
                let callee_ty = self.infer_expr(callee, Expectation::None);
                for &a in args {
                    self.infer_expr(a, Expectation::None);
                }
                match callee_ty {
                    Some(ty) => match self.types.lookup(ty) {
                        // a void function pointer's call yields unit.
                        TypeKind::Fn { ret, .. } => {
                            ret.or_else(|| Some(self.types.unit_ty()))
                        }
                        TypeKind::Error => Some(self.types.error_type()),
                        _ => {
                            let found = self.types.display(ty).to_string();
                            self.emit_at(callee, None, hir::core::TypeError::CallNonFunction { found });
                            Some(self.types.error_type())
                        }
                    },
                    None => Some(self.types.error_type()),
                }
            }
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

/// re-tag an expectation with a new cause, keeping its type (`None` stays
/// `None`). used to forward an expectation into an `if`/`match` branch or arm as
/// an `adopt-only` cause (`IfBranch`/`MatchArm`): the branch/arm literal adopts
/// the expected width, while the per-branch mismatch is reported by the
/// consistency check, not the funnel.
fn rebind(expected: &Expectation, cause: Cause) -> Expectation {
    match expected {
        Expectation::HasType(ty, _) => Expectation::HasType(*ty, cause),
        Expectation::None => Expectation::None,
    }
}
