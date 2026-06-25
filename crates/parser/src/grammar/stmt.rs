//! statement grammar: the `block` driver and the statement forms (`let`/`mut`
//! binding, plus the legacy `stmt`/`expr_stmt` helpers).

use super::*;
use crate::Parser;
use syntax::{SyntaxKind, T};
use text_size::TextRange;

pub(crate) fn block(p: &mut Parser, ctx: TextRange) {
    let m = p.open();
    let open_brace = p.cursor_range();
    let had_open = p.eat(T!['{']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedBlockOpen);
    }
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(T![let]) || p.at(T![mut]) {
            let_stmt(p);
        } else if p.at(T![const]) {
            // block-scope `const TYPE Ident = expr;` - the same constdef node
            // as the top-level form; HIR gives it lexical scope.
            const_def(p);
        } else if at_expr_start(p) {
            // a block-like expression (`if`, `loop`, raw block) does not need
            // a trailing `;` when followed by another stmt; everything else
            // does. either way, if it sits in tail position before `}` the
            // exprstmt marker is abandoned so the bare expr falls out as the
            // block's tail.
            let m_stmt = p.open();
            // a leading `if`/`loop`/`match` makes this expression block-like,
            // so it can stand as a statement without a trailing `;`. a bare
            // `{` is not accepted by `lhs` as an expression start today;
            // reserve the arm for a future block-as-expression form.
            let is_block_like = matches!(p.nth0(), T![if] | T![loop] | T![match]);
            let expr_start = p.cursor_range();
            if is_block_like {
                // statement-position boundary (rust-style, no-footgun): a
                // block-like expression is a complete statement, so parse only
                // the `if`/`loop`/`match` via `lhs` (it returns before the infix
                // pratt loop). a following `*`/`-`/`&` then starts the next
                // statement instead of folding as an infix operator on the
                // block's value - `if c {a} else {b} * p` is two statements, not
                // a multiply. expression position (a let initializer, a call
                // argument) still goes through the full `expr` parser, where the
                // block-like form is an operand and the operator binds.
                lhs(p);
            } else {
                expr(p);
            }

            if p.eat(T![;]) {
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            } else if p.at(T!['}']) {
                m_stmt.abandon(p);
                break;
            } else if is_block_like {
                // `if { ... } counter = ...;` - the if is a statement here,
                // no `;` required between block-like and the next stmt.
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            } else if p.at_eof() {
                // at EOF without `}` -- the block is unclosed. don't emit
                // "expected ;" because adding `}` would make this expression a
                // valid tail expression. `ExpectedBlockClose` is the root
                // cause.
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            } else {
                p.error_at(expr_start, crate::SyntaxError::ExpectedSemiAfterExpr);
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            }
        } else {
            // item keywords are sync points so an unclosed block cannot
            // consume subsequent items as error nodes. after sync the
            // block bails when no `;` follows -- either `}` (normal exit)
            // or an item keyword (the block is unclosed; expectedblockclose
            // fires below).
            p.sync(
                &[
                    T![;],
                    T!['}'],
                    T![structure],
                    T![union],
                    T![enum],
                    T![extern],
                ],
                crate::SyntaxError::ExpectedStatement,
            );
            if !p.eat(T![;]) {
                break;
            }
        }
    }
    if !p.eat(T!['}']) {
        let range = if had_open {
            // point to the last consumed content token (or the opening brace
            // if nothing was parsed inside the block)
            TextRange::new(open_brace.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedBlockClose);
    }
    m.complete(p, SyntaxKind::Block);
}

#[allow(dead_code)]
pub(crate) fn stmt(p: &mut Parser) {
    if p.at(T![let]) || p.at(T![mut]) {
        let_stmt(p);
    } else if at_expr_start(p) {
        expr_stmt(p);
    } else {
        p.error_and_advance(crate::SyntaxError::ExpectedStatement);
    }
}

/// `let_stmt` accepts these shapes, distinguished by a fixed two-token
/// lookahead after the `let`/`mut` keyword:
///
/// - inferred: `let x = expr;` (ident then `=` -> no type)
/// - explicit: `mut Point p = expr;` (ident then ident)
/// - pointer: `mut Point* p = expr;` (ident then `*`)
/// - explicit ref: `mut &Point r = expr;` (leading `&`)
/// - explicit arr: `let [int32; 3] xs = expr;` (leading `[`)
///
/// nested refs are written with a space (`& &Point`); the `&&` spelling lexes
/// as a single logical-and token, so it cannot denote a ref-to-ref type.
pub(crate) fn let_stmt(p: &mut Parser) {
    let m = p.open();
    let stmt_start = p.cursor_range(); // 'let' or 'mut' - anchor for diagnostics
    p.advance(); // 'let' or 'mut'
    // struct destructure: `let Point { x, y } = p`. the target is a struct
    // pattern (`Ident '{'`), not a `type name` binding. exhaustive field binding;
    // no `..`/ignore yet.
    if p.at(SyntaxKind::Ident) && p.nth(1) == T!['{'] {
        struct_pat(p);
    } else {
        // a leading type is present when the tokens after `let`/`mut` read as
        // `type name` rather than `name =`. a leading `&` begins a ref type, `[`
        // an array type, and `(` a function type (a binding name never starts
        // with any of these). an `Ident` is a type if the next token is another
        // `Ident` (`Point p`) or a postfix `*` (`Point* p`).
        let has_type = matches!(p.nth0(), T![&] | T!['['] | T!['('])
            || matches!(
                (p.nth0(), p.nth(1)),
                (SyntaxKind::Ident, SyntaxKind::Ident) | (SyntaxKind::Ident, T![*])
            );
        if has_type {
            type_ref(p);
        }
        p.expect_after(
            SyntaxKind::Ident,
            stmt_start,
            crate::SyntaxError::ExpectedBindingName,
        );
    }
    let had_eq = p.eat(T![=]);
    expr(p);
    let had_semi = p.eat(T![;]);
    if !had_eq {
        let span = TextRange::new(stmt_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedEqInBinding);
    }
    if !had_semi {
        let span = TextRange::new(stmt_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterStatement);
    }
    m.complete(p, SyntaxKind::LetStmt);
}

#[allow(dead_code)]
pub(crate) fn expr_stmt(p: &mut Parser) {
    let m = p.open();
    expr(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterExpr);
    m.complete(p, SyntaxKind::ExprStmt);
}
