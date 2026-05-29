//! CST-guided classification for identifiers and type names.

use ast::{
    AstNode, Block, ExternFn, Expr, Item, MatchArm, MatchExpr, SourceFile, Stmt, TypeRef,
};
use syntax::{SyntaxNode, SyntaxToken};
use text_size::TextRange;

use crate::legend;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClassifiedSpan {
    range: TextRange,
    token_type: u32,
}

pub(crate) fn classify_spans(root: &SyntaxNode) -> Vec<ClassifiedSpan> {
    let mut spans = Vec::new();
    let Some(file) = SourceFile::cast(root.clone()) else {
        return spans;
    };

    for item in file.items() {
        classify_item(&item, &mut spans);
    }
    spans
}

fn classify_item(item: &Item, spans: &mut Vec<ClassifiedSpan>) {
    match item {
        Item::StructDef(s) => {
            push_token(s.name(), legend::STRUCT, spans);
            for field in s.field_list().into_iter().flat_map(|fl| fl.fields()) {
                push_token(field.name(), legend::PROPERTY, spans);
            }
        }
        Item::EnumDef(e) => {
            push_token(e.name(), legend::ENUM, spans);
            for variant in e.variants() {
                push_token(variant.name(), legend::ENUM_MEMBER, spans);
            }
        }
        Item::UnionDef(u) => {
            push_token(u.name(), legend::STRUCT, spans);
            for field in u.field_list().into_iter().flat_map(|fl| fl.fields()) {
                push_token(field.name(), legend::PROPERTY, spans);
            }
        }
        Item::FnDef(f) => {
            push_token(f.name(), legend::FUNCTION, spans);
            if let Some(pl) = f.param_list() {
                for param in pl.params() {
                    push_token(param.name(), legend::PARAMETER, spans);
                }
            }
            if let Some(body) = f.body() {
                classify_block(&body, spans);
            }
        }
        Item::ExternBlock(eb) => {
            for ef in eb.fns() {
                classify_extern_fn(&ef, spans);
            }
        }
    }
}

fn classify_extern_fn(ef: &ExternFn, spans: &mut Vec<ClassifiedSpan>) {
    push_token(ef.name(), legend::FUNCTION, spans);
    if let Some(pl) = ef.param_list() {
        for param in pl.params() {
            push_token(param.name(), legend::PARAMETER, spans);
        }
    }
}

fn classify_block(block: &Block, spans: &mut Vec<ClassifiedSpan>) {
    for stmt in block.stmts() {
        classify_stmt(&stmt, spans);
    }
    if let Some(expr) = block.tail_expr() {
        classify_expr(&expr, spans);
    }
}

fn classify_stmt(stmt: &Stmt, spans: &mut Vec<ClassifiedSpan>) {
    match stmt {
        Stmt::LetStmt(l) => {
            push_token(l.name(), legend::VARIABLE, spans);
            if let Some(ty) = l.ty() {
                classify_type_ref(&ty, spans);
            }
            if let Some(init) = l.value() {
                classify_expr(&init, spans);
            }
        }
        Stmt::ExprStmt(e) => {
            if let Some(expr) = e.expr() {
                classify_expr(&expr, spans);
            }
        }
    }
}

fn classify_type_ref(ty: &TypeRef, spans: &mut Vec<ClassifiedSpan>) {
    match ty {
        TypeRef::IdentType(it) => push_token(it.name(), legend::TYPE, spans),
        TypeRef::RefType(rt) => {
            if let Some(inner) = rt.inner() {
                classify_type_ref(&inner, spans);
            }
        }
        TypeRef::PtrType(pt) => {
            if let Some(inner) = pt.inner() {
                classify_type_ref(&inner, spans);
            }
        }
        TypeRef::ArrayType(at) => {
            if let Some(elem) = at.elem() {
                classify_type_ref(&elem, spans);
            }
        }
    }
}

fn classify_expr(expr: &Expr, spans: &mut Vec<ClassifiedSpan>) {
    match expr {
        Expr::FieldExpr(f) => {
            if let Some(nr) = f.name_ref() {
                push_token(nr.name(), legend::PROPERTY, spans);
            }
            if let Some(base) = f.expr() {
                classify_expr(&base, spans);
            }
        }
        Expr::StructLit(sl) => {
            if let Some(nr) = sl.name_ref() {
                push_token(nr.name(), legend::TYPE, spans);
            }
            if let Some(fl) = sl.field_list() {
                for field in fl.fields() {
                    push_token(field.name(), legend::PROPERTY, spans);
                    if let Some(val) = field.value() {
                        classify_expr(&val, spans);
                    }
                }
            }
        }
        Expr::CallExpr(c) => {
            if let Some(callee) = c.callee() {
                classify_expr(&callee, spans);
            }
            if let Some(al) = c.arg_list() {
                for arg in al.args() {
                    classify_expr(&arg, spans);
                }
            }
        }
        Expr::MatchExpr(m) => classify_match(m, spans),
        Expr::IfExpr(i) => {
            if let Some(cond) = i.condition() {
                classify_expr(&cond, spans);
            }
            if let Some(then_b) = i.then_branch() {
                classify_block(&then_b, spans);
            }
            if let Some(else_b) = i.else_branch() {
                classify_block(&else_b, spans);
            }
        }
        Expr::LoopExpr(l) => {
            if let Some(body) = l.body() {
                classify_block(&body, spans);
            }
        }
        Expr::AssignExpr(a) => {
            if let Some(lhs) = a.lhs() {
                classify_expr(&lhs, spans);
            }
            if let Some(rhs) = a.rhs() {
                classify_expr(&rhs, spans);
            }
        }
        Expr::BinExpr(b) => {
            if let Some(lhs) = b.lhs() {
                classify_expr(&lhs, spans);
            }
            if let Some(rhs) = b.rhs() {
                classify_expr(&rhs, spans);
            }
        }
        Expr::PrefixExpr(p) => {
            if let Some(op) = p.operand() {
                classify_expr(&op, spans);
            }
        }
        Expr::RefExpr(r) => {
            if let Some(inner) = r.expr() {
                classify_expr(&inner, spans);
            }
        }
        Expr::DerefExpr(d) => {
            if let Some(inner) = d.expr() {
                classify_expr(&inner, spans);
            }
        }
        Expr::CastExpr(c) => {
            if let Some(operand) = c.operand() {
                classify_expr(&operand, spans);
            }
            if let Some(ty) = c.ty() {
                classify_type_ref(&ty, spans);
            }
        }
        Expr::IndexExpr(ie) => {
            if let Some(base) = ie.base() {
                classify_expr(&base, spans);
            }
            if let Some(index) = ie.index() {
                classify_expr(&index, spans);
            }
        }
        Expr::ArrayLit(al) => {
            for elem in al.elems() {
                classify_expr(&elem, spans);
            }
        }
        _ => {}
    }
}

fn classify_match(m: &MatchExpr, spans: &mut Vec<ClassifiedSpan>) {
    if let Some(scrut) = m.scrut() {
        classify_expr(&scrut, spans);
    }
    if let Some(list) = m.arm_list() {
        for arm in list.arms() {
            classify_match_arm(&arm, spans);
        }
    }
}

fn classify_match_arm(arm: &MatchArm, spans: &mut Vec<ClassifiedSpan>) {
    if let Some(pat) = arm.pat() {
        classify_pat(&pat, spans);
    }
    if let Some(body) = arm.body() {
        classify_expr(&body, spans);
    }
}

fn classify_pat(pat: &ast::Pat, spans: &mut Vec<ClassifiedSpan>) {
    match pat {
        ast::Pat::PathPat(pp) => {
            if let Some(q) = pp.qualifier() {
                push_token(q.name(), legend::ENUM, spans);
            }
            if let Some(n) = pp.name() {
                push_token(n.name(), legend::ENUM_MEMBER, spans);
            }
        }
        ast::Pat::BareIdentPat(bp) => {
            if let Some(n) = bp.name() {
                push_token(n.name(), legend::ENUM_MEMBER, spans);
            }
        }
        ast::Pat::WildcardPat(_) => {}
    }
}

fn push_token(token: Option<SyntaxToken>, token_type: u32, spans: &mut Vec<ClassifiedSpan>) {
    let Some(token) = token else {
        return;
    };
    spans.push(ClassifiedSpan {
        range: token.text_range(),
        token_type,
    });
}

pub(crate) fn lookup_ident(range: TextRange, spans: &[ClassifiedSpan]) -> Option<u32> {
    spans
        .iter()
        .rev()
        .find(|s| s.range == range)
        .map(|s| s.token_type)
}
