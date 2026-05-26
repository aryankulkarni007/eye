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
//!    yet. Duplicate declarations are *not* detected: a second definition with
//!    the same name silently overwrites the first in [`ItemScope`]. Both items
//!    still occupy arena slots.
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
pub type FnId = Idx<Function>;
pub type FieldId = Idx<Field>;
pub type ExprId = Idx<Expr>;
pub type StmtId = Idx<Stmt>;
pub type PatId = Idx<Pat>;
pub type LocalId = Idx<Local>;

// ---- module-level items ----

#[derive(Debug)]
pub struct Struct {
    pub name: Text,
    pub fields: Vec<FieldId>,
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

pub type BodyId = Idx<Body>;

// ---- types ----
//
// Stays *unresolved* at HIR time: just a name. Type inference / resolution
// runs in a later pass and produces real `Ty` ids. Builtins (`int32`, `bool`)
// are still recognized here as a convenience.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeRef {
    Path(Text),
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
    Block(Vec<StmtId>),
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
    Unresolved(Text),
}

// ---- top-level HIR ----

#[derive(Debug, Default)]
pub struct HIR {
    pub structs: Arena<Struct>,
    pub fields: Arena<Field>,
    pub functions: Arena<Function>,
    pub bodies: Arena<Body>,
    /// Module-level scope. Both namespaces flat for v0.1 since structs + fns
    /// don't collide (struct names start uppercase by convention, but the
    /// resolver treats them in one map until the language says otherwise).
    pub items: ItemScope,
}

#[derive(Debug, Default)]
pub struct ItemScope {
    pub values: FxHashMap<Text, FnId>,
    pub types: FxHashMap<Text, StructId>,
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
        if let Some(&id) = self.hir.items.values.get(name) {
            return Resolution::Fn(id);
        }
        if let Some(&id) = self.hir.items.types.get(name) {
            return Resolution::Struct(id);
        }
        Resolution::Unresolved(name.clone())
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> StmtId {
        let ptr = SyntaxNodePtr::new(stmt.syntax());
        match stmt {
            ast::Stmt::LetStmt(l) => {
                let name: Text = l
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                let ty = l.type_ref().map(|t| lower_type_ref(&t));
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
        let hir_expr = match expr {
            ast::Expr::Literal(lit) => Expr::Literal(lower_literal(lit)),
            ast::Expr::NameRef(nr) => {
                let name: Text = nr
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                Expr::Path(self.resolve(&name))
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
                Expr::Call { callee, args }
            }
            ast::Expr::StructLit(sl) => {
                let ty = match sl.name_ref().and_then(|n| n.name()) {
                    Some(t) => TypeRef::Path(SmolStr::from(t.text())),
                    None => TypeRef::Error,
                };
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
                                self.alloc_expr(Expr::Path(resolution), f_ptr)
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
                Expr::Unary { op, operand }
            }
            ast::Expr::FieldExpr(fe) => {
                let base = match fe.expr() {
                    Some(e) => self.lower_expr(&e),
                    None => self.alloc_expr(Expr::Missing, ptr),
                };

                let name: SmolStr = fe
                    .syntax()
                    .children()
                    .filter_map(ast::NameRef::cast)
                    .nth(1)
                    .and_then(|nr| nr.name())
                    .map(|t| SmolStr::from(t.text().trim()))
                    .unwrap_or_default();

                Expr::Field { base, name }
            }
        };
        self.alloc_expr(hir_expr, ptr)
    }
}

// ---- free helpers ----

fn lower_type_ref(ty: &ast::TypeRef) -> TypeRef {
    match ty.name() {
        Some(t) => TypeRef::Path(SmolStr::from(t.text())),
        None => TypeRef::Error,
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
/// without re-traversing the file.
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
                        let ty = match f.type_ref() {
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
                hir.items.types.insert(name, struct_id);
            }
            ast::Item::FnDef(f) => {
                let name: Text = f
                    .name()
                    .map(|t| SmolStr::from(t.text()))
                    .unwrap_or_default();
                // v0.1 grammar: ParamList is `( )`. No params to collect.
                let fn_id = hir.functions.alloc(Function {
                    name: name.clone(),
                    params: Vec::new(),
                    ret: None,
                    body: None,
                });
                hir.items.values.insert(name, fn_id);
                fn_asts.push((fn_id, f));
            }
        }
    }
    fn_asts
}

fn lower_fn_body(hir: &mut HIR, fn_ast: &ast::FnDef) -> BodyId {
    let mut body = {
        let mut ctx = LoweringCtx::new(hir);
        if let Some(block) = fn_ast.body() {
            for stmt in block.stmts() {
                let id = ctx.lower_stmt(&stmt);
                ctx.body.block.push(id);
            }
        }
        ctx.finish()
    };
    // tail expr unsupported in v0.1 grammar; leave None.
    body.tail = None;
    hir.bodies.alloc(body)
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
        assert!(hir.items.types.contains_key("Point"));
        assert!(hir.items.values.contains_key("main"));
    }

    #[test]
    fn shorthand_struct_lit_desugared() {
        let hir = lower(MAIN_EYE);
        let main_id = *hir.items.values.get("main").unwrap();
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
