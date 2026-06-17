use std::path::Path;

use clap::Parser;
use database::{Database, SourceFileInput};
use lexer::SourceText;

mod backend;
mod cli;
mod diagnostics;
mod dump;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    let input_path: &Path = cli.input.as_path();

    if input_path.extension().and_then(|e| e.to_str()) != Some("eye") {
        eprintln!(
            "error: expected a `.eye` source file, got `{}`",
            input_path.display()
        );
        return Err(anyhow::anyhow!("expected a `.eye` source file"));
    }

    let text = std::fs::read_to_string(input_path)?;
    let db = Database::default();

    // register the source file input; every query below is keyed on it and
    // memoized, so a phase shared between diagnostics, dumps, and c emission
    // (e.g. MIR lowering for `--dump-mir` and `c_code`) runs once.
    let file = SourceFileInput::new(&db, input_path.to_string_lossy().to_string(), text.clone());

    let source = SourceText::new(text);

    // --- lex ---
    let lexed = database::lex(&db, file);
    if !lexed.diags.is_empty() {
        diagnostics::render(
            &source,
            lexed.diags.clone().into_diags(),
            None,
            Some(input_path),
        );
        return Err(anyhow::anyhow!("lexer errors"));
    }

    if cli.dump_symbols {
        dump::symbols::dump_symbols(&lexed.interner);
    }

    // --- parse ---
    let parse = database::parse(&db, file);
    let root = parse.syntax();

    if cli.dump_cst {
        println!("--- CST ---");
        println!("{:#?}", root);
    }

    if !parse.diagnostics.is_empty() {
        diagnostics::render(
            &source,
            parse.diagnostics.clone().into_diags(),
            Some(&root),
            Some(input_path),
        );
        return Err(anyhow::anyhow!("parser errors"));
    }

    if cli.parse_only {
        return Ok(());
    }

    if cli.dump_ast {
        println!("--- AST ---");
        dump::ast::dump_ast(&parse.ast());
    }

    // --- HIR ---
    let checked = database::lowered_file(&db, file);
    let hir = &checked.hir;

    if cli.dump_hir {
        println!("--- HIR ---");
        dump::hir::dump_hir(hir);
    }
    if cli.dump_hir_raw {
        println!("--- HIR (raw) ---");
        dump::hir::dump_hir_raw(hir);
    }

    let mut front_diags = hir.diagnostics.clone();
    {
        let mut fn_ids: Vec<_> = checked.typeck.keys().copied().collect();
        fn_ids.sort_by_key(|id| id.raw_idx().into_u32());
        for fn_id in fn_ids {
            front_diags.extend(checked.typeck[&fn_id].diagnostics.clone());
        }
    }
    if !front_diags.is_empty() {
        diagnostics::render(
            &source,
            front_diags.into_diags(),
            Some(&root),
            Some(input_path),
        );
        return Err(anyhow::anyhow!("HIR lowering errors"));
    }

    // `--check` stops after lowering: every diagnostic phase has run (lexer
    // and parser errors exit above), nothing is generated or compiled.
    if cli.check {
        return Ok(());
    }

    // --- MIR (memoized: the dumps and `c_code` consume the same map) ---
    let mirs = database::mir_map(&db, file);
    if cli.dump_mir {
        println!("--- MIR ---");
        dump::mir::dump_mir(hir, &mirs);
    }
    if cli.dump_mir_raw {
        println!("--- MIR (raw) ---");
        dump::mir::dump_mir_raw(hir, &mirs);
    }

    // --- c ---
    let seed = typeck::expr_type_seed(&checked.typeck);
    let c_source = codegen::core::gen_mir(hir, &mirs, &seed);
    if cli.dump_c {
        println!("--- generated C ---");
        println!("{}", c_source);
    }

    backend::emit_and_compile(input_path, &c_source, cli.format, cli.release)
}
