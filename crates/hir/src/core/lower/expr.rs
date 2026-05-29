//! Expression lowering.

use ast::AstNode;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::LoweringCtx;
use super::types::{literal_type, lower_literal, lower_type_ref};
use crate::core::{
    Block, EnumId, Expr, ExprId, MatchArm, Pat, Resolution, StructLitField, Text, TypeRef,
};

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
                // Bare enum name in expression position is not a value.
                // `Shape.Circle` short-circuits before reaching here via the
                // FieldExpr arm, so an Enum resolution here is misuse.
                if matches!(resolution, Resolution::Enum(_)) {
                    self.diag(ptr, format!("`{name}` is an enum type, not a value"));
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
                let args = c
                    .arg_list()
                    .map(|al| al.args().map(|a| self.lower_expr(&a)).collect())
                    .unwrap_or_default();
                if let Expr::Path(Resolution::Fn(fn_id)) = &self.body.exprs[callee] {
                    expr_type = self.hir.functions[*fn_id].ret.clone();
                }
                Expr::Call { callee, args }
            }
            ast::Expr::ArrayLit(al) => {
                let elems: ThinVec<ExprId> = al.elems().map(|e| self.lower_expr(&e)).collect();
                // Type as [elem; N] when the first element's type is known.
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
                // Element type is the base's element/pointee type, when known.
                expr_type = self
                    .body
                    .expr_types
                    .get(base)
                    .cloned()
                    .and_then(|t| match t {
                        TypeRef::Array { elem, .. } => Some(*elem),
                        TypeRef::Ptr(inner) | TypeRef::Ref(inner) => Some(*inner),
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
                if let Some(fl) = sl.field_list() {
                    for f in fl.fields() {
                        let Some(fname_token) = f.name() else {
                            continue;
                        };
                        let fname = Self::text(Some(fname_token));
                        let value = match f.value() {
                            Some(v) => self.lower_expr(&v),
                            None => {
                                // shorthand desugar: synthesize Path expr.
                                let resolution = self.resolve(&fname);
                                let f_ptr = SyntaxNodePtr::new(f.syntax());
                                let inner_ty = match &resolution {
                                    Resolution::Local(local_id) => {
                                        self.body.locals[*local_id].ty.clone()
                                    }
                                    _ => None,
                                };
                                let id = self.alloc_expr(Expr::Path(resolution), f_ptr);
                                if let Some(t) = inner_ty {
                                    self.body.expr_types.insert(id, t);
                                }
                                id
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
                    self.diag(
                        SyntaxNodePtr::new(sl.syntax()),
                        format!(
                            "union literal `{name}` must set exactly one field, found {}",
                            fields.len()
                        ),
                    );
                }
                Expr::StructLit { ty, fields }
            }
            ast::Expr::BinExpr(b) => {
                let Some(op) = b.op() else {
                    return self.missing_expr(ptr);
                };
                let lhs = self.lower_required_expr(b.lhs(), ptr);
                let rhs = self.lower_required_expr(b.rhs(), ptr);
                // Infer type from the left operand (simplified).
                expr_type = self.body.expr_types.get(lhs).cloned();
                Expr::Binary { op, lhs, rhs }
            }
            ast::Expr::PrefixExpr(p) => {
                let Some(op) = p.op() else {
                    return self.missing_expr(ptr);
                };
                let operand = self.lower_required_expr(p.operand(), ptr);
                expr_type = self.body.expr_types.get(operand).cloned();
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
                            self.diag(ptr, format!("enum `{base_name}` has no variant `{name}`"));
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
                expr_type = Some(self.lookup_field_type(&base_ty, &name));
                Expr::Field { base, name }
            }
            ast::Expr::AssignExpr(a) => {
                let lhs = self.lower_required_expr(a.lhs(), ptr);
                let rhs = self.lower_required_expr(a.rhs(), ptr);
                // Assignment type is the type of the RHS.
                expr_type = self.body.expr_types.get(rhs).cloned();
                Expr::Assign { lhs, rhs }
            }
            ast::Expr::IfExpr(i) => {
                let cond = self.lower_required_expr(i.condition(), ptr);

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
            self.diag(ptr, "match scrutinee type is not a known enum".to_string());
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
                    self.diag(
                        arm_ptr,
                        "unreachable match arm after `_` wildcard".to_string(),
                    );
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
                                self.diag(
                                    arm_ptr,
                                    format!("duplicate match arm for variant `{vname}`"),
                                );
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
            let missing: Vec<String> = self.hir.enums[eid]
                .variants
                .iter()
                .enumerate()
                .filter(|(i, _)| !covered[*i])
                .map(|(_, v)| v.name.to_string())
                .collect();
            if !missing.is_empty() {
                let enum_name = self.hir.enums[eid].name.clone();
                self.diag(
                    ptr,
                    format!(
                        "non-exhaustive match on enum `{}`: missing {}",
                        enum_name,
                        missing
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }

        // Type of the whole match mirrors `if`: the first arm's body
        // type. Good enough for M5 codegen + M6 e2e.
        *expr_type = arm_type;
        Expr::Match { scrut, arms }
    }
}
