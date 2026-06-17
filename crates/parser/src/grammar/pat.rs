//! pattern grammar: `pat` (wildcard / literal / qualified path / bare ident),
//! the struct pattern (`let` destructure), and the `at_pat_start` predicate.

use crate::Parser;
use syntax::{SyntaxKind, T};
use text_size::TextRange;

/// true if `p` is at a token that can begin a pattern.
pub(crate) fn at_pat_start(p: &Parser) -> bool {
    matches!(
        p.nth0(),
        SyntaxKind::Ident
            | T![_]
            | SyntaxKind::Int
            | SyntaxKind::Char
            | SyntaxKind::True
            | SyntaxKind::False
    )
}

/// patterns:
/// - `_` -> `WildcardPat`
/// - int / char / bool literal -> `LiteralPat`
/// - `Enum '.' Variant` -> `PathPat` (qualified)
/// - `Ident` -> `BareIdentPat`
///
/// float and string literals are intentionally not patterns: float equality is a
/// footgun and a string is an array, not a kernel discriminant domain.
pub(crate) fn pat(p: &mut Parser) {
    if p.at(T![_]) {
        let m = p.open();
        p.advance(); // '_'
        m.complete(p, SyntaxKind::WildcardPat);
        return;
    }
    if matches!(
        p.nth0(),
        SyntaxKind::Int | SyntaxKind::Char | SyntaxKind::True | SyntaxKind::False
    ) {
        let m = p.open();
        // wrap the token in a `Literal` node so HIR reuses `lower_literal`.
        let lit = p.open();
        p.advance(); // the literal token
        lit.complete(p, SyntaxKind::Literal);
        m.complete(p, SyntaxKind::LiteralPat);
        return;
    }
    if p.at(SyntaxKind::Ident) {
        // `Ident '{'` is a struct pattern. the grammar permits these only in a
        // `let` destructure, not a match arm (s3, deferred). parse the shape so
        // the error spans the whole pattern and recovery lands on `->`; the
        // resulting `StructPat` is not an `ast::Pat`, so HIR reads the arm
        // pattern as missing rather than miscompiling.
        if p.nth(1) == T!['{'] {
            let start = p.cursor_range();
            struct_pat(p);
            let span = TextRange::new(start.start(), p.last_consumed_range().end());
            p.error_at(span, crate::GrammarError::StructPatInMatchArm);
            return;
        }
        if p.nth(1) == T![.] {
            let m = p.open();
            // qualifier name ref
            let nm = p.open();
            p.advance(); // qualifier ident
            nm.complete(p, SyntaxKind::NameRef);
            let dot_range = p.cursor_range();
            p.advance(); // '.'
            // variant name ref
            let nm = p.open();
            p.expect_after(
                SyntaxKind::Ident,
                dot_range,
                crate::SyntaxError::ExpectedVariantNameAfterDot,
            );
            nm.complete(p, SyntaxKind::NameRef);
            m.complete(p, SyntaxKind::PathPat);
        } else {
            let m = p.open();
            let nm = p.open();
            p.advance(); // ident
            nm.complete(p, SyntaxKind::NameRef);
            m.complete(p, SyntaxKind::BareIdentPat);
        }
        return;
    }
    p.error_and_advance(crate::SyntaxError::ExpectedPattern);
}

/// `Type { field, field: binding, ... }` - an irrefutable struct pattern. the
/// caller detects the opening `Ident '{'`. used by `let` destructure today; match
/// arms gain it (with guards) later. field binding is exhaustive - no `..`/ignore.
pub(crate) fn struct_pat(p: &mut Parser) {
    let m = p.open();
    let ctx = p.cursor_range(); // struct type - context for missing '{'
    let nm = p.open();
    p.advance(); // struct type Ident
    nm.complete(p, SyntaxKind::NameRef);
    struct_pat_field_list(p, ctx);
    m.complete(p, SyntaxKind::StructPat);
}

pub(crate) fn struct_pat_field_list(p: &mut Parser, ctx: TextRange) {
    let m = p.open();
    let open_brace = p.cursor_range();
    let had_open = p.eat(T!['{']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedStructLitOpen);
    }
    while !p.at(T!['}']) && !p.at_eof() {
        if !p.at(SyntaxKind::Ident) {
            p.sync(&[T![,], T!['}']], crate::SyntaxError::ExpectedField);
            if p.eat(T![,]) {
                continue;
            }
            break;
        }
        struct_pat_field(p);
        if !p.eat(T![,]) && !p.at(T!['}']) && !p.at_eof() {
            p.error(crate::SyntaxError::ExpectedCommaAfterField);
        }
    }
    if !p.eat(T!['}']) {
        let range = if had_open {
            TextRange::new(open_brace.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedStructLitClose);
    }
    m.complete(p, SyntaxKind::StructPatFieldList);
}

/// `name` (shorthand: binds the field) or `name ':' binding` (binds it to a new
/// name).
pub(crate) fn struct_pat_field(p: &mut Parser) {
    let m = p.open();
    p.advance(); // field name Ident
    if p.eat(T![:]) {
        let colon_range = p.last_consumed_range();
        let nm = p.open();
        p.expect_after(
            SyntaxKind::Ident,
            colon_range,
            crate::SyntaxError::ExpectedBindingName,
        );
        nm.complete(p, SyntaxKind::NameRef);
    }
    m.complete(p, SyntaxKind::StructPatField);
}
