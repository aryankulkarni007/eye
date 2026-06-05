//! AST type and literal lowering helpers.

use ast::AstNode;
use diagnostics::Sink;
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

use super::const_eval::{ConstEnv, fold_const_length};
use crate::core::{ConstError, HirError, Literal, TypeInterner, TypeKind, TypeRef};

pub(super) fn lower_type_ref(
    ty: &ast::TypeRef,
    diagnostics: &mut Sink<HirError>,
    consts: &dyn ConstEnv,
    types: &mut TypeInterner,
) -> TypeRef {
    match ty {
        ast::TypeRef::IdentType(it) => match it.name() {
            Some(t) => types.intern(TypeKind::Path(SmolStr::from(t.text()))),
            None => types.intern(TypeKind::Error),
        },
        ast::TypeRef::RefType(rt) => {
            let inner = rt
                .inner()
                .map(|t| lower_type_ref(&t, diagnostics, consts, types))
                .unwrap_or_else(|| types.intern(TypeKind::Error));
            types.intern(TypeKind::Ref(inner))
        }
        ast::TypeRef::PtrType(pt) => {
            let inner = pt
                .inner()
                .map(|t| lower_type_ref(&t, diagnostics, consts, types))
                .unwrap_or_else(|| types.intern(TypeKind::Error));
            types.intern(TypeKind::Ptr(inner))
        }
        ast::TypeRef::ArrayType(at) => {
            let elem = at
                .elem()
                .map(|t| lower_type_ref(&t, diagnostics, consts, types))
                .unwrap_or_else(|| types.intern(TypeKind::Error));
            let Some(len) = array_len(at.len(), diagnostics, consts) else {
                return types.intern(TypeKind::Error);
            };
            types.intern(TypeKind::Array { elem, len })
        }
        ast::TypeRef::FnType(ft) => {
            let params = ft
                .params()
                .map(|p| {
                    p.ty()
                        .map(|t| lower_type_ref(&t, diagnostics, consts, types))
                        .unwrap_or_else(|| types.intern(TypeKind::Error))
                })
                .collect();
            let ret = ft
                .ret_type()
                .map(|t| lower_type_ref(&t, diagnostics, consts, types));
            types.intern(TypeKind::Fn { params, ret })
        }
    }
}

/// Extract an array length from the `[T; N]` length slot. `N` is a bounded
/// const-expr: an integer literal, a `const` reference, or arithmetic over
/// those (A6). A non-const length, a non-integer const, or zero is a
/// [`ConstError`].
pub(super) fn array_len(
    len: Option<ast::Expr>,
    diagnostics: &mut Sink<HirError>,
    consts: &dyn ConstEnv,
) -> Option<u64> {
    // Fast path: a bare integer literal needs no const map (and gives the
    // ArrayLenZero / ArrayLenTooLarge diagnostics on the literal itself).
    let expr = len?;
    if let ast::Expr::Literal(lit) = &expr
        && matches!(lit.literal_kind(), Some(ast::LiteralKind::Int))
    {
        let token = lit.token()?;
        return match parse_int_literal(token.text()) {
            Some(0) => {
                diagnostics.emit(
                    SyntaxNodePtr::new(lit.syntax()),
                    HirError::Const(ConstError::ArrayLenZero),
                );
                None
            }
            Some(v) => u64::try_from(v).ok().or_else(|| {
                diagnostics.emit(
                    SyntaxNodePtr::new(lit.syntax()),
                    HirError::Const(ConstError::ArrayLenTooLarge),
                );
                None
            }),
            None => {
                diagnostics.emit(
                    SyntaxNodePtr::new(lit.syntax()),
                    HirError::Const(ConstError::ArrayLenTooLarge),
                );
                None
            }
        };
    }
    // General path: fold a const-expr against the finished const map.
    match fold_const_length(&expr, consts, diagnostics) {
        Some(0) => {
            diagnostics.emit(
                SyntaxNodePtr::new(expr.syntax()),
                HirError::Const(ConstError::ArrayLenZero),
            );
            None
        }
        other => other,
    }
}

/// Parse an integer literal's text into its value, honoring a base prefix:
/// `0x`/`0X` hex, `0b`/`0B` binary, `0o`/`0O` octal, decimal otherwise. The
/// lexer regex already constrains the digit set per base, so the only failure
/// reachable here is overflow of `u128`. Shared by every int-literal parse site
/// (`lower_literal` and array lengths).
pub(super) fn parse_int_literal(text: &str) -> Option<u128> {
    let (radix, digits) = match text.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, &text[2..]),
        [b'0', b'b' | b'B', ..] => (2, &text[2..]),
        [b'0', b'o' | b'O', ..] => (8, &text[2..]),
        _ => (10, text),
    };
    u128::from_str_radix(digits, radix).ok()
}

pub(super) fn literal_type(lit: &Literal, types: &mut TypeInterner) -> TypeRef {
    match lit {
        Literal::Int(_) => types.intern(TypeKind::Path(SmolStr::new_static("int32"))),
        Literal::Float(_) => types.intern(TypeKind::Path(SmolStr::new_static("float64"))),
        // A string literal is `&[uint8; N]` (HORIZON0 C3): a reference to a fixed
        // byte array, `N` the *decoded* byte count (escapes expanded, NUL
        // excluded). This reuses the array machine (`len`, indexing, OOB) and
        // gives FFI a real pointer; codegen backs it with a NUL-terminated static
        // of the same decoded bytes, so `N` here and the static agree.
        Literal::String(s) => {
            let uint8 = types.intern(TypeKind::Path(SmolStr::new_static("uint8")));
            let arr = types.intern(TypeKind::Array {
                elem: uint8,
                len: crate::core::decode_string_literal(s).len() as u64,
            });
            types.intern(TypeKind::Ref(arr))
        }
        Literal::Bool(_) => types.intern(TypeKind::Path(SmolStr::new_static("bool"))),
        Literal::Char(_) => types.intern(TypeKind::Path(SmolStr::new_static("char"))),
    }
}

pub(super) fn lower_literal(lit: &ast::Literal) -> Literal {
    let Some(token) = lit.token() else {
        return Literal::Int(0);
    };
    let text = token.text();
    match lit.literal_kind() {
        Some(ast::LiteralKind::Int) => parse_int_literal(text)
            .map(Literal::Int)
            .unwrap_or(Literal::Int(0)),
        Some(ast::LiteralKind::Float) => Literal::Float(SmolStr::from(text)),
        Some(ast::LiteralKind::String) => {
            // strip surrounding double quotes; escapes left raw for v0.1
            let s = text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(text);
            Literal::String(SmolStr::from(s))
        }
        Some(ast::LiteralKind::Bool) => Literal::Bool(text == "true"),
        Some(ast::LiteralKind::Char) => {
            let inner = text
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(text);
            // Decode escapes (`'\n'`, `'\t'`, ...) so the stored char is the real
            // byte, not the backslash; codegen re-escapes on the way to C.
            Literal::Char(crate::core::decode_char_literal(inner))
        }
        None => Literal::Int(0),
    }
}
