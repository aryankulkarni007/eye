use lexer::{Interner, Symbol};

/// prints the interned string table - every identifier and string literal,
/// deduplicated, in intern order. proof the lexer populated the interner
/// handed off in `Lexed`; HIR name resolution will re-intern against it.
pub fn dump_symbols(interner: &Interner) {
    println!("\n--- SYMBOLS ({}) ---", interner.len());
    for i in 0..interner.len() {
        println!("  #{i} {:?}", interner.lookup(Symbol(i as u32)));
    }
}
