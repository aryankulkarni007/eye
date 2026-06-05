#![no_main]

use libfuzzer_sys::fuzz_target;

use ast::AstNode;
use hir::core::lower_source_file;
use lexer::{Lexer, SourceText};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let source = String::from_utf8_lossy(data).into_owned();
    let text = SourceText::new(source);

    // Phase 1: lex
    let lexed = Lexer::new(&text).tokenize();
    if !lexed.diags.is_empty() {
        return; // skip – the lexer already found errors
    }

    // Phase 2: parse
    let parsed = parser::parse(&lexed.tokens, &text);
    if !parsed.diagnostics.is_empty() {
        return; // skip – the parser already found errors
    }

    // Phase 3: AST wrapper
    let Ok(file_ast) = ast::SourceFile::cast(parsed.green.clone())
        .ok_or(())
    else {
        return; // skip – not a valid SourceFile root
    };

    // Phase 4: HIR lowering + diagnostics (must not panic)
    let hir = lower_source_file(file_ast);
    if !hir.diagnostics.is_empty() {
        return; // skip – semantic errors
    }

    // Phase 5: MIR lowering + C codegen (must not panic)
    let _c_source = codegen::core::gen_mir(&hir);
});
