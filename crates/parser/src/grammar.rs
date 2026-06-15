//! the eye grammar - the full v0.7 surface: items (struct, union, enum,
//! `extern` FFI, fn), references / pointers / fixed arrays in the type system,
//! the operator set (arithmetic, bitwise, comparison, logical, compound
//! assignment), `match`, `as` casts, and array literals / indexing. exercised
//! end to end by `eyesrc/*.eye` (see `eyesrc/operators.eye` for the operator surface
//! and `eyesrc/arrays.eye` for arrays).
//!
//! ```text
//! source_file := item*
//! item := const_def | struct_def | union_def | extern_block | enum_def | fn_def
//! const_def := 'const' type_ref ident '=' expr ';' // compile-time value
//! // also valid as a stmt
//! struct_def := 'structure' ident field_list ';'
//! union_def := 'union' ident field_list ';'
//! extern_block := 'extern' '{' (extern_fn | extern_type)* '}'
//! extern_fn := ident param_list ('->' type_ref)? ';'
//! extern_type := 'type' ident ';' // opaque FFI type
//! enum_def := 'enum' ident '=' '|'? variant ('|' variant)* ';'
//! variant := ident // leading '|' before the first is optional
//! fn_def := ident param_list ('->' type_ref)? block
//! field_list := '{' (field ',')* '}' // the ',' terminates every field
//! field := type_ref ident
//! param_list := '(' (param (',' param)* ','?)? '...'? ')' // '...' extern-only, last
//! param := type_ref ident
//! type_ref := array_type | ('&' type_ref) | (ident postfix_ptr*)
//! array_type := '[' type_ref ';' expr ']' // fixed-size array
//! postfix_ptr := '*' // wraps the base in a ptrtype
//!
//! block := '{' (stmt)* expr? '}'
//! stmt := let_stmt | const_def | expr_stmt
//! let_stmt := ('let' | 'mut') ((type_ref? ident) | struct_pat) '=' expr ';'
//! expr_stmt := expr ';' // or block-like expr w/o ';'
//! expr := pratt
//! pratt := prefix (infix prefix)*
//! prefix := '-' prefix | '~' prefix | '!' prefix // prefixexpr
//! | '&' prefix | '*' prefix | postfix // ref/deref expr
//! postfix := base (call | index | struct_body | '.' ident | 'as' type_ref)*
//! call := '(' (expr (',' expr)* ','?)? ')'
//! index := '[' expr ']'
//! base := '(' expr ')' | atom // parenthesized group or atom
//! atom := int | float | string | true | false | char | nameref
//! | if_expr | loop_expr | break_expr | continue_expr
//! | return_expr | match_expr | array_lit
//! array_lit := '[' (expr ((';' expr) | (',' expr)* ','?))? ']' // list or `[v; N]` repeat
//! if_expr := 'if' expr_no_struct block ('else' (if_expr | block))?
//! loop_expr := 'loop' block
//! break_expr := 'break' expr?
//! continue_expr:= 'continue'
//! return_expr := 'return' expr?
//! match_expr := 'match' expr_no_struct '{' match_arm* '}'
//! match_arm := pat ('if' expr)? '->' expr ','? // ',' optional on last arm
//! pat := '_' | (nameref '.' nameref) | literal | nameref
//! // precedence is rust-style (no-footgun): every bitwise op binds tighter
//! // than comparison, and comparison is non-associative (no chaining). '=' and
//! // the compound forms are right-associative and lowest; 'as' / call / index /
//! // field bind tightest, above every prefix unary.
//! infix := '=' | '+=' | '-=' | '||' | '&&' | comparison
//! | '|' | '^' | '&' | '<<' | '>>' | '+' | '-' | '*' | '/' | '%'
//! comparison := '==' | '!=' | '<' | '>' | '<=' | '>='
//! struct_body := '{' (struct_lit_field (',' struct_lit_field)* ','?)? '}'
//! struct_lit_field := ident (':' expr)? | expr // last is positional
//! ```
//!
//! every function opens a [`Marker`], parses, and completes it with a node
//! kind. parsing is resilient: an unexpected token is wrapped in an
//! `ErrorNode` and skipped - the parser never bails, so a tree always comes
//! out.
//!
//! [`Marker`]: crate::marker

use crate::{CompletedMarker, Parser};
use syntax::{SyntaxKind, T};
use text_size::TextRange;

/// true if `p` is positioned at a token that can begin an expression - an
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
            | T![return]
            | T![match]
    )
}

pub(crate) fn source_file(p: &mut Parser) {
    let m = p.open();
    while !p.at_eof() {
        if p.at(T![const]) {
            const_def(p);
        } else if p.at(T![let]) || p.at(T![mut]) {
            global_def(p);
        } else if p.at(T![structure]) {
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
            p.sync(
                &[
                    T![const],
                    T![let],
                    T![mut],
                    T![structure],
                    T![union],
                    T![extern],
                    T![enum],
                    SyntaxKind::Ident,
                ],
                crate::SyntaxError::ExpectedItem,
            );
        }
    }
    m.complete(p, SyntaxKind::SourceFile);
}

// `const TYPE Ident = expr;` - a compile-time constant value, at the top level
// or as a statement inside a block (same node either way; HIR scopes the local
// form lexically). the type is always explicit (no inference at the floor); the
// initializer is a const-expr folded in HIR. a const is a value, not storage -
// it has no guaranteed address (`&const` is illegal, enforced in HIR).
fn const_def(p: &mut Parser) {
    let m = p.open();
    let def_start = p.cursor_range(); // 'const' - anchor for diagnostics
    p.advance(); // 'const'
    type_ref(p);
    p.expect_after(
        SyntaxKind::Ident,
        def_start,
        crate::SyntaxError::ExpectedConstName,
    );
    let had_eq = p.eat(T![=]);
    expr(p);
    let had_semi = p.eat(T![;]);
    // deferred diagnostics with spans covering the full construct
    if !had_eq {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedEqInConst);
    }
    if !had_semi {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterConst);
    }
    m.complete(p, SyntaxKind::ConstDef);
}

// `let TYPE Ident = expr;` / `mut TYPE Ident = expr;` at the top level - a
// global: addressable static storage. the type is explicit at the floor (no
// inference); the initializer must be const-evaluable (HIR folds it). `let` is
// read-only, `mut` is mutable. distinct from a const (a value with no address).
fn global_def(p: &mut Parser) {
    let m = p.open();
    let def_start = p.cursor_range(); // 'let' or 'mut' - anchor for diagnostics
    p.advance(); // 'let' or 'mut'
    type_ref(p);
    p.expect_after(
        SyntaxKind::Ident,
        def_start,
        crate::SyntaxError::ExpectedBindingName,
    );
    let had_eq = p.eat(T![=]);
    expr(p);
    let had_semi = p.eat(T![;]);
    if !had_eq {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedEqInBinding);
    }
    if !had_semi {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterStatement);
    }
    m.complete(p, SyntaxKind::GlobalDef);
}

fn struct_def(p: &mut Parser) {
    let m = p.open();
    let kw = p.cursor_range(); // 'structure' - anchor for diagnostics
    p.advance(); // 'structure'
    p.expect_after(
        SyntaxKind::Ident,
        kw,
        crate::SyntaxError::ExpectedStructName,
    );
    let header = TextRange::new(kw.start(), p.cursor_range().start());
    field_list(p, header);
    let had_semi = p.eat(T![;]);
    if !had_semi {
        let span = TextRange::new(kw.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterStruct);
    }
    m.complete(p, SyntaxKind::StructDef);
}

// a union reuses the struct field-list verbatim; only the keyword and the
// emitted node kind differ (overlapping storage instead of a product type).
fn union_def(p: &mut Parser) {
    let m = p.open();
    let kw = p.cursor_range(); // 'union' - anchor for diagnostics
    p.advance(); // 'union'
    p.expect_after(SyntaxKind::Ident, kw, crate::SyntaxError::ExpectedUnionName);
    let header = TextRange::new(kw.start(), p.cursor_range().start());
    field_list(p, header);
    let had_semi = p.eat(T![;]);
    if !had_semi {
        let span = TextRange::new(kw.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterUnion);
    }
    m.complete(p, SyntaxKind::UnionDef);
}

// `extern { sig; sig; }` - a batch of c function signatures with no bodies.
// each name enters the top-level namespace and resolves at link time.
fn extern_block(p: &mut Parser) {
    let m = p.open();
    let ctx = p.cursor_range(); // 'extern' keyword - context for missing '{'
    p.advance(); // 'extern'
    let open_brace = p.cursor_range();
    let had_open = p.eat(T!['{']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedExternOpen);
    }
    while !p.at(T!['}']) && !p.at_eof() {
        if p.at(T![type]) {
            extern_type(p);
        } else if p.at(SyntaxKind::Ident) {
            extern_fn(p);
        } else {
            p.sync(
                &[T!['}'], SyntaxKind::Ident, T![type]],
                crate::SyntaxError::ExpectedExternSignature,
            );
        }
    }
    if !p.eat(T!['}']) {
        let range = if had_open {
            TextRange::new(open_brace.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedExternClose);
    }
    m.complete(p, SyntaxKind::ExternBlock);
}

// a bodyless fn signature: `name(Type arg, ...) -> Ret;`. mirrors `fn_def`
// but terminates on `;` where a fn would open its block.
fn extern_fn(p: &mut Parser) {
    let m = p.open();
    let sig_start = p.cursor_range(); // function name - anchor for diagnostics
    let ctx = sig_start; // context for missing '('
    p.advance(); // function name
    param_list(p, ctx, true);
    if p.eat(T![->]) {
        type_ref(p);
    }
    let had_semi = p.eat(T![;]);
    if !had_semi {
        let span = TextRange::new(sig_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterExternSig);
    }
    m.complete(p, SyntaxKind::ExternFn);
}

// `type Name;` inside an extern block: an opaque FFI type. eye never sees its
// layout, so it is legal only behind a pointer/reference; codegen emits a
// forward typedef and no definition.
fn extern_type(p: &mut Parser) {
    let m = p.open();
    let kw = p.cursor_range(); // 'type' keyword - anchor for diagnostics
    p.advance(); // 'type'
    p.expect_after(
        SyntaxKind::Ident,
        kw,
        crate::SyntaxError::ExpectedExternTypeName,
    );
    let had_semi = p.eat(T![;]);
    if !had_semi {
        let span = TextRange::new(kw.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterExternType);
    }
    m.complete(p, SyntaxKind::ExternTypeDef);
}

fn field_list(p: &mut Parser, ctx: TextRange) {
    let m = p.open();
    let open_brace = p.cursor_range();
    let had_open = p.eat(T!['{']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedFieldListOpen);
    }
    while !p.at(T!['}']) && !p.at_eof() {
        // a field type starts with an ident, `&` (ref), `[` (array), or `(`
        // (function pointer).
        if p.at(SyntaxKind::Ident) || p.at(T![&]) || p.at(T!['[']) || p.at(T!['(']) {
            field(p);
            // the separating ',' is a child of fieldlist, not of field
            p.expect(T![,], crate::SyntaxError::ExpectedCommaAfterField);
        } else {
            // item keywords are sync points so an unclosed field-list `{`
            // cannot consume subsequent items as error nodes. after sync
            // the loop bails when no `,` follows -- either `}` (normal exit)
            // or an item keyword (the field list is unclosed).
            // EXPERIMENTAL - vamous
            p.sync(
                &[T![,], T!['}'], T![structure], T![union], T![enum], T![extern]],
                crate::SyntaxError::ExpectedField,
            );
            if !p.eat(T![,]) {
                break;
            }
        }
    }
    if !p.eat(T!['}']) {
        let range = if had_open {
            TextRange::new(open_brace.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedFieldListClose);
    }
    m.complete(p, SyntaxKind::FieldList);
}

fn field(p: &mut Parser) {
    let m = p.open();
    type_ref(p);
    let field_start = p.cursor_range();
    p.expect_after(
        SyntaxKind::Ident,
        field_start,
        crate::SyntaxError::ExpectedFieldName,
    );
    m.complete(p, SyntaxKind::Field);
}

fn enum_def(p: &mut Parser) {
    let m = p.open();
    let def_start = p.cursor_range(); // 'enum' - anchor for diagnostics
    p.advance(); // 'enum'
    p.expect_after(
        SyntaxKind::Ident,
        def_start,
        crate::SyntaxError::ExpectedEnumName,
    );
    let had_eq = p.eat(T![=]);

    // first variant. at least one variant required. leading `|` is always
    // optional - stylistic only, accepted inline or multi-line.
    let v_m = p.open();
    if p.at(T![|]) {
        p.advance();
    }
    let had_first_variant = p.at(SyntaxKind::Ident);
    if had_first_variant {
        p.advance(); // variant ident
        v_m.complete(p, SyntaxKind::Variant);
    } else {
        v_m.abandon(p);
    }

    // subsequent variants: '|' mandatory as a separator.
    let mut had_any_variant = had_first_variant;
    while p.at(T![|]) {
        let v_m = p.open();
        let pipe_range = p.cursor_range();
        p.advance(); // '|'
        p.expect_after(
            SyntaxKind::Ident,
            pipe_range,
            crate::SyntaxError::ExpectedVariantNameAfterPipe,
        );
        v_m.complete(p, SyntaxKind::Variant);
        had_any_variant = true;
    }

    let had_semi = p.eat(T![;]);

    if !had_eq {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedEqAfterEnumName);
    }
    if !had_any_variant {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedAtLeastOneVariant);
    }
    if !had_semi {
        let span = TextRange::new(def_start.start(), p.last_consumed_range().end());
        p.error_at(span, crate::SyntaxError::ExpectedSemiAfterEnum);
    }
    m.complete(p, SyntaxKind::EnumDef);
}

fn type_ref(p: &mut Parser) {
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
        // function type `(T, T) -> R`. a `(` in type position can only begin a
        // function type: eye has no tuple or parenthesized-group types. the
        // return arrow is optional (omitted = returns nothing), mirroring a
        // function declaration.
        let m = p.open();
        let open_paren = p.cursor_range();
        p.advance(); // '('
        while !p.at(T![')']) && !p.at_eof() {
            let param_m = p.open();
            type_ref(p);
            p.eat(T![,]); // optional separator; trailing comma allowed
            param_m.complete(p, SyntaxKind::FnTypeParam);
        }
        if !p.eat(T![')']) {
            let range = TextRange::new(open_paren.start(), p.last_consumed_range().end());
            p.error_at(range, crate::SyntaxError::ExpectedCloseParen);
        }
        if p.eat(T![->]) {
            type_ref(p); // return type
        }
        m.complete(p, SyntaxKind::FnType)
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
    // contextual effect annotations precede the fn name: `io render(...)`.
    // the keyword-less fn grammar makes `IDENT+ IDENT (` unambiguous - every
    // ident before the name (the one immediately followed by `(`) is an effect.
    // effect names are contextual: not reserved, validated downstream against
    // the atom set (EFFECT.md), so the parser accepts any ident sequence here.
    if p.at(SyntaxKind::Ident) && p.nth(1) == SyntaxKind::Ident {
        let em = p.open();
        while p.at(SyntaxKind::Ident) && p.nth(1) == SyntaxKind::Ident {
            p.advance(); // one effect annotation
        }
        em.complete(p, SyntaxKind::EffectList);
    }
    let ctx = p.cursor_range(); // function name - context for missing '('
    p.advance(); // function name
    param_list(p, ctx, false);
    if p.eat(T![->]) {
        type_ref(p);
    }
    let sig_range = TextRange::new(ctx.start(), p.cursor_range().end());
    block(p, sig_range);
    m.complete(p, SyntaxKind::FnDef);
}

/// `variadic_ok` is true only for an `extern` signature: `...` is a c-ABI
/// marker with no eye-side varargs access, so a defined fn cannot take it.
fn param_list(p: &mut Parser, ctx: TextRange, variadic_ok: bool) {
    let m = p.open();
    // `(` and `)` are separate tokens; an empty `()` is just a paramlist
    // with no params - unit is inferred from the absence of content
    let open_paren = p.cursor_range();
    let had_open = p.eat(T!['(']);
    if !had_open {
        p.error_at(ctx, crate::SyntaxError::ExpectedOpenParen);
    }
    let mut named_params = 0usize;
    while !p.at(T![')']) && !p.at_eof() {
        // when '(' is missing, item-level keywords are never valid params.
        // bail immediately so subsequent items aren't consumed as params.
        // EXPERIMENTAL - vamous
        if !had_open
            && matches!(p.nth0(), T![structure] | T![union] | T![enum] | T![extern])
        {
            break;
        }
        if p.at(T![...]) {
            let dots = p.cursor_range();
            let var_m = p.open();
            p.advance(); // '...'
            var_m.complete(p, SyntaxKind::Variadic);
            if !variadic_ok {
                p.error_at(dots, crate::GrammarError::VariadicOutsideExtern);
            } else if named_params == 0 {
                // the c calling convention needs a named parameter before
                // `...` (C99); the floor keeps that rule
                p.error_at(dots, crate::GrammarError::VariadicNeedsNamedParam);
            }
            if !p.at(T![')']) && !p.at_eof() {
                // one diagnostic for the whole region after `...`
                p.sync(&[T![')']], crate::GrammarError::VariadicNotLast);
            }
            break;
        }
        let param_m = p.open();
        type_ref(p);
        let param_start = p.cursor_range();
        p.expect_after(
            SyntaxKind::Ident,
            param_start,
            crate::SyntaxError::ExpectedParamName,
        );
        param_m.complete(p, SyntaxKind::Param);
        named_params += 1;
        if !p.eat(T![,]) {
            break;
        }
    }
    if !p.eat(T![')']) {
        let range = if had_open {
            TextRange::new(open_paren.start(), p.last_consumed_range().end())
        } else {
            p.cursor_range()
        };
        p.error_at(range, crate::SyntaxError::ExpectedCloseParen);
    }
    m.complete(p, SyntaxKind::ParamList);
}

fn block(p: &mut Parser, ctx: TextRange) {
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
            } else if p.at_eof() {
                // at EOF without `}` -- the block is unclosed. don't emit
                // "expected ;" because adding `}` would make this expression a
                // valid tail expression. `ExpectedBlockClose` is the root
                // cause. EXPERIMENTAL - vamous
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
            // fires below). EXPERIMENTAL - vamous
            p.sync(
                &[T![;], T!['}'], T![structure], T![union], T![enum], T![extern]],
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
/// - inferred: `let x = expr;` (ident then `=` -> no type)
/// - explicit: `mut Point p = expr;` (ident then ident)
/// - pointer: `mut Point* p = expr;` (ident then `*`)
/// - explicit ref: `mut &Point r = expr;` (leading `&`)
/// - explicit arr: `let [int32; 3] xs = expr;` (leading `[`)
///
/// nested refs are written with a space (`& &Point`); the `&&` spelling lexes
/// as a single logical-and token, so it cannot denote a ref-to-ref type.
fn let_stmt(p: &mut Parser) {
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
fn expr_stmt(p: &mut Parser) {
    let m = p.open();
    expr(p);
    p.expect(T![;], crate::SyntaxError::ExpectedSemiAfterExpr);
    m.complete(p, SyntaxKind::ExprStmt);
}

/// an expression. precedence is resolved by the pratt loop in [`expr_bp`];
/// [`lhs`] parses a prefix-unary form or an atom with its postfix forms.
fn expr(p: &mut Parser) {
    expr_bp(p, 0);
}

/// left/right binding power of an infix operator, or `None` if `kind` is not
/// one. most operators are left-associative (`l_bp < r_bp`). assignment is
/// right-associative (`l_bp > r_bp`) and has the lowest precedence.
fn infix_binding_power(kind: SyntaxKind) -> Option<(u8, u8)> {
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
fn is_comparison(kind: SyntaxKind) -> bool {
    matches!(kind, T![==] | T![!=] | T![<] | T![>] | T![<=] | T![>=])
}

/// right binding power of any prefix unary - above every infix operator, so
/// `-a * b` parses as `(-a) * b`.
const PREFIX_BP: u8 = 21;

/// pratt loop: parse a left-hand side, then fold in infix operators while
/// their left binding power is at least `min_bp`. each operator wraps the
/// already-parsed LHS via [`CompletedMarker::precede`], so the event buffer
/// stays append-only.
fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
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
fn lhs(p: &mut Parser) -> Option<CompletedMarker> {
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

fn if_expr(p: &mut Parser) -> CompletedMarker {
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

fn loop_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.open();
    let ctx = p.cursor_range(); // 'loop' keyword - context for missing body '{'
    p.advance(); // 'loop'
    block(p, ctx);
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

fn return_expr(p: &mut Parser) -> CompletedMarker {
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
fn match_expr(p: &mut Parser) -> CompletedMarker {
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

fn match_arm_list(p: &mut Parser, ctx: TextRange) {
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
fn match_arm(p: &mut Parser) -> bool {
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

/// true if `p` is at a token that can begin a pattern.
fn at_pat_start(p: &Parser) -> bool {
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
fn pat(p: &mut Parser) {
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
fn struct_pat(p: &mut Parser) {
    let m = p.open();
    let ctx = p.cursor_range(); // struct type - context for missing '{'
    let nm = p.open();
    p.advance(); // struct type Ident
    nm.complete(p, SyntaxKind::NameRef);
    struct_pat_field_list(p, ctx);
    m.complete(p, SyntaxKind::StructPat);
}

fn struct_pat_field_list(p: &mut Parser, ctx: TextRange) {
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
fn struct_pat_field(p: &mut Parser) {
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

/// `[a, b, c]` array literal. its own struct-lit context: a suppressed flag
/// from an enclosing if/loop condition does not apply inside the elements.
/// trailing comma is allowed.
fn array_lit(p: &mut Parser) -> CompletedMarker {
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
fn paren_expr(p: &mut Parser) -> CompletedMarker {
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

fn arg_list(p: &mut Parser) {
    let m = p.open();
    let open_paren = p.cursor_range();
    // span the full call expression when `(` is missing, not just the next
    // token. EXPERIMENTAL - vamous
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

fn struct_body(p: &mut Parser) {
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
