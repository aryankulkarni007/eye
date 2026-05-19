use eye::ast;
use eye::lexer::{Interner, Lexer, SourceText, Symbol};
use eye::parser;
use eye::syntax::SyntaxToken;

/// A token's text, or a placeholder when the parse left the slot empty.
fn tok_text(t: Option<SyntaxToken>) -> String {
    t.map(|t| t.text().to_string())
        .unwrap_or_else(|| "<missing>".to_string())
}

/// One-line summary of an expression — recurses through calls.
fn describe_expr(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Literal(l) => match l.literal_kind() {
            Some(k) => format!("{k:?}({})", tok_text(l.token())),
            None => format!("literal({})", tok_text(l.token())),
        },
        ast::Expr::NameRef(n) => format!("name {}", tok_text(n.name())),
        ast::Expr::CallExpr(c) => {
            let callee = c
                .callee()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            let argc = c.arg_list().map(|a| a.args().count()).unwrap_or(0);
            format!("call {callee} ({argc} args)")
        }
        ast::Expr::StructLit(s) => {
            let name = s.name_ref().and_then(|n| n.name());
            let fieldc = s.field_list().map(|fl| fl.fields().count()).unwrap_or(0);
            format!("struct-lit {} ({fieldc} fields)", tok_text(name))
        }
    }
}

/// Prints a statement under a function body.
fn dump_stmt(stmt: &ast::Stmt) {
    match stmt {
        ast::Stmt::LetStmt(l) => {
            let kw = match l.kind() {
                Some(ast::LetKind::Const) => "const",
                Some(ast::LetKind::Var) => "var",
                None => "<missing>",
            };
            let ty = l.type_ref().and_then(|t| t.name());
            let value = l
                .value()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!(
                "    {kw} {} {} = {value}",
                tok_text(ty),
                tok_text(l.name()),
            );
        }
        ast::Stmt::ExprStmt(e) => {
            let expr = e
                .expr()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!("    expr {expr}");
        }
    }
}

/// Walks the typed AST and prints a structured summary — a visible check
/// that the typed layer reads the CST correctly.
fn dump_ast(file: &ast::SourceFile) {
    println!("\n--- AST ---");
    for item in file.items() {
        match item {
            ast::Item::StructDef(s) => {
                println!("structure {}", tok_text(s.name()));
                if let Some(fl) = s.field_list() {
                    for f in fl.fields() {
                        let ty = f.type_ref().and_then(|t| t.name());
                        println!("  field {} {}", tok_text(ty), tok_text(f.name()));
                    }
                }
            }
            ast::Item::FnDef(fd) => {
                println!("fn {}()", tok_text(fd.name()));
                if let Some(body) = fd.body() {
                    for stmt in body.stmts() {
                        dump_stmt(&stmt);
                    }
                }
            }
        }
    }
}

/// Prints the interned string table — every identifier and string literal,
/// deduplicated, in intern order. Proof the lexer populated the [`Interner`]
/// handed off in `Lexed`; HIR name resolution will re-intern against it.
fn dump_symbols(interner: &Interner) {
    println!("\n--- SYMBOLS ({}) ---", interner.len());
    for i in 0..interner.len() {
        println!("  #{i} {:?}", interner.lookup(Symbol(i as u32)));
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // check usage
    if args.len() < 2 {
        eprintln!("usage: {} <file.eye>", &args[0]);
        std::process::exit(-1);
    }

    let file = std::fs::File::open(&args[1])?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };
    let source = SourceText::from_mmap(mmap);

    let lexed = Lexer::new(&source).tokenize();

    if !lexed.diags.is_empty() {
        eprintln!("{} lexer diagnostic(s):", lexed.diags.len());
        for diag in &lexed.diags {
            let lc = source.line_col(diag.span.start);
            eprintln!("  {}:{}: {}", lc.line, lc.col, diag.msg);
        }
        std::process::exit(1);
    }

    let parse = parser::parse(&lexed.tokens, &source);
    println!("{:#?}", parse.green);

    if let Some(file) = ast::AstNode::cast(parse.green.clone()) {
        dump_ast(&file);
    }
    dump_symbols(&lexed.interner);

    if !parse.errors.is_empty() {
        eprintln!("\n{} parse diagnostic(s):", parse.errors.len());
        for err in &parse.errors {
            let lc = source.line_col(err.span.start);
            eprintln!("  {}:{}: {}", lc.line, lc.col, err.msg);
        }
        std::process::exit(1);
    }

    Ok(())
}
