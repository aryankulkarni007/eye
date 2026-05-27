use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

use ast::AstNode;
use clap::Parser;
use codegen::core::CGen;
use hir::core::{HIR, lower_source_file};
use lexer::{Interner, Lexer, SourceText, Symbol};
use syntax::SyntaxToken;

/// Command-line surface for the `eye` driver. Dump flags are off by default
/// so a normal compile stays quiet; pass any subset to surface the matching
/// IR for debugging.
#[derive(Parser, Debug)]
#[command(
    name = "eye",
    about = "Eye compiler driver (transpiles .eye -> C -> native via clang)"
)]
struct Cli {
    /// Source file to compile. Must have a `.eye` extension.
    input: PathBuf,

    /// Print the lossless rowan CST before parsing diagnostics are checked.
    #[arg(long)]
    dump_cst: bool,

    /// Print the typed AST as a structured summary.
    #[arg(long)]
    dump_ast: bool,

    /// Print the interner contents (every identifier and string literal).
    #[arg(long)]
    dump_symbols: bool,

    /// Print the fully-lowered HIR (items, bodies, expr arenas, types).
    #[arg(long)]
    dump_hir: bool,
}

/// a token's text, or a placeholder when the parse left the slot empty.
fn tok_text(t: Option<SyntaxToken>) -> String {
    t.map(|t| t.text().to_string())
        .unwrap_or_else(|| "<missing>".to_string())
}

/// one-line summary of an expression - recurses through calls.
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

            let field = field_expr
                .name_ref()
                .and_then(|n| n.name())
                .map(|t| tok_text(Some(t)))
                .unwrap_or_else(|| "<missing>".to_string());

            format!("{base}.{field}")
        }
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
        // v0.2 expression kinds: dump as opaque placeholders until the
        // pretty-printer learns each form.
        ast::Expr::AssignExpr(_) => "<assign>".to_string(),
        ast::Expr::IfExpr(_) => "<if>".to_string(),
        ast::Expr::LoopExpr(_) => "<loop>".to_string(),
        ast::Expr::BreakExpr(_) => "<break>".to_string(),
        ast::Expr::ContinueExpr(_) => "<continue>".to_string(),
        ast::Expr::RefExpr(_) => "<ref>".to_string(),
        ast::Expr::DerefExpr(_) => "<deref>".to_string(),
    }
}

/// Render an `ast::TypeRef` as the surface form it covers in the source.
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
            // a `let_stmt` type is optional (`ty?`): absent means the type
            // is inferred, not a recovery hole. A present `TypeRef` is
            // rendered via [`describe_type_ref`].
            let ty = match l.ty() {
                None => "<inferred>".to_string(),
                Some(t) => describe_type_ref(&t),
            };
            let value = l
                .value()
                .map(|e| describe_expr(&e))
                .unwrap_or_else(|| "<missing>".to_string());
            println!("    {kw} {ty} {} = {value}", tok_text(l.name()));
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

/// walks the typed ast and prints a structured summary - a visible check
/// that the typed layer reads the CST correctly.
fn dump_ast(file: &ast::SourceFile) {
    println!("\n--- AST ---");
    for item in file.items() {
        match item {
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
                println!("fn {}()", tok_text(fd.name()));
                if let Some(body) = fd.body() {
                    for stmt in body.stmts() {
                        dump_stmt(&stmt);
                    }
                }
            }
            ast::Item::EnumDef(e) => {
                println!("enum {}", tok_text(e.name()));
                for v in e.variants() {
                    println!("  | {}", tok_text(v.name()));
                }
            }
        }
    }
}

/// Prints the interned string table - every identifier and string literal,
/// deduplicated, in intern order. Proof the lexer populated the [`Interner`]
/// handed off in `Lexed`; HIR name resolution will re-intern against it.
fn dump_symbols(interner: &Interner) {
    println!("\n--- SYMBOLS ({}) ---", interner.len());
    for i in 0..interner.len() {
        println!("  #{i} {:?}", interner.lookup(Symbol(i as u32)));
    }
}

/// Dump the full HIR to stderr for debugging. Call this from `main.rs`
/// or any binary target to inspect what lowering produced.
pub fn dump_hir(hir: &HIR) {
    eprintln!("===== HIR DUMP =====");
    eprintln!("--- Structs ({}) ---", hir.structs.len());
    for (id, s) in hir.structs.iter() {
        eprintln!("  Struct({:?}): name={}, fields={:?}", id, s.name, s.fields);
    }
    eprintln!("--- Enums ({}) ---", hir.enums.len());
    for (id, e) in hir.enums.iter() {
        eprintln!(
            "  Enum({:?}): name={}, variants={:?}",
            id, e.name, e.variants
        );
    }
    eprintln!("--- Fields ({}) ---", hir.fields.len());
    for (id, f) in hir.fields.iter() {
        eprintln!("  Field({:?}): name={}, ty={:?}", id, f.name, f.ty);
    }
    eprintln!("--- Functions ({}) ---", hir.functions.len());
    for (id, f) in hir.functions.iter() {
        eprintln!(
            "  Fn({:?}): name={}, params={:?}, ret={:?}, body={:?}",
            id, f.name, f.params, f.ret, f.body
        );
    }
    eprintln!("--- ItemScope ---");
    eprintln!("  functions: {:?}", hir.items.functions);
    eprintln!("  structs:   {:?}", hir.items.structs);
    eprintln!("  enums:     {:?}", hir.items.enums);
    eprintln!("--- Bodies ---");
    for (id, body) in hir.bodies.iter() {
        eprintln!("  Body({:?}):", id);
        eprintln!("    locals ({})", body.locals.len());
        for (lid, local) in body.locals.iter() {
            eprintln!(
                "      Local({:?}): name='{}', ty={:?}, mutable={}",
                lid, local.name, local.ty, local.mutable
            );
        }
        eprintln!("    pats ({})", body.pats.len());
        for (pid, pat) in body.pats.iter() {
            eprintln!("      Pat({:?}): {:?}", pid, pat);
        }
        eprintln!("    exprs ({})", body.exprs.len());
        for (eid, expr) in body.exprs.iter() {
            let ty = body.expr_types.get(eid);
            eprintln!("      Expr({:?}): {:?}", eid, expr);
            if let Some(t) = ty {
                eprintln!("        type: {:?}", t);
            }
        }
        eprintln!("    stmts ({})", body.stmts.len());
        for (sid, stmt) in body.stmts.iter() {
            eprintln!("      Stmt({:?}): {:?}", sid, stmt);
        }
        eprintln!("    blocks ({})", body.blocks.len());
        for (bid, block) in body.blocks.iter() {
            eprintln!(
                "      Block({:?}): stmts={:?}, tail={:?}",
                bid, block.stmts, block.tail
            );
        }
        eprintln!("    body.block: {:?}", body.block);
        eprintln!("    body.tail:  {:?}", body.tail);
    }
    eprintln!("--- Diagnostics ({}) ---", hir.diagnostics.len());
    for d in &hir.diagnostics {
        eprintln!("  {}: {:?}", d.msg, d.ptr);
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let input_path: &Path = cli.input.as_path();

    // Validate input extension so we never overwrite a non-eye source when
    // deriving the C output path below.
    if input_path.extension().and_then(|e| e.to_str()) != Some("eye") {
        eprintln!(
            "error: expected a `.eye` source file, got `{}`",
            input_path.display()
        );
        std::process::exit(1);
    }

    let file = std::fs::File::open(input_path)?;
    let mmap = unsafe { memmap2::Mmap::map(&file)? };
    let source = SourceText::from_mmap(mmap);

    let lexed = Lexer::new(&source).tokenize();

    if !lexed.diags.is_empty() {
        eprintln!("{} lexer diagnostic(s):", lexed.diags.len());
        for diag in &lexed.diags {
            let lc = source.line_col(diag.range.start());
            eprintln!("  {}:{}: {}", lc.line, lc.col, diag.msg);
        }
        std::process::exit(1);
    }

    if cli.dump_symbols {
        dump_symbols(&lexed.interner);
    }

    let parse = parser::parse(&lexed.tokens, &source);

    if cli.dump_cst {
        println!("\n--- CST ---");
        println!("{:#?}", parse.green);
    }

    // Error check before proceeding to code generation
    if !parse.errors.is_empty() {
        eprintln!("\n{} parse diagnostic(s):", parse.errors.len());
        for err in &parse.errors {
            let lc = source.line_col(err.range.start());
            eprintln!("  {}:{}: {}", lc.line, lc.col, err.msg);
        }
        std::process::exit(1);
    }

    let file_ast = ast::SourceFile::cast(parse.green.clone())
        .ok_or_else(|| anyhow::anyhow!("Root node is not a valid SourceFile"))?;

    if cli.dump_ast {
        dump_ast(&file_ast);
    }

    println!("compiling...");
    println!("lowering AST to HIR...");
    let hir = lower_source_file(file_ast);

    if cli.dump_hir {
        dump_hir(&hir);
    }

    if !hir.diagnostics.is_empty() {
        eprintln!("\n{} hir diagnostic(s):", hir.diagnostics.len());
        for diag in &hir.diagnostics {
            let lc = source.line_col(diag.ptr.text_range().start());
            eprintln!("  {}:{}: {}", lc.line, lc.col, diag.msg);
        }
        std::process::exit(1);
    }

    println!("generating c code...");
    let generator = CGen::new(&hir);
    let mut generated_c = generator.gen_all();

    println!("formatting c code...");
    generated_c = format_with_clang_format(generated_c);

    let c_output_path = input_path.with_extension("c");
    let binary_path = input_path.with_extension("");
    let mut c_file = File::create(&c_output_path)?;
    c_file.write_all(generated_c.as_bytes())?;
    println!("c source written to {}", c_output_path.display());

    println!("invoking c compiler...");
    let compile_status = Command::new("clang")
        .arg(&c_output_path)
        .arg("-o")
        .arg(&binary_path)
        .arg("-O2")
        .status();

    match compile_status {
        Ok(status) if status.success() => {
            println!("build successful: run `{}`", binary_path.display());
        }
        Ok(status) => {
            eprintln!("\nbackend compilation failed: {}", status);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("\nFailed to launch C compiler (is clang installed?): {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Pipe `source` through `clang-format`, returning the formatted text or the
/// original input on any failure (with a diagnostic). Drains stdin from a
/// dedicated writer thread so the call cannot deadlock when both pipes fill.
fn format_with_clang_format(source: String) -> String {
    let mut child = match Command::new("clang-format")
        .arg("--fallback-style=LLVM")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            println!("  (Note: clang-format missing from system; writing raw C layout)");
            return source;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            eprintln!("  (clang-format stdin unavailable; using raw layout)");
            return source;
        }
    };

    let input_bytes = source.clone().into_bytes();
    let writer = thread::spawn(move || stdin.write_all(&input_bytes));

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            let _ = writer.join();
            eprintln!("  (clang-format wait failed: {}; using raw layout)", e);
            return source;
        }
    };

    if let Ok(Err(e)) = writer.join() {
        eprintln!(
            "  (clang-format stdin write failed: {}; using raw layout)",
            e
        );
        return source;
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "  (clang-format exited {}; using raw layout)",
            output.status
        );
        if !stderr.trim().is_empty() {
            eprintln!("  clang-format stderr: {}", stderr.trim());
        }
        return source;
    }

    match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "  (clang-format produced non-UTF-8 output: {}; using raw layout)",
                e
            );
            source
        }
    }
}
