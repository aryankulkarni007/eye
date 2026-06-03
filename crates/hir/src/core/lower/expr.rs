//! Expression lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::{literal_type, lower_literal, lower_type_ref};
use crate::core::{
    Block, ConstError, EnumId, Expr, ExprId, Literal, MatchArm, Pat, PatternError, Resolution,
    ResolveError, StructLitField, Text, TypeError, TypeRef,
};

/// The source spelling of a binary operator, for diagnostics.
fn bin_op_str(op: ast::BinOp) -> &'static str {
    use ast::BinOp::*;
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

impl<'a> LoweringCtx<'a> {
    pub(super) fn lower_expr(&mut self, expr: &ast::Expr) -> ExprId {
        let ptr = SyntaxNodePtr::new(expr.syntax());
        let mut expr_type: Option<TypeRef> = None;

        let hir_expr = match expr {
            ast::Expr::Literal(lit) => {
                let literal = lower_literal(lit);
                expr_type = Some(literal_type(&literal));
                Expr::Literal(literal)
            }
            ast::Expr::NameRef(nr) => {
                let name: Text = Self::text(nr.name());
                let resolution = self.resolve(&name);
                // Is this name the direct callee of a call (`f(...)`)? A function
                // and the `print`/`len` intrinsics are usable there but are not
                // bare values.
                let is_callee = nr
                    .syntax()
                    .parent()
                    .and_then(ast::CallExpr::cast)
                    .and_then(|c| c.callee())
                    .is_some_and(|callee| callee.syntax() == nr.syntax());
                // A name in value position must denote a value: a local or an
                // enum variant constant (or, in callee position, a function /
                // `print` / `len`). Every other resolution is misuse and is
                // rejected here, so a `Path` reaching codegen always denotes a
                // value - MIR relies on this (REDESIGN I2). Exhaustive over
                // `Resolution` so a new variant must decide its value-ness.
                let not_value: Option<ResolveError> = match &resolution {
                    Resolution::Local(_) | Resolution::Variant { .. } => None,
                    Resolution::Fn(_) if is_callee => None,
                    Resolution::Unresolved(n) if is_callee && (n == "print" || n == "len") => None,
                    Resolution::Enum(_) => {
                        Some(ResolveError::EnumNameAsValue { name: name.clone() })
                    }
                    Resolution::Struct(_) => {
                        Some(ResolveError::StructNameAsValue { name: name.clone() })
                    }
                    Resolution::Fn(_) => Some(ResolveError::FnAsValue { name: name.clone() }),
                    Resolution::Unresolved(_) => {
                        Some(ResolveError::UnresolvedName { name: name.clone() })
                    }
                };
                if let Some(err) = not_value {
                    self.emit(ptr, err);
                    return self.missing_expr(ptr);
                }
                // look up the type of the resolved entity.
                expr_type = match &resolution {
                    Resolution::Local(local_id) => self.body.locals[*local_id].ty.clone(),
                    Resolution::Variant { enum_id, .. } => {
                        Some(TypeRef::Path(self.hir.enums[*enum_id].name.clone()))
                    }
                    _ => None,
                };
                Expr::Path(resolution)
            }
            ast::Expr::CallExpr(c) => {
                let callee = self.lower_required_expr(c.callee(), ptr);
                let args: ThinVec<ExprId> = c
                    .arg_list()
                    .map(|al| al.args().map(|a| self.lower_expr(&a)).collect())
                    .unwrap_or_default();
                // `len(arr)` kernel intrinsic: folds to a compile-time `usize`
                // equal to the argument's static array length. Recognized by
                // name like `print`, so a user-defined `len` shadows it. Length
                // is type-level, so this returns a literal and the argument is
                // not evaluated. The `.len()` method form awaits a real backend.
                if let Expr::Path(Resolution::Unresolved(name)) = &self.body.exprs[callee]
                    && name == "len"
                {
                    return self.lower_len_intrinsic(&args, ptr);
                }
                if let Expr::Path(Resolution::Unresolved(name)) = &self.body.exprs[callee]
                    && name == "print"
                {
                    self.check_print_args(&args, ptr);
                }
                if let Expr::Path(Resolution::Fn(fn_id)) = &self.body.exprs[callee] {
                    let fn_id = *fn_id;
                    expr_type = self.hir.functions[fn_id].ret.clone();
                    // Coerce array-literal arguments to the declared param's
                    // array type, same as a `let`/return. The shared helper
                    // recurses through nested array literals and is
                    // length-guarded at every level, so an arity mismatch is
                    // not masked.
                    let param_tys: Vec<TypeRef> = self.hir.functions[fn_id]
                        .params
                        .iter()
                        .map(|p| p.ty.clone())
                        .collect();
                    for (arg, pty) in args.iter().zip(param_tys.iter()) {
                        self.coerce_array_literal_type(pty, *arg);
                    }
                }
                Expr::Call { callee, args }
            }
            ast::Expr::ArrayLit(al) => {
                let elems: ThinVec<ExprId> = al.elems().map(|e| self.lower_expr(&e)).collect();
                // Type as [elem; N] when the first element's type is known.
                // Integer-literal elements default to int32; a declared array
                // type (let/return/param) later re-types the whole nested
                // literal via `coerce_array_literal_type`.
                if let Some(&first) = elems.first()
                    && let Some(elem_ty) = self.body.expr_types.get(first).cloned()
                {
                    expr_type = Some(TypeRef::Array {
                        elem: Box::new(elem_ty),
                        len: elems.len() as u64,
                    });
                }
                Expr::ArrayLit(elems)
            }
            ast::Expr::IndexExpr(ie) => {
                let base = self.lower_required_expr(ie.base(), ptr);
                let index = self.lower_required_expr(ie.index(), ptr);
                let base_ty = self.body.expr_types.get(base).cloned();
                // A4: a literal index past a known fixed length is a hard error.
                // C would only warn. Dynamic indices stay unchecked (runtime
                // safety is deferred). Peel one ref/ptr so `r[i]` on `&[T; N]`
                // is checked too.
                let arr_len = match &base_ty {
                    Some(TypeRef::Array { len, .. }) => Some(*len),
                    Some(TypeRef::Ref(inner) | TypeRef::Ptr(inner)) => match inner.as_ref() {
                        TypeRef::Array { len, .. } => Some(*len),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(len) = arr_len {
                    if let Some(v) = self.const_uint_index(index) {
                        if v >= len as u128 {
                            self.emit(ptr, ConstError::IndexOutOfBounds { index: v, len });
                        }
                    } else if let Expr::Unary {
                        op: ast::UnaryOp::Neg,
                        operand,
                    } = &self.body.exprs[index]
                    {
                        // A negative literal lowers to `-(int)`; out of bounds
                        // for any length.
                        if matches!(&self.body.exprs[*operand], Expr::Literal(Literal::Int(v)) if *v > 0)
                        {
                            self.emit(ptr, ConstError::NegativeIndex);
                        }
                    }
                }
                // Element type is the base's element/pointee type, when known.
                // A reference or pointer to an array peels to the element type
                // so `r[i]` on `&[T; N]` yields `T` (not the whole array, which
                // would spuriously trip the binary-op-on-array check); a
                // reference or pointer to a non-array peels to the pointee.
                expr_type = base_ty.and_then(|t| match t {
                    TypeRef::Array { elem, .. } => Some(*elem),
                    TypeRef::Ptr(inner) | TypeRef::Ref(inner) => match *inner {
                        TypeRef::Array { elem, .. } => Some(*elem),
                        other => Some(other),
                    },
                    _ => None,
                });
                Expr::Index { base, index }
            }
            ast::Expr::StructLit(sl) => {
                let ty = match sl.name_ref().and_then(|n| n.name()) {
                    Some(t) => TypeRef::Path(Self::text(Some(t))),
                    None => TypeRef::Error,
                };
                expr_type = Some(ty.clone());
                let mut fields = ThinVec::new();
                // A positional initializer (`Point { 1, 2 }`) carries no field
                // name, so the exhaustiveness check below can't match by name -
                // it is suppressed when any positional field is present.
                let mut saw_positional = false;
                if let Some(fl) = sl.field_list() {
                    for f in fl.fields() {
                        let Some(fname_token) = f.name() else {
                            saw_positional = true;
                            continue;
                        };
                        let fname = Self::text(Some(fname_token));
                        let value = match f.value() {
                            Some(v) => self.lower_expr(&v),
                            None => {
                                // shorthand desugar: synthesize Path expr.
                                let resolution = self.resolve(&fname);
                                let f_ptr = SyntaxNodePtr::new(f.syntax());
                                // An unresolved shorthand names an undeclared
                                // local - a hard error, same as a bare name in
                                // value position (the `NameRef` arm). Diagnosing
                                // it here keeps every reachable `Unresolved` path
                                // rejected before codegen (I2). No `print`/`len`
                                // exception: a struct field is never a call, so
                                // those would be genuinely undeclared here.
                                if let Resolution::Unresolved(_) = &resolution {
                                    self.emit(
                                        f_ptr,
                                        ResolveError::UnresolvedName {
                                            name: fname.clone(),
                                        },
                                    );
                                    self.missing_expr(f_ptr)
                                } else {
                                    let inner_ty = match &resolution {
                                        Resolution::Local(local_id) => {
                                            self.body.locals[*local_id].ty.clone()
                                        }
                                        _ => None,
                                    };
                                    let id = self.alloc_expr(Expr::Path(resolution), f_ptr);
                                    // FIXME: Add a regression test for unresolved
                                    // shorthand fields so this never reaches MIR.
                                    if let Some(t) = inner_ty {
                                        self.body.expr_types.insert(id, t);
                                    }
                                    id
                                }
                            }
                        };
                        fields.push(StructLitField { name: fname, value });
                    }
                }
                // A union literal sets exactly one member (overlapping
                // storage). More than one would silently overwrite; zero
                // leaves the value uninitialized.
                if let TypeRef::Path(name) = &ty
                    && self.hir.items.unions.contains_key(name)
                    && fields.len() != 1
                {
                    self.emit(
                        SyntaxNodePtr::new(sl.syntax()),
                        TypeError::UnionLiteralFieldCount {
                            name: name.clone(),
                            found: fields.len(),
                        },
                    );
                }
                // F3 / S1: a struct literal must name every declared field
                // exactly once - missing fields leave silent garbage in C,
                // unknown fields are typos. Skipped for positional literals
                // (no names to match) and unions (handled above).
                if !saw_positional
                    && let TypeRef::Path(name) = &ty
                    && let Some(&sid) = self.hir.items.structs.get(name)
                {
                    let declared: Vec<Text> = self.hir.structs[sid]
                        .fields
                        .iter()
                        .map(|&fid| self.hir.fields[fid].name.clone())
                        .collect();
                    let sl_ptr = SyntaxNodePtr::new(sl.syntax());
                    let missing: Vec<Text> = declared
                        .iter()
                        .filter(|d| !fields.iter().any(|f| &f.name == *d))
                        .cloned()
                        .collect();
                    if !missing.is_empty() {
                        self.emit(
                            sl_ptr,
                            TypeError::StructLitMissingFields {
                                name: name.clone(),
                                fields: missing,
                            },
                        );
                    }
                    let unknown: Vec<Text> = fields
                        .iter()
                        .map(|f| f.name.clone())
                        .filter(|fname| !declared.iter().any(|d| d == fname))
                        .collect();
                    if !unknown.is_empty() {
                        self.emit(
                            sl_ptr,
                            TypeError::StructLitUnknownFields {
                                name: name.clone(),
                                fields: unknown,
                            },
                        );
                    }
                }
                Expr::StructLit { ty, fields }
            }
            ast::Expr::BinExpr(b) => {
                let Some(op) = b.op() else {
                    return self.missing_expr(ptr);
                };
                let lhs = self.lower_required_expr(b.lhs(), ptr);
                let rhs = self.lower_required_expr(b.rhs(), ptr);
                // A whole array is a struct in the C backend; any binary operator
                // on it emits invalid C. Reject it here (a reference compares as
                // a pointer, so only value arrays are caught).
                let on_array = matches!(self.body.expr_types.get(lhs), Some(TypeRef::Array { .. }))
                    || matches!(self.body.expr_types.get(rhs), Some(TypeRef::Array { .. }));

                if on_array {
                    self.emit(ptr, TypeError::OpOnArray { op: bin_op_str(op) });
                    return self.missing_expr(ptr);
                }

                // Comparison and logical operators produce `bool`; arithmetic
                // operators take the left operand's type (a simplification until
                // full inference exists).
                use ast::BinOp;
                expr_type = match op {
                    BinOp::Eq
                    | BinOp::Neq
                    | BinOp::Lt
                    | BinOp::Gt
                    | BinOp::Leq
                    | BinOp::Geq
                    | BinOp::And
                    | BinOp::Or => Some(TypeRef::Path(Text::from("bool"))),
                    // Arithmetic, modulo, and bitwise all take the left
                    // operand's type (a simplification until full inference).
                    BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::Rem
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr => self.body.expr_types.get(lhs).cloned(),
                };
                Expr::Binary { op, lhs, rhs }
            }
            ast::Expr::PrefixExpr(p) => {
                let Some(op) = p.op() else {
                    return self.missing_expr(ptr);
                };
                let operand = self.lower_required_expr(p.operand(), ptr);
                // `!` is logical-not: always `bool`. `-`/`~` preserve the
                // operand's type.
                use ast::UnaryOp;
                expr_type = match op {
                    UnaryOp::Not => Some(TypeRef::Path(Text::from("bool"))),
                    UnaryOp::Neg | UnaryOp::BitNot => self.body.expr_types.get(operand).cloned(),
                };
                Expr::Unary { op, operand }
            }
            ast::Expr::FieldExpr(fe) => {
                // Field name: the last NameRef child, not the first (avoids the
                // bug where the base is a bare NameRef).
                let name: Text = fe
                    .syntax()
                    .children()
                    .filter_map(ast::NameRef::cast)
                    .last()
                    .and_then(|nr| nr.name())
                    .map(|t| Text::from(t.text().trim()))
                    .unwrap_or_default();

                // Variant access shortcut: a bare NameRef base whose name is an
                // enum makes this `Enum.Variant`, not field access. Inspect the
                // AST before `lower_expr` so the NameRef arm's "enum as value"
                // diagnostic doesn't fire here.
                if let Some(ast::Expr::NameRef(nr)) = fe.expr() {
                    let base_name: Text = Self::text(nr.name());
                    if let Some(&enum_id) = self.hir.items.enums.get(&base_name) {
                        let enum_def = &self.hir.enums[enum_id];
                        if let Some(&idx) = enum_def.variant_index.get(&name) {
                            let res = Resolution::Variant { enum_id, idx };
                            let ty = TypeRef::Path(enum_def.name.clone());
                            let id = self.alloc_expr(Expr::Path(res), ptr);
                            self.body.expr_types.insert(id, ty);
                            return id;
                        } else {
                            self.emit(
                                ptr,
                                ResolveError::NoSuchVariant {
                                    enum_name: base_name.clone(),
                                    variant: name.clone(),
                                },
                            );
                            return self.missing_expr(ptr);
                        }
                    }
                }

                let base = self.lower_required_expr(fe.expr(), ptr);
                let base_ty = self
                    .body
                    .expr_types
                    .get(base)
                    .cloned()
                    .unwrap_or(TypeRef::Error);
                // `.len` on an array is reserved for a future `.len()` method
                // (needs a real backend). Today length is read with the
                // `len(x)` intrinsic; steer there instead of emitting a field
                // access against the wrapper's nonexistent `len` member. One
                // ref/ptr is peeled so the steer fires through `&[T; N]` too.
                if name == "len" && Self::peeled_array_len(&base_ty).is_some() {
                    self.emit(ptr, TypeError::LenFieldOnArray);
                    return self.missing_expr(ptr);
                }
                expr_type = Some(self.lookup_field_type(&base_ty, &name));
                Expr::Field { base, name }
            }
            ast::Expr::AssignExpr(a) => {
                let op = a.op().unwrap_or(ast::AssignOp::Assign);
                let lhs = self.lower_required_expr(a.lhs(), ptr);
                let rhs = self.lower_required_expr(a.rhs(), ptr);
                // Assignment type is the type of the RHS.
                expr_type = self.body.expr_types.get(rhs).cloned();
                Expr::Assign { op, lhs, rhs }
            }
            ast::Expr::IfExpr(i) => {
                let cond = self.lower_required_expr(i.condition(), ptr);
                // F2 (`if x = 5` is C's assignment-in-condition footgun) is
                // rejected in the parser now (GrammarError::AssignInIfCondition).

                let then_block =
                    i.then_branch()
                        .map(|b| self.lower_block(b))
                        .unwrap_or_else(|| {
                            let empty = Block {
                                stmts: ThinVec::new(),
                                tail: None,
                            };
                            self.alloc_block(empty, ptr)
                        });

                let else_block = i.else_branch().map(|b| self.lower_block(b));

                // The type of the if-expression is the type of the then-branch tail
                // (or else-branch tail as fallback).
                expr_type = self
                    .block_tail_type(then_block)
                    .or_else(|| else_block.and_then(|b| self.block_tail_type(b)));

                Expr::If {
                    cond,
                    then_branch: then_block,
                    else_branch: else_block,
                }
            }
            ast::Expr::LoopExpr(l) => {
                let body = l.body().map(|b| self.lower_block(b)).unwrap_or_else(|| {
                    let empty = Block {
                        stmts: ThinVec::new(),
                        tail: None,
                    };
                    self.alloc_block(empty, ptr)
                });
                Expr::Loop { body }
            }
            ast::Expr::BreakExpr(_) => Expr::Break,
            ast::Expr::ContinueExpr(_) => Expr::Continue,
            ast::Expr::RefExpr(r) => {
                let operand = self.lower_required_expr(r.expr(), ptr);
                let inner_ty = self
                    .body
                    .expr_types
                    .get(operand)
                    .cloned()
                    .unwrap_or(TypeRef::Error);
                expr_type = Some(TypeRef::Ref(Box::new(inner_ty)));
                Expr::Ref { operand }
            }
            ast::Expr::ParenExpr(pe) => {
                // A group is a pure precedence override - lower it to its inner
                // expression directly so no ParenExpr survives into HIR/codegen.
                return match pe.expr() {
                    Some(inner) => self.lower_expr(&inner),
                    None => self.missing_expr(ptr),
                };
            }
            ast::Expr::MatchExpr(me) => self.lower_match_expr(me, ptr, &mut expr_type),
            ast::Expr::DerefExpr(d) => {
                let operand = self.lower_required_expr(d.expr(), ptr);
                let op_ty = self
                    .body
                    .expr_types
                    .get(operand)
                    .cloned()
                    .unwrap_or(TypeRef::Error);
                let deref_ty = match &op_ty {
                    TypeRef::Ref(inner) | TypeRef::Ptr(inner) => (**inner).clone(),
                    _ => TypeRef::Error,
                };
                expr_type = Some(deref_ty);
                Expr::Deref { operand }
            }
            ast::Expr::CastExpr(c) => {
                let operand = self.lower_required_expr(c.operand(), ptr);
                let ty = c
                    .ty()
                    .map(|t| lower_type_ref(&t, &mut self.diagnostics))
                    .unwrap_or(TypeRef::Error);
                // A cast's value is its target type.
                expr_type = Some(ty.clone());
                Expr::Cast { operand, ty }
            }
        };

        // allocate the expression and record its type if known
        let id = self.alloc_expr(hir_expr, ptr);
        if let Some(ty) = expr_type {
            self.body.expr_types.insert(id, ty);
        }
        id
    }

    /// A statically-known non-negative index: a bare integer literal, or one
    /// behind a cast - notably the `(usize)N` a `len(x)` fold lowers to, so
    /// `a[len(a)]` is still caught as a static out-of-bounds. `None` if the
    /// index is not a compile-time constant.
    fn const_uint_index(&self, idx: ExprId) -> Option<u128> {
        match &self.body.exprs[idx] {
            Expr::Literal(Literal::Int(v)) => Some(*v),
            Expr::Cast { operand, .. } => match &self.body.exprs[*operand] {
                Expr::Literal(Literal::Int(v)) => Some(*v),
                _ => None,
            },
            _ => None,
        }
    }

    /// A place expression: one that names existing storage rather than
    /// computing a fresh value (a variable, field, index, or deref). Used to
    /// gate `len`, which reads a length from the type without evaluating the
    /// operand - restricting it to a place keeps a side-effecting expression
    /// like `len(f())` from being silently discarded. Note a place can still
    /// contain a call in an index position (`len(arr[f()])`), which this does
    /// not reject; that residual matches C's `sizeof` and is documented.
    fn is_place_expr(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Path(Resolution::Local(_))
                | Expr::Field { .. }
                | Expr::Index { .. }
                | Expr::Deref { .. }
        )
    }

    /// The static length of an array type, peeling one `&`/`*` so a reference
    /// or pointer to an array reports the same length as the array itself.
    /// `None` for any non-array type.
    fn peeled_array_len(ty: &TypeRef) -> Option<u64> {
        match ty {
            TypeRef::Array { len, .. } => Some(*len),
            TypeRef::Ref(inner) | TypeRef::Ptr(inner) => match inner.as_ref() {
                TypeRef::Array { len, .. } => Some(*len),
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower the `len(arr)` intrinsic to a compile-time `usize` literal equal to
    /// the argument's static array length. Accepts `[T; N]` and `&[T; N]` (one
    /// ref/ptr is peeled). Wrong arity or a non-array argument is a diagnostic;
    /// the result is then a `0` placeholder, still typed `usize`, so downstream
    /// type information stays intact.
    /// `print` is a primitive-only intrinsic (not a trait or macro yet): it has
    /// no format for a compound value. Reject array/struct/union arguments.
    fn check_print_args(&mut self, args: &[ExprId], ptr: SyntaxNodePtr) {
        for &arg in args.iter().skip(1) {
            let kind = match self.body.expr_types.get(arg) {
                Some(TypeRef::Array { .. }) => Some("an array"),
                Some(TypeRef::Path(name)) if self.hir.items.structs.contains_key(name) => {
                    Some("a struct")
                }
                Some(TypeRef::Path(name)) if self.hir.items.unions.contains_key(name) => {
                    Some("a union")
                }
                _ => None,
            };
            if let Some(kind) = kind {
                self.emit(
                    self.expr_ptr(arg, ptr),
                    TypeError::PrintCannotFormat { kind },
                );
            }
        }
    }

    fn lower_len_intrinsic(&mut self, args: &[ExprId], ptr: SyntaxNodePtr) -> ExprId {
        let len = if args.len() != 1 {
            self.emit(ptr, TypeError::LenArity { found: args.len() });
            0
        } else if !Self::is_place_expr(&self.body.exprs[args[0]]) {
            // `len` reads the length from the operand's static type and never
            // evaluates the operand - just like C's `sizeof`. So `len(f())`
            // would silently discard the call. Restrict the operand to a place
            // (variable, field, index, or deref), where nothing is computed, so
            // that footgun cannot arise. Go's `len` has the same shape.
            self.emit(self.expr_ptr(args[0], ptr), TypeError::LenNotAPlace);
            0
        } else {
            let arg_ty = self
                .body
                .expr_types
                .get(args[0])
                .cloned()
                .unwrap_or(TypeRef::Error);
            match Self::peeled_array_len(&arg_ty) {
                Some(len) => len,
                None => {
                    self.emit(self.expr_ptr(args[0], ptr), TypeError::LenNotArray);
                    0
                }
            }
        };
        // Emit as `(usize)N` so the C literal carries `size_t` type. Printed with
        // `%zu` a bare `int` literal would be a varargs type mismatch (UB on LP64).
        let lit = self.alloc_expr(Expr::Literal(Literal::Int(len as u128)), ptr);
        let usize_ty = TypeRef::Path(Text::from("usize"));
        let id = self.alloc_expr(
            Expr::Cast {
                operand: lit,
                ty: usize_ty.clone(),
            },
            ptr,
        );
        self.body.expr_types.insert(id, usize_ty);
        id
    }

    fn lower_match_expr(
        &mut self,
        me: &ast::MatchExpr,
        ptr: SyntaxNodePtr,
        expr_type: &mut Option<TypeRef>,
    ) -> Expr {
        let scrut = self.lower_required_expr(me.scrut(), ptr);

        // Identify the scrutinee enum (if any). Only TypeRef::Path
        // pointing at a known enum carries match semantics; anything
        // else still lowers but skips exhaustiveness so user keeps
        // typing without a cascade of follow-on diagnostics.
        let scrut_enum: Option<EnumId> = match self.body.expr_types.get(scrut) {
            Some(TypeRef::Path(name)) => self.hir.items.enums.get(name).copied(),
            _ => None,
        };
        if scrut_enum.is_none() {
            self.emit(self.expr_ptr(scrut, ptr), TypeError::MatchScrutineeNotEnum);
        }

        let mut arms: ThinVec<MatchArm> = ThinVec::new();
        let mut covered: Vec<bool> = match scrut_enum {
            Some(eid) => vec![false; self.hir.enums[eid].variants.len()],
            None => Vec::new(),
        };
        let mut saw_wildcard = false;
        let mut arm_type: Option<TypeRef> = None;

        if let Some(arm_list) = me.arm_list() {
            for arm in arm_list.arms() {
                let arm_ptr = SyntaxNodePtr::new(arm.syntax());
                let after_wildcard = saw_wildcard;
                let pat_id = match arm.pat() {
                    Some(p) => self.lower_match_pat(&p, scrut_enum),
                    None => self.alloc_pat(Pat::Missing, arm_ptr),
                };
                if after_wildcard {
                    self.emit(arm_ptr, PatternError::UnreachableAfterWildcard);
                }
                match &self.body.pats[pat_id] {
                    Pat::Wildcard => saw_wildcard = true,
                    Pat::Variant { idx, .. } => {
                        let i = (*idx) as usize;
                        if let Some(slot) = covered.get_mut(i) {
                            if *slot {
                                let vname = scrut_enum
                                    .map(|eid| self.hir.enums[eid].variants[i].name.clone())
                                    .unwrap_or_default();
                                self.emit(arm_ptr, PatternError::DuplicateArm { variant: vname });
                            }
                            *slot = true;
                        }
                    }
                    _ => {}
                }
                let body_id = self.lower_required_expr(arm.body(), arm_ptr);
                if arm_type.is_none() {
                    arm_type = self.body.expr_types.get(body_id).cloned();
                }
                arms.push(MatchArm {
                    pat: pat_id,
                    body: body_id,
                });
            }
        }

        // NOTE: IMPORTANT! Exhaustiveness: every variant must be covered unless `_`
        // catches the rest. Skipped when scrutinee isn't a known enum
        // (the upstream diag already told the user).
        if !saw_wildcard && let Some(eid) = scrut_enum {
            let missing: Vec<Text> = self.hir.enums[eid]
                .variants
                .iter()
                .enumerate()
                .filter(|(i, _)| !covered[*i])
                .map(|(_, v)| v.name.clone())
                .collect();
            if !missing.is_empty() {
                let enum_name = self.hir.enums[eid].name.clone();
                self.emit(ptr, PatternError::NonExhaustive { enum_name, missing });
            }
        }

        // Type of the whole match mirrors `if`: the first arm's body
        // type. Good enough for M5 codegen + M6 e2e.
        *expr_type = arm_type;
        Expr::Match { scrut, arms }
    }
}
