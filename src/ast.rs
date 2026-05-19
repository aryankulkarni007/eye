//! The typed AST — a thin, typed *view* over the lossless CST.
//!
//! The CST ([`SyntaxNode`]) is untyped: every node is the same Rust type and
//! the only thing distinguishing them is a [`SyntaxKind`] tag. That is what
//! makes it lossless and cheap to build, but it is miserable to walk — every
//! access is a `match` on a kind.
//!
//! This module layers typed wrappers on top. Each grammar node gets a
//! zero-cost newtype around the `SyntaxNode` it wraps; the wrapper exposes
//! named accessors (`.name()`, `.fields()`, …) instead of raw child iteration.
//! Nothing is copied — an [`AstNode`] is one `SyntaxNode` (an `Arc` handle),
//! so casting is a kind check and a move.
//!
//! The view is *partial and lazy*: accessors return `Option`/iterators and
//! recompute on every call. A malformed parse simply yields `None` for the
//! missing piece — the AST never has to be "valid", it only ever reflects
//! whatever the resilient parser produced.
//!
//! Grammar coverage mirrors [`crate::grammar`] exactly — the v0.1 subset.

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// The shared interface of every typed node: a checked downcast from the
/// untyped [`SyntaxNode`] and a borrow back to it.
pub trait AstNode {
    /// True if a node of this [`SyntaxKind`] can be cast to `Self`.
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;

    /// Downcast an untyped node. Returns `None` if the kind does not match.
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;

    /// The untyped node underneath — the escape hatch back to the CST.
    fn syntax(&self) -> &SyntaxNode;
}

/// Defines a typed wrapper for a single concrete node kind. The generated
/// newtype is one `SyntaxNode` wide; `cast` is a kind check.
macro_rules! ast_node {
    ($(#[$attr:meta])* $name:ident = $kind:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name {
            syntax: SyntaxNode,
        }

        impl AstNode for $name {
            #[inline]
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }

            #[inline]
            fn cast(syntax: SyntaxNode) -> Option<Self> {
                if Self::can_cast(syntax.kind()) {
                    Some(Self { syntax })
                } else {
                    None
                }
            }

            #[inline]
            fn syntax(&self) -> &SyntaxNode {
                &self.syntax
            }
        }
    };
}

/// Defines a typed *sum* over several node kinds — an `enum` whose `cast`
/// dispatches to the first variant that accepts the kind. The variant name
/// and its payload type are the same identifier, so the wrapped node types
/// must already exist (declared via [`ast_node!`]).
macro_rules! ast_enum {
    ($(#[$attr:meta])* $name:ident { $($variant:ident),* $(,)? }) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant($variant)),*
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                $(<$variant>::can_cast(kind))||*
            }

            fn cast(syntax: SyntaxNode) -> Option<Self> {
                let kind = syntax.kind();
                $(
                    if <$variant>::can_cast(kind) {
                        return <$variant>::cast(syntax).map(Self::$variant);
                    }
                )*
                None
            }

            fn syntax(&self) -> &SyntaxNode {
                match self {
                    $(Self::$variant(node) => node.syntax()),*
                }
            }
        }
    };
}

// ---- child-access helpers ----
//
// Every accessor below is one of these three calls. They recompute on each
// invocation — there is no caching — but each is a cheap cursor walk over a
// node's immediate children.

/// The first child node castable to `N`.
fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

/// Every child node castable to `N`, in source order.
fn children<N: AstNode>(parent: &SyntaxNode) -> impl Iterator<Item = N> {
    parent.children().filter_map(N::cast)
}

/// The first *direct* child token of exactly `kind`. Tokens nested inside a
/// child node are not direct children, so this never reaches into them.
fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

// ---- items ----

ast_node! {
    /// The whole file — the CST root. Cast a parse's `green` node to this.
    SourceFile = SourceFile
}

impl SourceFile {
    /// Every top-level item, in source order.
    pub fn items(&self) -> impl Iterator<Item = Item> {
        children(&self.syntax)
    }
}

ast_enum! {
    /// A top-level definition.
    Item { StructDef, FnDef }
}

ast_node! { StructDef = StructDef }

impl StructDef {
    /// The struct's name.
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }

    pub fn field_list(&self) -> Option<FieldList> {
        child(&self.syntax)
    }
}

ast_node! { FieldList = FieldList }

impl FieldList {
    pub fn fields(&self) -> impl Iterator<Item = Field> {
        children(&self.syntax)
    }
}

ast_node! { Field = Field }

impl Field {
    pub fn type_ref(&self) -> Option<TypeRef> {
        child(&self.syntax)
    }

    /// The field's name. The *type* is a nested [`TypeRef`] node, so the only
    /// direct `Ident` token is the name.
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }
}

ast_node! {
    /// A type position. The v0.1 subset has only bare-name types.
    TypeRef = TypeRef
}

impl TypeRef {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }
}

ast_node! { FnDef = FnDef }

impl FnDef {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }

    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.syntax)
    }

    pub fn body(&self) -> Option<Block> {
        child(&self.syntax)
    }
}

ast_node! {
    /// A parameter list. Always empty in the v0.1 subset — kept as a node so
    /// the accessor and the grammar stay one-to-one.
    ParamList = ParamList
}

// ---- statements ----

ast_node! { Block = Block }

impl Block {
    pub fn stmts(&self) -> impl Iterator<Item = Stmt> {
        children(&self.syntax)
    }
}

ast_enum! {
    /// A statement inside a [`Block`].
    Stmt { LetStmt, ExprStmt }
}

/// Whether a binding is immutable (`const`) or mutable (`var`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LetKind {
    Const,
    Var,
}

ast_node! { LetStmt = LetStmt }

impl LetStmt {
    /// `const` vs `var` — the leading keyword.
    pub fn kind(&self) -> Option<LetKind> {
        self.syntax
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| match t.kind() {
                SyntaxKind::Const => Some(LetKind::Const),
                SyntaxKind::Var => Some(LetKind::Var),
                _ => None,
            })
    }

    /// The optional explicit type annotation.
    pub fn type_ref(&self) -> Option<TypeRef> {
        child(&self.syntax)
    }

    /// The bound name. The annotation, if present, is a nested [`TypeRef`],
    /// so the only direct `Ident` token is the name.
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }

    /// The right-hand side expression.
    pub fn value(&self) -> Option<Expr> {
        child(&self.syntax)
    }
}

ast_node! { ExprStmt = ExprStmt }

impl ExprStmt {
    pub fn expr(&self) -> Option<Expr> {
        child(&self.syntax)
    }
}

// ---- expressions ----

ast_enum! {
    /// Any expression form in the v0.1 subset.
    Expr { Literal, NameRef, CallExpr, StructLit }
}

/// Which kind of literal a [`Literal`] node holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiteralKind {
    Int,
    Float,
    String,
    Bool,
    Char,
}

ast_node! { Literal = Literal }

impl Literal {
    /// The single literal token. Leading trivia can land inside the node, so
    /// this skips trivia rather than taking the first token blindly.
    pub fn token(&self) -> Option<SyntaxToken> {
        self.syntax
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
    }

    /// The literal's category, derived from its token kind.
    pub fn literal_kind(&self) -> Option<LiteralKind> {
        Some(match self.token()?.kind() {
            SyntaxKind::Int => LiteralKind::Int,
            SyntaxKind::Float => LiteralKind::Float,
            SyntaxKind::String => LiteralKind::String,
            SyntaxKind::True | SyntaxKind::False => LiteralKind::Bool,
            SyntaxKind::Char => LiteralKind::Char,
            _ => return None,
        })
    }
}

ast_node! {
    /// A reference to a name — a use of an identifier as an expression.
    NameRef = NameRef
}

impl NameRef {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }
}

ast_node! { CallExpr = CallExpr }

impl CallExpr {
    /// The callee — the expression being applied.
    pub fn callee(&self) -> Option<Expr> {
        child(&self.syntax)
    }

    pub fn arg_list(&self) -> Option<ArgList> {
        child(&self.syntax)
    }
}

ast_node! { ArgList = ArgList }

impl ArgList {
    pub fn args(&self) -> impl Iterator<Item = Expr> {
        children(&self.syntax)
    }
}

ast_node! { StructLit = StructLit }

impl StructLit {
    /// The struct name being constructed.
    pub fn name_ref(&self) -> Option<NameRef> {
        child(&self.syntax)
    }

    pub fn field_list(&self) -> Option<StructLitFieldList> {
        child(&self.syntax)
    }
}

ast_node! { StructLitFieldList = StructLitFieldList }

impl StructLitFieldList {
    pub fn fields(&self) -> impl Iterator<Item = StructLitField> {
        children(&self.syntax)
    }
}

ast_node! {
    /// A field initializer in a struct literal. The v0.1 subset only supports
    /// the shorthand form (`Point { x, y }`), so a field is just a name.
    StructLitField = StructLitField
}

impl StructLitField {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.syntax, SyntaxKind::Ident)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{Lexer, SourceText};

    /// Lex + parse `src` and cast the CST root to a typed [`SourceFile`].
    fn source_file(src: &str) -> SourceFile {
        let source = SourceText::new(src.to_string());
        let tokens = Lexer::new(&source).tokenize().tokens;
        let parse = crate::parser::parse(&tokens, &source);
        SourceFile::cast(parse.green).expect("root is a SourceFile")
    }

    /// The canonical `main.eye` program — exercises every v0.1 node kind.
    const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    const x = 0;
    const y = 0;
    var Point p = Point { x, y };

    print(\"{}\", p);
}
";

    #[test]
    fn struct_def_fields() {
        let file = source_file(MAIN_EYE);
        let items: Vec<_> = file.items().collect();
        assert_eq!(items.len(), 2);

        let Item::StructDef(s) = &items[0] else {
            panic!("first item is a struct");
        };
        assert_eq!(s.name().unwrap().text(), "Point");

        let fields: Vec<_> = s.field_list().unwrap().fields().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(
            fields[0].type_ref().unwrap().name().unwrap().text(),
            "int32"
        );
        assert_eq!(fields[0].name().unwrap().text(), "x");
        assert_eq!(fields[1].name().unwrap().text(), "y");
    }

    #[test]
    fn fn_def_body() {
        let file = source_file(MAIN_EYE);
        let Item::FnDef(f) = file.items().nth(1).unwrap() else {
            panic!("second item is a function");
        };
        assert_eq!(f.name().unwrap().text(), "main");
        assert!(f.param_list().is_some());

        let stmts: Vec<_> = f.body().unwrap().stmts().collect();
        assert_eq!(stmts.len(), 4);
    }

    #[test]
    fn let_stmt_shapes() {
        let file = source_file(MAIN_EYE);
        let Item::FnDef(f) = file.items().nth(1).unwrap() else {
            panic!("expected a function");
        };
        let stmts: Vec<_> = f.body().unwrap().stmts().collect();

        // `const x = 0;` — inferred type, no annotation
        let Stmt::LetStmt(x) = &stmts[0] else {
            panic!("first stmt is a let");
        };
        assert_eq!(x.kind(), Some(LetKind::Const));
        assert!(x.type_ref().is_none());
        assert_eq!(x.name().unwrap().text(), "x");
        let Some(Expr::Literal(lit)) = x.value() else {
            panic!("value is a literal");
        };
        assert_eq!(lit.literal_kind(), Some(LiteralKind::Int));

        // `var Point p = Point { x, y };` — explicit type, struct literal
        let Stmt::LetStmt(p) = &stmts[2] else {
            panic!("third stmt is a let");
        };
        assert_eq!(p.kind(), Some(LetKind::Var));
        assert_eq!(p.type_ref().unwrap().name().unwrap().text(), "Point");
        assert_eq!(p.name().unwrap().text(), "p");
        let Some(Expr::StructLit(sl)) = p.value() else {
            panic!("value is a struct literal");
        };
        assert_eq!(sl.name_ref().unwrap().name().unwrap().text(), "Point");
        let lit_fields: Vec<_> = sl.field_list().unwrap().fields().collect();
        assert_eq!(lit_fields.len(), 2);
        assert_eq!(lit_fields[0].name().unwrap().text(), "x");
    }

    #[test]
    fn call_expr_args() {
        let file = source_file(MAIN_EYE);
        let Item::FnDef(f) = file.items().nth(1).unwrap() else {
            panic!("expected a function");
        };
        let stmts: Vec<_> = f.body().unwrap().stmts().collect();

        // `print("{}", p);`
        let Stmt::ExprStmt(es) = &stmts[3] else {
            panic!("last stmt is an expr stmt");
        };
        let Some(Expr::CallExpr(call)) = es.expr() else {
            panic!("expr is a call");
        };
        let Some(Expr::NameRef(callee)) = call.callee() else {
            panic!("callee is a name");
        };
        assert_eq!(callee.name().unwrap().text(), "print");

        let args: Vec<_> = call.arg_list().unwrap().args().collect();
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], Expr::Literal(_)));
        assert!(matches!(args[1], Expr::NameRef(_)));
    }
}
