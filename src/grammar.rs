//! The eye grammar — v0.1 subset covering exactly `main.eye`.
//!
//! ```text
//! source_file := item*
//! item        := struct_def | fn_def
//! struct_def  := 'structure' Ident field_list ';'
//! field_list  := '{' field* '}'
//! field       := type_ref Ident ','
//! type_ref    := Ident
//! fn_def      := Ident param_list block
//! param_list  := '(' ')'
//! block       := '{' stmt* '}'
//! stmt        := let_stmt | expr_stmt
//! let_stmt    := ('const' | 'var') type_ref? Ident '=' expr ';'
//! expr_stmt   := expr ';'
//! expr        := atom (arg_list | struct_body)*
//! atom        := Int | Float | String | True | False | Char | NameRef
//! ```
//!
//! Every function opens a [`Marker`], parses, and completes it with a node
//! kind. Parsing is resilient: an unexpected token is wrapped in an
//! `ErrorNode` and skipped — the parser never bails, so a tree always comes
//! out.
//!
//! [`Marker`]: crate::parser::Marker

use crate::T;
use crate::parser::{CompletedMarker, Parser};
use crate::syntax::SyntaxKind;

/// True if `p` is positioned at a token that can begin an expression.
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
    p.expect(T![,], "expected ',' after field");
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

/// An expression: an atom followed by any run of postfix forms. Each postfix
/// form uses [`CompletedMarker::precede`] to retroactively wrap the
/// already-parsed left-hand side.
fn expr(p: &mut Parser) {
    let Some(mut lhs) = atom(p) else {
        return;
    };
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
            let f = p.open();
            p.advance(); // shorthand field initializer
            f.complete(p, SyntaxKind::StructLitField);
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
