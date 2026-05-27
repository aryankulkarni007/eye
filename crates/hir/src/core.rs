//! HIR lowering: AST -> name-resolved, desugared, arena-allocated IR.
//!
//! Two layers per crate:
//! - **ItemTree**: module-level signatures (structs, fn headers). One per file.
//!   Forward references work because we collect all items before lowering any body.
//! - **Body**: per-function expression/statement/pattern arenas plus a source
//!   map back to syntax pointers. Per-fn so editing one fn body invalidates
//!   only that body, not the whole crate.
//!
//! Pipeline runs in three passes from [`lower_source_file`]:
//! 1. `collect_items` registers every top-level [`Struct`] and [`Function`]
//!    in [`HIR::items`]. Forward refs work because bodies have not been walked
//!    yet. Duplicate declarations emit an [`HirDiagnostic`]; the later
//!    definition still overwrites the earlier one in [`ItemScope`], and both
//!    items keep their arena slots so existing IDs do not invalidate.
//! 2. Name resolution. Type resolution is deferred to codegen for v0.1: a
//!    [`TypeRef`] stays as a `Path(name)` string with no `StructId` attached.
//!    Value resolution (locals + items) is folded into pass 3 since lexical
//!    scopes only exist inside a body.
//! 3. `lower_fn_body` walks each fn's `Block` with a fresh [`LoweringCtx`].
//!    Each [`Expr::Path`] carries its [`Resolution`] so later passes never
//!    redo the lookup.

use ast::{AstNode, BinOp, UnaryOp};
use la_arena::{Arena, ArenaMap, Idx};
use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

pub type Text = SmolStr;

// ---- IDs ----

pub type StructId = Idx<Struct>;
pub type EnumId = Idx<Enum>;
pub type FnId = Idx<Function>;
pub type FieldId = Idx<Field>;
pub type ExprId = Idx<Expr>;
pub type StmtId = Idx<Stmt>;
pub type PatId = Idx<Pat>;
pub type LocalId = Idx<Local>;
pub type BlockId = Idx<Block>;
pub type BodyId = Idx<Body>;

// ---- module-level items ----

#[derive(Debug)]
pub struct Struct {
    pub name: Text,
    pub fields: Vec<FieldId>,
}

#[derive(Debug)]
pub struct Enum {
    pub name: Text,
    pub variants: Vec<Variant>,
}

#[derive(Debug)]
pub struct Variant {
    pub name: Text,
}

#[derive(Debug)]
pub struct Field {
    pub name: Text,
    pub ty: TypeRef,
}

#[derive(Debug)]
pub struct Function {
    pub name: Text,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    /// Body lives in its own arena keyed by [`FnId`] on [`HIR`].
    pub body: Option<BodyId>,
}

#[derive(Debug)]
pub struct Param {
    pub name: Text,
    pub ty: TypeRef,
}

// ---- types ----
//
// Stays *unresolved* at HIR time: just a name. Type inference / resolution
// runs in a later pass and produces real `Ty` ids. Builtins (`int32`, `bool`)
// are still recognized here as a convenience.

// TODO: store types in arena instead of Box
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeRef {
    Path(Text),
    Ref(Box<TypeRef>), // &T
    Ptr(Box<TypeRef>), // *T
    Error,
}

// ---- bodies ----

#[derive(Debug, Default)]
pub struct Body {
    pub exprs: Arena<Expr>,
    pub stmts: Arena<Stmt>,
    pub pats: Arena<Pat>,
    pub locals: Arena<Local>,
    /// Top-level statements of the fn body, in source order.
    pub block: Vec<StmtId>,
    /// Optional tail expression of the body block (none for v0.1).
    pub tail: Option<ExprId>,
    pub source_map: BodySourceMap,
    pub blocks: Arena<Block>,
    pub block_source_map: ArenaMap<BlockId, SyntaxNodePtr>,
    pub expr_types: ArenaMap<ExprId, TypeRef>,
}

#[derive(Debug, Default)]
pub struct BodySourceMap {
    pub expr: ArenaMap<ExprId, SyntaxNodePtr>,
    pub stmt: ArenaMap<StmtId, SyntaxNodePtr>,
    pub pat: ArenaMap<PatId, SyntaxNodePtr>,
}

#[derive(Debug)]
pub struct Local {
    pub name: Text,
    pub ty: Option<TypeRef>,
    pub mutable: bool,
    pub pat: PatId,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<StmtId>,
    pub tail: Option<ExprId>,
}

#[derive(Debug)]
pub enum Stmt {
    Let {
        pat: PatId,
        ty: Option<TypeRef>,
        init: Option<ExprId>,
        mutable: bool,
    },
    Expr(ExprId),
}

#[derive(Debug)]
pub enum Pat {
    Bind(LocalId),
    Missing,
}

#[derive(Debug)]
pub enum Expr {
    Missing,
    Literal(Literal),
    /// Resolved local, function, or unknown name. Resolution result is stored
    /// here so later passes don't redo the lookup.
    Path(Resolution),
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    StructLit {
        ty: TypeRef,
        fields: Vec<StructLitField>,
    },
    Field {
        base: ExprId,
        name: Text,
    },
    Assign {
        lhs: ExprId,
        rhs: ExprId,
    },
    If {
        cond: ExprId,
        then_branch: BlockId,
        else_branch: Option<BlockId>,
    },
    Loop {
        body: BlockId,
    },
    Break,
    Continue,
    Ref {
        operand: ExprId,
    },
    Deref {
        operand: ExprId,
    },
    Block(BlockId),
}

#[derive(Debug)]
pub struct StructLitField {
    pub name: Text,
    /// Always materialized. Shorthand `Point { x }` is desugared at lowering
    /// into `Point { x: x }` where the value is a synthesized `Path` expr
    /// whose source-map entry points at the same `StructLitField` syntax node
    /// as the field name.
    pub value: ExprId,
}

#[derive(Debug)]
pub enum Literal {
    Int(u128),
    Float(SmolStr),
    String(SmolStr),
    Bool(bool),
    Char(char),
}

/// Result of name resolution for a `NameRef`. Diagnostic-friendly: unresolved
/// becomes [`Resolution::Unresolved`] (not a hard error here).
#[derive(Debug, Clone)]
pub enum Resolution {
    Local(LocalId),
    Fn(FnId),
    Struct(StructId),
    Enum(EnumId),
    Unresolved(Text),
}

// ---- top-level HIR ----

#[derive(Debug, Default)]
pub struct HIR {
    pub structs: Arena<Struct>,
    pub enums: Arena<Enum>,
    pub fields: Arena<Field>,
    pub functions: Arena<Function>,
    pub bodies: Arena<Body>,
    /// Module-level scope. Both namespaces flat for v0.1 since structs + fns
    /// don't collide (struct names start uppercase by convention, but the
    /// resolver treats them in one map until the language says otherwise).
    pub items: ItemScope,
    /// Diagnostics produced during lowering. Non-empty means the input had
    /// semantic issues even if the parser was happy.
    pub diagnostics: Vec<HirDiagnostic>,
}

/// A semantic diagnostic raised during HIR lowering.
#[derive(Debug, Clone)]
pub struct HirDiagnostic {
    pub ptr: SyntaxNodePtr,
    pub msg: String,
}

#[derive(Debug, Default)]
pub struct ItemScope {
    pub functions: FxHashMap<Text, FnId>,
    pub structs: FxHashMap<Text, StructId>,
    pub enums: FxHashMap<Text, EnumId>,
}

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
    // TODO: diagnostics sink
}

impl<'a> LoweringCtx<'a> {
    pub fn new(hir: &'a HIR) -> Self {
        Self {
            hir,
            body: Body::default(),
            scopes: Scopes::new(),
        }
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

    fn finish(self) -> Body {
        self.body
    }

    /// Resolve a `NameRef`. Lexical scopes first, then module-level values,
    /// then types. Unknown names produce [`Resolution::Unresolved`] so later
    /// diagnostics still have the original text.
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
        Resolution::Unresolved(name.clone())
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
                // look up the type of the resolved entity.
                expr_type = match &resolution {
                    Resolution::Local(local_id) => self.body.locals[*local_id].ty.clone(),
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
                let base = match fe.expr() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };
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
                // We don't store the optional value yet; could be extended later.
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
    hir.bodies.alloc(ctx.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{AstNode, SourceFile};
    use lexer::{Lexer, SourceText};

    fn lower(src: &str) -> HIR {
        let source = SourceText::new(src.to_string());
        let tokens = Lexer::new(&source).tokenize().tokens;
        let parse = parser::parse(&tokens, &source);
        let file = SourceFile::cast(parse.green).expect("root is SourceFile");
        lower_source_file(file)
    }

    const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    const x = 0;
    const y = 0;
    var Point p = Point { x, y };

    print(\"{}\", p);
}
";

    #[test]
    fn items_collected() {
        let hir = lower(MAIN_EYE);
        assert_eq!(hir.structs.len(), 1);
        assert_eq!(hir.functions.len(), 1);
        assert!(hir.items.structs.contains_key("Point"));
        assert!(hir.items.functions.contains_key("main"));
    }

    #[test]
    fn shorthand_struct_lit_desugared() {
        let hir = lower(MAIN_EYE);
        let main_id = *hir.items.functions.get("main").unwrap();
        let body_id = hir.functions[main_id].body.expect("main has body");
        let body = &hir.bodies[body_id];

        // find the StructLit init of `p`
        let mut sl_field_count = 0;
        for (_, expr) in body.exprs.iter() {
            if let Expr::StructLit { fields, .. } = expr {
                sl_field_count = fields.len();
                for f in fields {
                    // shorthand must be materialized: every field has a real
                    // ExprId (no Option). The synthesized expr resolves to
                    // the local of the same name.
                    let inner = &body.exprs[f.value];
                    match inner {
                        Expr::Path(Resolution::Local(_)) => {}
                        other => panic!(
                            "shorthand field {} did not desugar to a Local path: {:?}",
                            f.name, other
                        ),
                    }
                }
            }
        }
        assert_eq!(sl_field_count, 2, "Point literal has two fields");
    }

    #[test]
    fn duplicate_struct_emits_diagnostic() {
        let hir = lower(
            "\
structure Point {
    int32 x,
};

structure Point {
    int32 y,
};

main() {}
",
        );
        assert_eq!(
            hir.diagnostics.len(),
            1,
            "expected one diagnostic, got: {:?}",
            hir.diagnostics
        );
        assert!(
            hir.diagnostics[0].msg.contains("duplicate item `Point`"),
            "unexpected message: {}",
            hir.diagnostics[0].msg
        );
        // both struct arena slots persist so existing IDs stay valid
        assert_eq!(hir.structs.len(), 2);
    }

    #[test]
    fn duplicate_fn_emits_diagnostic() {
        let hir = lower(
            "\
main() {}
main() {}
",
        );
        assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
        assert!(
            hir.diagnostics[0].msg.contains("duplicate item `main`"),
            "unexpected message: {}",
            hir.diagnostics[0].msg
        );
        assert_eq!(hir.functions.len(), 2);
    }

    #[test]
    fn fn_and_struct_with_same_name_collide() {
        // Cross-namespace collision should still be flagged: in v0.1 the
        // resolver treats both namespaces as one for name-resolution.
        let hir = lower(
            "\
structure Foo {
    int32 x,
};

Foo() {}
",
        );
        assert_eq!(hir.diagnostics.len(), 1, "{:?}", hir.diagnostics);
        assert!(
            hir.diagnostics[0].msg.contains("duplicate item `Foo`"),
            "unexpected message: {}",
            hir.diagnostics[0].msg
        );
    }

    #[test]
    fn well_formed_program_has_no_diagnostics() {
        let hir = lower(MAIN_EYE);
        assert!(
            hir.diagnostics.is_empty(),
            "expected zero diagnostics, got: {:?}",
            hir.diagnostics
        );
    }

    /// Regression for the `NameRef::nth(1)` bug: when the base of a field
    /// access is itself a field expression (`a.b.c`), the outer FieldExpr
    /// has only one direct NameRef child (the field name); `nth(1)` would
    /// silently return `None` and drop the name.
    #[test]
    fn nested_field_access_resolves_field_name() {
        let src = "\
main() {
    print(\"{}\", a.b.c);
}
";
        let hir = lower(src);
        let main_id = *hir.items.functions.get("main").unwrap();
        let body_id = hir.functions[main_id].body.expect("main has body");
        let body = &hir.bodies[body_id];

        // collect every Expr::Field name; expect `c` and `b` to be present.
        let mut names: Vec<&str> = body
            .exprs
            .iter()
            .filter_map(|(_, e)| match e {
                Expr::Field { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["b", "c"], "nested field access dropped a name");
    }

    /// Regression for the lower-block ordering bug: a block's tail expression
    /// (typically a `loop { ... }` body) used to be lowered *before* the
    /// preceding stmts, so locals defined by those stmts were not yet in
    /// scope. NameRefs inside the loop body fell through to
    /// `Resolution::Unresolved`, which downstream made auto-deref on field
    /// access impossible.
    #[test]
    fn tail_expression_sees_locals_defined_by_preceding_stmts() {
        let src = "\
structure P {
    int32 x,
};

main() {
    var P p = P { x: 0 };
    var &P p_ref = &p;
    loop {
        if p_ref.x > 10 { break; }
        p_ref.x = p_ref.x + 1;
    }
}
";
        let hir = lower(src);
        let main_id = *hir.items.functions.get("main").unwrap();
        let body_id = hir.functions[main_id].body.expect("main has body");
        let body = &hir.bodies[body_id];

        // Every `Path` expression that names `p_ref` must resolve to a
        // Local, not fall through to Unresolved.
        let unresolved_p_ref = body.exprs.iter().any(|(_, e)| {
            matches!(e, Expr::Path(Resolution::Unresolved(n)) if n.as_str() == "p_ref")
        });
        assert!(
            !unresolved_p_ref,
            "p_ref inside the tail loop body did not resolve to the outer local"
        );
    }

    /// Manual dump - run with `cargo test -p eye-hir dump -- --nocapture`.
    #[test]
    fn dump_main_eye() {
        let hir = lower(MAIN_EYE);
        eprintln!("---- HIR.items ----\n{:#?}", hir.items);
        eprintln!("---- HIR.structs ----\n{:#?}", hir.structs);
        eprintln!("---- HIR.fields ----\n{:#?}", hir.fields);
        eprintln!("---- HIR.functions ----\n{:#?}", hir.functions);
        for (id, body) in hir.bodies.iter() {
            eprintln!("---- Body {:?} ----", id);
            eprintln!("locals: {:#?}", body.locals);
            eprintln!("pats:   {:#?}", body.pats);
            eprintln!("stmts:  {:#?}", body.stmts);
            eprintln!("exprs:  {:#?}", body.exprs);
            eprintln!("block:  {:?}", body.block);
        }
    }
}
