//! expression grammar: the pratt loop (`expr_bp` / `lhs` / binding powers), the
//! block-like expression forms (`if`/`loop`/`break`/`continue`/`return`/`match`
//! + arms), atoms, array / paren / call-arg / struct literals, and the
//! `at_expr_start` predicate.

use super::*;
use crate::{CompletedMarker, Parser};
use syntax::{SyntaxKind, T};
use text_size::TextRange;

/// true if `p` is positioned at a token that can begin an expression - an
/// atom, a prefix operator, or a block-like expression keyword.
pub(crate) fn at_expr_start(p: &Parser) -> bool {
    matches!(
        p.nth0(),
        SyntaxKind::Int
            | SyntaxKind::Float
            | SyntaxKind::String
            | SyntaxKind::True
            | SyntaxKind::False
            | SyntaxKind::Char
            | SyntaxKind::Ident
            | T![-]
            | T![~]
            | T![!]
            | T![&]
            | T![*]
            | T!['(']
            | T!['[']
            | T![if]
            | T![loop]
            | T![break]
            | T![continue]
            | T![return]
            | T![match]
    )
}

/// an expression. precedence is resolved by the pratt loop in [`expr_bp`];
/// [`lhs`] parses a prefix-unary form or an atom with its postfix forms.
pub(crate) fn expr(p: &mut Parser) {
    expr_bp(p, 0);
}

/// left/right binding power of an infix operator, or `None` if `kind` is not
/// one. most operators are left-associative (`l_bp < r_bp`). assignment is
/// right-associative (`l_bp > r_bp`) and has the lowest precedence.
pub(crate) fn infix_binding_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        // assignment (plain `=` and every compound form) is right-associative
        // and lowest.
        T![=]
        | T![+=]
        | T![-=]
        | T![*=]
        | T![/=]
        | T![%=]
        | T![&=]
        | T![|=]
        | T![^=]
        | T![<<=]
        | T![>>=] => (2, 1),
        T![||] => (3, 4),
        T![&&] => (5, 6),
        // comparison is its own tier; f1 makes it non-associative (see
        // `expr_bp`) so `a < b < c` is a hard error, not c's silent chain.
        T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=] => (7, 8),
        // no-footgun precedence (rust-style, not c-style): every bitwise op
        // binds TIGHTER than comparison, so `a & b == c` is `(a & b) == c`,
        // never c's `a & (b == c)`. each bitwise op gets its own tier.
        T![|] => (9, 10),
        T![^] => (11, 12),
        T![&] => (13, 14),
        T![<<] | T![>>] => (15, 16),
        T![+] | T![-] => (17, 18),
        T![*] | T![/] | T![%] => (19, 20),
        _ => return None,
    })
}

/// true if `kind` is a comparison/equality operator. these form one tier and
/// are non-associative (f1): chaining two of them at the same level is an
/// error, so `a < b < c` must be parenthesized.
pub(crate) fn is_comparison(kind: SyntaxKind) -> bool {
    matches!(kind, T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=])
}

/// right binding power of any prefix unary - above every infix operator, so
/// `-a * b` parses as `(-a) * b`.
const PREFIX_BP: u8 = 21;

/// pratt loop: parse a left-hand side, then fold in infix operators while
/// their left binding power is at least `min_bp`. each operator wraps the
/// already-parsed LHS via [`CompletedMarker::precede`], so the event buffer
/// stays append-only.
pub(crate) fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = lhs(p)?;
    // tracks whether the operator folded last at *this* level was a comparison.
    // two comparisons in a row at the same level is a non-associativity error
    // (f1). a same-tier comparison never appears in an operator's right operand
    // (r_bp > l_bp breaks it out), so a chain only ever shows up here.
    let mut prev_was_cmp = false;
    while let Some((l_bp, r_bp)) = infix_binding_power(p.nth0()) {
        if l_bp < min_bp {
            break;
        }
        let op = p.nth0();
        let is_cmp = is_comparison(op);
        if is_cmp && prev_was_cmp {
            p.error(crate::GrammarError::ComparisonChain);
        }
        let kind = if matches!(
            op,
            T![=]
                | T![+=]
                | T![-=]
                | T![*=]
                | T![/=]
                | T![%=]
                | T![&=]
                | T![|=]
                | T![^=]
                | T![<<=]
                | T![>>=]
        ) {
            SyntaxKind::AssignExpr
        } else {
            SyntaxKind::BinExpr
        };
        let m = lhs.precede(p);
        p.advance(); // the operator token
        expr_bp(p, r_bp);
        lhs = m.complete(p, kind);
        prev_was_cmp = is_cmp;
    }
    Some(lhs)
}

/// a prefix-unary form, or an atom followed by any run of postfix forms. each
/// postfix form uses [`CompletedMarker::precede`] to wrap its operand.
pub(crate) fn lhs(p: &mut Parser) -> Option<CompletedMarker> {
    // prefix-unary: `-` negate, `~` bitwise-complement, `!` logical-not. the
    // operand binds at prefix_bp (above every infix op) so `-a * b` is `(-a) * b`.
    if p.at(T![-]) || p.at(T![~]) || p.at(T![!]) {
        let m = p.open();
        p.advance(); // the prefix operator
        expr_bp(p, PREFIX_BP);
        return Some(m.complete(p, SyntaxKind::PrefixExpr));
    }
    if p.at(T![&]) {
        let m = p.open();
        p.advance(); // '&'
        expr_bp(p, PREFIX_BP);
        return Some(m.complete(p, SyntaxKind::RefExpr));
    }
    if p.at(T![*]) {
        let m = p.open();
        p.advance(); // '*'
        expr_bp(p, PREFIX_BP);
        return Some(m.complete(p, SyntaxKind::DerefExpr));
    }
    if p.at(T![if]) {
        return Some(if_expr(p));
    }
    if p.at(T![loop]) {
        return Some(loop_expr(p));
    }
    if p.at(T![break]) {
        return Some(break_expr(p));
    }
    if p.at(T![continue]) {
        return Some(continue_expr(p));
    }
    if p.at(T![return]) {
        return Some(return_expr(p));
    }
    if p.at(T![match]) {
        return Some(match_expr(p));
    }
    if p.at(T!['[']) {
        return Some(array_lit(p));
    }

    // the base of the postfix chain: a parenthesized group or an atom. a
    // leading `(` is unambiguously a group here (a postfix `(` - a call - only
    // appears after a base is already parsed, handled in the loop below).
    let mut lhs = if p.at(T!['(']) {
        paren_expr(p)
    } else {
        atom(p)?
    };
    loop {
        if p.at(T!['(']) {
            let call = lhs.precede(p);
            arg_list(p);
            lhs = call.complete(p, SyntaxKind::CallExpr);
        } else if p.at(T!['[']) {
            // postfix index `base[i]` - binds as tightly as call/field.
            let index = lhs.precede(p);
            let open_bracket = p.cursor_range();
            p.advance(); // '['
            expr(p);
            if !p.eat(T![']']) {
                let range = TextRange::new(open_bracket.start(), p.last_consumed_range().end());
                p.error_at(range, crate::SyntaxError::ExpectedIndexClose);
            }
            lhs = index.complete(p, SyntaxKind::IndexExpr);
        } else if p.at(T!['{']) && !p.no_struct_lit() {
            let lit = lhs.precede(p);
            struct_body(p);
            lhs = lit.complete(p, SyntaxKind::StructLit);
        } else if p.at(T![.]) {
            let field_expr = lhs.precede(p);
            let dot_range = p.cursor_range();
            p.advance();
            let name_m = p.open();
            p.expect_after(
                SyntaxKind::Ident,
                dot_range,
                crate::SyntaxError::ExpectedFieldIdentAfterDot,
            );
            name_m.complete(p, SyntaxKind::NameRef);
            lhs = field_expr.complete(p, SyntaxKind::FieldExpr);
        } else if p.at(T![as]) {
            // `expr as Type` - a postfix cast. binds as tightly as call/field,
            // so `a + b as T` parses as `a + (b as T)`.
            let cast = lhs.precede(p);
            p.advance(); // 'as'
            type_ref(p);
            lhs = cast.complete(p, SyntaxKind::CastExpr);
        } else {
            break;
        }
    }
    Some(lhs)
}

pub(crate) fn if_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let if_start = p.cursor_range(); // 'if' keyword - anchor for condition span
    p.advance(); // 'if'
    let prev = p.set_no_struct_lit(true);
    // reject `if x = y { ... }`: an assignment in a condition is the classic
    // `=`/`==` footgun. anchored at the condition's first token, not the cursor.
    let cond_start = p.cursor_range();
    let cond = expr_bp(p, 0);
    p.set_no_struct_lit(prev);
    if matches!(cond, Some(cm) if cm.kind() == SyntaxKind::AssignExpr) {
        p.error_at(cond_start, crate::GrammarError::AssignInIfCondition);
    }
    // full condition range: from 'if' through end of condition expr
    let cond_range = TextRange::new(if_start.start(), p.cursor_range().start());
    block(p, cond_range);
    if p.at(T![else]) {
        let else_range = p.cursor_range(); // 'else' keyword - anchor for missing else body '{'
        p.advance(); // 'else'
        if p.at(T![if]) {
            // `else if` desugars to `else { if ... }`: wrap the chained if in a
            // synthetic block so the else-branch stays a block end-to-end
            // (AST/HIR/codegen are unchanged). codegen flattens the trivial
            // `else { if }` back to `else if` so the c output does not nest.
            let blk = p.open();
            if_expr(p);
            blk.complete(p, SyntaxKind::Block);
        } else {
            block(p, else_range);
        }
    }
    m.complete(p, SyntaxKind::IfExpr)
}

pub(crate) fn loop_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let ctx = p.cursor_range(); // 'loop' keyword - context for missing body '{'
    p.advance(); // 'loop'
    block(p, ctx);
    m.complete(p, SyntaxKind::LoopExpr)
}

pub(crate) fn break_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'break'
    // a `break` may carry a value (`break expr`); a `;` or `}` ends it bare.
    if at_expr_start(p) {
        expr(p);
    }
    m.complete(p, SyntaxKind::BreakExpr)
}

pub(crate) fn continue_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'continue'
    m.complete(p, SyntaxKind::ContinueExpr)
}

pub(crate) fn return_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'return'
    // a `return` may carry a value (`return expr`); a `;` or `}` ends it bare.
    if at_expr_start(p) {
        expr(p);
    }
    m.complete(p, SyntaxKind::ReturnExpr)
}

/// `match scrut { arm, arm, ... }`. mirrors `if_expr` for the scrutinee: the
/// `no_struct_lit` gate is set so `match sh { Circle -> 1 }` does not parse
/// `sh { Circle -> 1 }` as a struct literal. the gate is cleared inside the
/// arm block - arm body expressions are unrestricted.
pub(crate) fn match_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let match_start = p.cursor_range(); // 'match' keyword - anchor for scrutinee span
    p.advance(); // 'match'
    let prev = p.set_no_struct_lit(true);
    expr(p);
    p.set_no_struct_lit(prev);
    let scrutinee_range = TextRange::new(match_start.start(), p.cursor_range().start());
    match_arm_list(p, scrutinee_range);
    m.complete(p, SyntaxKind::MatchExpr)
}

pub(crate) fn match_arm_list(p: &mut Parser, ctx: TextRange) {
    let m = p.open();
    let open_brace = p.cursor_range();
    let had_open = p.eat(T!['{']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedMatchArmsOpen);
    }
    // arm body expressions can contain struct literals freely
    let prev = p.set_no_struct_lit(false);
    while !p.at(T!['}']) && !p.at_eof() {
        if !at_pat_start(p) {
            p.sync(&[T![,], T!['}']], crate::SyntaxError::ExpectedMatchArm);
            if p.eat(T![,]) {
                continue;
            }
            break;
        }
        let had_comma = match_arm(p);
        // `,` is the arm separator. it is mandatory between arms; only the
        // final arm before `}` may omit it.
        if !had_comma && !p.at(T!['}']) && !p.at_eof() {
            p.error(crate::SyntaxError::ExpectedCommaBetweenMatchArms);
        }
    }
    p.set_no_struct_lit(prev);
    if !p.eat(T!['}']) {
        let range = if had_open {
            TextRange::new(open_brace.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedMatchArmsClose);
    }
    m.complete(p, SyntaxKind::MatchArmList);
}

/// parse one arm. returns `true` if a trailing `,` was consumed - the arm
/// list uses that to enforce the "comma required between arms" rule.
///
/// an optional `if guard_expr` between the pattern and
/// the `->` arrow makes the arm conditional: the body runs only when both the
/// pattern matches and the guard evaluates to true.
pub(crate) fn match_arm(p: &mut Parser) -> bool {
    let m = p.open();
    let arm_start = p.cursor_range(); // pattern start - anchor for diagnostics
    pat(p);
    // match arm guard: `pat if expr -> body`
    if p.at(T![if]) {
        let gm = p.open();
        p.advance(); // 'if'
        expr(p);
        gm.complete(p, SyntaxKind::MatchGuard);
    }
    let had_arrow = p.eat(T![->]);
    expr(p);
    let body_end = p.last_consumed_range();
    let had_comma = p.eat(T![,]);
    if !had_arrow {
        let span = TextRange::new(arm_start.start(), body_end.end());
        p.error_at(span, crate::SyntaxError::ExpectedArrowAfterPattern);
    }
    m.complete(p, SyntaxKind::MatchArm);
    had_comma
}

pub(crate) fn atom(p: &mut Parser) -> Option<CompletedMarker> {
    let m = p.open();
    let kind = match p.nth0() {
        SyntaxKind::Int
        | SyntaxKind::Float
        | SyntaxKind::String
        | SyntaxKind::True
        | SyntaxKind::False
        | SyntaxKind::Char => {
            p.advance();
            SyntaxKind::Literal
        }
        SyntaxKind::Ident => {
            p.advance();
            SyntaxKind::NameRef
        }
        _ => {
            m.abandon(p);
            p.error(crate::SyntaxError::ExpectedExpression);
            return None;
        }
    };
    Some(m.complete(p, kind))
}

/// `[a, b, c]` array literal. its own struct-lit context: a suppressed flag
/// from an enclosing if/loop condition does not apply inside the elements.
/// trailing comma is allowed.
pub(crate) fn array_lit(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let open_bracket = p.cursor_range();
    p.advance(); // '['
    let prev = p.set_no_struct_lit(false);
    let mut first = true;
    while !p.at(T![']']) && !p.at_eof() {
        if at_expr_start(p) {
            expr(p);
            // a `;` after the first element selects the repeat form
            // `[value; count]`, distinct from the list form `[a, b, c]`.
            if first && p.at(T![;]) {
                p.advance(); // ';'
                if at_expr_start(p) {
                    expr(p); // count
                } else {
                    p.error(crate::SyntaxError::ExpectedExpression);
                }
                p.set_no_struct_lit(prev);
                if !p.eat(T![']']) {
                    let range = TextRange::new(open_bracket.start(), p.last_consumed_range().end());
                    p.error_at(range, crate::SyntaxError::ExpectedArrayLitClose);
                }
                return m.complete(p, SyntaxKind::ArrayRepeat);
            }
            first = false;
            if !p.eat(T![,]) {
                break;
            }
        } else {
            p.sync(&[T![,], T![']']], crate::SyntaxError::ExpectedArrayElement);
            if p.eat(T![,]) {
                continue;
            }
            break;
        }
    }
    p.set_no_struct_lit(prev);
    if !p.eat(T![']']) {
        let range = TextRange::new(open_bracket.start(), p.last_consumed_range().end());
        p.error_at(range, crate::SyntaxError::ExpectedArrayLitClose);
    }
    m.complete(p, SyntaxKind::ArrayLit)
}

/// `( expr )` - a parenthesized group. purely a precedence override; HIR
/// lowers it to its inner expression, so it leaves no trace past the AST. a
/// group is its own struct-lit context, like an arg list or array element.
pub(crate) fn paren_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let open_paren = p.cursor_range();
    p.advance(); // '('
    let prev = p.set_no_struct_lit(false);
    expr(p);
    p.set_no_struct_lit(prev);
    if !p.eat(T![')']) {
        let range = TextRange::new(open_paren.start(), p.last_consumed_range().end());
        p.error_at(range, crate::SyntaxError::ExpectedParenExprClose);
    }
    m.complete(p, SyntaxKind::ParenExpr)
}

pub(crate) fn arg_list(p: &mut Parser) {
    let m = p.open();
    let open_paren = p.cursor_range();
    // span the full call expression when `(` is missing, not just the next
    // token.
    p.expect_after(T!['('], open_paren, crate::SyntaxError::ExpectedOpenParen);
    // an arg list is its own struct-lit context: a suppressed flag from an
    // enclosing if/loop condition does not apply inside the arguments.
    let prev = p.set_no_struct_lit(false);
    while !p.at(T![')']) && !p.at_eof() {
        if at_expr_start(p) {
            expr(p);
        } else {
            p.sync(&[T![,], T![')']], crate::SyntaxError::ExpectedExpression);
        }
        if !p.eat(T![,]) {
            break;
        }
    }
    p.set_no_struct_lit(prev);
    if !p.eat(T![')']) {
        let range = TextRange::new(open_paren.start(), p.last_consumed_range().end());
        p.error_at(range, crate::SyntaxError::ExpectedArgListClose);
    }
    m.complete(p, SyntaxKind::ArgList);
}

pub(crate) fn struct_body(p: &mut Parser) {
    let m = p.open();
    let open_brace = p.cursor_range();
    p.expect(T!['{'], crate::SyntaxError::ExpectedStructLitOpen);
    // a struct body's fields are independent of any outer no-struct-lit gate
    let prev = p.set_no_struct_lit(false);
    while !p.at(T!['}']) && !p.at_eof() {
        if at_expr_start(p) {
            struct_lit_field(p);
            if !p.eat(T![,]) {
                break;
            }
        } else {
            p.sync(&[T![,], T!['}']], crate::SyntaxError::ExpectedFieldInit);
            if p.eat(T![,]) {
                continue;
            }
            break;
        }
    }
    p.set_no_struct_lit(prev);
    if !p.eat(T!['}']) {
        let range = TextRange::new(open_brace.start(), p.last_consumed_range().end());
        p.error_at(range, crate::SyntaxError::ExpectedStructLitClose);
    }
    m.complete(p, SyntaxKind::StructLitFieldList);
}

/// a field initializer in a struct literal. three forms:
///
/// - `Ident` followed by `,` or `}` - shorthand: `Point { x, y }`
/// - `Ident ':' expr` - explicit: `Point { x: 0 }`
/// - any other expression - positional: `Point { 10, 20 }`
///
/// one node kind serves all three; the presence of a direct ident token vs.
/// an expr child distinguishes them in the typed AST.
pub(crate) fn struct_lit_field(p: &mut Parser) {
    let m = p.open();
    let named = p.at(SyntaxKind::Ident) && matches!(p.nth(1), T![,] | T!['}'] | T![:]);
    if named {
        p.advance(); // field name
        if p.eat(T![:]) {
            expr(p);
        }
    } else {
        // positional form: a full expression is the field's value
        expr(p);
    }
    m.complete(p, SyntaxKind::StructLitField);
}
