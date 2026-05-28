//! Per-function body IR: the expression, statement, pattern, local, and block
//! arenas plus a source map back to syntax pointers. One [`Body`] per fn so
//! editing a single fn body invalidates only that body.

use ast::{BinOp, UnaryOp};
use la_arena::{Arena, ArenaMap};
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;

use super::*;

#[derive(Debug, Default)]
pub struct Body {
    pub exprs: Arena<Expr>,
    pub stmts: Arena<Stmt>,
    pub pats: Arena<Pat>,
    pub locals: Arena<Local>,
    /// Top-level statements of the fn body, in source order.
    pub block: Vec<StmtId>,
    /// Optional tail expression of the body block (none for v0.1).
    pub tail: Option<ExprId>,
    pub source_map: BodySourceMap,
    pub blocks: Arena<Block>,
    pub block_source_map: ArenaMap<BlockId, SyntaxNodePtr>,
    pub expr_types: ArenaMap<ExprId, TypeRef>,
}

#[derive(Debug, Default)]
pub struct BodySourceMap {
    pub expr: ArenaMap<ExprId, SyntaxNodePtr>,
    pub stmt: ArenaMap<StmtId, SyntaxNodePtr>,
    pub pat: ArenaMap<PatId, SyntaxNodePtr>,
}

#[derive(Debug)]
pub struct Local {
    pub name: Text,
    pub ty: Option<TypeRef>,
    pub mutable: bool,
    pub pat: PatId,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<StmtId>,
    pub tail: Option<ExprId>,
}

#[derive(Debug)]
pub enum Stmt {
    Let {
        pat: PatId,
        ty: Option<TypeRef>,
        init: Option<ExprId>,
        mutable: bool,
    },
    Expr(ExprId),
}

#[derive(Debug)]
pub enum Pat {
    Bind(LocalId),
    /// `Enum.Variant` qualified or bare variant pattern in a match arm.
    /// Resolved at lowering against the scrutinee enum.
    Variant {
        enum_id: EnumId,
        idx: u32,
    },
    /// `_` wildcard in a match arm.
    Wildcard,
    Missing,
}

#[derive(Debug)]
pub enum Expr {
    Missing,
    Literal(Literal),
    /// Resolved local, function, or unknown name. Resolution result is stored
    /// here so later passes don't redo the lookup.
    Path(Resolution),
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Unary {
        op: UnaryOp,
        operand: ExprId,
    },
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    StructLit {
        ty: TypeRef,
        fields: Vec<StructLitField>,
    },
    Field {
        base: ExprId,
        name: Text,
    },
    Assign {
        lhs: ExprId,
        rhs: ExprId,
    },
    If {
        cond: ExprId,
        then_branch: BlockId,
        else_branch: Option<BlockId>,
    },
    Loop {
        body: BlockId,
    },
    Break,
    Continue,
    Ref {
        operand: ExprId,
    },
    Deref {
        operand: ExprId,
    },
    Match {
        scrut: ExprId,
        arms: Vec<MatchArm>,
    },
    Block(BlockId),
}

#[derive(Debug)]
pub struct MatchArm {
    pub pat: PatId,
    pub body: ExprId,
}

#[derive(Debug)]
pub struct StructLitField {
    pub name: Text,
    /// Always materialized. Shorthand `Point { x }` is desugared at lowering
    /// into `Point { x: x }` where the value is a synthesized `Path` expr
    /// whose source-map entry points at the same `StructLitField` syntax node
    /// as the field name.
    pub value: ExprId,
}

#[derive(Debug)]
pub enum Literal {
    Int(u128),
    Float(SmolStr),
    String(SmolStr),
    Bool(bool),
    Char(char),
}

/// Result of name resolution for a `NameRef`. Diagnostic-friendly: unresolved
/// becomes [`Resolution::Unresolved`] (not a hard error here).
#[derive(Debug, Clone)]
pub enum Resolution {
    Local(LocalId),
    Fn(FnId),
    Struct(StructId),
    Enum(EnumId),
    /// A specific variant of an enum. Produced either by qualified access
    /// (`Shape.Circle` lowers the whole `FieldExpr` to this) or by
    /// type-directed lookup in a typed context (`const Shape sh = Circle;`).
    Variant {
        enum_id: EnumId,
        idx: u32,
    },
    Unresolved(Text),
}
