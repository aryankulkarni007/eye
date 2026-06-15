#![no_main]

use libfuzzer_sys::fuzz_target;

use lexer::{Lexer, SourceText};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let source = String::from_utf8_lossy(data).into_owned();
    let text = SourceText::new(source.clone());

    // phase 1: lex – must not panic.
    let lexed = Lexer::new(&text).tokenize();

    // phase 2: parse – must not panic even on garbage tokens.
    // the parser is designed to recover from any input; we test
    // that it never unwraps or indexes out of bounds.
    let _parse = parser::parse(&lexed.tokens, &text);
});
