//! The eye grammar - v0.3 covering `eyesrc/design.eye` plus the v0.3
//! `match` surface from `eyesrc/v03.eye`.
//!
//! ```text
//! source_file  := item*
//! item         := struct_def | enum_def | fn_def
//! struct_def   := 'structure' Ident field_list ';'
//! field_list   := '{' (field ',')* '}'
//! field        := type_ref Ident
//! enum_def     := 'enum' Ident '=' variant ('|' variant)* ';'
//! variant      := '|'? Ident          // leading '|' optional on first variant
//! fn_def       := Ident param_list ('->' type_ref)? block
//! param_list   := '(' (param (',' param)*)? ')'
//! param        := type_ref Ident
//! type_ref     := ('&' type_ref) | (Ident postfix_ptr*)
//! postfix_ptr  := '*'
//!
//! block        := '{' (stmt)* expr? '}'
//! stmt         := let_stmt | expr_stmt
//! let_stmt     := ('let' | 'mut') type_ref? Ident '=' expr ';'
//! expr_stmt    := expr ';'                        // or block-like expr w/o ';'
//! expr         := pratt
//! pratt        := prefix (infix prefix)*
//! prefix       := '-' prefix | '&' prefix | '*' prefix | postfix
//! postfix      := atom (call | struct_body | '.' Ident)*
//! atom         := Int | Float | String | True | False | Char | NameRef
//!               | if_expr | loop_expr | break_expr | continue_expr
//! if_expr      := 'if' expr_no_struct block ('else' block)?
//! loop_expr    := 'loop' block
//! break_expr   := 'break' expr?
//! continue_expr:= 'continue'
//! infix        := '=' | '||' | '&&' | comparison | '+' | '-' | '*' | '/'
//! struct_body  := '{' (struct_lit_field (',' struct_lit_field)*)? '}'
//! struct_lit_field := Ident (':' expr)? | expr           // last is positional
//! ```
//!
//! Every function opens a [`Marker`], parses, and completes it with a node
//! kind. Parsing is resilient: an unexpected token is wrapped in an
//! `ErrorNode` and skipped - the parser never bails, so a tree always comes
//! out.
//!
//! [`Marker`]: crate::Marker

use crate::{CompletedMarker, Parser};
use syntax::{SyntaxKind, T};

/// True if `p` is positioned at a token that can begin an expression - an
/// atom, a prefix operator, or a block-like expression keyword.
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
            | T![-]
            | T![&]
            | T![*]
            | T!['[']
            | T![if]
            | T![loop]
            | T![break]
            | T![continue]
            | T![match]
    )
}

pub(crate) fn source_file(p: &mut Parser) {
    let m = p.open();
    while !p.at_eof() {
        if p.at(T![structure]) {
            struct_def(p);
        } else if p.at(T![union]) {
            union_def(p);
        } else if p.at(T![extern]) {
            extern_block(p);
        } else if p.at(T![enum]) {
            enum_def(p);
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

// A union reuses the struct field-list verbatim; only the keyword and the
// emitted node kind differ (overlapping storage instead of a product type).
fn union_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'union'
    p.expect(SyntaxKind::Ident, "expected a union name");
    field_list(p);
    p.expect(T![;], "expected ';' after union definition");
    m.complete(p, SyntaxKind::UnionDef);
}

// `extern { sig; sig; }` - a batch of C function signatures with no bodies.
// Each name enters the top-level namespace and resolves at link time.
fn extern_block(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'extern'
    p.expect(T!['{'], "expected '{' to open extern block");
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(SyntaxKind::Ident) {
            extern_fn(p);
        } else {
            p.error_and_advance("expected an extern function signature");
        }
    }
    p.expect(T!['}'], "expected '}' to close extern block");
    m.complete(p, SyntaxKind::ExternBlock);
}

// A bodyless fn signature: `name(Type arg, ...) -> Ret;`. Mirrors `fn_def`
// but terminates on `;` where a fn would open its block.
fn extern_fn(p: &mut Parser) {
    let m = p.open();
    p.advance(); // function name
    param_list(p);
    if p.eat(T![->]) {
        type_ref(p);
    }
    p.expect(T![;], "expected ';' after extern signature");
    m.complete(p, SyntaxKind::ExternFn);
}

fn field_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open field list");
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(SyntaxKind::Ident) || p.at(T![&]) {
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

fn enum_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'enum'
    p.expect(SyntaxKind::Ident, "expected enum name");
    p.expect(T![=], "expected '=' after enum name");

    // First variant. At least one variant required. Leading `|` is always
    // optional - stylistic only, accepted inline or multi-line.
    let v_m = p.open();
    if p.at(T![|]) {
        p.advance();
    }
    if p.at(SyntaxKind::Ident) {
        p.advance(); // variant ident
        v_m.complete(p, SyntaxKind::Variant);
    } else {
        v_m.abandon(p);
        p.error("expected at least one variant");
    }

    // Subsequent variants: '|' mandatory as a separator.
    while p.at(T![|]) {
        let v_m = p.open();
        p.advance(); // '|'
        p.expect(SyntaxKind::Ident, "expected variant name after '|'");
        v_m.complete(p, SyntaxKind::Variant);
    }

    p.expect(T![;], "expected ';' after enum definition");
    m.complete(p, SyntaxKind::EnumDef);
}

fn type_ref(p: &mut Parser) {
    // parse the base type (either &ref, [T; N] array, or ident)
    let mut m = if p.at(T!['[']) {
        // `[T; N]` fixed-size array. N is an expression in the grammar but
        // restricted to an integer literal in lowering (no const-expr yet).
        let m = p.open();
        p.advance(); // '['
        type_ref(p); // element type
        p.expect(T![;], "expected ';' between array element type and length");
        expr(p); // length
        p.expect(T![']'], "expected ']' to close array type");
        m.complete(p, SyntaxKind::ArrayType)
    } else if p.at(T![&]) {
        let m = p.open();
        p.advance(); // '&'
        type_ref(p);
        m.complete(p, SyntaxKind::RefType)
    } else if p.at(SyntaxKind::Ident) {
        let m = p.open();
        p.advance(); // ident
        m.complete(p, SyntaxKind::IdentType)
    } else {
        p.error_and_advance("expected a type");
        return;
    };

    // parse any postfix pointer '*' operators
    while p.at(T![*]) {
        let ptr_m = m.precede(p);
        p.advance(); // '*'
        m = ptr_m.complete(p, SyntaxKind::PtrType);
    }
}

fn fn_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // function name
    param_list(p);
    if p.eat(T![->]) {
        type_ref(p);
    }
    block(p);
    m.complete(p, SyntaxKind::FnDef);
}

fn param_list(p: &mut Parser) {
    let m = p.open();
    // `(` and `)` are separate tokens; an empty `()` is just a ParamList
    // with no params - unit is inferred from the absence of content
    p.expect(T!['('], "expected '('");
    while !p.at(T![')']) && !p.at_eof() {
        let param_m = p.open();
        type_ref(p);
        p.expect(SyntaxKind::Ident, "expected parameter name");
        param_m.complete(p, SyntaxKind::Param);
        if !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![')'], "expected ')'");
    m.complete(p, SyntaxKind::ParamList);
}

fn block(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open block");
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(T![let]) || p.at(T![mut]) {
            let_stmt(p);
        } else if at_expr_start(p) {
            // A block-like expression (`if`, `loop`, raw block) does not need
            // a trailing `;` when followed by another stmt; everything else
            // does. Either way, if it sits in tail position before `}` the
            // ExprStmt marker is abandoned so the bare expr falls out as the
            // block's tail.
            let m_stmt = p.open();
            // a leading `if`/`loop`/`match` makes this expression block-like,
            // so it can stand as a statement without a trailing `;`. A bare
            // `{` is not accepted by `lhs` as an expression start today;
            // reserve the arm for a future block-as-expression form.
            let is_block_like = matches!(p.nth0(), T![if] | T![loop] | T![match]);
            expr(p);

            if p.eat(T![;]) {
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            } else if p.at(T!['}']) {
                m_stmt.abandon(p);
                break;
            } else if is_block_like {
                // `if { ... } counter = ...;` - the if is a statement here,
                // no `;` required between block-like and the next stmt.
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            } else {
                p.error("expected ';' after expression");
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            }
        } else {
            p.error_and_advance("expected a statement");
        }
    }
    p.expect(T!['}'], "expected '}' to close block");
    m.complete(p, SyntaxKind::Block);
}

#[allow(dead_code)]
fn stmt(p: &mut Parser) {
    if p.at(T![let]) || p.at(T![mut]) {
        let_stmt(p);
    } else if at_expr_start(p) {
        expr_stmt(p);
    } else {
        p.error_and_advance("expected a statement");
    }
}

/// `let_stmt` accepts three shapes:
///
/// - inferred:    `let x = expr;`
/// - explicit:    `mut Point p = expr;`         (Ident then Ident)
/// - explicit ref: `mut &Point r = expr;`       (& then Ident then Ident)
///
/// The pointer-suffix form `T*` is not yet disambiguated here; a future v0.3
/// will look further ahead.
fn let_stmt(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'let' or 'mut'
    // A leading type is present when the tokens after `let`/`mut` read as
    // `type name` rather than `name =`. A leading `&` begins a ref type and a
    // leading `[` begins an array type (a binding name never starts with
    // either). An `Ident` is a type if the next token is another `Ident`
    // (`Point p`) or a postfix `*` (`Point* p`).
    let has_type = matches!(p.nth0(), T![&] | T!['['])
        || matches!(
            (p.nth0(), p.nth(1)),
            (SyntaxKind::Ident, SyntaxKind::Ident) | (SyntaxKind::Ident, T![*])
        );
    if has_type {
        type_ref(p);
    }
    p.expect(SyntaxKind::Ident, "expected a binding name");
    p.expect(T![=], "expected '=' in binding");
    expr(p);
    p.expect(T![;], "expected ';' after statement");
    m.complete(p, SyntaxKind::LetStmt);
}

#[allow(dead_code)]
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
/// one. Most operators are left-associative (`l_bp < r_bp`). Assignment is
/// right-associative (`l_bp > r_bp`) and has the lowest precedence.
fn infix_binding_power(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        T![=] => (2, 1),
        T![||] => (3, 4),
        T![&&] => (5, 6),
        T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=] => (7, 8),
        T![+] | T![-] => (9, 10),
        T![*] | T![/] => (11, 12),
        _ => return None,
    })
}

/// Right binding power of any prefix unary - above every infix operator, so
/// `-a * b` parses as `(-a) * b`.
const PREFIX_BP: u8 = 13;

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
        let op = p.nth0();
        let kind = if op == T![=] {
            SyntaxKind::AssignExpr
        } else {
            SyntaxKind::BinExpr
        };
        let m = lhs.precede(p);
        p.advance(); // the operator token
        expr_bp(p, r_bp);
        lhs = m.complete(p, kind);
    }
    Some(lhs)
}

/// A prefix-unary form, or an atom followed by any run of postfix forms. Each
/// postfix form uses [`CompletedMarker::precede`] to wrap its operand.
fn lhs(p: &mut Parser) -> Option<CompletedMarker> {
    if p.at(T![-]) {
        let m = p.open();
        p.advance(); // '-'
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
    if p.at(T![match]) {
        return Some(match_expr(p));
    }
    if p.at(T!['[']) {
        return Some(array_lit(p));
    }

    let mut lhs = atom(p)?;
    loop {
        if p.at(T!['(']) {
            let call = lhs.precede(p);
            arg_list(p);
            lhs = call.complete(p, SyntaxKind::CallExpr);
        } else if p.at(T!['[']) {
            // postfix index `base[i]` - binds as tightly as call/field.
            let index = lhs.precede(p);
            p.advance(); // '['
            expr(p);
            p.expect(T![']'], "expected ']' to close index");
            lhs = index.complete(p, SyntaxKind::IndexExpr);
        } else if p.at(T!['{']) && !p.no_struct_lit() {
            let lit = lhs.precede(p);
            struct_body(p);
            lhs = lit.complete(p, SyntaxKind::StructLit);
        } else if p.at(T![.]) {
            let field_expr = lhs.precede(p);
            p.advance();
            let name_m = p.open();
            p.expect(SyntaxKind::Ident, "expected field identifier after '.'");
            name_m.complete(p, SyntaxKind::NameRef);
            lhs = field_expr.complete(p, SyntaxKind::FieldExpr);
        } else if p.at(T![as]) {
            // `expr as Type` - a postfix cast. Binds as tightly as call/field,
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

fn if_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'if'
    let prev = p.set_no_struct_lit(true);
    expr(p);
    p.set_no_struct_lit(prev);
    block(p);
    if p.eat(T![else]) {
        if p.at(T![if]) {
            // `else if` desugars to `else { if ... }`: wrap the chained if in a
            // synthetic Block so the else-branch stays a Block end-to-end
            // (AST/HIR/codegen are unchanged). Codegen flattens the trivial
            // `else { if }` back to `else if` so the C output does not nest.
            let blk = p.open();
            if_expr(p);
            blk.complete(p, SyntaxKind::Block);
        } else {
            block(p);
        }
    }
    m.complete(p, SyntaxKind::IfExpr)
}

fn loop_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'loop'
    block(p);
    m.complete(p, SyntaxKind::LoopExpr)
}

fn break_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'break'
    // a `break` may carry a value (`break expr`); a `;` or `}` ends it bare.
    if at_expr_start(p) {
        expr(p);
    }
    m.complete(p, SyntaxKind::BreakExpr)
}

fn continue_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'continue'
    m.complete(p, SyntaxKind::ContinueExpr)
}

/// `match scrut { arm, arm, ... }`. Mirrors `if_expr` for the scrutinee: the
/// `no_struct_lit` gate is set so `match sh { Circle -> 1 }` does not parse
/// `sh { Circle -> 1 }` as a struct literal. The gate is cleared inside the
/// arm block - arm body expressions are unrestricted.
fn match_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // 'match'
    let prev = p.set_no_struct_lit(true);
    expr(p);
    p.set_no_struct_lit(prev);
    match_arm_list(p);
    m.complete(p, SyntaxKind::MatchExpr)
}

fn match_arm_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open match arms");
    // arm body expressions can contain struct literals freely
    let prev = p.set_no_struct_lit(false);
    while !p.at(T!['}']) && !p.at_eof() {
        if !at_pat_start(p) {
            p.error_and_advance("expected a match arm");
            continue;
        }
        let had_comma = match_arm(p);
        // `,` is the arm separator. It is mandatory between arms; only the
        // final arm before `}` may omit it.
        if !had_comma && !p.at(T!['}']) && !p.at_eof() {
            p.error("expected ',' between match arms");
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T!['}'], "expected '}' to close match arms");
    m.complete(p, SyntaxKind::MatchArmList);
}

/// Parse one arm. Returns `true` if a trailing `,` was consumed - the arm
/// list uses that to enforce the "comma required between arms" rule.
fn match_arm(p: &mut Parser) -> bool {
    let m = p.open();
    pat(p);
    p.expect(T![->], "expected '->' after match pattern");
    expr(p);
    let had_comma = p.eat(T![,]);
    m.complete(p, SyntaxKind::MatchArm);
    had_comma
}

/// True if `p` is at a token that can begin a pattern.
fn at_pat_start(p: &Parser) -> bool {
    matches!(p.nth0(), SyntaxKind::Ident | T![_])
}

/// Patterns. Three forms in v0.3:
///   - `_`                         -> `WildcardPat`
///   - `Enum '.' Variant`          -> `PathPat` (qualified)
///   - `Ident`                     -> `BareIdentPat`
fn pat(p: &mut Parser) {
    if p.at(T![_]) {
        let m = p.open();
        p.advance(); // '_'
        m.complete(p, SyntaxKind::WildcardPat);
        return;
    }
    if p.at(SyntaxKind::Ident) {
        if p.nth(1) == T![.] {
            let m = p.open();
            // qualifier name ref
            let nm = p.open();
            p.advance(); // qualifier ident
            nm.complete(p, SyntaxKind::NameRef);
            p.advance(); // '.'
            // variant name ref
            let nm = p.open();
            p.expect(SyntaxKind::Ident, "expected variant name after '.'");
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
    p.error_and_advance("expected a pattern");
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

/// `[a, b, c]` array literal. Its own struct-lit context: a suppressed flag
/// from an enclosing if/loop condition does not apply inside the elements.
/// Trailing comma is allowed.
fn array_lit(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // '['
    let prev = p.set_no_struct_lit(false);
    while !p.at(T![']']) && !p.at_eof() {
        if at_expr_start(p) {
            expr(p);
            if !p.eat(T![,]) {
                break;
            }
        } else {
            p.error_and_advance("expected an array element");
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T![']'], "expected ']' to close array literal");
    m.complete(p, SyntaxKind::ArrayLit)
}

fn arg_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['('], "expected '('");
    // an arg list is its own struct-lit context: a suppressed flag from an
    // enclosing if/loop condition does not apply inside the arguments.
    let prev = p.set_no_struct_lit(false);
    while !p.at(T![')']) && !p.at_eof() {
        expr(p);
        if !p.eat(T![,]) {
            break;
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T![')'], "expected ')' to close argument list");
    m.complete(p, SyntaxKind::ArgList);
}

fn struct_body(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], "expected '{' to open struct literal");
    // a struct body's fields are independent of any outer no-struct-lit gate
    let prev = p.set_no_struct_lit(false);
    while !p.at(T!['}']) && !p.at_eof() {
        if at_expr_start(p) {
            struct_lit_field(p);
            if !p.eat(T![,]) {
                break;
            }
        } else {
            p.error_and_advance("expected a field initializer");
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T!['}'], "expected '}' to close struct literal");
    m.complete(p, SyntaxKind::StructLitFieldList);
}

/// A field initializer in a struct literal. Three forms:
///
/// - `Ident` followed by `,` or `}`  - shorthand:   `Point { x, y }`
/// - `Ident ':' expr`                - explicit:    `Point { x: 0 }`
/// - any other expression            - positional:  `Point { 10, 20 }`
///
/// One node kind serves all three; the presence of a direct Ident token vs.
/// an Expr child distinguishes them in the typed AST.
fn struct_lit_field(p: &mut Parser) {
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
