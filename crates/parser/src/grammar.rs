//! The eye grammar — v0.1 subset covering exactly `main.eye`.
//!
//! ```text
//! source_file := item*
//! item        := struct_def | fn_def
//! struct_def  := 'structure' Ident field_list ';'
//! field_list  := '{' (field ',')* '}'
//! field       := type_ref Ident
//! type_ref    := Ident
//! fn_def      := Ident param_list block
//! param_list  := '(' ')'
//! block       := '{' stmt* '}'
//! stmt        := let_stmt | expr_stmt
//! let_stmt    := ('const' | 'var') type_ref? Ident '=' expr ';'
//! expr_stmt   := expr ';'
//! expr        := infix
//! infix       := prefix (binop prefix)*
//! prefix      := '-' prefix | postfix
//! postfix     := atom (arg_list | struct_body)*
//! atom        := Int | Float | String | True | False | Char | NameRef
//! binop       := '+' | '-' | '*' | '/' | '&&' | '||'
//!              | '==' | '!=' | '<' | '>' | '<=' | '>='
//! struct_body := '{' (struct_field (',' struct_field)*)? '}'
//! struct_field := Ident (':' expr)?
//! ```
//!
//! Every function opens a [`Marker`], parses, and completes it with a node
//! kind. Parsing is resilient: an unexpected token is wrapped in an
//! `ErrorNode` and skipped — the parser never bails, so a tree always comes
//! out.
//!
//! [`Marker`]: crate::Marker

use crate::{CompletedMarker, Parser};
use syntax::{SyntaxKind, T};

/// True if `p` is positioned at a token that can begin an expression — an
/// atom, or the prefix `-`.
fn at_expr_start(p: &Parser) -> bool {
    matches!(
        p.nth0(),
        SyntaxKind::Int
            | SyntaxKind::Float
            | SyntaxKind::String
            | SyntaxKind::True
            | SyntaxKind::False
            | SyntaxKind::Char
            | SyntaxKind::Ident
            | SyntaxKind::Minus
    )
}

pub(crate) fn source_file(p: &mut Parser) {
    let m = p.open();
    while !p.at_eof() {
        if p.at(T![structure]) {
            struct_def(p);
        } else if p.at(SyntaxKind::Ident) {
            fn_def(p);
        } else {
            p.error_and_advance("expected an item");
        }
    }
    m.complete(p, SyntaxKind::SourceFile);
}

fn struct_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'structure'
    p.expect(SyntaxKind::Ident, "expected a struct name");
    field_list(p);
    p.expect(T![;], "expected ';' after struct definition");
    m.complete(p, SyntaxKind::StructDef);
}

fn field_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open field list");
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(SyntaxKind::Ident) {
            field(p);
            // the separating ',' is a child of FieldList, not of Field
            p.expect(T![,], "expected ',' after field");
        } else {
            p.error_and_advance("expected a field");
        }
    }
    p.expect(T!['}'], "expected '}' to close field list");
    m.complete(p, SyntaxKind::FieldList);
}

fn field(p: &mut Parser) {
    let m = p.open();
    type_ref(p);
    p.expect(SyntaxKind::Ident, "expected a field name");
    m.complete(p, SyntaxKind::Field);
}

fn type_ref(p: &mut Parser) {
    let m = p.open();
    p.expect(SyntaxKind::Ident, "expected a type");
    m.complete(p, SyntaxKind::TypeRef);
}

fn fn_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // function name
    param_list(p);
    block(p);
    m.complete(p, SyntaxKind::FnDef);
}

fn param_list(p: &mut Parser) {
    let m = p.open();
    // `(` and `)` are separate tokens; an empty `()` is just a ParamList
    // with no params — unit is inferred from the absence of content
    p.expect(T!['('], "expected '('");
    p.expect(T![')'], "expected ')'");
    m.complete(p, SyntaxKind::ParamList);
}

fn block(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open block");
    while !p.at(T!['}']) && !p.at_eof() {
        stmt(p);
    }
    p.expect(T!['}'], "expected '}' to close block");
    m.complete(p, SyntaxKind::Block);
}

fn stmt(p: &mut Parser) {
    if p.at(T![const]) || p.at(T![var]) {
        let_stmt(p);
    } else if at_expr_start(p) {
        expr_stmt(p);
    } else {
        p.error_and_advance("expected a statement");
    }
}

fn let_stmt(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'const' or 'var'
    // an explicit type precedes the name: `<type> <name>` is two idents
    if p.at(SyntaxKind::Ident) && p.nth(1) == SyntaxKind::Ident {
        type_ref(p);
    }
    p.expect(SyntaxKind::Ident, "expected a binding name");
    p.expect(T![=], "expected '=' in binding");
    expr(p);
    p.expect(T![;], "expected ';' after statement");
    m.complete(p, SyntaxKind::LetStmt);
}

fn expr_stmt(p: &mut Parser) {
    let m = p.open();
    expr(p);
    p.expect(T![;], "expected ';' after expression");
    m.complete(p, SyntaxKind::ExprStmt);
}

/// An expression. Precedence is resolved by the Pratt loop in [`expr_bp`];
/// [`lhs`] parses a prefix-unary form or an atom with its postfix forms.
fn expr(p: &mut Parser) {
    expr_bp(p, 0);
}

/// Left/right binding power of an infix operator, or `None` if `kind` is not
/// one. Operators are left-associative: the left power is the lower of the
/// pair, so an equal-precedence operator on the right does not re-associate.
fn infix_binding_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    use SyntaxKind::*;
    Some(match kind {
        Or => (1, 2),
        And => (3, 4),
        Eq | Neq | Lt | Gt | Leq | Geq => (5, 6),
        Plus | Minus => (7, 8),
        Star | Slash => (9, 10),
        _ => return None,
    })
}

/// Right binding power of the prefix `-` — above every infix operator, so
/// `-a * b` parses as `(-a) * b`.
const PREFIX_BP: u8 = 11;

/// Pratt loop: parse a left-hand side, then fold in infix operators while
/// their left binding power is at least `min_bp`. Each operator wraps the
/// already-parsed LHS via [`CompletedMarker::precede`], so the event buffer
/// stays append-only.
fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = lhs(p)?;
    while let Some((l_bp, r_bp)) = infix_binding_power(p.nth0()) {
        if l_bp < min_bp {
            break;
        }
        let m = lhs.precede(p);
        p.advance(); // the operator token
        expr_bp(p, r_bp);
        lhs = m.complete(p, SyntaxKind::BinExpr);
    }
    Some(lhs)
}

/// A prefix-unary form, or an atom followed by any run of postfix forms. Each
/// postfix form uses [`CompletedMarker::precede`] to wrap its operand.
fn lhs(p: &mut Parser) -> Option<CompletedMarker> {
    if p.at(SyntaxKind::Minus) {
        let m = p.open();
        p.advance(); // '-'
        expr_bp(p, PREFIX_BP);
        return Some(m.complete(p, SyntaxKind::PrefixExpr));
    }

    let mut lhs = atom(p)?;
    loop {
        if p.at(T!['(']) {
            let call = lhs.precede(p);
            arg_list(p);
            lhs = call.complete(p, SyntaxKind::CallExpr);
        } else if p.at(T!['{']) {
            let lit = lhs.precede(p);
            struct_body(p);
            lhs = lit.complete(p, SyntaxKind::StructLit);
        } else {
            break;
        }
    }
    Some(lhs)
}

fn atom(p: &mut Parser) -> Option<CompletedMarker> {
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
            p.error("expected an expression");
            return None;
        }
    };
    Some(m.complete(p, kind))
}

fn arg_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['('], "expected '('");
    while !p.at(T![')']) && !p.at_eof() {
        expr(p);
        if !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![')'], "expected ')' to close argument list");
    m.complete(p, SyntaxKind::ArgList);
}

fn struct_body(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open struct literal");
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(SyntaxKind::Ident) {
            struct_lit_field(p);
            if !p.eat(T![,]) {
                break;
            }
        } else {
            p.error_and_advance("expected a field initializer");
        }
    }
    p.expect(T!['}'], "expected '}' to close struct literal");
    m.complete(p, SyntaxKind::StructLitFieldList);
}

/// A field initializer in a struct literal. A bare `Ident` is the shorthand
/// form (`Point { x }`); `Ident ':' expr` is the explicit form
/// (`Point { x: 0 }`). One node kind serves both — the presence of a value
/// expression distinguishes them.
fn struct_lit_field(p: &mut Parser) {
    let m = p.open();
    p.advance(); // field name — the caller checked it is an Ident
    if p.eat(T![:]) {
        expr(p);
    }
    m.complete(p, SyntaxKind::StructLitField);
}
