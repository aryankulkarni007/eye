//! The eye grammar - the full v0.7 surface: items (struct, union, enum,
//! `extern` FFI, fn), references / pointers / fixed arrays in the type system,
//! the operator set (arithmetic, bitwise, comparison, logical, compound
//! assignment), `match`, `as` casts, and array literals / indexing. Exercised
//! end to end by `eyesrc/*.eye` (see `eyesrc/v06.eye` for the operator surface
//! and `eyesrc/arrays.eye` for arrays).
//!
//! ```text
//! source_file  := item*
//! item         := struct_def | union_def | extern_block | enum_def | fn_def
//! struct_def   := 'structure' Ident field_list ';'
//! union_def    := 'union' Ident field_list ';'
//! extern_block := 'extern' '{' extern_fn* '}'
//! extern_fn    := Ident param_list ('->' type_ref)? ';'
//! enum_def     := 'enum' Ident '=' '|'? variant ('|' variant)* ';'
//! variant      := Ident                // leading '|' before the first is optional
//! fn_def       := Ident param_list ('->' type_ref)? block
//! field_list   := '{' (field ',')* '}' // the ',' terminates every field
//! field        := type_ref Ident
//! param_list   := '(' (param (',' param)* ','?)? ')'
//! param        := type_ref Ident
//! type_ref     := array_type | ('&' type_ref) | (Ident postfix_ptr*)
//! array_type   := '[' type_ref ';' expr ']'        // fixed-size array
//! postfix_ptr  := '*'                              // wraps the base in a PtrType
//!
//! block        := '{' (stmt)* expr? '}'
//! stmt         := let_stmt | expr_stmt
//! let_stmt     := ('let' | 'mut') type_ref? Ident '=' expr ';'
//! expr_stmt    := expr ';'                        // or block-like expr w/o ';'
//! expr         := pratt
//! pratt        := prefix (infix prefix)*
//! prefix       := '-' prefix | '~' prefix | '!' prefix    // PrefixExpr
//!               | '&' prefix | '*' prefix | postfix        // Ref/Deref expr
//! postfix      := base (call | index | struct_body | '.' Ident | 'as' type_ref)*
//! call         := '(' (expr (',' expr)* ','?)? ')'
//! index        := '[' expr ']'
//! base         := '(' expr ')' | atom            // parenthesized group or atom
//! atom         := Int | Float | String | True | False | Char | NameRef
//!               | if_expr | loop_expr | break_expr | continue_expr
//!               | match_expr | array_lit
//! array_lit    := '[' (expr (',' expr)* ','?)? ']'
//! if_expr      := 'if' expr_no_struct block ('else' (if_expr | block))?
//! loop_expr    := 'loop' block
//! break_expr   := 'break' expr?
//! continue_expr:= 'continue'
//! match_expr   := 'match' expr_no_struct '{' match_arm* '}'
//! match_arm    := pat '->' expr ','?              // ',' optional on the last arm
//! pat          := '_' | (NameRef '.' NameRef) | NameRef
//! // precedence is Rust-style (no-footgun): every bitwise op binds tighter
//! // than comparison, and comparison is non-associative (no chaining). '=' and
//! // the compound forms are right-associative and lowest; 'as' / call / index /
//! // field bind tightest, above every prefix unary.
//! infix        := '=' | '+=' | '-=' | '||' | '&&' | comparison
//!               | '|' | '^' | '&' | '<<' | '>>' | '+' | '-' | '*' | '/' | '%'
//! comparison   := '==' | '!=' | '<' | '>' | '<=' | '>='
//! struct_body  := '{' (struct_lit_field (',' struct_lit_field)* ','?)? '}'
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
            p.error_and_advance(crate::SyntaxError::ExpectedItem);
        }
    }
    m.complete(p, SyntaxKind::SourceFile);
}

fn struct_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'structure'
    p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedStructName);
    field_list(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterStruct);
    m.complete(p, SyntaxKind::StructDef);
}

// A union reuses the struct field-list verbatim; only the keyword and the
// emitted node kind differ (overlapping storage instead of a product type).
fn union_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'union'
    p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedUnionName);
    field_list(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterUnion);
    m.complete(p, SyntaxKind::UnionDef);
}

// `extern { sig; sig; }` - a batch of C function signatures with no bodies.
// Each name enters the top-level namespace and resolves at link time.
fn extern_block(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'extern'
    p.expect(T!['{'], crate::SyntaxError::ExpectedExternOpen);
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(SyntaxKind::Ident) {
            extern_fn(p);
        } else {
            p.error_and_advance(crate::SyntaxError::ExpectedExternSignature);
        }
    }
    p.expect(T!['}'], crate::SyntaxError::ExpectedExternClose);
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
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterExternSig);
    m.complete(p, SyntaxKind::ExternFn);
}

fn field_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], crate::SyntaxError::ExpectedFieldListOpen);
    while !p.at(T!['}']) && !p.at_eof() {
        // `[` starts an array field type; HIR rejects array-typed fields with a
        // clear message, which is better than a cryptic crate::SyntaxError::ExpectedField.
        if p.at(SyntaxKind::Ident) || p.at(T![&]) || p.at(T!['[']) {
            field(p);
            // the separating ',' is a child of FieldList, not of Field
            p.expect(T![,], crate::SyntaxError::ExpectedCommaAfterField);
        } else {
            p.error_and_advance(crate::SyntaxError::ExpectedField);
        }
    }
    p.expect(T!['}'], crate::SyntaxError::ExpectedFieldListClose);
    m.complete(p, SyntaxKind::FieldList);
}

fn field(p: &mut Parser) {
    let m = p.open();
    type_ref(p);
    p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedFieldName);
    m.complete(p, SyntaxKind::Field);
}

fn enum_def(p: &mut Parser) {
    let m = p.open();
    p.advance(); // 'enum'
    p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedEnumName);
    p.expect(T![=], crate::SyntaxError::ExpectedEqAfterEnumName);

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
        p.error(crate::SyntaxError::ExpectedAtLeastOneVariant);
    }

    // Subsequent variants: '|' mandatory as a separator.
    while p.at(T![|]) {
        let v_m = p.open();
        p.advance(); // '|'
        p.expect(
            SyntaxKind::Ident,
            crate::SyntaxError::ExpectedVariantNameAfterPipe,
        );
        v_m.complete(p, SyntaxKind::Variant);
    }

    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterEnum);
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
        p.expect(T![;], crate::SyntaxError::ExpectedSemiInArrayType);
        expr(p); // length
        p.expect(T![']'], crate::SyntaxError::ExpectedArrayTypeClose);
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
    p.expect(T!['('], crate::SyntaxError::ExpectedOpenParen);
    while !p.at(T![')']) && !p.at_eof() {
        let param_m = p.open();
        type_ref(p);
        p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedParamName);
        param_m.complete(p, SyntaxKind::Param);
        if !p.eat(T![,]) {
            break;
        }
    }
    p.expect(T![')'], crate::SyntaxError::ExpectedCloseParen);
    m.complete(p, SyntaxKind::ParamList);
}

fn block(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['{'], crate::SyntaxError::ExpectedBlockOpen);
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
                p.error(crate::SyntaxError::ExpectedSemiAfterExpr);
                m_stmt.complete(p, SyntaxKind::ExprStmt);
            }
        } else {
            p.error_and_advance(crate::SyntaxError::ExpectedStatement);
        }
    }
    p.expect(T!['}'], crate::SyntaxError::ExpectedBlockClose);
    m.complete(p, SyntaxKind::Block);
}

#[allow(dead_code)]
fn stmt(p: &mut Parser) {
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
/// - inferred:     `let x = expr;`                (Ident then `=` -> no type)
/// - explicit:     `mut Point p = expr;`          (Ident then Ident)
/// - pointer:      `mut Point* p = expr;`         (Ident then `*`)
/// - explicit ref: `mut &Point r = expr;`         (leading `&`)
/// - explicit arr: `let [int32; 3] xs = expr;`    (leading `[`)
///
/// Nested refs are written with a space (`& &Point`); the `&&` spelling lexes
/// as a single logical-and token, so it cannot denote a ref-to-ref type.
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
    p.expect(SyntaxKind::Ident, crate::SyntaxError::ExpectedBindingName);
    p.expect(T![=], crate::SyntaxError::ExpectedEqInBinding);
    expr(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterStatement);
    m.complete(p, SyntaxKind::LetStmt);
}

#[allow(dead_code)]
fn expr_stmt(p: &mut Parser) {
    let m = p.open();
    expr(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterExpr);
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
        // Assignment (incl. compound `+=`/`-=`) is right-associative and lowest.
        T![=] | T![+=] | T![-=] => (2, 1),
        T![||] => (3, 4),
        T![&&] => (5, 6),
        // Comparison is its own tier; F1 makes it non-associative (see
        // `expr_bp`) so `a < b < c` is a hard error, not C's silent chain.
        T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=] => (7, 8),
        // No-footgun precedence (Rust-style, not C-style): every bitwise op
        // binds TIGHTER than comparison, so `a & b == c` is `(a & b) == c`,
        // never C's `a & (b == c)`. Each bitwise op gets its own tier.
        T![|] => (9, 10),
        T![^] => (11, 12),
        T![&] => (13, 14),
        T![<<] | T![>>] => (15, 16),
        T![+] | T![-] => (17, 18),
        T![*] | T![/] | T![%] => (19, 20),
        _ => return None,
    })
}

/// True if `kind` is a comparison/equality operator. These form one tier and
/// are non-associative (F1): chaining two of them at the same level is an
/// error, so `a < b < c` must be parenthesized.
fn is_comparison(kind: SyntaxKind) -> bool {
    matches!(kind, T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=])
}

/// Right binding power of any prefix unary - above every infix operator, so
/// `-a * b` parses as `(-a) * b`.
const PREFIX_BP: u8 = 21;

/// Pratt loop: parse a left-hand side, then fold in infix operators while
/// their left binding power is at least `min_bp`. Each operator wraps the
/// already-parsed LHS via [`CompletedMarker::precede`], so the event buffer
/// stays append-only.
fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = lhs(p)?;
    // Tracks whether the operator folded last at *this* level was a comparison.
    // Two comparisons in a row at the same level is a non-associativity error
    // (F1). A same-tier comparison never appears in an operator's right operand
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
        let kind = if matches!(op, T![=] | T![+=] | T![-=]) {
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

/// A prefix-unary form, or an atom followed by any run of postfix forms. Each
/// postfix form uses [`CompletedMarker::precede`] to wrap its operand.
fn lhs(p: &mut Parser) -> Option<CompletedMarker> {
    // Prefix-unary: `-` negate, `~` bitwise-complement, `!` logical-not. The
    // operand binds at PREFIX_BP (above every infix op) so `-a * b` is `(-a) * b`.
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
    if p.at(T![match]) {
        return Some(match_expr(p));
    }
    if p.at(T!['[']) {
        return Some(array_lit(p));
    }

    // The base of the postfix chain: a parenthesized group or an atom. A
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
            p.advance(); // '['
            expr(p);
            p.expect(T![']'], crate::SyntaxError::ExpectedIndexClose);
            lhs = index.complete(p, SyntaxKind::IndexExpr);
        } else if p.at(T!['{']) && !p.no_struct_lit() {
            let lit = lhs.precede(p);
            struct_body(p);
            lhs = lit.complete(p, SyntaxKind::StructLit);
        } else if p.at(T![.]) {
            let field_expr = lhs.precede(p);
            p.advance();
            let name_m = p.open();
            p.expect(
                SyntaxKind::Ident,
                crate::SyntaxError::ExpectedFieldIdentAfterDot,
            );
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
    // Reject `if x = y { ... }`: an assignment in a condition is the classic
    // `=`/`==` footgun. Anchored at the condition's first token, not the cursor.
    let cond_start = p.cursor_range();
    let cond = expr_bp(p, 0);
    p.set_no_struct_lit(prev);
    if matches!(cond, Some(cm) if cm.kind() == SyntaxKind::AssignExpr) {
        p.error_at(cond_start, crate::GrammarError::AssignInIfCondition);
    }
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
    p.expect(T!['{'], crate::SyntaxError::ExpectedMatchArmsOpen);
    // arm body expressions can contain struct literals freely
    let prev = p.set_no_struct_lit(false);
    while !p.at(T!['}']) && !p.at_eof() {
        if !at_pat_start(p) {
            p.error_and_advance(crate::SyntaxError::ExpectedMatchArm);
            continue;
        }
        let had_comma = match_arm(p);
        // `,` is the arm separator. It is mandatory between arms; only the
        // final arm before `}` may omit it.
        if !had_comma && !p.at(T!['}']) && !p.at_eof() {
            p.error(crate::SyntaxError::ExpectedCommaBetweenMatchArms);
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T!['}'], crate::SyntaxError::ExpectedMatchArmsClose);
    m.complete(p, SyntaxKind::MatchArmList);
}

/// Parse one arm. Returns `true` if a trailing `,` was consumed - the arm
/// list uses that to enforce the "comma required between arms" rule.
fn match_arm(p: &mut Parser) -> bool {
    let m = p.open();
    pat(p);
    p.expect(T![->], crate::SyntaxError::ExpectedArrowAfterPattern);
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
            p.expect(
                SyntaxKind::Ident,
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
            p.error(crate::SyntaxError::ExpectedExpression);
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
            p.error_and_advance(crate::SyntaxError::ExpectedArrayElement);
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T![']'], crate::SyntaxError::ExpectedArrayLitClose);
    m.complete(p, SyntaxKind::ArrayLit)
}

/// `( expr )` - a parenthesized group. Purely a precedence override; HIR
/// lowers it to its inner expression, so it leaves no trace past the AST. A
/// group is its own struct-lit context, like an arg list or array element.
fn paren_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    p.advance(); // '('
    let prev = p.set_no_struct_lit(false);
    expr(p);
    p.set_no_struct_lit(prev);
    p.expect(T![')'], crate::SyntaxError::ExpectedParenExprClose);
    m.complete(p, SyntaxKind::ParenExpr)
}

fn arg_list(p: &mut Parser) {
    let m = p.open();
    p.expect(T!['('], crate::SyntaxError::ExpectedOpenParen);
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
    p.expect(T![')'], crate::SyntaxError::ExpectedArgListClose);
    m.complete(p, SyntaxKind::ArgList);
}

fn struct_body(p: &mut Parser) {
    let m = p.open();
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
            p.error_and_advance(crate::SyntaxError::ExpectedFieldInit);
        }
    }
    p.set_no_struct_lit(prev);
    p.expect(T!['}'], crate::SyntaxError::ExpectedStructLitClose);
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
