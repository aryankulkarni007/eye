#![no_main]

use libfuzzer_sys::fuzz_target;

use lexer::{Lexer, SourceText};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // Convert to valid UTF-8 (lossy) so we never hit the
    // `from_utf8_unchecked` UB in SourceText::as_str().
    let source = String::from_utf8_lossy(data).into_owned();
    let text = SourceText::new(source);

    // Lexer must never panic, regardless of input.
    let _lexed = Lexer::new(&text).tokenize();
});
