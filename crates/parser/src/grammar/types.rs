//! type grammar: `type_ref` parses a type - a fixed array `[T; N]`, a reference
//! `&T`, a unit / function type `()` / `(T) -> R`, or a named type with postfix
//! `*` pointers.

use super::*;
use crate::Parser;
use syntax::{SyntaxKind, T};
use text_size::TextRange;

pub(crate) fn type_ref(p: &mut Parser) {
    // parse the base type (either &ref, [t; n] array, or ident)
    let mut m = if p.at(T!['[']) {
        // `[T; N]` fixed-size array. n is an expression in the grammar but
        // restricted to an integer literal in lowering (no const-expr yet).
        let m = p.open();
        let open_bracket = p.cursor_range();
        p.advance(); // '['
        type_ref(p); // element type
        p.expect(T![;], crate::SyntaxError::ExpectedSemiInArrayType);
        expr(p); // length
        if !p.eat(T![']']) {
            let range = TextRange::new(open_bracket.start(), p.last_consumed_range().end());
            p.error_at(range, crate::SyntaxError::ExpectedArrayTypeClose);
        }
        m.complete(p, SyntaxKind::ArrayType)
    } else if p.at(T![&]) {
        let m = p.open();
        p.advance(); // '&'
        type_ref(p);
        m.complete(p, SyntaxKind::RefType)
    } else if p.at(T!['(']) {
        // a `(` in type position is either the unit type `()` or a function type
        // `(T, T) -> R`: eye has no tuple or parenthesized-group types. an empty
        // `()` with no return arrow is unit; `() -> R` is a paramless function
        // pointer. the return arrow is otherwise optional (omitted = returns
        // nothing), mirroring a function declaration.
        let m = p.open();
        let open_paren = p.cursor_range();
        p.advance(); // '('
        let mut param_count = 0u32;
        while !p.at(T![')']) && !p.at_eof() {
            let param_m = p.open();
            type_ref(p);
            p.eat(T![,]); // optional separator; trailing comma allowed
            param_m.complete(p, SyntaxKind::FnTypeParam);
            param_count += 1;
        }
        if !p.eat(T![')']) {
            let range = TextRange::new(open_paren.start(), p.last_consumed_range().end());
            p.error_at(range, crate::SyntaxError::ExpectedCloseParen);
        }
        if p.eat(T![->]) {
            type_ref(p); // return type
            m.complete(p, SyntaxKind::FnType)
        } else if param_count == 0 {
            // `()` with no arrow is the unit type, not a paramless fn pointer.
            m.complete(p, SyntaxKind::UnitType)
        } else {
            // `(T)` / `(T, U)` with no `->`: a function pointer returning nothing.
            m.complete(p, SyntaxKind::FnType)
        }
    } else if p.at(SyntaxKind::Ident) {
        let m = p.open();
        p.advance(); // ident
        m.complete(p, SyntaxKind::IdentType)
    } else {
        p.error_and_advance(crate::SyntaxError::ExpectedType);
        return;
    };

    // parse any postfix pointer '*' operators
    while p.at(T![*]) {
        let ptr_m = m.precede(p);
        p.advance(); // '*'
        m = ptr_m.complete(p, SyntaxKind::PtrType);
    }
}
