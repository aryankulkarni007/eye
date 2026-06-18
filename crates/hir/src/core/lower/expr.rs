//! expression lowering.

use ast::AstNode;
use rustc_hash::FxHashSet;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::{lower_literal, lower_type_ref};
use crate::core::{
    Block, ConstError, Expr, ExprId, Literal, MatchArm, Pat, Resolution, ResolveError,
    StructLitField, Text, TypeError, TypeKind,
};

impl<'a> LoweringCtx<'a> {
    pub(super) fn lower_expr(&mut self, expr: &ast::Expr) -> ExprId {
        let ptr = SyntaxNodePtr::new(expr.syntax());

        let hir_expr = match expr {
            ast::Expr::Literal(lit) => {
                let literal = lower_literal(lit);
                self.check_char_literal(&literal, ptr);
                Expr::Literal(literal)
            }
            ast::Expr::NameRef(nr) => {
                let name: Text = self.text(nr.name());
                let resolution = self.resolve(&name);
                // is this name the direct callee of a call (`f(...)`)? a function
                // and the `print`/`len` intrinsics are usable there but are not
                // bare values.
                let is_callee = nr
                    .syntax()
                    .parent()
                    .and_then(ast::CallExpr::cast)
                    .and_then(|c| c.callee())
                    .is_some_and(|callee| callee.syntax() == nr.syntax());
                // a name in value position must denote a value: a local or an
                // enum variant constant (or, in callee position, a function /
                // `print` / `len`). every other resolution is misuse and is
                // rejected here, so a `Path` reaching codegen always denotes a
                // value - MIR relies on this (REDESIGN I2). exhaustive over
                // `Resolution` so a new variant must decide its value-ness.
                let not_value: Option<ResolveError> = match &resolution {
                    Resolution::Local(_) | Resolution::Variant { .. } => None,
                    // a const is a compile-time value, usable anywhere a value is.
                    Resolution::Const(_) | Resolution::LocalConst(_) => None,
                    // a global is addressable static storage: a readable value
                    // (and an assignable place when `mut`).
                    Resolution::Global(_) => None,
                    // a function name is a value (a function pointer) as well as
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
                Expr::Path(resolution)
            }
            ast::Expr::CallExpr(c) => {
                let callee = self.lower_required_expr(c.callee(), ptr);
                // `sizeof(T)` kernel intrinsic: its argument is a *type*, not a
                // value, so it must be read from the AST before arg-lowering -
                // lowering `int32`/`Point` as a value would emit a spurious
                // `UnresolvedName`/`StructNameAsValue`. recognized by an
                // unresolved callee name, so a user-defined `sizeof` (which
                // resolves to a `Fn`) shadows it, like `print`/`len`.
                if let Expr::Path(Resolution::Unresolved(name)) = &self.body.exprs[callee]
                    && name == "sizeof"
                {
                    return self.lower_sizeof_intrinsic(c, ptr);
                }
                let args: ThinVec<ExprId> = c
                    .arg_list()
                    .map(|al| al.args().map(|a| self.lower_expr(&a)).collect())
                    .unwrap_or_default();
                // `len(arr)` kernel intrinsic: folds to a compile-time `usize`
                // equal to the argument's static array length. recognized by
                // name like `print`, so a user-defined `len` shadows it. length
                // is type-level, so this returns a literal and the argument is
                // not evaluated. the `.len()` method form awaits a real backend.
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
                    // typeck pass. a variadic extern (`printf(string, ...)`)
                    // sets a minimum (its named parameters) instead of an
                    // exact count. indirect calls through a function-pointer
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
                    // argument-type coercion + checks moved to the typeck pass
                    // (S2C); only the argument *count* is checked above.
                } else if matches!(&self.body.exprs[callee], Expr::Path(Resolution::Unresolved(n)) if n == "println")
                {
                    // println intrinsic: arguments are checked above; the result
                    // is not a typed value.
                }
                // a value callee (an indirect call through a function-pointer
                // value) needs no work here: the callnonfunction judgment and the
                // result type both live in the typeck pass (S2C C5).
                Expr::Call { callee, args }
            }
            ast::Expr::ArrayLit(al) => {
                let elems: ThinVec<ExprId> = al.elems().map(|e| self.lower_expr(&e)).collect();
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
                // a failed `count` (none) has already emitted a diagnostic; 0 is
                // an inert placeholder the resolved pipeline never reaches.
                Expr::ArrayRepeat {
                    value,
                    count: count.unwrap_or(0),
                }
            }
            ast::Expr::IndexExpr(ie) => {
                let base = self.lower_required_expr(ie.base(), ptr);
                let index = self.lower_required_expr(ie.index(), ptr);
                // index judgments (L7 ptr-index, A4 const out-of-bounds) and the
                // element type both live in the typeck pass (S2C).
                Expr::Index { base, index }
            }
            ast::Expr::StructLit(sl) => {
                let lit_name: Option<Text> = sl
                    .name_ref()
                    .and_then(|n| n.name())
                    .map(|t| self.text(Some(t)));
                // L5: the literal's name must denote a declared struct or
                // union. an unknown name would otherwise be interned as
                // `Path("Foo")` and emitted verbatim into c ("use of
                // undeclared identifier"). the ids also drive the per-field
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
                let mut fields = ThinVec::new();
                // a positional initializer field (`Point { 1, 2 }`) carries no
                // field name. lowering carries fields by name only, so the
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
                                // shorthand desugar: synthesize path expr.
                                let resolution = self.resolve(&fname);
                                let f_ptr = SyntaxNodePtr::new(f.syntax());
                                // an unresolved shorthand names an undeclared
                                // local - a hard error, same as a bare name in
                                // value position (the `NameRef` arm). diagnosing
                                // it here keeps every reachable `Unresolved` path
                                // rejected before codegen (I2). no `print`/`len`
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
                                    self.alloc_expr(Expr::Path(resolution), f_ptr)
                                }
                            }
                        };
                        // field-value type coercion + the unknown-field check
                        // against the declared field type moved to the typeck
                        // pass (S2C); lowering carries the value verbatim.
                        fields.push(StructLitField { name: fname, value });
                    }
                }
                // a union literal sets exactly one member (overlapping
                // storage). more than one would silently overwrite; zero
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
                // exactly once - missing fields produce undefined behavior in C,
                // unknown fields are typos. skipped for positional literals
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
                // operator judgments (whole-array operands, `ptr` arithmetic,
                // opaque-enum arithmetic, float modulo) and the result type live
                // in the typeck pass (S2C); lowering builds the node regardless.
                Expr::Binary { op, lhs, rhs }
            }
            ast::Expr::PrefixExpr(p) => {
                let Some(op) = p.op() else {
                    return self.missing_expr(ptr);
                };
                let operand = self.lower_required_expr(p.operand(), ptr);
                // the opaque-enum judgment for `-`/`~` and the result type live
                // in the typeck pass (S2C).
                Expr::Unary { op, operand }
            }
            ast::Expr::FieldExpr(fe) => {
                // field name: the last nameref child, not the first (avoids the
                // bug where the base is a bare nameref).
                let name: Text = fe
                    .syntax()
                    .children()
                    .filter_map(ast::NameRef::cast)
                    .last()
                    .and_then(|nr| nr.name())
                    .map(|t| Text::from(t.text().trim()))
                    .unwrap_or_default();

                // variant access shortcut: a bare nameref base whose name is an
                // enum makes this `Enum.Variant`, not field access. inspect the
                // AST before `lower_expr` so the nameref arm's "enum as value"
                // diagnostic doesn't fire here.
                if let Some(ast::Expr::NameRef(nr)) = fe.expr() {
                    let base_name: Text = self.text(nr.name());
                    if let Some(&enum_id) = self.hir.items.enums.get(&base_name) {
                        let enum_def = &self.hir.enums[enum_id];
                        if let Some(&idx) = enum_def.variant_index.get(&name) {
                            let res = Resolution::Variant { enum_id, idx };
                            return self.alloc_expr(Expr::Path(res), ptr);
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
                // the `.len`-on-array steer (lenfieldonarray) and the field's
                // result type both need the base type, so they moved to the
                // typeck pass (S2C); lowering builds the projection verbatim.
                Expr::Field { base, name }
            }
            ast::Expr::AssignExpr(a) => {
                let op = a.op().unwrap_or(ast::AssignOp::Assign);
                let lhs = self.lower_required_expr(a.lhs(), ptr);
                let rhs = self.lower_required_expr(a.rhs(), ptr);
                // a const is a value, not storage: `MAX = ..` is rejected. (a
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
                // immutable-by-default: writing a `let` binding (directly, or
                // through a field/index projection rooted in it) is rejected;
                // `mut` opts in. covers every assignment form, plain and
                // compound. a write through a pointer is untracked (see
                // `immutable_assign_target`).
                if let Some(name) = self.immutable_assign_target(lhs) {
                    self.emit(
                        self.expr_ptr(lhs, ptr),
                        TypeError::AssignToImmutable { name },
                    );
                }
                Expr::Assign { op, lhs, rhs }
            }
            ast::Expr::IfExpr(i) => {
                let cond = self.lower_required_expr(i.condition(), ptr);
                // F2 (`if x = 5` is c's assignment-in-condition footgun) is
                // rejected in the parser now (grammarerror::assigninifcondition).

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
                // the return value's coercion + arity/type check moved to the
                // typeck pass (S2C, `check_explicit_return`).
                let value = r.expr().map(|e| self.lower_expr(&e));
                Expr::Return(value)
            }
            ast::Expr::RefExpr(r) => {
                let operand = self.lower_required_expr(r.expr(), ptr);
                // `&const` is illegal: a const is a value with no guaranteed
                // address (it is inlined). reject it before it reaches MIR,
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
                // `&` requires a place (ruled 2026-06-12): a non-place
                // operand (`&(a + b)`) would spill to a MIR temp and silently
                // take the temp's address, with no visible lifetime. places:
                // local, global, field, index, deref. an already-diagnosed
                // operand (missing / unresolved) passes without a second
                // error.
                let is_place = matches!(
                    &self.body.exprs[operand],
                    Expr::Path(Resolution::Local(_))
                        | Expr::Path(Resolution::Global(_))
                        | Expr::Path(Resolution::Unresolved(_))
                        | Expr::Field { .. }
                        | Expr::Index { .. }
                        | Expr::Deref { .. }
                        | Expr::Missing
                );
                if !is_place {
                    self.emit(ptr, TypeError::RefOfNonPlace);
                    return self.missing_expr(ptr);
                }
                Expr::Ref { operand }
            }
            ast::Expr::ParenExpr(pe) => {
                // a group is a pure precedence override - lower it to its inner
                // expression directly so no parenexpr survives into HIR/codegen.
                return match pe.expr() {
                    Some(inner) => self.lower_expr(&inner),
                    None => self.missing_expr(ptr),
                };
            }
            ast::Expr::MatchExpr(me) => self.lower_match_expr(me, ptr),
            ast::Expr::DerefExpr(d) => {
                let operand = self.lower_required_expr(d.expr(), ptr);
                // the `ptr`-deref judgment (no pointee type) and the pointee
                // type both live in the typeck pass (S2C).
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
                    Some(t) => lower_type_ref(&t, &mut self.diagnostics, &consts, self.types),
                    None => self.types.error_type(),
                };
                // R012: the cast target's type names must be declared. a
                // c-only type needs an `extern { type Name; }` declaration
                // first (the FFI opaque-type story).
                self.check_type_names(ty, ptr);
                Expr::Cast { operand, ty }
            }
        };

        // allocate the expression. lowering no longer types expressions (S2C
        // C5): the typeck pass is the sole source of expression types.
        self.alloc_expr(hir_expr, ptr)
    }

    /// a place expression: one that names existing storage rather than
    /// computing a fresh value (a variable, field, index, or deref). used to
    /// gate `len`, which reads a length from the type without evaluating the
    /// operand - restricting it to a place keeps a side-effecting expression
    /// like `len(f())` from being silently discarded. note a place can still
    /// contain a call in an index position (`len(arr[f()])`), which this does
    /// not reject; that residual matches c's `sizeof` and is documented.
    fn is_place_expr(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Path(Resolution::Local(_))
                | Expr::Field { .. }
                | Expr::Index { .. }
                | Expr::Deref { .. }
        )
    }

    /// if an assignment target ultimately writes an immutable `let` binding,
    /// return its name. the target roots in a local through field/index
    /// projections (`s.f = ..`, `a[i] = ..`); a deref (`*p = ..`) writes
    /// through a pointer and is deliberately not tracked - the raw-pointer
    /// escape, consistent with eye's runtime-freedom model (KERNEL.md). mutable
    /// bindings and non-local targets return `None`.
    fn immutable_assign_target(&self, place: ExprId) -> Option<Text> {
        match &self.body.exprs[place] {
            Expr::Path(Resolution::Local(id)) => {
                let local = &self.body.locals[*id];
                (!local.mutable).then(|| local.name.clone())
            }
            // a `let` global is read-only static storage; a `mut` global opts in.
            // same immutable-by-default rule as a local binding.
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

    /// lower the `len(arr)` intrinsic to a compile-time `usize` literal equal to
    /// the argument's static array length. accepts `[T; N]` and `&[T; N]` (one
    /// ref/ptr is peeled). wrong arity or a non-array argument is a diagnostic;
    /// the result is then a `0` placeholder, still typed `usize`, so downstream
    /// type information stays intact.
    /// `println` is a primitive-only intrinsic (not a trait or macro yet): it
    /// has no format for a compound value. reject array/struct/union arguments.
    /// also checked here (U5): a format string must exist, and when it is a
    /// literal its `{}` count must equal the value-argument count - otherwise
    /// codegen emits an unmatched `%d` or forwards surplus printf varargs.
    /// reject a char literal outside ASCII (T034): `char` is one byte, and the
    /// multibyte c char constant the backend would emit for it has an
    /// implementation-defined value. runs on every lowered literal - expression
    /// and match-pattern position both.
    pub(super) fn check_char_literal(&mut self, lit: &Literal, ptr: SyntaxNodePtr) {
        if let Literal::Char(c) = lit
            && !c.is_ascii()
        {
            self.emit(ptr, TypeError::CharLiteralNotAscii { ch: *c });
        }
    }

    fn check_println_args(&mut self, args: &[ExprId], ptr: SyntaxNodePtr) {
        let Some((&fmt, values)) = args.split_first() else {
            self.emit(ptr, TypeError::PrintlnMissingFormat);
            return;
        };
        // only a literal format string can be counted at compile time; the
        // scan mirrors codegen's exactly: `{{` and `}}` are escapes for a
        // literal brace, `{}` is a placeholder, a lone `{`/`}` prints
        // literally.
        if let Expr::Literal(Literal::String(s)) = &self.body.exprs[fmt] {
            let mut placeholders = 0usize;
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '{' if chars.peek() == Some(&'{') => {
                        chars.next();
                    }
                    '{' if chars.peek() == Some(&'}') => {
                        chars.next();
                        placeholders += 1;
                    }
                    '}' if chars.peek() == Some(&'}') => {
                        chars.next();
                    }
                    _ => {}
                }
            }
            if placeholders != values.len() {
                self.emit(
                    ptr,
                    TypeError::PrintlnArityMismatch {
                        placeholders,
                        args: values.len(),
                    },
                );
            }
        }
        // the not-formattable judgment (printcannotformat: an array/struct/union
        // argument) needs the argument types, so it moved to the typeck pass
        // (S2C); only the placeholder arity is checked here.
    }

    fn lower_len_intrinsic(&mut self, args: &[ExprId], ptr: SyntaxNodePtr) -> ExprId {
        if args.len() != 1 {
            self.emit(ptr, TypeError::LenArity { found: args.len() });
            return self.missing_expr(ptr);
        }
        let operand = args[0];
        // `len` reads the length from the operand's static type and never
        // evaluates the operand - just like c's `sizeof`. so `len(f())` would
        // silently discard the call. restrict the operand to a place (variable,
        // field, index, or deref), where nothing is computed, so that footgun
        // cannot arise. go's `len` has the same shape.
        if !Self::is_place_expr(&self.body.exprs[operand]) {
            self.emit(self.expr_ptr(operand, ptr), TypeError::LenNotAPlace);
        }
        // the operand-must-be-an-array judgment (lennotarray) needs the operand
        // type, so it moved to the typeck pass (S2C). the `len` node is typed
        // `usize` there; MIR folds it to the operand's static element count.
        self.alloc_expr(Expr::Len(operand), ptr)
    }

    /// lower `sizeof(T)` to an `Expr::SizeOf` of type `usize`. the argument is a
    /// type read straight from the AST (see the call site): the floor accepts a
    /// bare named type only (`sizeof(int32)`, `sizeof(Point)`), matching the
    /// lenient type-name handling elsewhere - the name is not validated here, the
    /// c backend is the layout authority. compound types (`sizeof(&T)`,
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
        self.alloc_expr(Expr::SizeOf(ty), ptr)
    }

    fn lower_match_expr(&mut self, me: &ast::MatchExpr, ptr: SyntaxNodePtr) -> Expr {
        let scrut = self.lower_required_expr(me.scrut(), ptr);

        // structural lowering only. pattern classification (variant vs binding)
        // is name-based (`lower_match_pat`), so no scrutinee type is read here.
        // every type-directed judgment - scrutinee domain, a variant of the
        // wrong enum or over a primitive, coverage, exhaustiveness, duplicate
        // and unreachable arms, the match's result type - moved to the typeck
        // match pass (`crates/typeck/src/infer.rs`, `check_matches`).
        let mut arms: ThinVec<MatchArm> = ThinVec::new();

        if let Some(arm_list) = me.arm_list() {
            for arm in arm_list.arms() {
                let arm_ptr = SyntaxNodePtr::new(arm.syntax());
                // each arm gets its own scope so an arm binding (`x -> ..`) is
                // visible only in that arm's body. `lower_match_pat` defines the
                // binding into this scope, so the body (lowered next) sees it.
                self.scopes.push();
                let pat_id = match arm.pat() {
                    Some(p) => self.lower_match_pat(&p),
                    None => self.alloc_pat(Pat::Missing, arm_ptr),
                };
                // a guard is allowed on any arm, including an irrefutable one
                // (`x if ..` / `_ if ..`); MIR lowers a guarded catch-all to an
                // ordered `Always` arm with fall-through.
                let guard_id = arm
                    .guard()
                    .map(|g| self.lower_required_expr(g.expr(), arm_ptr));
                let body_id = self.lower_required_expr(arm.body(), arm_ptr);
                arms.push(MatchArm {
                    pat: pat_id,
                    guard: guard_id,
                    body: body_id,
                    ptr: arm_ptr,
                });
                self.scopes.pop();
            }
        }

        Expr::Match { scrut, arms }
    }
}
