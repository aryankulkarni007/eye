// create a lib entry point so that
// the flamegraph script can work

// global allocator – move it here so both the binary and the library benefit.
// the binary will automatically use it because it's part of the same crate.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::Path;

use ast::AstNode;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};

/// compile a single `.eye` source file down to c code (no backend).
///
/// this is a **library‑only** entry point – no cli, no rendering, no dumps.
/// it returns `Ok(())` if compilation succeeds, otherwise an `anyhow` error.
pub fn compile_file(input_path: &Path) -> anyhow::Result<()> {
    let source = SourceText::new(std::fs::read_to_string(input_path)?);

    // lex
    let lexed = Lexer::new(&source).tokenize();
    if !lexed.diags.is_empty() {
        return Err(anyhow::anyhow!(
            "Lexical errors in {}",
            input_path.display()
        ));
    }

    // parse
    let parse = parser::parse(&lexed.tokens, &source);
    if !parse.diagnostics.is_empty() {
        return Err(anyhow::anyhow!("Parse errors in {}", input_path.display()));
    }

    // ast wrapper
    let file_ast = ast::SourceFile::cast(parse.green.clone())
        .ok_or_else(|| anyhow::anyhow!("Root node is not a valid SourceFile"))?;

    // hir lowering + typeck
    let mut hir = lower_source_file(file_ast, &lexed.interner);
    if !hir.diagnostics.is_empty() {
        return Err(anyhow::anyhow!("HIR errors in {}", input_path.display()));
    }
    let typeck = typeck::check_file(&mut hir);
    if typeck.values().any(|r| !r.diagnostics.is_empty()) {
        return Err(anyhow::anyhow!("type errors in {}", input_path.display()));
    }

    // mir lowering + c code generation
    let mirs = mir::lower_all(&hir, &typeck);
    let seed = typeck::expr_type_seed(&typeck);
    let _c_source = codegen::core::gen_mir(&hir, &mirs, &seed);
    Ok(())
}
