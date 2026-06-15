//! the typed AST - a thin, typed *view* over the lossless CST.
//!
//! the CST ([`SyntaxNode`]) is untyped: every node is the same rust type and
//! the only thing distinguishing them is a [`SyntaxKind`] tag. that is what
//! makes it lossless and cheap to build, but it is miserable to walk - every
//! access is a `match` on a kind.
//!
//! this module layers typed wrappers on top. each grammar node gets a
//! zero-cost newtype around the `SyntaxNode` it wraps; the wrapper exposes
//! named accessors (`.name()`, `.fields()`, …) instead of raw child iteration.
//! nothing is copied - an [`AstNode`] is one `SyntaxNode` (an `Arc` handle),
//! so casting is a kind check and a move.
//!
//! ## generated vs. hand-written
//!
//! the structural layer - every node/enum struct and its child accessors -
//! is **generated** from `eye.ungram` into [`generated`] by `cargo xtask
//! codegen`. this module hand-writes only what a structural generator cannot
//! derive: the [`AstNode`] trait, the [`support`] helpers, and the four
//! semantic accessors ([`LetStmt::kind`], [`BinExpr::op`], [`PrefixExpr::op`],
//! [`Literal::literal_kind`]) plus their operator/kind enums.
//!
//! the view is *partial and lazy*: accessors return `Option`/iterators and
//! recompute on every call. a malformed parse simply yields `None` for the
//! missing piece.

use std::marker::PhantomData;

use syntax::{SyntaxKind, SyntaxNode, SyntaxNodeChildren, SyntaxToken, T};

mod generated;
pub use generated::*;

/// the shared interface of every typed node: a checked downcast from the
/// untyped [`SyntaxNode`] and a borrow back to it.
pub trait AstNode {
    /// true if a node of this [`SyntaxKind`] can be cast to `Self`.
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;

    /// downcast an untyped node. returns `None` if the kind does not match.
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;

    /// the untyped node underneath - the escape hatch back to the CST.
    fn syntax(&self) -> &SyntaxNode;
}

/// a lazy iterator over the children of a node castable to `N`. the named
/// type lets generated accessor signatures stay concrete.
pub struct AstChildren<N> {
    inner: SyntaxNodeChildren,
    ph: PhantomData<N>,
}

impl<N> AstChildren<N> {
    fn new(parent: &SyntaxNode) -> Self {
        AstChildren {
            inner: parent.children(),
            ph: PhantomData,
        }
    }
}

impl<N: AstNode> Iterator for AstChildren<N> {
    type Item = N;

    fn next(&mut self) -> Option<N> {
        self.inner.by_ref().find_map(N::cast)
    }
}

/// child-access helpers the generated accessors are built from. each is a
/// cheap cursor walk over a node's immediate children - recomputed on every
/// call, never cached.
pub mod support {
    use super::{AstChildren, AstNode};
    use syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

    /// the first child node castable to `N`.
    pub fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
        parent.children().find_map(N::cast)
    }

    /// every child node castable to `N`, in source order.
    pub fn children<N: AstNode>(parent: &SyntaxNode) -> AstChildren<N> {
        AstChildren::new(parent)
    }

    /// the first *direct* child token of exactly `kind`. tokens nested inside
    /// a child node are not direct children, so this never reaches into them.
    pub fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
        parent
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == kind)
    }
}

// ---- hand-written semantic accessors ----
//
// the structural generator emits child/token accessors; it cannot derive
// meaning. the four nodes below carry a category that lives in a token kind:
// these `impl` blocks layer that on top of the generated structs.

/// whether a binding is immutable (`let`) or mutable (`mut`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LetKind {
    Let,
    Mut,
}

impl LetStmt {
    /// `let` vs `mut` - the leading keyword.
    pub fn kind(&self) -> Option<LetKind> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| match t.kind() {
                T![let] => Some(LetKind::Let),
                T![mut] => Some(LetKind::Mut),
                _ => None,
            })
    }
}

impl GlobalDef {
    /// `let` (read-only static) vs `mut` (mutable static) - the leading keyword.
    pub fn kind(&self) -> Option<LetKind> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| match t.kind() {
                T![let] => Some(LetKind::Let),
                T![mut] => Some(LetKind::Mut),
                _ => None,
            })
    }
}

/// which kind of literal a [`Literal`] node holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiteralKind {
    Int,
    Float,
    String,
    Bool,
    Char,
}

impl Literal {
    /// the single literal token. leading trivia can land inside the node, so
    /// this skips trivia rather than taking the first token blindly.
    pub fn token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
    }

    /// the literal's category, derived from its token kind.
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

/// a binary operator. mirrors the operator token kinds the grammar folds into
/// a [`BinExpr`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Eq,
    Neq,
    Lt,
    Gt,
    Leq,
    Geq,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl BinOp {
    /// the operator a token kind denotes, or `None` for a non-operator kind.
    fn from_kind(kind: SyntaxKind) -> Option<BinOp> {
        Some(match kind {
            T![+] => BinOp::Add,
            T![-] => BinOp::Sub,
            T![*] => BinOp::Mul,
            T![/] => BinOp::Div,
            T![%] => BinOp::Rem,
            T![&&] => BinOp::And,
            T![||] => BinOp::Or,
            T![==] => BinOp::Eq,
            T![!=] => BinOp::Neq,
            T![<] => BinOp::Lt,
            T![>] => BinOp::Gt,
            T![<=] => BinOp::Leq,
            T![>=] => BinOp::Geq,
            T![&] => BinOp::BitAnd,
            T![|] => BinOp::BitOr,
            T![^] => BinOp::BitXor,
            T![<<] => BinOp::Shl,
            T![>>] => BinOp::Shr,
            _ => return None,
        })
    }
}

use std::fmt;

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op_str = match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Eq => "==",
            BinOp::Neq => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Leq => "<=",
            BinOp::Geq => ">=",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        };
        write!(f, "{}", op_str)
    }
}

impl BinExpr {
    /// the operator token - the direct child token between the two operands.
    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| BinOp::from_kind(t.kind()).is_some())
    }

    /// the operator.
    pub fn op(&self) -> Option<BinOp> {
        BinOp::from_kind(self.op_token()?.kind())
    }
}

/// a prefix-unary operator: `-` negate, `~` bitwise-complement, `!` logical-not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

impl UnaryOp {
    /// the prefix operator a token kind denotes, or `None` otherwise.
    fn from_kind(kind: SyntaxKind) -> Option<UnaryOp> {
        Some(match kind {
            T![-] => UnaryOp::Neg,
            T![!] => UnaryOp::Not,
            T![~] => UnaryOp::BitNot,
            _ => return None,
        })
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op_str = match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
        };
        write!(f, "{}", op_str)
    }
}

impl PrefixExpr {
    /// the operator - whichever prefix token leads the expression.
    pub fn op(&self) -> Option<UnaryOp> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| UnaryOp::from_kind(t.kind()))
    }
}

/// an assignment operator: plain `=` or a compound form (`+=`, `-=`, `*=`,
/// `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    RemAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    ShlAssign,
    ShrAssign,
}

impl AssignOp {
    /// the assignment operator a token kind denotes, or `None` otherwise.
    fn from_kind(kind: SyntaxKind) -> Option<AssignOp> {
        Some(match kind {
            T![=] => AssignOp::Assign,
            T![+=] => AssignOp::AddAssign,
            T![-=] => AssignOp::SubAssign,
            T![*=] => AssignOp::MulAssign,
            T![/=] => AssignOp::DivAssign,
            T![%=] => AssignOp::RemAssign,
            T![&=] => AssignOp::BitAndAssign,
            T![|=] => AssignOp::BitOrAssign,
            T![^=] => AssignOp::BitXorAssign,
            T![<<=] => AssignOp::ShlAssign,
            T![>>=] => AssignOp::ShrAssign,
            _ => return None,
        })
    }

    /// the binary operator a compound assignment desugars to (`a += b` is
    /// `a = a + b`), or `None` for the plain `=`. lets MIR lowering map every
    /// compound form uniformly instead of enumerating each one.
    pub fn to_bin_op(self) -> Option<BinOp> {
        Some(match self {
            AssignOp::Assign => return None,
            AssignOp::AddAssign => BinOp::Add,
            AssignOp::SubAssign => BinOp::Sub,
            AssignOp::MulAssign => BinOp::Mul,
            AssignOp::DivAssign => BinOp::Div,
            AssignOp::RemAssign => BinOp::Rem,
            AssignOp::BitAndAssign => BinOp::BitAnd,
            AssignOp::BitOrAssign => BinOp::BitOr,
            AssignOp::BitXorAssign => BinOp::BitXor,
            AssignOp::ShlAssign => BinOp::Shl,
            AssignOp::ShrAssign => BinOp::Shr,
        })
    }
}

impl fmt::Display for AssignOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op_str = match self {
            AssignOp::Assign => "=",
            AssignOp::AddAssign => "+=",
            AssignOp::SubAssign => "-=",
            AssignOp::MulAssign => "*=",
            AssignOp::DivAssign => "/=",
            AssignOp::RemAssign => "%=",
            AssignOp::BitAndAssign => "&=",
            AssignOp::BitOrAssign => "|=",
            AssignOp::BitXorAssign => "^=",
            AssignOp::ShlAssign => "<<=",
            AssignOp::ShrAssign => ">>=",
        };
        write!(f, "{}", op_str)
    }
}

impl AssignExpr {
    /// the assignment operator - the direct child token between the operands.
    pub fn op(&self) -> Option<AssignOp> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| AssignOp::from_kind(t.kind()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexer::{Lexer, SourceText};

    /// lex + parse `src` and cast the CST root to a typed [`SourceFile`].
    fn source_file(src: &str) -> SourceFile {
        let source = SourceText::new(src.to_string());
        let tokens = Lexer::new(&source).tokenize().tokens;
        let parse = parser::parse(&tokens, &source);
        SourceFile::cast(parse.green).expect("root is a SourceFile")
    }

    /// the canonical `main.eye` program - exercises every v0.1 node kind.
    const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    let x = 0;
    let y = 0;
    mut Point p = Point { x, y };

    println(\"{}\", p);
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
        let TypeRef::IdentType(it) = fields[0].ty().unwrap() else {
            panic!("field type is an ident type");
        };
        assert_eq!(it.name().unwrap().text(), "int32");
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

        // `let x = 0;` - inferred type, no annotation
        let Stmt::LetStmt(x) = &stmts[0] else {
            panic!("first stmt is a let");
        };
        assert_eq!(x.kind(), Some(LetKind::Let));
        assert!(x.ty().is_none());
        assert_eq!(x.name().unwrap().text(), "x");
        let Some(Expr::Literal(lit)) = x.value() else {
            panic!("value is a literal");
        };
        assert_eq!(lit.literal_kind(), Some(LiteralKind::Int));

        // `mut Point p = Point { x, y };` - explicit type, struct literal
        let Stmt::LetStmt(p) = &stmts[2] else {
            panic!("third stmt is a let");
        };
        assert_eq!(p.kind(), Some(LetKind::Mut));
        let TypeRef::IdentType(pty) = p.ty().unwrap() else {
            panic!("let type is an ident type");
        };
        assert_eq!(pty.name().unwrap().text(), "Point");
        assert_eq!(p.name().unwrap().text(), "p");
        let Some(Expr::StructLit(sl)) = p.value() else {
            panic!("value is a struct literal");
        };
        assert_eq!(sl.name_ref().unwrap().name().unwrap().text(), "Point");
        let lit_fields: Vec<_> = sl.field_list().unwrap().fields().collect();
        assert_eq!(lit_fields.len(), 2);
        assert_eq!(lit_fields[0].name().unwrap().text(), "x");
    }

    /// digs out the value expression of the first statement, which must be a
    /// `let` - a shorthand for the operator tests below.
    fn first_let_value(src: &str) -> Expr {
        let file = source_file(src);
        let Some(Item::FnDef(f)) = file.items().next() else {
            panic!("first item is a function");
        };
        let Some(Stmt::LetStmt(l)) = f.body().unwrap().stmts().next() else {
            panic!("first stmt is a let");
        };
        l.value().expect("let has a value")
    }

    #[test]
    fn operator_precedence_nests_left_assoc() {
        // `1 + 2 * 3 - 4` parses as `((1 + (2 * 3)) - 4)`
        let Expr::BinExpr(top) = first_let_value("main() {\n    let r = 1 + 2 * 3 - 4;\n}\n")
        else {
            panic!("top expr is a binop");
        };
        assert_eq!(top.op(), Some(BinOp::Sub));
        assert!(matches!(top.rhs(), Some(Expr::Literal(_))));

        let Some(Expr::BinExpr(add)) = top.lhs() else {
            panic!("left of '-' is the '+' binop");
        };
        assert_eq!(add.op(), Some(BinOp::Add));
        assert!(matches!(add.lhs(), Some(Expr::Literal(_))));

        let Some(Expr::BinExpr(mul)) = add.rhs() else {
            panic!("right of '+' is the '*' binop - '*' binds tighter");
        };
        assert_eq!(mul.op(), Some(BinOp::Mul));
    }

    #[test]
    fn bitwise_binds_above_equality() {
        // no-footgun precedence (rust-style): `a & b == c` parses as
        // `(a & b) == c`, never c's `a & (b == c)`.
        let Expr::BinExpr(top) = first_let_value("main() {\n    let r = a & b == c;\n}\n") else {
            panic!("top expr is a binop");
        };
        assert_eq!(top.op(), Some(BinOp::Eq));
        let Some(Expr::BinExpr(lhs)) = top.lhs() else {
            panic!("left of `==` is the `&` binop - bitand binds tighter");
        };
        assert_eq!(lhs.op(), Some(BinOp::BitAnd));
    }

    #[test]
    fn paren_group_overrides_precedence() {
        // `a * (b + c)` - the group forces the add underneath the multiply.
        let Expr::BinExpr(top) = first_let_value("main() {\n    let r = a * (b + c);\n}\n") else {
            panic!("top expr is the `*` binop");
        };
        assert_eq!(top.op(), Some(BinOp::Mul));
        let Some(Expr::ParenExpr(group)) = top.rhs() else {
            panic!("right of `*` is the parenthesized group");
        };
        let Some(Expr::BinExpr(inner)) = group.expr() else {
            panic!("the group wraps the `+` binop");
        };
        assert_eq!(inner.op(), Some(BinOp::Add));
    }

    #[test]
    fn prefix_minus_binds_tighter_than_infix() {
        // `-a * b` parses as `((-a) * b)`, not `-(a * b)`
        let Expr::BinExpr(top) = first_let_value("main() {\n    let r = -a * b;\n}\n") else {
            panic!("top expr is the '*' binop");
        };
        assert_eq!(top.op(), Some(BinOp::Mul));

        let Some(Expr::PrefixExpr(neg)) = top.lhs() else {
            panic!("left of '*' is the prefix '-'");
        };
        assert_eq!(neg.op(), Some(UnaryOp::Neg));
        assert!(matches!(neg.operand(), Some(Expr::NameRef(_))));
    }

    #[test]
    fn struct_lit_named_fields() {
        // explicit `name: value` form - the value is a full expression
        let Expr::StructLit(sl) =
            first_let_value("main() {\n    let p = Point { x: 0, y: 1 };\n}\n")
        else {
            panic!("value is a struct literal");
        };
        let fields: Vec<_> = sl.field_list().unwrap().fields().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name().unwrap().text(), "x");
        let Some(Expr::Literal(v)) = fields[0].value() else {
            panic!("named field has an explicit value expression");
        };
        assert_eq!(v.literal_kind(), Some(LiteralKind::Int));
    }

    #[test]
    fn struct_lit_shorthand_has_no_value() {
        // bare-name form - `value()` is `None`, `name()` still resolves
        let Expr::StructLit(sl) = first_let_value("main() {\n    let p = Point { x, y };\n}\n")
        else {
            panic!("value is a struct literal");
        };
        let fields: Vec<_> = sl.field_list().unwrap().fields().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name().unwrap().text(), "x");
        assert!(fields[0].value().is_none());
    }

    #[test]
    fn call_expr_args() {
        let file = source_file(MAIN_EYE);
        let Item::FnDef(f) = file.items().nth(1).unwrap() else {
            panic!("expected a function");
        };
        let stmts: Vec<_> = f.body().unwrap().stmts().collect();

        // `println("{}", p);`
        let Stmt::ExprStmt(es) = &stmts[3] else {
            panic!("last stmt is an expr stmt");
        };
        let Some(Expr::CallExpr(call)) = es.expr() else {
            panic!("expr is a call");
        };
        let Some(Expr::NameRef(callee)) = call.callee() else {
            panic!("callee is a name");
        };
        assert_eq!(callee.name().unwrap().text(), "println");

        let args: Vec<_> = call.arg_list().unwrap().args().collect();
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], Expr::Literal(_)));
        assert!(matches!(args[1], Expr::NameRef(_)));
    }
}
