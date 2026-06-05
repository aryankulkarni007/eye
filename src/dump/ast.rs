use ast::AstNode;
use syntax::SyntaxToken;

/// A token's text, or a placeholder when the parse left the slot empty.
fn tok_text(t: Option<SyntaxToken>) -> String {
    t.map(|t| t.text().to_string())
        .unwrap_or_else(|| "<missing>".to_string())
}

fn join_exprs(exprs: impl Iterator<Item = ast::Expr>) -> String {
    exprs
        .map(|e| describe_expr(&e))
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_block_summary(block: ast::Block) -> String {
    let stmt_count = block.stmts().count();
    let tail = block
        .tail_expr()
        .map(|e| describe_expr(&e))
        .unwrap_or_else(|| "<none>".to_string());
    format!("{{ {stmt_count} stmt(s); tail {tail} }}")
}

fn describe_name_ref(name_ref: Option<ast::NameRef>) -> String {
    name_ref
        .and_then(|n| n.name())
        .map(|t| tok_text(Some(t)))
        .unwrap_or_else(|| "<missing>".to_string())
}

fn describe_field_name(field_expr: &ast::FieldExpr) -> String {
    ast::support::children::<ast::NameRef>(field_expr.syntax())
        .last()
        .and_then(|n| n.name())
        .map(|t| tok_text(Some(t)))
        .unwrap_or_else(|| "<missing>".to_string())
}

fn describe_pat(pat: &ast::Pat) -> String {
    match pat {
        ast::Pat::PathPat(p) => {
            let qualifier = describe_name_ref(p.qualifier());
            let name = describe_name_ref(p.name());
            format!("{qualifier}.{name}")
        }
        ast::Pat::BareIdentPat(p) => describe_name_ref(p.name()),
        ast::Pat::WildcardPat(_) => "_".to_string(),
        ast::Pat::LiteralPat(p) => p
            .literal()
            .and_then(|l| l.token())
            .map(|t| t.text().to_string())
            .unwrap_or_else(|| "<missing>".to_string()),
    }
}

fn describe_struct_lit_field(field: &ast::StructLitField) -> String {
    match (field.name(), field.value()) {
        (Some(name), Some(value)) => format!("{}: {}", tok_text(Some(name)), describe_expr(&value)),
        (Some(name), None) => tok_text(Some(name)),
        (None, Some(value)) => describe_expr(&value),
        (None, None) => "<missing>".to_string(),
    }
}

fn describe_params(params: Option<ast::ParamList>) -> String {
    params
        .map(|pl| {
            let mut parts: Vec<String> = pl
                .params()
                .map(|p| {
                    let ty = p
                        .ty()
                        .map(|t| describe_type_ref(&t))
                        .unwrap_or_else(|| "<missing>".to_string());
                    format!("{ty} {}", tok_text(p.name()))
                })
                .collect();
            if pl.variadic().is_some() {
                parts.push("...".to_string());
            }
            parts.join(", ")
        })
        .unwrap_or_else(|| "<missing>".to_string())
}

fn describe_ret_type(ret_type: Option<ast::TypeRef>) -> String {
    ret_type
        .map(|t| describe_type_ref(&t))
        .unwrap_or_else(|| "void".to_string())
}

fn describe_expr(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Literal(l) => match l.literal_kind() {
            Some(k) => format!("{k:?}({})", tok_text(l.token())),
            None => format!("literal({})", tok_text(l.token())),
        },
        ast::Expr::NameRef(n) => format!("name {}", tok_text(n.name())),
        ast::Expr::FieldExpr(field_expr) => {
            let base = field_expr
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let field = describe_field_name(field_expr);
            format!("{base}.{field}")
        }
        ast::Expr::CallExpr(c) => {
            let callee = c
                .callee()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let args = c
                .arg_list()
                .map(|a| join_exprs(a.args()))
                .unwrap_or_default();
            format!("{callee}({args})")
        }
        ast::Expr::StructLit(s) => {
            let name = s.name_ref().and_then(|n| n.name());
            let fields = s
                .field_list()
                .map(|fl| {
                    fl.fields()
                        .map(|f| describe_struct_lit_field(&f))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "<missing>".to_string());
            format!("{} {{ {fields} }}", tok_text(name))
        }
        ast::Expr::ArrayLit(a) => {
            format!("[{}]", join_exprs(a.elems()))
        }
        ast::Expr::ArrayRepeat(ar) => {
            let value = ar
                .value()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let count = ar
                .count()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("[{value}; {count}]")
        }
        ast::Expr::IndexExpr(ie) => {
            let base = ie
                .base()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let index = ie
                .index()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("index {base}[{index}]")
        }
        ast::Expr::BinExpr(b) => {
            let lhs = b
                .lhs()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let rhs = b
                .rhs()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            match b.op() {
                Some(op) => format!("({lhs} {op:?} {rhs})"),
                None => format!("({lhs} ? {rhs})"),
            }
        }
        ast::Expr::PrefixExpr(u) => {
            let operand = u
                .operand()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            match u.op() {
                Some(op) => format!("({op:?} {operand})"),
                None => format!("(? {operand})"),
            }
        }
        ast::Expr::AssignExpr(a) => {
            let lhs = a
                .lhs()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let rhs = a
                .rhs()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("({lhs} = {rhs})")
        }
        ast::Expr::IfExpr(i) => {
            let condition = i
                .condition()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let then_branch = i
                .then_branch()
                .map(describe_block_summary)
                .unwrap_or_else(|| "{ <missing> }".to_string());
            match i.else_branch() {
                Some(else_branch) => {
                    format!(
                        "if {condition} {then_branch} else {}",
                        describe_block_summary(else_branch)
                    )
                }
                None => format!("if {condition} {then_branch}"),
            }
        }
        ast::Expr::LoopExpr(l) => {
            let body = l
                .body()
                .map(describe_block_summary)
                .unwrap_or_else(|| "{ <missing> }".to_string());
            format!("loop {body}")
        }
        ast::Expr::BreakExpr(b) => match b.expr() {
            Some(expr) => format!("break {}", describe_expr(&expr)),
            None => "break".to_string(),
        },
        ast::Expr::ContinueExpr(_) => "continue".to_string(),
        ast::Expr::ReturnExpr(r) => match r.expr() {
            Some(expr) => format!("return {}", describe_expr(&expr)),
            None => "return".to_string(),
        },
        ast::Expr::RefExpr(r) => {
            let expr = r
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("&{expr}")
        }
        ast::Expr::DerefExpr(d) => {
            let expr = d
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("*{expr}")
        }
        ast::Expr::CastExpr(c) => {
            let operand = c
                .operand()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let ty = c
                .ty()
                .map(|t| describe_type_ref(&t))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("({operand} as {ty})")
        }
        ast::Expr::MatchExpr(m) => {
            let scrut = m
                .scrut()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let arms = m
                .arm_list()
                .map(|al| {
                    al.arms()
                        .map(|arm| {
                            let pat = arm
                                .pat()
                                .map(|p| describe_pat(&p))
                                .unwrap_or_else(|| "<missing>".to_string());
                            let body = arm
                                .body()
                                .map(|e| describe_expr(&e))
                                .unwrap_or_else(|| "<missing>".to_string());
                            format!("{pat} -> {body}")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "<missing>".to_string());
            format!("match {scrut} {{ {arms} }}")
        }
        ast::Expr::ParenExpr(pe) => {
            let expr = pe
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("({expr})")
        }
    }
}

fn describe_type_ref(t: &ast::TypeRef) -> String {
    match t {
        ast::TypeRef::IdentType(it) => tok_text(it.name()),
        ast::TypeRef::RefType(r) => match r.inner() {
            Some(inner) => format!("&{}", describe_type_ref(&inner)),
            None => "&<missing>".to_string(),
        },
        ast::TypeRef::PtrType(p) => match p.inner() {
            Some(inner) => format!("{}*", describe_type_ref(&inner)),
            None => "<missing>*".to_string(),
        },
        ast::TypeRef::ArrayType(a) => {
            let elem = a
                .elem()
                .map(|e| describe_type_ref(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let len = a
                .len()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            format!("[{elem}; {len}]")
        }
        ast::TypeRef::FnType(ft) => {
            let params = ft
                .params()
                .map(|p| {
                    p.ty()
                        .map(|t| describe_type_ref(&t))
                        .unwrap_or_else(|| "<missing>".to_string())
                })
                .collect::<Vec<_>>()
                .join(", ");
            match ft.ret_type() {
                Some(ret) => format!("({params}) -> {}", describe_type_ref(&ret)),
                None => format!("({params})"),
            }
        }
    }
}

fn dump_stmt(stmt: &ast::Stmt, indent: &str) {
    match stmt {
        ast::Stmt::LetStmt(l) => {
            let kw = match l.kind() {
                Some(ast::LetKind::Let) => "let",
                Some(ast::LetKind::Mut) => "mut",
                None => "<missing>",
            };
            let ty = match l.ty() {
                None => "<inferred>".to_string(),
                Some(t) => describe_type_ref(&t),
            };
            let value = l
                .value()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!("{indent}{kw} {ty} {} = {value}", tok_text(l.name()));
        }
        ast::Stmt::ExprStmt(e) => {
            let expr = e
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!("{indent}expr {expr}");
        }
        ast::Stmt::ConstDef(c) => {
            let ty = c
                .ty()
                .map(|t| describe_type_ref(&t))
                .unwrap_or_else(|| "<missing>".to_string());
            let value = c
                .value()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!("{indent}const {ty} {} = {value}", tok_text(c.name()));
        }
    }
}

fn dump_block(block: ast::Block, indent: &str) {
    for stmt in block.stmts() {
        dump_stmt(&stmt, indent);
    }
    if let Some(tail) = block.tail_expr() {
        println!("{indent}tail {}", describe_expr(&tail));
    }
}

/// Walks the typed AST and prints a structured summary.
pub fn dump_ast(file: &ast::SourceFile) {
    for item in file.items() {
        match item {
            ast::Item::ConstDef(c) => {
                let ty = c
                    .ty()
                    .map(|t| describe_type_ref(&t))
                    .unwrap_or_else(|| "<missing>".to_string());
                let value = c
                    .value()
                    .map(|e| describe_expr(&e))
                    .unwrap_or_else(|| "<missing>".to_string());
                println!("const {ty} {} = {value}", tok_text(c.name()));
            }
            ast::Item::GlobalDef(g) => {
                let kw = match g.kind() {
                    Some(ast::LetKind::Mut) => "mut",
                    _ => "let",
                };
                let ty = g
                    .ty()
                    .map(|t| describe_type_ref(&t))
                    .unwrap_or_else(|| "<missing>".to_string());
                let value = g
                    .value()
                    .map(|e| describe_expr(&e))
                    .unwrap_or_else(|| "<missing>".to_string());
                println!("{kw} {ty} {} = {value}", tok_text(g.name()));
            }
            ast::Item::StructDef(s) => {
                println!("structure {}", tok_text(s.name()));
                if let Some(fl) = s.field_list() {
                    for f in fl.fields() {
                        let ty = f
                            .ty()
                            .map(|t| describe_type_ref(&t))
                            .unwrap_or_else(|| "<missing>".to_string());
                        println!("  field {ty} {}", tok_text(f.name()));
                    }
                }
            }
            ast::Item::FnDef(fd) => {
                let params = describe_params(fd.param_list());
                let ret = describe_ret_type(fd.ret_type());
                println!("fn {}({params}) -> {ret}", tok_text(fd.name()));
                if let Some(body) = fd.body() {
                    dump_block(body, "    ");
                }
            }
            ast::Item::EnumDef(e) => {
                println!("enum {}", tok_text(e.name()));
                for v in e.variants() {
                    println!("  | {}", tok_text(v.name()));
                }
            }
            ast::Item::UnionDef(u) => {
                println!("union {}", tok_text(u.name()));
                if let Some(fl) = u.field_list() {
                    for f in fl.fields() {
                        let ty = f
                            .ty()
                            .map(|t| describe_type_ref(&t))
                            .unwrap_or_else(|| "<missing>".to_string());
                        println!("  field {ty} {}", tok_text(f.name()));
                    }
                }
            }
            ast::Item::ExternBlock(eb) => {
                println!("extern");
                for item in eb.items() {
                    match item {
                        ast::ExternItem::ExternFn(ef) => {
                            let params = describe_params(ef.param_list());
                            let ret = describe_ret_type(ef.ret_type());
                            println!("  extern fn {}({params}) -> {ret}", tok_text(ef.name()));
                        }
                        ast::ExternItem::ExternTypeDef(et) => {
                            println!("  extern type {}", tok_text(et.name()));
                        }
                    }
                }
            }
        }
    }
}
