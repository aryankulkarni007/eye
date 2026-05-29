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
        // No parse tree yet; lexer spans are tight byte ranges.
        diagnostics::render(&source, lexed.diags.into_diags(), None);
        std::process::exit(1);
    }

    if cli.dump_symbols {
        dump::symbols::dump_symbols(&lexed.interner);
    }

    let parse = parser::parse(&lexed.tokens, &source);

    if cli.dump_cst {
        println!("\n--- CST ---");
        println!("{:#?}", parse.green);
    }

    if !parse.diagnostics.is_empty() {
        diagnostics::render(&source, parse.diagnostics.into_diags(), Some(&parse.green));
        std::process::exit(1);
    }

    // Parse-stage oracle: lexer and parser were both clean above, which is all
    // tree-sitter can verify. Stop before HIR so semantic errors (which
    // tree-sitter never sees) can't masquerade as grammar drift in the parity
    // gate.
    if cli.check {
        return Ok(());
    }

    let file_ast = ast::SourceFile::cast(parse.green.clone())
        .ok_or_else(|| anyhow::anyhow!("Root node is not a valid SourceFile"))?;

    if cli.dump_ast {
        dump::ast::dump_ast(&file_ast);
    }

    println!("compiling...");
    println!("lowering AST to HIR...");
    let hir = lower_source_file(file_ast);

    if cli.dump_hir {
        dump::hir::dump_hir(&hir);
    }

    if !hir.diagnostics.is_empty() {
        diagnostics::render(&source, hir.diagnostics.into_diags(), Some(&parse.green));
        std::process::exit(1);
    }

    backend::emit_and_compile(input_path, &hir)
}
