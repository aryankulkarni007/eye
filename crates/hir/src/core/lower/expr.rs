//! Expression lowering.

use ast::AstNode;
use rustc_hash::FxHashSet;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::{literal_type, lower_literal, lower_type_ref};
use crate::core::{
    Block, ConstError, EnumId, Expr, ExprId, Literal, MatchArm, Pat, PatternError, Resolution,
    ResolveError, StructLitField, Text, TypeError, TypeInterner, TypeKind, TypeRef,
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
                expr_type = Some(literal_type(&literal, &mut self.types));
                Expr::Literal(literal)
            }
            ast::Expr::NameRef(nr) => {
                let name: Text = self.text(nr.name());
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
                    // A const is a compile-time value, usable anywhere a value is.
                    Resolution::Const(_) | Resolution::LocalConst(_) => None,
                    // A global is addressable static storage: a readable value
                    // (and an assignable place when `mut`).
                    Resolution::Global(_) => None,
                    // A function name is a value (a function pointer) as well as
                    // a callee, so it is allowed in either position.
                    Resolution::Fn(_) => None,
                    Resolution::Unresolved(n)
                        if is_callee && (n == "println" || n == "len" || n == "sizeof") =>
                    {
                        None
                    }
                    Resolution::Enum(_) => {
                        Some(ResolveError::EnumNameAsValue { name: name.clone() })
                    }
                    Resolution::Struct(_) => {
                        Some(ResolveError::StructNameAsValue { name: name.clone() })
                    }
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
                    Resolution::Local(local_id) => self.body.locals[*local_id].ty,
                    // A const reference carries its declared type; the value is
                    // inlined at MIR lowering.
                    Resolution::Const(cid) => Some(self.hir.consts[*cid].ty),
                    Resolution::LocalConst(lcid) => Some(self.body.local_consts[*lcid].ty),
                    // A global reference carries its declared type; MIR reads the
                    // named C symbol (a place), not an inlined value.
                    Resolution::Global(gid) => Some(self.hir.globals[*gid].ty),
                    Resolution::Variant { enum_id, .. } => Some(
                        self.types.intern(TypeKind::Path(self.hir.enums[*enum_id].name.clone())),
                    ),
                    // A bare function name in value position is a function-pointer
                    // value of its signature (`let op = f;`). As a direct callee
                    // the type is unused - the call reads the function's return
                    // directly - and recording it would force a function-pointer
                    // typedef in codegen for every called function, so the callee
                    // case is left untyped.
                    Resolution::Fn(fn_id) if !is_callee => self.hir.functions[*fn_id].fn_type,
                    _ => None,
                };
                Expr::Path(resolution)
            }
            ast::Expr::CallExpr(c) => {
                let callee = self.lower_required_expr(c.callee(), ptr);
                // `sizeof(T)` kernel intrinsic: its argument is a *type*, not a
                // value, so it must be read from the AST before arg-lowering -
                // lowering `int32`/`Point` as a value would emit a spurious
                // `UnresolvedName`/`StructNameAsValue`. Recognized by an
                // unresolved callee name, so a user-defined `sizeof` (which
                // resolves to a `Fn`) shadows it, like `print`/`len`.
                if let Expr::Path(Resolution::Unresolved(name)) = &self.body.exprs[callee]
                    && name == "sizeof"
                {
                    return self.lower_sizeof_intrinsic(c, ptr);
                }
                let mut args: ThinVec<ExprId> = c
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
                    && name == "println"
                {
                    self.check_println_args(&args, ptr);
                }
                if let Expr::Path(Resolution::Fn(fn_id)) = &self.body.exprs[callee] {
                    let fn_id = *fn_id;
                    let fn_name = &self.hir.functions[fn_id].name;
                    let param_count = self.hir.functions[fn_id].params.len();
                    let variadic = self.hir.functions[fn_id].variadic;
                    // L3: the argument *count* is checked here - it never
                    // needs inference - while argument *types* wait for the
                    // typeck pass. A variadic extern (`printf(string, ...)`)
                    // sets a minimum (its named parameters) instead of an
                    // exact count. Indirect calls through a function-pointer
                    // value are not checked: `TypeKind::Fn` does not carry
                    // the variadic flag, so an exact-count check there would
                    // falsely reject a pointer to a variadic extern.
                    let arity_bad = if variadic {
                        args.len() < param_count
                    } else {
                        args.len() != param_count
                    };
                    if arity_bad {
                        self.emit(
                            ptr,
                            TypeError::CallArityMismatch {
                                name: fn_name.clone(),
                                expected: param_count,
                                found: args.len(),
                                variadic,
                            },
                        );
                    }
                    expr_type = self.hir.functions[fn_id].ret;
                    // Each argument with a declared parameter type goes
                    // through the single coercion point (array-literal
                    // re-typing, integer-literal typing, `&[T; N]` decay).
                    let param_tys: Vec<&TypeRef> = self.hir.functions[fn_id]
                        .params
                        .iter()
                        .map(|p| &p.ty)
                        .collect();
                    for i in 0..args.len().min(param_tys.len()) {
                        args[i] = self.coerce(param_tys[i], args[i]);
                    }
                } else if matches!(&self.body.exprs[callee], Expr::Path(Resolution::Unresolved(n)) if n == "println")
                {
                    // println intrinsic: arguments are checked above; the result
                    // is not a typed value.
                } else {
                    // A value callee: an indirect call, valid only through a
                    // function-pointer value.
                    if let Some(callee_ty) = self.body.expr_types.get(callee.into()).copied() {
                        let types = &self.types;
                        match types.lookup(callee_ty) {
                            TypeKind::Fn { ret, .. } => {
                                expr_type = *ret;
                            }
                            TypeKind::Error => {}
                            _ => {
                                let anchor = self.expr_ptr(callee, ptr);
                                let found = self.types.display(callee_ty).to_string();
                                self.emit(anchor, TypeError::CallNonFunction { found });
                                return self.missing_expr(ptr);
                            }
                        }
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
                    && let Some(elem_ty) = self.body.expr_types.get(first.into()).cloned()
                {
                    expr_type = Some(self.types.intern(TypeKind::Array {
                        elem: elem_ty,
                        len: elems.len() as u64,
                    }));
                }
                Expr::ArrayLit(elems)
            }
            ast::Expr::ArrayRepeat(ar) => {
                let value = self.lower_required_expr(ar.value(), ptr);
                // `count` is a const length, resolved by the same machinery as a
                // `[T; N]` type length: an integer literal or a const-expr,
                // `> 0`, not too large (diagnosed otherwise).
                let count = {
                    let consts = super::const_eval::ScopedConsts {
                        scopes: &self.scopes,
                        local_consts: &self.body.local_consts,
                        globals: self.const_values,
                    };
                    super::types::array_len(ar.count(), &mut self.diagnostics, &consts)
                };
                // Type as [elem; count] once the value's element type is known.
                // A declared array type (let/return/param) re-types the value
                // via `coerce_array_literal_type`.
                if let Some(count) = count
                    && let Some(elem_ty) = self.body.expr_types.get(value.into()).cloned()
                {
                    expr_type = Some(self.types.intern(TypeKind::Array {
                        elem: elem_ty,
                        len: count,
                    }));
                }
                // A failed `count` (None) has already emitted a diagnostic; 0 is
                // an inert placeholder the resolved pipeline never reaches.
                Expr::ArrayRepeat {
                    value,
                    count: count.unwrap_or(0),
                }
            }
            ast::Expr::IndexExpr(ie) => {
                let base = self.lower_required_expr(ie.base(), ptr);
                let index = self.lower_required_expr(ie.index(), ptr);
                let base_ty = self.body.expr_types.get(base.into()).cloned();
                // EXPERIMENTAL(L7): reject indexing on opaque `ptr` (void*),
                // which has no element type and would emit void-subscript C.
                if let Some(ty) = base_ty {
                    let types = &self.types;
                    if matches!(types.lookup(ty), TypeKind::Path(n) if n == "ptr") {
                        self.emit(ptr, TypeError::IndexOnPtr);
                    }
                }
                // A4: a literal index past a known fixed length is a hard error.
                // C would only warn. Dynamic indices stay unchecked (runtime
                // safety is deferred). Peel one ref/ptr so `r[i]` on `&[T; N]`
                // is checked too.
                let arr_len = base_ty.and_then(|ty| {
                    let types = &self.types;
                    match types.lookup(ty) {
                        TypeKind::Array { len, .. } => Some(*len),
                        TypeKind::Ref(inner) | TypeKind::Ptr(inner) => match types.lookup(*inner) {
                            TypeKind::Array { len, .. } => Some(*len),
                            _ => None,
                        },
                        _ => None,
                    }
                });
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
                expr_type = base_ty.and_then(|ty| {
                    let types = &self.types;
                    match types.lookup(ty) {
                        TypeKind::Array { elem, .. } => Some(*elem),
                        TypeKind::Ptr(inner) | TypeKind::Ref(inner) => match types.lookup(*inner) {
                            TypeKind::Array { elem, .. } => Some(*elem),
                            _ => Some(*inner),
                        },
                        _ => None,
                    }
                });
                Expr::Index { base, index }
            }
            ast::Expr::StructLit(sl) => {
                let lit_name: Option<Text> = sl
                    .name_ref()
                    .and_then(|n| n.name())
                    .map(|t| self.text(Some(t)));
                // L5: the literal's name must denote a declared struct or
                // union. An unknown name would otherwise be interned as
                // `Path("Foo")` and emitted verbatim into C ("use of
                // undeclared identifier"). The ids also drive the per-field
                // coercion below.
                let struct_id = lit_name
                    .as_ref()
                    .and_then(|n| self.hir.items.structs.get(n).copied());
                let union_id = lit_name
                    .as_ref()
                    .and_then(|n| self.hir.items.unions.get(n).copied());
                if let Some(ref name) = lit_name
                    && struct_id.is_none()
                    && union_id.is_none()
                {
                    self.emit(
                        SyntaxNodePtr::new(sl.syntax()),
                        ResolveError::UnknownStructLiteral { name: name.clone() },
                    );
                }
                let ty = match &lit_name {
                    Some(name) => self.types.intern(TypeKind::Path(name.clone())),
                    None => self.types.error_type(),
                };
                expr_type = Some(ty);
                let mut fields = ThinVec::new();
                // A positional initializer field (`Point { 1, 2 }`) carries no
                // field name. Lowering carries fields by name only, so the
                // value would be silently dropped (the struct would
                // zero-initialize) - rejected hard (M4). `saw_positional`
                // still suppresses the exhaustiveness check below so the
                // rejection does not cascade into "missing fields".
                let mut saw_positional = false;
                if let Some(fl) = sl.field_list() {
                    for f in fl.fields() {
                        let Some(fname_token) = f.name() else {
                            self.emit(
                                SyntaxNodePtr::new(f.syntax()),
                                TypeError::StructLitPositional,
                            );
                            saw_positional = true;
                            continue;
                        };
                        let fname = self.text(Some(fname_token));
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
                                            self.body.locals[*local_id].ty
                                        }
                                        _ => None,
                                    };
                                    let id = self.alloc_expr(Expr::Path(resolution), f_ptr);
                                    // FIXME: Add a regression test for unresolved
                                    // shorthand fields so this never reaches MIR.
                                    if let Some(t) = inner_ty {
                                        self.body.expr_types.insert(id.into(), t);
                                    }
                                    id
                                }
                            }
                        };
                        // The 5th coercion site (L1, the lang.eye compile
                        // blocker): a field value with a known declared field
                        // type goes through the single coercion point, so
                        // string decay and integer-literal typing work in
                        // field position like everywhere else. Unknown fields
                        // are diagnosed below (StructLitUnknownFields).
                        let field_ty: Option<TypeRef> = struct_id
                            .and_then(|sid| self.hir.structs[sid].field_index.get(&fname).copied())
                            .or_else(|| {
                                union_id.and_then(|uid| {
                                    self.hir.unions[uid].field_index.get(&fname).copied()
                                })
                            })
                            .map(|fid| self.hir.fields[fid].ty);
                        let value = match field_ty {
                            Some(ft) => self.coerce(&ft, value),
                            None => value,
                        };
                        fields.push(StructLitField { name: fname, value });
                    }
                }
                // A union literal sets exactly one member (overlapping
                // storage). More than one would silently overwrite; zero
                // leaves the value uninitialized.
                let name_union = {
                    let types = &self.types;
                    if let TypeKind::Path(name) = types.lookup(ty) {
                        Some(name.clone())
                    } else {
                        None
                    }
                };
                if let Some(ref name) = name_union
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
                let name_path = {
                    let types = &self.types;
                    if let TypeKind::Path(name) = types.lookup(ty) {
                        Some(name.clone())
                    } else {
                        None
                    }
                };
                if !saw_positional
                    && let Some(ref name) = name_path
                    && let Some(&sid) = self.hir.items.structs.get(name)
                {
                    let declared: Vec<Text> = self.hir.structs[sid]
                        .fields
                        .iter()
                        .map(|&fid| self.hir.fields[fid].name.clone())
                        .collect();
                    let field_names: FxHashSet<&Text> = fields.iter().map(|f| &f.name).collect();
                    let sl_ptr = SyntaxNodePtr::new(sl.syntax());
                    let missing: Vec<Text> = declared
                        .iter()
                        .filter(|d| !field_names.contains(d))
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
                    let declared_set: FxHashSet<&Text> = declared.iter().collect();
                    let unknown: Vec<Text> = fields
                        .iter()
                        .map(|f| f.name.clone())
                        .filter(|fname| !declared_set.contains(fname))
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
                let on_array = {
                    let types = &self.types;
                    let is_arr = |id: ExprId| {
                        self.body
                            .expr_types
                            .get(id.into())
                            .is_some_and(|ty| matches!(types.lookup(*ty), TypeKind::Array { .. }))
                    };
                    is_arr(lhs) || is_arr(rhs)
                };

                if on_array {
                    self.emit(ptr, TypeError::OpOnArray { op: bin_op_str(op) });
                    return self.missing_expr(ptr);
                }

                // P1: arithmetic/bitwise on `ptr` (the untyped pointer) would
                // emit C `void*` arithmetic - a GNU extension, rejected under
                // `-pedantic-errors`, with no element size to scale by.
                // Comparisons stay allowed (pointer equality/ordering are
                // well-defined); typed pointers (`T*`) keep C semantics.
                let is_comparison = matches!(
                    op,
                    ast::BinOp::Eq
                        | ast::BinOp::Neq
                        | ast::BinOp::Lt
                        | ast::BinOp::Gt
                        | ast::BinOp::Leq
                        | ast::BinOp::Geq
                        | ast::BinOp::And
                        | ast::BinOp::Or
                );
                if !is_comparison {
                    let on_ptr = {
                        let types = &self.types;
                        let is_ptr = |id: ExprId| {
                            self.body.expr_types.get(id.into()).is_some_and(
                                |ty| matches!(types.lookup(*ty), TypeKind::Path(n) if n == "ptr"),
                            )
                        };
                        is_ptr(lhs) || is_ptr(rhs)
                    };
                    if on_ptr {
                        self.emit(ptr, TypeError::ArithmeticOnPtr { op: bin_op_str(op) });
                        return self.missing_expr(ptr);
                    }
                }

                // `%` is integer-only. On a float it would lower to `double % double`,
                // which is invalid C (a raw clang error). Reject it here instead.
                if matches!(op, ast::BinOp::Rem) {
                    let is_float = |id: ExprId| {
                        let types = &self.types;
                        self.body.expr_types.get(id.into()).is_some_and(|ty| {
                            matches!(types.lookup(*ty), TypeKind::Path(p) if p == "float32" || p == "float64")
                        })
                    };
                    if is_float(lhs) || is_float(rhs) {
                        self.emit(ptr, TypeError::ModuloOnFloat);
                        return self.missing_expr(ptr);
                    }
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
                    | BinOp::Or => Some(
                        self.types.intern(TypeKind::Path(Text::from("bool"))),
                    ),
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
                    | BinOp::Shr => self.body.expr_types.get(lhs.into()).cloned(),
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
                    UnaryOp::Not => Some(
                        self.types.intern(TypeKind::Path(Text::from("bool"))),
                    ),
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        self.body.expr_types.get(operand.into()).cloned()
                    }
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
                    let base_name: Text = self.text(nr.name());
                    if let Some(&enum_id) = self.hir.items.enums.get(&base_name) {
                        let enum_def = &self.hir.enums[enum_id];
                        if let Some(&idx) = enum_def.variant_index.get(&name) {
                            let res = Resolution::Variant { enum_id, idx };
                            let ty = self.types.intern(TypeKind::Path(enum_def.name.clone()));
                            let id = self.alloc_expr(Expr::Path(res), ptr);
                            self.body.expr_types.insert(id.into(), ty);
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
                    .get(base.into())
                    .cloned()
                    .unwrap_or_else(|| self.types.error_type());
                // `.len` on an array is reserved for a future `.len()` method
                // (needs a real backend). Today length is read with the
                // `len(x)` intrinsic; steer there instead of emitting a field
                // access against the wrapper's nonexistent `len` member. One
                // ref/ptr is peeled so the steer fires through `&[T; N]` too.
                if name == "len"
                    && Self::peeled_array_len(base_ty, &self.types).is_some()
                {
                    self.emit(ptr, TypeError::LenFieldOnArray);
                    return self.missing_expr(ptr);
                }
                expr_type = Some(self.lookup_field_type(base_ty, &name));
                Expr::Field { base, name }
            }
            ast::Expr::AssignExpr(a) => {
                let op = a.op().unwrap_or(ast::AssignOp::Assign);
                let lhs = self.lower_required_expr(a.lhs(), ptr);
                let rhs = self.lower_required_expr(a.rhs(), ptr);
                // A const is a value, not storage: `MAX = ..` is rejected. (A
                // const is scalar, so it never roots a field/index projection.)
                let const_name = match &self.body.exprs[lhs] {
                    Expr::Path(Resolution::Const(cid)) => Some(self.hir.consts[*cid].name.clone()),
                    Expr::Path(Resolution::LocalConst(lcid)) => {
                        Some(self.body.local_consts[*lcid].name.clone())
                    }
                    _ => None,
                };
                if let Some(name) = const_name {
                    self.emit(self.expr_ptr(lhs, ptr), ConstError::AssignToConst { name });
                }
                // Immutable-by-default: writing a `let` binding (directly, or
                // through a field/index projection rooted in it) is rejected;
                // `mut` opts in. Covers every assignment form, plain and
                // compound. A write through a pointer is untracked (see
                // `immutable_assign_target`).
                if let Some(name) = self.immutable_assign_target(lhs) {
                    self.emit(
                        self.expr_ptr(lhs, ptr),
                        TypeError::AssignToImmutable { name },
                    );
                }
                // Assignment type is the type of the RHS.
                expr_type = self.body.expr_types.get(rhs.into()).cloned();
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
            ast::Expr::ReturnExpr(r) => {
                let value = r.expr().map(|e| self.lower_expr(&e));
                // The return value goes through the single coercion point
                // against the declared return type (decay, array-literal
                // re-typing, integer-literal typing).
                let value = match (self.fn_ret, value) {
                    (Some(ret), Some(id)) => Some(self.coerce(&ret, id)),
                    _ => value,
                };
                self.check_explicit_return(value, ptr);
                Expr::Return(value)
            }
            ast::Expr::RefExpr(r) => {
                let operand = self.lower_required_expr(r.expr(), ptr);
                // `&const` is illegal: a const is a value with no guaranteed
                // address (it is inlined). Reject it before it reaches MIR,
                // where it would silently take the address of an inlined temp.
                let const_name = match &self.body.exprs[operand] {
                    Expr::Path(Resolution::Const(cid)) => Some(self.hir.consts[*cid].name.clone()),
                    Expr::Path(Resolution::LocalConst(lcid)) => {
                        Some(self.body.local_consts[*lcid].name.clone())
                    }
                    _ => None,
                };
                if let Some(name) = const_name {
                    self.emit(ptr, ConstError::RefOfConst { name });
                    return self.missing_expr(ptr);
                }
                let inner_ty = self
                    .body
                    .expr_types
                    .get(operand.into())
                    .cloned()
                    .unwrap_or_else(|| self.types.error_type());
                expr_type = Some(self.types.intern(TypeKind::Ref(inner_ty)));
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
                    .get(operand.into())
                    .cloned()
                    .unwrap_or_else(|| self.types.error_type());
                // `ptr` (the untyped pointer, C `void*`) has no pointee type:
                // `*p` would emit a void indirection, a clang error under
                // `-pedantic`. Sibling of the L7 indexing reject; cast to a
                // typed pointer (`T*`) first.
                {
                    let is_ptr = {
                        let types = &self.types;
                        matches!(types.lookup(op_ty), TypeKind::Path(n) if n == "ptr")
                    };
                    if is_ptr {
                        self.emit(ptr, TypeError::DerefOfPtr);
                        return self.missing_expr(ptr);
                    }
                }
                let deref_ty = {
                    let types = &self.types;
                    match types.lookup(op_ty) {
                        TypeKind::Ref(inner) | TypeKind::Ptr(inner) => *inner,
                        _ => {
                            self.types.error_type()
                        }
                    }
                };
                expr_type = Some(deref_ty);
                Expr::Deref { operand }
            }
            ast::Expr::CastExpr(c) => {
                let operand = self.lower_required_expr(c.operand(), ptr);
                let consts = super::const_eval::ScopedConsts {
                    scopes: &self.scopes,
                    local_consts: &self.body.local_consts,
                    globals: self.const_values,
                };
                let ty = match c.ty() {
                    Some(t) => {
                        lower_type_ref(&t, &mut self.diagnostics, &consts, &mut self.types)
                    }
                    None => self.types.error_type(),
                };
                // R012: the cast target's type names must be declared. A
                // C-only type needs an `extern { type Name; }` declaration
                // first (the FFI opaque-type story).
                self.check_type_names(ty, ptr);
                expr_type = Some(ty);
                Expr::Cast { operand, ty }
            }
        };

        // allocate the expression and record its type if known
        let id = self.alloc_expr(hir_expr, ptr);
        if let Some(ty) = expr_type {
            self.body.expr_types.insert(id.into(), ty);
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

    /// If an assignment target ultimately writes an immutable `let` binding,
    /// return its name. The target roots in a local through field/index
    /// projections (`s.f = ..`, `a[i] = ..`); a deref (`*p = ..`) writes
    /// through a pointer and is deliberately not tracked - the raw-pointer
    /// escape, consistent with Eye's runtime-freedom model (KERNEL.md). Mutable
    /// bindings and non-local targets return `None`.
    fn immutable_assign_target(&self, place: ExprId) -> Option<Text> {
        match &self.body.exprs[place] {
            Expr::Path(Resolution::Local(id)) => {
                let local = &self.body.locals[*id];
                (!local.mutable).then(|| local.name.clone())
            }
            // A `let` global is read-only static storage; a `mut` global opts in.
            // Same immutable-by-default rule as a local binding.
            Expr::Path(Resolution::Global(gid)) => {
                let global = &self.hir.globals[*gid];
                (!global.mutable).then(|| global.name.clone())
            }
            Expr::Field { base, .. } | Expr::Index { base, .. } => {
                self.immutable_assign_target(*base)
            }
            _ => None,
        }
    }

    /// The static length of an array type, peeling one `&`/`*` so a reference
    /// or pointer to an array reports the same length as the array itself.
    /// `None` for any non-array type.
    fn peeled_array_len(ty: TypeRef, types: &TypeInterner) -> Option<u64> {
        match types.lookup(ty) {
            &TypeKind::Array { len, .. } => Some(len),
            &TypeKind::Ref(inner) | &TypeKind::Ptr(inner) => match types.lookup(inner) {
                &TypeKind::Array { len, .. } => Some(len),
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
    /// `println` is a primitive-only intrinsic (not a trait or macro yet): it
    /// has no format for a compound value. Reject array/struct/union arguments.
    fn check_println_args(&mut self, args: &[ExprId], ptr: SyntaxNodePtr) {
        for &arg in args.iter().skip(1) {
            let kind = {
                let types = &self.types;
                self.body
                    .expr_types
                    .get(arg.into())
                    .and_then(|ty| match types.lookup(*ty) {
                        TypeKind::Array { .. } => Some("an array"),
                        TypeKind::Path(name) if self.hir.items.structs.contains_key(name) => {
                            Some("a struct")
                        }
                        TypeKind::Path(name) if self.hir.items.unions.contains_key(name) => {
                            Some("a union")
                        }
                        _ => None,
                    })
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
                .get(args[0].into())
                .cloned()
                .unwrap_or_else(|| self.types.error_type());
            match Self::peeled_array_len(arg_ty, &self.types) {
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
        let usize_ty = self.types.usize_ty();
        let id = self.alloc_expr(
            Expr::Cast {
                operand: lit,
                ty: usize_ty,
            },
            ptr,
        );
        self.body.expr_types.insert(id.into(), usize_ty);
        id
    }

    /// Lower `sizeof(T)` to an `Expr::SizeOf` of type `usize`. The argument is a
    /// type read straight from the AST (see the call site): the floor accepts a
    /// bare named type only (`sizeof(int32)`, `sizeof(Point)`), matching the
    /// lenient type-name handling elsewhere - the name is not validated here, the
    /// C backend is the layout authority. Compound types (`sizeof(&T)`,
    /// `sizeof([T; N])`) and value arguments are rejected.
    fn lower_sizeof_intrinsic(&mut self, c: &ast::CallExpr, ptr: SyntaxNodePtr) -> ExprId {
        let args: Vec<ast::Expr> = c
            .arg_list()
            .map(|al| al.args().collect())
            .unwrap_or_default();
        let ty = if args.len() != 1 {
            self.emit(ptr, TypeError::SizeofArity { found: args.len() });
            self.types.error_type()
        } else if let ast::Expr::NameRef(nr) = &args[0] {
            let name = self.text(nr.name());
            self.types.intern(TypeKind::Path(name))
        } else {
            self.emit(
                SyntaxNodePtr::new(args[0].syntax()),
                TypeError::SizeofNotAType,
            );
            self.types.error_type()
        };
        let id = self.alloc_expr(Expr::SizeOf(ty), ptr);
        let usize_ty = self.types.usize_ty();
        self.body.expr_types.insert(id.into(), usize_ty);
        id
    }

    fn lower_match_expr(
        &mut self,
        me: &ast::MatchExpr,
        ptr: SyntaxNodePtr,
        expr_type: &mut Option<TypeRef>,
    ) -> Expr {
        let scrut = self.lower_required_expr(me.scrut(), ptr);

        // Classify the scrutinee's discriminant domain. enum / int / char / bool
        // are matchable; anything else still lowers (so the user keeps typing)
        // but the domain error below fires and per-arm domain checks are skipped.
        let domain = self.match_domain(scrut);
        let scrut_enum = match domain {
            MatchDomain::Enum(eid) => Some(eid),
            _ => None,
        };
        let scrut_ty_name: Text = match self.body.expr_types.get(scrut.into()).copied() {
            Some(ty) => {
                let types = &self.types;
                match types.lookup(ty) {
                    TypeKind::Path(name) => name.clone(),
                    _ => Text::from("<unknown>"),
                }
            }
            None => Text::from("<unknown>"),
        };
        if matches!(domain, MatchDomain::Other) {
            self.emit(self.expr_ptr(scrut, ptr), TypeError::MatchScrutineeNotEnum);
        }

        let mut arms: ThinVec<MatchArm> = ThinVec::new();
        let mut covered: Vec<bool> = match scrut_enum {
            Some(eid) => vec![false; self.hir.enums[eid].variants.len()],
            None => Vec::new(),
        };
        let mut saw_true = false;
        let mut saw_false = false;
        let mut saw_wildcard = false;
        let mut arm_type: Option<TypeRef> = None;

        if let Some(arm_list) = me.arm_list() {
            for arm in arm_list.arms() {
                let arm_ptr = SyntaxNodePtr::new(arm.syntax());
                let after_wildcard = saw_wildcard;
                // A guarded arm does NOT discharge coverage of its discriminant:
                // its guard may be false, leaving that case unmatched. So a
                // guarded arm marks nothing covered (and a guarded `_`/binding is
                // not the totalizing wildcard). Without this a guarded
                // full-coverage match with no `_` would be accepted, and the
                // value-position hoist temp could be read uninitialized.
                let has_guard = arm.guard().is_some();

                // Each arm gets its own scope so an arm binding (`x -> ..`) is
                // visible only in that arm's body.
                self.scopes.push();

                // A bare ident over a primitive scrutinee (int / char / bool) is
                // a BINDING, not a variant (the type-directed bare-ident rule):
                // there is no variant namespace to resolve against. It is
                // irrefutable, so it acts as a named wildcard. Over an enum a bare
                // ident stays a variant (handled by `lower_match_pat`).
                let bare_binding = matches!(
                    (arm.pat(), domain),
                    (
                        Some(ast::Pat::BareIdentPat(_)),
                        MatchDomain::Int | MatchDomain::Char | MatchDomain::Bool
                    )
                );
                let pat_id =
                    if let (true, Some(ast::Pat::BareIdentPat(bp))) = (bare_binding, arm.pat()) {
                        let name: Text = self.text(bp.name().and_then(|n| n.name()));
                        let bind_ty = self.types.intern(TypeKind::Path(scrut_ty_name.clone()));
                        let (pat_id, local_id) =
                            self.alloc_bind_pat(name.clone(), Some(bind_ty), false, arm_ptr);
                        self.scopes.define(name, local_id);
                        pat_id
                    } else {
                        match arm.pat() {
                            Some(p) => self.lower_match_pat(&p, scrut_enum),
                            None => self.alloc_pat(Pat::Missing, arm_ptr),
                        }
                    };
                if after_wildcard {
                    self.emit(arm_ptr, PatternError::UnreachableAfterWildcard);
                }
                // Read what we need out of the arena before any `self.emit`
                // (which borrows `self` mutably): copy the pattern shape so the
                // arena borrow ends here.
                let arm_pat = match &self.body.pats[pat_id] {
                    Pat::Wildcard => ArmPatShape::Wildcard,
                    Pat::Bind(_) => ArmPatShape::Binding,
                    Pat::Variant { enum_id, idx } => ArmPatShape::Variant(*enum_id, *idx),
                    Pat::Literal(lit) => ArmPatShape::Literal(lit.clone()),
                    // A struct pattern in a match arm is rejected at the parser
                    // (`GrammarError::StructPatInMatchArm`) and never reaches HIR
                    // as a `Pat::Struct`; `Pat::Missing` falls through to `Other`.
                    _ => ArmPatShape::Other,
                };
                match arm_pat {
                    // A binding is an irrefutable named wildcard: it makes the
                    // match total and shadows any following arm.
                    ArmPatShape::Wildcard | ArmPatShape::Binding => {
                        if !has_guard {
                            saw_wildcard = true;
                        }
                    }
                    ArmPatShape::Variant(enum_id, idx) => match domain {
                        MatchDomain::Enum(_) => {
                            let i = idx as usize;
                            if let Some(slot) = covered.get_mut(i) {
                                if *slot {
                                    let vname = self.hir.enums[enum_id].variants[i].name.clone();
                                    self.emit(
                                        arm_ptr,
                                        PatternError::DuplicateArm { variant: vname },
                                    );
                                }
                                if !has_guard {
                                    *slot = true;
                                }
                            }
                        }
                        MatchDomain::Bool | MatchDomain::Int | MatchDomain::Char => {
                            let vname = self.hir.enums[enum_id].variants[idx as usize].name.clone();
                            self.emit(
                                arm_ptr,
                                PatternError::PatternDomainMismatch {
                                    scrutinee: scrut_ty_name.clone(),
                                    pattern: vname,
                                },
                            );
                        }
                        MatchDomain::Other => {}
                    },
                    ArmPatShape::Literal(lit) => match domain {
                        // int and char are both integer comparisons in C, so a
                        // char literal against an int scrutinee (and vice versa)
                        // is allowed; only bool/enum are kept strict.
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
                        _ => self.emit(
                            arm_ptr,
                            PatternError::PatternDomainMismatch {
                                scrutinee: scrut_ty_name.clone(),
                                pattern: literal_pat_text(&lit),
                            },
                        ),
                    },
                    ArmPatShape::Other => {}
                }
                // Lower the optional guard expression. A guard is allowed on any
                // arm, including an irrefutable one (`x if ..` / `_ if ..`): MIR
                // lowers a guarded catch-all to an ordered `Always` arm with
                // fall-through, so a false guard moves on to the next arm. A
                // guarded arm does not discharge coverage (see `has_guard` above),
                // so a match with guards still needs an unconditional catch-all to
                // be exhaustive.
                let guard_id = arm
                    .guard()
                    .map(|g| self.lower_required_expr(g.expr(), arm_ptr));
                let body_id = self.lower_required_expr(arm.body(), arm_ptr);
                if arm_type.is_none() {
                    arm_type = self.body.expr_types.get(body_id.into()).cloned();
                }
                arms.push(MatchArm {
                    pat: pat_id,
                    guard: guard_id,
                    body: body_id,
                });

                self.scopes.pop();
            }
        }

        // Exhaustiveness, by domain. enum and bool have finite known universes,
        // so a missing case is an error; int and char are too large to
        // enumerate, so totality requires an explicit `_`.
        if !saw_wildcard {
            match domain {
                MatchDomain::Enum(eid) => {
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
                MatchDomain::Bool => {
                    let mut missing: Vec<Text> = Vec::new();
                    if !saw_false {
                        missing.push(Text::from("false"));
                    }
                    if !saw_true {
                        missing.push(Text::from("true"));
                    }
                    if !missing.is_empty() {
                        self.emit(
                            ptr,
                            PatternError::NonExhaustivePrimitive {
                                ty: Text::from("bool"),
                                missing,
                            },
                        );
                    }
                }
                MatchDomain::Int | MatchDomain::Char => self.emit(
                    ptr,
                    PatternError::NonExhaustivePrimitive {
                        ty: scrut_ty_name.clone(),
                        missing: Vec::new(),
                    },
                ),
                MatchDomain::Other => {}
            }
        }

        // Type of the whole match mirrors `if`: the first arm's body type.
        *expr_type = arm_type;
        Expr::Match { scrut, arms }
    }

    /// Classify a match scrutinee into its discriminant domain. Only a
    /// `TypeRef::Path` naming an enum or a primitive scalar is matchable.
    fn match_domain(&self, scrut: ExprId) -> MatchDomain {
        match self.body.expr_types.get(scrut.into()).copied() {
            Some(ty) => {
                let types = &self.types;
                match types.lookup(ty) {
                    TypeKind::Path(name) => {
                        if let Some(&eid) = self.hir.items.enums.get(name) {
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
            None => MatchDomain::Other,
        }
    }
}

/// A match scrutinee's discriminant domain (see `lower_match_expr`).
#[derive(Clone, Copy)]
enum MatchDomain {
    Enum(EnumId),
    Bool,
    Int,
    Char,
    Other,
}

/// A match arm pattern's shape, copied out of the body arena so the borrow ends
/// before the per-arm diagnostics (which borrow `self` mutably) run.
enum ArmPatShape {
    Wildcard,
    /// A bare-ident binding over a primitive scrutinee - an irrefutable named
    /// wildcard.
    Binding,
    Variant(EnumId, u32),
    Literal(Literal),
    Other,
}

fn is_int_type_name(n: &str) -> bool {
    matches!(
        n,
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "usize"
            | "isize"
    )
}

/// Display text for a literal pattern, used in a domain-mismatch diagnostic.
fn literal_pat_text(lit: &Literal) -> Text {
    match lit {
        Literal::Int(v) => Text::from(v.to_string()),
        Literal::Char(c) => Text::from(format!("'{c}'")),
        Literal::Bool(b) => Text::from(if *b { "true" } else { "false" }),
        // Float / string never reach a pattern (the parser excludes them).
        Literal::Float(s) | Literal::String(s) => Text::from(s.as_str()),
    }
}
