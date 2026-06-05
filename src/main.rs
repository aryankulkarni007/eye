use std::path::Path;

use ast::AstNode;
use clap::Parser;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};

mod backend;
mod cli;
mod diagnostics;
mod dump;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    let input_path: &Path = cli.input.as_path();

    // validate input extension so we never overwrite a non‑eye source when
    // deriving the c output path below.
    if input_path.extension().and_then(|e| e.to_str()) != Some("eye") {
        eprintln!(
            "error: expected a `.eye` source file, got `{}`",
            input_path.display()
        );
        return Err(anyhow::anyhow!("expected a `.eye` source file"));
    }

    let source = SourceText::new(std::fs::read_to_string(input_path)?);

    let lexed = Lexer::new(&source).tokenize();

    if !lexed.diags.is_empty() {
        diagnostics::render(&source, lexed.diags.into_diags(), None, Some(input_path));
        return Err(anyhow::anyhow!("lexer errors"));
    }

    if cli.dump_symbols {
        dump::symbols::dump_symbols(&lexed.interner);
    }

    let parse = parser::parse(&lexed.tokens, &source);

    if cli.dump_cst {
        println!("--- CST ---");
        println!("{:#?}", parse.green);
    }

    if !parse.diagnostics.is_empty() {
        diagnostics::render(
            &source,
            parse.diagnostics.into_diags(),
            Some(&parse.green),
            Some(input_path),
        );
        return Err(anyhow::anyhow!("parser errors"));
    }

    // parse‑stage oracle: lexer and parser were both clean above, which is all
    // tree‑sitter can verify. Stop before HIR so semantic errors (which
    // tree‑sitter never sees) can't masquerade as grammar drift in the parity
    // gate.
    if cli.check {
        return Ok(());
    }

    let file_ast = ast::SourceFile::cast(parse.green.clone())
        .ok_or_else(|| anyhow::anyhow!("Root node is not a valid SourceFile"))?;

    if cli.dump_ast {
        println!("--- AST ---");
        dump::ast::dump_ast(&file_ast);
    }

    println!("lowering AST to HIR...");
    let hir = lower_source_file(file_ast);

    if cli.dump_hir {
        println!("--- HIR ---");
        dump::hir::dump_hir(&hir);
    }
    if cli.dump_hir_raw {
        println!("--- HIR (raw) ---");
        dump::hir::dump_hir_raw(&hir);
    }

    if !hir.diagnostics.is_empty() {
        diagnostics::render(
            &source,
            hir.diagnostics.into_diags(),
            Some(&parse.green),
            Some(input_path),
        );
        return Err(anyhow::anyhow!("HIR lowering errors"));
    }

    println!("lowering HIR to MIR...");
    if cli.dump_mir {
        println!("--- MIR ---");
        dump::mir::dump_mir(&hir);
    }
    if cli.dump_mir_raw {
        println!("--- MIR (raw) ---");
        dump::mir::dump_mir_raw(&hir);
    }

    // A1: gen_mir re-lowers every function body to MIR internally
    // (mir::lower::lower_function). When --dump-mir is active, the
    // dump pass already called lower_function, making this redundant.
    // Cache lowered MirBodies or gate dump to reuse them.
    println!("generating c code...");
    let c_source = codegen::core::gen_mir(&hir);

    if cli.dump_c {
        println!("--- generated C ---");
        println!("{}", c_source);
    }

    backend::emit_and_compile(input_path, &c_source, cli.format, cli.release)
}
