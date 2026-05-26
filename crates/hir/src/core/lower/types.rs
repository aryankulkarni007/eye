//! AST type and literal lowering helpers.

use ast::AstNode;
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

use crate::core::{HirDiagnostic, Literal, TypeRef};

pub(super) fn lower_type_ref(ty: &ast::TypeRef, diagnostics: &mut Vec<HirDiagnostic>) -> TypeRef {
    match ty {
        ast::TypeRef::IdentType(it) => match it.name() {
            Some(t) => TypeRef::Path(SmolStr::from(t.text())),
            None => TypeRef::Error,
        },
        ast::TypeRef::RefType(rt) => {
            let inner = rt
                .inner()
                .map(|t| lower_type_ref(&t, diagnostics))
                .unwrap_or(TypeRef::Error);
            TypeRef::Ref(Box::new(inner))
        }
        ast::TypeRef::PtrType(pt) => {
            let inner = pt
                .inner()
                .map(|t| lower_type_ref(&t, diagnostics))
                .unwrap_or(TypeRef::Error);
            TypeRef::Ptr(Box::new(inner))
        }
        ast::TypeRef::ArrayType(at) => {
            let elem = at
                .elem()
                .map(|t| lower_type_ref(&t, diagnostics))
                .unwrap_or(TypeRef::Error);
            let Some(len) = array_len(at.len(), diagnostics) else {
                return TypeRef::Error;
            };
            TypeRef::Array {
                elem: Box::new(elem),
                len,
            }
        }
    }
}

/// Extract an array length from the `[T; N]` length slot. Restricted to an
/// integer literal for now.
fn array_len(len: Option<ast::Expr>, diagnostics: &mut Vec<HirDiagnostic>) -> Option<u64> {
    let lit = match len {
        Some(ast::Expr::Literal(lit)) => lit,
        Some(expr) => {
            // FIXME: change when we allow compile time constant length arrays.
            diagnostics.push(HirDiagnostic {
                ptr: SyntaxNodePtr::new(expr.syntax()),
                msg: "array length must be an integer literal".to_string(),
            });
            return None;
        }
        None => return None,
    };
    if !matches!(lit.literal_kind(), Some(ast::LiteralKind::Int)) {
        // FIXME: change when we allow compile time constant length arrays.
        diagnostics.push(HirDiagnostic {
            ptr: SyntaxNodePtr::new(lit.syntax()),
            msg: "array length must be an integer literal".to_string(),
        });
        return None;
    }
    let token = lit.token()?;
    match token.text().parse::<u64>() {
        Ok(len) => Some(len),
        Err(_) => {
            diagnostics.push(HirDiagnostic {
                ptr: SyntaxNodePtr::new(lit.syntax()),
                msg: "array length integer literal is too large".to_string(),
            });
            None
        }
    }
}

pub(super) fn literal_type(lit: &Literal) -> TypeRef {
    match lit {
        Literal::Int(_) => TypeRef::Path(SmolStr::new_static("int32")),
        Literal::Float(_) => TypeRef::Path(SmolStr::new_static("float64")),
        Literal::String(_) => TypeRef::Path(SmolStr::new_static("string")),
        Literal::Bool(_) => TypeRef::Path(SmolStr::new_static("bool")),
        Literal::Char(_) => TypeRef::Path(SmolStr::new_static("char")),
    }
}

pub(super) fn lower_literal(lit: &ast::Literal) -> Literal {
    let Some(token) = lit.token() else {
        return Literal::Int(0);
    };
    let text = token.text();
    match lit.literal_kind() {
        Some(ast::LiteralKind::Int) => text
            .parse::<u128>()
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
            Literal::Char(inner.chars().next().unwrap_or('\0'))
        }
        None => Literal::Int(0),
    }
}
