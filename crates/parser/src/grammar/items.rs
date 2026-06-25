//! item grammar: the top-level `source_file` driver and each item form -
//! const / global, struct / union (+ field list), the `extern` FFI block, enum,
//! and `fn` (+ its param list).

use super::*;
use crate::Parser;
use syntax::{SyntaxKind, T};
use text_size::TextRange;

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
pub(crate) fn const_def(p: &mut Parser) {
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
pub(crate) fn global_def(p: &mut Parser) {
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

pub(crate) fn struct_def(p: &mut Parser) {
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
pub(crate) fn union_def(p: &mut Parser) {
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
pub(crate) fn extern_block(p: &mut Parser) {
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
pub(crate) fn extern_fn(p: &mut Parser) {
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
pub(crate) fn extern_type(p: &mut Parser) {
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

pub(crate) fn field_list(p: &mut Parser, ctx: TextRange) {
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
            p.sync(
                &[
                    T![,],
                    T!['}'],
                    T![structure],
                    T![union],
                    T![enum],
                    T![extern],
                ],
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

pub(crate) fn field(p: &mut Parser) {
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

pub(crate) fn enum_def(p: &mut Parser) {
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

pub(crate) fn fn_def(p: &mut Parser) {
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
pub(crate) fn param_list(p: &mut Parser, ctx: TextRange, variadic_ok: bool) {
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
        if !had_open && matches!(p.nth0(), T![structure] | T![union] | T![enum] | T![extern]) {
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
