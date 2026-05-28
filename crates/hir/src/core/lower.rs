//! The lowering logic: AST -> arena HIR with name resolution and the v0.3
//! match exhaustiveness check. Entry point is [`lower_source_file`].
//!
//! Pipeline runs in three passes:
//! 1. `collect_items` registers every top-level [`Struct`], [`Enum`], and
//!    [`Function`] in [`HIR::items`]. Forward refs work because bodies have
//!    not been walked yet. Duplicate declarations emit an [`HirDiagnostic`];
//!    the later definition still overwrites the earlier one in [`ItemScope`],
//!    and both items keep their arena slots so existing IDs do not invalidate.
//! 2. Name resolution. Type resolution is deferred to codegen: a [`TypeRef`]
//!    stays as a `Path(name)` string with no `StructId` attached. Value
//!    resolution (locals + items) is folded into pass 3 since lexical scopes
//!    only exist inside a body.
//! 3. `lower_fn_body` walks each fn's `Block` with a fresh [`LoweringCtx`].
//!    Each [`Expr::Path`] carries its [`Resolution`] so later passes never
//!    redo the lookup.

use ast::AstNode;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

use super::*;

// ---- scopes (lexical, inside a body) ----

#[derive(Debug, Default)]
pub struct Scopes {
    stack: Vec<FxHashMap<Text, LocalId>>,
}

impl Scopes {
    pub fn new() -> Self {
        Self {
            stack: vec![FxHashMap::default()],
        }
    }

    pub fn push(&mut self) {
        self.stack.push(FxHashMap::default());
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    pub fn define(&mut self, name: Text, id: LocalId) {
        self.stack
            .last_mut()
            .expect("at least one scope frame")
            .insert(name, id);
    }

    pub fn lookup(&self, name: &Text) -> Option<LocalId> {
        self.stack.iter().rev().find_map(|f| f.get(name).copied())
    }
}

// ---- lowering context ----

pub struct LoweringCtx<'a> {
    hir: &'a HIR,
    body: Body,
    scopes: Scopes,
    diagnostics: Vec<HirDiagnostic>,
}

impl<'a> LoweringCtx<'a> {
    pub fn new(hir: &'a HIR) -> Self {
        Self {
            hir,
            body: Body::default(),
            scopes: Scopes::new(),
            diagnostics: Vec::new(),
        }
    }

    fn diag(&mut self, ptr: SyntaxNodePtr, msg: String) {
        self.diagnostics.push(HirDiagnostic { ptr, msg });
    }

    #[allow(dead_code)]
    fn alloc_expr_with_type(&mut self, expr: Expr, ptr: SyntaxNodePtr, ty: TypeRef) -> ExprId {
        let id = self.body.exprs.alloc(expr);
        self.body.source_map.expr.insert(id, ptr);
        self.body.expr_types.insert(id, ty);
        id
    }

    fn alloc_expr(&mut self, expr: Expr, ptr: SyntaxNodePtr) -> ExprId {
        let id = self.body.exprs.alloc(expr);
        self.body.source_map.expr.insert(id, ptr);
        id
    }

    fn alloc_stmt(&mut self, stmt: Stmt, ptr: SyntaxNodePtr) -> StmtId {
        let id = self.body.stmts.alloc(stmt);
        self.body.source_map.stmt.insert(id, ptr);
        id
    }

    fn alloc_pat(&mut self, pat: Pat, ptr: SyntaxNodePtr) -> PatId {
        let id = self.body.pats.alloc(pat);
        self.body.source_map.pat.insert(id, ptr);
        id
    }

    fn alloc_block(&mut self, block: Block, ptr: SyntaxNodePtr) -> BlockId {
        let id = self.body.blocks.alloc(block);
        self.body.block_source_map.insert(id, ptr);
        id
    }

    fn finish(self) -> (Body, Vec<HirDiagnostic>) {
        (self.body, self.diagnostics)
    }

    /// Resolve a `NameRef`. Lexical scopes first, then module-level values,
    /// then types, then enum variants (flat across every enum). Unknown
    /// names produce [`Resolution::Unresolved`] so later diagnostics still
    /// have the original text.
    fn resolve(&self, name: &Text) -> Resolution {
        if let Some(id) = self.scopes.lookup(name) {
            return Resolution::Local(id);
        }
        if let Some(&id) = self.hir.items.functions.get(name) {
            return Resolution::Fn(id);
        }
        if let Some(&id) = self.hir.items.structs.get(name) {
            return Resolution::Struct(id);
        }
        if let Some(&id) = self.hir.items.enums.get(name) {
            return Resolution::Enum(id);
        }
        if let Some(&(enum_id, idx)) = self.hir.items.variants.get(name) {
            return Resolution::Variant { enum_id, idx };
        }
        Resolution::Unresolved(name.clone())
    }

    /// Lower a match arm pattern. Bare-ident and qualified-path patterns are
    /// resolved against the scrutinee enum directly (spec says no bindings),
    /// so a name that doesn't match a variant of `scrut_enum` is an error
    /// rather than silently introducing a binding. Failure produces
    /// `Pat::Missing`; the caller's coverage check treats Missing as
    /// "uncovered" so a typo can't accidentally satisfy exhaustiveness.
    fn lower_match_pat(&mut self, pat: &ast::Pat, scrut_enum: Option<EnumId>) -> PatId {
        let ptr = SyntaxNodePtr::new(pat.syntax());
        match pat {
            ast::Pat::WildcardPat(_) => self.alloc_pat(Pat::Wildcard, ptr),
            ast::Pat::PathPat(pp) => {
                let qual: Text = pp
                    .qualifier()
                    .and_then(|n| n.name())
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let vname: Text = pp
                    .name()
                    .and_then(|n| n.name())
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let Some(&qual_enum) = self.hir.items.enums.get(&qual) else {
                    self.diag(ptr, format!("unknown enum `{qual}` in match pattern"));
                    return self.alloc_pat(Pat::Missing, ptr);
                };
                if let Some(scrut_eid) = scrut_enum
                    && scrut_eid != qual_enum
                {
                    let scrut_name = self.hir.enums[scrut_eid].name.clone();
                    self.diag(
                        ptr,
                        format!("pattern is from enum `{qual}`, but scrutinee is `{scrut_name}`"),
                    );
                    return self.alloc_pat(Pat::Missing, ptr);
                }
                let enum_def = &self.hir.enums[qual_enum];
                match enum_def
                    .variants
                    .iter()
                    .enumerate()
                    .find(|(_, v)| v.name == vname)
                {
                    Some((idx, _)) => self.alloc_pat(
                        Pat::Variant {
                            enum_id: qual_enum,
                            idx: idx as u32,
                        },
                        ptr,
                    ),
                    None => {
                        self.diag(ptr, format!("enum `{qual}` has no variant `{vname}`"));
                        self.alloc_pat(Pat::Missing, ptr)
                    }
                }
            }
            ast::Pat::BareIdentPat(bp) => {
                let name: Text = bp
                    .name()
                    .and_then(|n| n.name())
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                // Scrutinee enum known: resolve strictly against its variants
                // so cross-enum bare patterns become a clean diagnostic.
                if let Some(eid) = scrut_enum {
                    let enum_def = &self.hir.enums[eid];
                    if let Some((idx, _)) = enum_def
                        .variants
                        .iter()
                        .enumerate()
                        .find(|(_, v)| v.name == name)
                    {
                        return self.alloc_pat(
                            Pat::Variant {
                                enum_id: eid,
                                idx: idx as u32,
                            },
                            ptr,
                        );
                    }
                    let enum_name = enum_def.name.clone();
                    self.diag(ptr, format!("enum `{enum_name}` has no variant `{name}`"));
                    return self.alloc_pat(Pat::Missing, ptr);
                }
                // Scrutinee type unknown: fall back to the global variant
                // index. Still no bindings - an unresolved name is an error,
                // not a fresh local.
                if let Some(&(enum_id, idx)) = self.hir.items.variants.get(&name) {
                    return self.alloc_pat(Pat::Variant { enum_id, idx }, ptr);
                }
                self.diag(ptr, format!("unknown variant `{name}` in match pattern"));
                self.alloc_pat(Pat::Missing, ptr)
            }
        }
    }

    fn block_tail_type(&self, block_id: BlockId) -> Option<TypeRef> {
        let block = &self.body.blocks[block_id];
        block
            .tail
            .and_then(|expr_id| self.body.expr_types.get(expr_id).cloned())
    }

    /// look up the type of a struct field given the struct type and field name.
    fn lookup_field_type(&self, struct_ty: &TypeRef, field_name: &Text) -> TypeRef {
        match struct_ty {
            TypeRef::Path(name) => {
                if let Some(&struct_id) = self.hir.items.structs.get(name) {
                    let struct_def = &self.hir.structs[struct_id];
                    for &field_id in &struct_def.fields {
                        let field = &self.hir.fields[field_id];
                        if &field.name == field_name {
                            return field.ty.clone();
                        }
                    }
                }
                TypeRef::Error
            }
            TypeRef::Ref(inner) | TypeRef::Ptr(inner) => {
                // NOTE: auto-deref: look through one level of indirection
                self.lookup_field_type(inner, field_name)
            }
            TypeRef::Error => TypeRef::Error,
        }
    }

    fn lower_block(&mut self, block: ast::Block) -> BlockId {
        // Stmts must lower before the tail expression: the parser already
        // ensures `block.stmts()` and `block.tail_expr()` are disjoint
        // (the abandoned-marker form in the block parser puts a bare Expr
        // in the tail slot only when no `;` follows). Locals defined by
        // those stmts have to be in scope when the tail - typically a
        // `loop { ... }` or `if { ... }` body - references them.
        let ptr = SyntaxNodePtr::new(block.syntax());
        let mut stmts = Vec::new();
        let mut tail = None;

        self.scopes.push();

        for s in block.stmts() {
            stmts.push(self.lower_stmt(&s));
        }

        if let Some(tail_expr) = block.tail_expr() {
            tail = Some(self.lower_expr(&tail_expr));
        }

        self.scopes.pop();

        self.alloc_block(Block { stmts, tail }, ptr)
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> StmtId {
        let ptr = SyntaxNodePtr::new(stmt.syntax());
        match stmt {
            ast::Stmt::LetStmt(l) => {
                let name: Text = l
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let ty = l.ty().map(|t| lower_type_ref(&t));
                let mutable = matches!(l.kind(), Some(ast::LetKind::Var));
                let init = l.value().map(|e| self.lower_expr(&e));

                // pat <-> local back-reference: allocate Pat::Missing first so
                // the local can point at a valid PatId, then patch the pat to
                // Bind(local_id).
                let pat_id = self.alloc_pat(Pat::Missing, ptr);
                let local_id = self.body.locals.alloc(Local {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable,
                    pat: pat_id,
                });
                self.body.pats[pat_id] = Pat::Bind(local_id);
                self.scopes.define(name, local_id);

                self.alloc_stmt(
                    Stmt::Let {
                        pat: pat_id,
                        ty,
                        init,
                        mutable,
                    },
                    ptr,
                )
            }
            ast::Stmt::ExprStmt(e) => {
                let expr = match e.expr() {
                    Some(x) => self.lower_expr(&x),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
                self.alloc_stmt(Stmt::Expr(expr), ptr)
            }
        }
    }

    fn lower_expr(&mut self, expr: &ast::Expr) -> ExprId {
        let ptr = SyntaxNodePtr::new(expr.syntax());
        let mut expr_type: Option<TypeRef> = None;

        let hir_expr = match expr {
            ast::Expr::Literal(lit) => {
                let literal = lower_literal(lit);
                expr_type = Some(literal_type(&literal));
                Expr::Literal(literal)
            }
            ast::Expr::NameRef(nr) => {
                let name: Text = nr
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let resolution = self.resolve(&name);
                // Bare enum name in expression position is not a value.
                // `Shape.Circle` short-circuits before reaching here via the
                // FieldExpr arm, so an Enum resolution here is misuse.
                if matches!(resolution, Resolution::Enum(_)) {
                    self.diag(ptr, format!("`{name}` is an enum type, not a value"));
                    return self.alloc_expr(Expr::Missing, ptr);
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
                let callee = match c.callee() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
                let args = c
                    .arg_list()
                    .map(|al| al.args().map(|a| self.lower_expr(&a)).collect())
                    .unwrap_or_default();
                // call return type is unknown for now.
                Expr::Call { callee, args }
            }
            ast::Expr::StructLit(sl) => {
                let ty = match sl.name_ref().and_then(|n| n.name()) {
                    Some(t) => TypeRef::Path(SmolStr::from(t.text())),
                    None => TypeRef::Error,
                };
                expr_type = Some(ty.clone());
                let mut fields = Vec::new();
                if let Some(fl) = sl.field_list() {
                    for f in fl.fields() {
                        let fname: Text = match f.name() {
                            Some(t) => SmolStr::from(t.text()),
                            None => continue,
                        };
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
                Expr::StructLit { ty, fields }
            }
            ast::Expr::BinExpr(b) => {
                let Some(op) = b.op() else {
                    return self.alloc_expr(Expr::Missing, ptr);
                };
                let lhs = match b.lhs() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
                let rhs = match b.rhs() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
                // Infer type from the left operand (simplified).
                expr_type = self.body.expr_types.get(lhs).cloned();
                Expr::Binary { op, lhs, rhs }
            }
            ast::Expr::PrefixExpr(p) => {
                let Some(op) = p.op() else {
                    return self.alloc_expr(Expr::Missing, ptr);
                };
                let operand = match p.operand() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
                expr_type = self.body.expr_types.get(operand).cloned();
                Expr::Unary { op, operand }
            }
            ast::Expr::FieldExpr(fe) => {
                // Field name: the last NameRef child, not the first (avoids the
                // bug where the base is a bare NameRef).
                let name: SmolStr = fe
                    .syntax()
                    .children()
                    .filter_map(ast::NameRef::cast)
                    .last()
                    .and_then(|nr| nr.name())
                    .map(|t| SmolStr::from(t.text().trim()))
                    .unwrap_or_default();

                // Variant access shortcut: a bare NameRef base whose name is an
                // enum makes this `Enum.Variant`, not field access. Inspect the
                // AST before `lower_expr` so the NameRef arm's "enum as value"
                // diagnostic doesn't fire here.
                if let Some(ast::Expr::NameRef(nr)) = fe.expr() {
                    let base_name: Text = nr
                        .name()
                        .map(|t| SmolStr::from(t.text()))
                        .unwrap_or_default();
                    if let Some(&enum_id) = self.hir.items.enums.get(&base_name) {
                        let enum_def = &self.hir.enums[enum_id];
                        if let Some((idx, _)) = enum_def
                            .variants
                            .iter()
                            .enumerate()
                            .find(|(_, v)| v.name == name)
                        {
                            let res = Resolution::Variant {
                                enum_id,
                                idx: idx as u32,
                            };
                            let ty = TypeRef::Path(enum_def.name.clone());
                            let id = self.alloc_expr(Expr::Path(res), ptr);
                            self.body.expr_types.insert(id, ty);
                            return id;
                        } else {
                            self.diag(ptr, format!("enum `{base_name}` has no variant `{name}`"));
                            return self.alloc_expr(Expr::Missing, ptr);
                        }
                    }
                }

                let base = match fe.expr() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
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
                let lhs = a
                    .lhs()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));
                let rhs = a
                    .rhs()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));
                // Assignment type is the type of the RHS.
                expr_type = self.body.expr_types.get(rhs).cloned();
                Expr::Assign { lhs, rhs }
            }
            ast::Expr::IfExpr(i) => {
                let cond = i
                    .condition()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));

                let then_block =
                    i.then_branch()
                        .map(|b| self.lower_block(b))
                        .unwrap_or_else(|| {
                            let empty = Block {
                                stmts: vec![],
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
                        stmts: vec![],
                        tail: None,
                    };
                    self.alloc_block(empty, ptr)
                });
                // Loop type is unit (void).
                Expr::Loop { body }
            }
            ast::Expr::BreakExpr(_) => {
                // we don't store the optional value yet; could be extended later.
                Expr::Break
            }
            ast::Expr::ContinueExpr(_) => Expr::Continue,
            ast::Expr::RefExpr(r) => {
                let operand = r
                    .expr()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));
                let inner_ty = self
                    .body
                    .expr_types
                    .get(operand)
                    .cloned()
                    .unwrap_or(TypeRef::Error);
                expr_type = Some(TypeRef::Ref(Box::new(inner_ty)));
                Expr::Ref { operand }
            }
            ast::Expr::MatchExpr(me) => {
                let scrut = me
                    .scrut()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));

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

                let mut arms: Vec<MatchArm> = Vec::new();
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
                        let body_id = match arm.body() {
                            Some(b) => self.lower_expr(&b),
                            None => self.alloc_expr(Expr::Missing, arm_ptr),
                        };
                        if arm_type.is_none() {
                            arm_type = self.body.expr_types.get(body_id).cloned();
                        }
                        arms.push(MatchArm {
                            pat: pat_id,
                            body: body_id,
                        });
                    }
                }

                // Exhaustiveness: every variant must be covered unless `_`
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
                expr_type = arm_type;
                Expr::Match { scrut, arms }
            }
            ast::Expr::DerefExpr(d) => {
                let operand = d
                    .expr()
                    .map(|e| self.lower_expr(&e))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, ptr));
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
        };

        // allocate the expression and record its type if known
        let id = self.alloc_expr(hir_expr, ptr);
        if let Some(ty) = expr_type {
            self.body.expr_types.insert(id, ty);
        }
        id
    }
}

// ---- free helpers ----

fn lower_type_ref(ty: &ast::TypeRef) -> TypeRef {
    // v0.1 only emits IdentType; ref/ptr types are deferred to later passes.
    // NOTE: now on v0.2 <- implemented ref and ptr types
    match ty {
        ast::TypeRef::IdentType(it) => match it.name() {
            Some(t) => TypeRef::Path(SmolStr::from(t.text())),
            None => TypeRef::Error,
        },
        ast::TypeRef::RefType(rt) => {
            let inner = rt
                .inner()
                .map(|t| lower_type_ref(&t))
                .unwrap_or(TypeRef::Error);
            TypeRef::Ref(Box::new(inner))
        }
        ast::TypeRef::PtrType(pt) => {
            let inner = pt
                .inner()
                .map(|t| lower_type_ref(&t))
                .unwrap_or(TypeRef::Error);
            TypeRef::Ptr(Box::new(inner))
        }
    }
}

fn literal_type(lit: &Literal) -> TypeRef {
    match lit {
        Literal::Int(_) => TypeRef::Path(SmolStr::new_static("int32")),
        Literal::Float(_) => TypeRef::Path(SmolStr::new_static("float64")),
        Literal::String(_) => TypeRef::Path(SmolStr::new_static("string")),
        Literal::Bool(_) => TypeRef::Path(SmolStr::new_static("bool")),
        Literal::Char(_) => TypeRef::Path(SmolStr::new_static("char")),
    }
}

fn lower_literal(lit: &ast::Literal) -> Literal {
    let Some(token) = lit.token() else {
        return Literal::Int(0);
    };
    let text = token.text();
    match lit.literal_kind() {
        Some(ast::LiteralKind::Int) => text
            .parse::<u128>()
            .map(Literal::Int)
            .unwrap_or(Literal::Int(0)),
        Some(ast::LiteralKind::Float) => Literal::Float(SmolStr::from(text)),
        Some(ast::LiteralKind::String) => {
            // strip surrounding double quotes; escapes left raw for v0.1
            let s = text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(text);
            Literal::String(SmolStr::from(s))
        }
        Some(ast::LiteralKind::Bool) => Literal::Bool(text == "true"),
        Some(ast::LiteralKind::Char) => {
            let inner = text
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(text);
            Literal::Char(inner.chars().next().unwrap_or('\0'))
        }
        None => Literal::Int(0),
    }
}

// ---- entry points ----

/// Lower a parsed file into a fresh [`HIR`]. See module docs for pass layout.
pub fn lower_source_file(file: ast::SourceFile) -> HIR {
    let mut hir = HIR::default();

    // pass 1: collect every top-level item. Forward refs resolve because
    // bodies have not been walked yet. Duplicate names are *not* rejected;
    // [`ItemScope`] silently overwrites earlier bindings.
    let fn_asts = collect_items(&mut hir, &file);

    // pass 2: name resolution.
    //   - type resolution: deferred. TypeRef stays as Path(name); codegen
    //     will look up the StructId itself.
    //   - value resolution: folded into pass 3 (scopes only exist inside a
    //     body, and resolution is recorded per-Expr::Path).

    // pass 3: lower each fn body.
    for (fn_id, fn_ast) in fn_asts {
        let body_id = lower_fn_body(&mut hir, &fn_ast);
        hir.functions[fn_id].body = Some(body_id);
    }

    hir
}

/// Walk top-level items, allocate signatures, populate [`ItemScope`].
/// Returns the AST nodes for each function so pass 3 can lower their bodies
/// without re-traversing the file. Emits a diagnostic on duplicate names
/// (later definitions still take effect; the original slot stays allocated
/// but is shadowed in the scope map).
fn collect_items(hir: &mut HIR, file: &ast::SourceFile) -> Vec<(FnId, ast::FnDef)> {
    let mut fn_asts = Vec::new();
    for item in file.items() {
        match item {
            ast::Item::StructDef(s) => {
                let name: Text = s
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let mut fields = Vec::new();
                if let Some(fl) = s.field_list() {
                    for f in fl.fields() {
                        let fname: Text = f
                            .name()
                            .map(|t| SmolStr::from(t.text()))
                            .unwrap_or_default();
                        let ty = match f.ty() {
                            Some(t) => lower_type_ref(&t),
                            None => TypeRef::Error,
                        };
                        let field_id = hir.fields.alloc(Field { name: fname, ty });
                        fields.push(field_id);
                    }
                }
                let struct_id = hir.structs.alloc(Struct {
                    name: name.clone(),
                    fields,
                });
                if hir.items.structs.contains_key(&name) || hir.items.functions.contains_key(&name)
                {
                    hir.diagnostics.push(HirDiagnostic {
                        ptr: SyntaxNodePtr::new(s.syntax()),
                        msg: format!("duplicate item `{name}`"),
                    });
                }
                hir.items.structs.insert(name, struct_id);
            }
            ast::Item::FnDef(f) => {
                let name: Text = f
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                // NOTE: v0.1 grammar: ParamList is `( )`. No params to collect.
                // now <- v0.2
                let mut params = Vec::new();
                if let Some(pl) = f.param_list() {
                    for param_ast in pl.params() {
                        let pname = param_ast
                            .name()
                            .map(|t| SmolStr::from(t.text()))
                            .unwrap_or_default();
                        let pty = match param_ast.ty() {
                            Some(t) => lower_type_ref(&t),
                            None => TypeRef::Error,
                        };
                        params.push(Param {
                            name: pname,
                            ty: pty,
                        });
                    }
                }
                let ret = f.ret_type().map(|t| lower_type_ref(&t));
                let fn_id = hir.functions.alloc(Function {
                    name: name.clone(),
                    params,
                    ret,
                    body: None,
                });
                if hir.items.functions.contains_key(&name) || hir.items.structs.contains_key(&name)
                {
                    hir.diagnostics.push(HirDiagnostic {
                        ptr: SyntaxNodePtr::new(f.syntax()),
                        msg: format!("duplicate item `{name}`"),
                    });
                }
                hir.items.functions.insert(name, fn_id);
                fn_asts.push((fn_id, f));
            }
            // v0.2 EnumDef: collection deferred. Skipping leaves no item in
            // [`ItemScope`] for it, but the body lowering still walks the
            // file's other items normally.
            ast::Item::EnumDef(e) => {
                let name: Text = e
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let mut variants = Vec::new();
                for v in e.variants() {
                    let vname = v
                        .name()
                        .map(|t| SmolStr::from(t.text()))
                        .unwrap_or_default();
                    variants.push(Variant { name: vname });
                }
                let enum_id = hir.enums.alloc(Enum {
                    name: name.clone(),
                    variants,
                });
                if hir.items.structs.contains_key(&name) || hir.items.functions.contains_key(&name)
                {
                    hir.diagnostics.push(HirDiagnostic {
                        ptr: SyntaxNodePtr::new(e.syntax()),
                        msg: format!("duplicate item `{name}`"),
                    });
                }
                // Register each variant in the flat index. A second enum
                // claiming the same variant name conflicts with the first
                // and is a hard error (the lookup would otherwise be
                // ambiguous, and the C backend would emit two enum
                // constants with the same name).
                let enum_def = &hir.enums[enum_id];
                for (idx, v) in enum_def.variants.iter().enumerate() {
                    let vname = v.name.clone();
                    if let Some(&(other_enum, _)) = hir.items.variants.get(&vname) {
                        let other_name = hir.enums[other_enum].name.clone();
                        hir.diagnostics.push(HirDiagnostic {
                            ptr: SyntaxNodePtr::new(e.syntax()),
                            msg: format!(
                                "variant `{vname}` already declared in enum `{other_name}`"
                            ),
                        });
                    } else {
                        hir.items.variants.insert(vname, (enum_id, idx as u32));
                    }
                }
                hir.items.enums.insert(name, enum_id);
            }
        }
    }
    fn_asts
}

fn lower_fn_body(hir: &mut HIR, fn_ast: &ast::FnDef) -> BodyId {
    let mut ctx = LoweringCtx::new(hir);

    if let Some(block) = fn_ast.body() {
        // lower_block will push its own scope. We need parameters to be
        // visible inside that scope, so push a scope first, add params,
        // then lower_block will push another scope.
        ctx.scopes.push();
        if let Some(param_list) = fn_ast.param_list() {
            for param_ast in param_list.params() {
                let name: Text = param_ast
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let ty = param_ast.ty().map(|t| lower_type_ref(&t));
                let ptr = SyntaxNodePtr::new(param_ast.syntax());
                let pat_id = ctx.alloc_pat(Pat::Missing, ptr);
                let local_id = ctx.body.locals.alloc(Local {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable: false,
                    pat: pat_id,
                });
                ctx.body.pats[pat_id] = Pat::Bind(local_id);
                ctx.scopes.define(name, local_id);
            }
        }

        let block_id = ctx.lower_block(block);
        let lowered_block = &ctx.body.blocks[block_id];
        ctx.body.block = lowered_block.stmts.clone();
        ctx.body.tail = lowered_block.tail;
        ctx.scopes.pop();
    }
    let (body, diagnostics) = ctx.finish();
    hir.diagnostics.extend(diagnostics);
    hir.bodies.alloc(body)
}
