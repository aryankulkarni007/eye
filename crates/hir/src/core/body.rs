//! per-function body IR: the expression, statement, pattern, local, and block
//! arenas plus a source map back to syntax pointers. one [`Body`] per fn so
//! editing a single fn body invalidates only that body.

use ast::{AssignOp, BinOp, UnaryOp};
use la_arena::{ArenaMap, Idx};
use smol_str::SmolStr;
use syntax::SyntaxNodePtr;
use thin_vec::ThinVec;

use super::*;

/// per-function body IR.
///
/// EXPERIMENTAL(typed-arena): arena fields use [`TypedArena<T, XId>`], paired
/// with the corresponding `*Id` newtypes from [`ids`](super::ids).
#[derive(Debug, Default)]
pub struct Body {
    pub exprs: TypedArena<Expr, ExprId>,
    pub stmts: TypedArena<Stmt, StmtId>,
    pub pats: TypedArena<Pat, PatId>,
    pub locals: TypedArena<Local, LocalId>,
    /// top-level statements of the fn body, in source order.
    pub block: ThinVec<StmtId>,
    /// optional tail expression of the body block (none for v0.1).
    pub tail: Option<ExprId>,
    pub source_map: BodySourceMap,
    pub blocks: TypedArena<Block, BlockId>,
    pub block_source_map: ArenaMap<Idx<Block>, SyntaxNodePtr>,
    /// the fn body block's syntax pointer, anchoring whole-body diagnostics
    /// (a missing return value). set once per body at fn lowering; read by the
    /// typeck pass's return-type enforcement.
    pub fn_block_ptr: Option<SyntaxNodePtr>,
    /// block-scope `const` declarations. same value/no-storage semantics as a
    /// top-level [`Const`](super::const), but scoped to the declaring block,
    /// so they live in the body, not the module-level arena (which sits behind
    /// `&HIR` and cannot grow during body lowering).
    pub local_consts: TypedArena<LocalConst, LocalConstId>,
}

#[derive(Debug, Default)]
pub struct BodySourceMap {
    pub expr: ArenaMap<Idx<Expr>, SyntaxNodePtr>,
    pub stmt: ArenaMap<Idx<Stmt>, SyntaxNodePtr>,
    pub pat: ArenaMap<Idx<Pat>, SyntaxNodePtr>,
}

#[derive(Debug)]
pub struct Local {
    pub name: Text,
    pub ty: Option<TypeRef>,
    pub mutable: bool,
    pub pat: PatId,
}

/// a block-scope `const`: a compile-time value folded at lowering against the
/// consts visible at the declaration site (top-level consts plus enclosing
/// local consts). like a top-level const it is inlined at MIR lowering, has no
/// address (`&` of it is rejected), and is not an assignable place. `value` is
/// `None` when the fold failed (already diagnosed - poison).
#[derive(Debug)]
pub struct LocalConst {
    pub name: Text,
    /// the declared type (always explicit at the floor - no inference).
    pub ty: TypeRef,
    pub value: Option<ConstValue>,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: ThinVec<StmtId>,
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
    /// a block-scope `const` declaration. purely compile-time: the value is
    /// already folded into [`Body::local_consts`] and every reference inlines
    /// it, so MIR lowering emits nothing for this statement.
    Const(LocalConstId),
}

#[derive(Debug)]
pub enum Pat {
    Bind(LocalId),
    /// `Enum.Variant` qualified or bare variant pattern in a match arm.
    /// resolved at lowering against the scrutinee enum.
    Variant {
        enum_id: EnumId,
        idx: u32,
    },
    /// an int / char / bool literal pattern (`1`, `'a'`, `true`). float and
    /// string literals are not patterns (see the parser's `pat`). matched by
    /// equality against the scrutinee; the domain check lives in the match
    /// lowering.
    Literal(Literal),
    /// irrefutable struct destructure (`Point { x, y }` / `Point { x: px }`):
    /// each field binds a local. exhaustive - every struct field is bound (no
    /// `..`/ignore yet). used by `let` today; match arms gain it (with guards)
    /// later. MIR expands it into one field-projection `Let` per binding.
    Struct {
        ty: Text,
        fields: ThinVec<StructPatBinding>,
    },
    /// `_` wildcard in a match arm.
    Wildcard,
    Missing,
}

/// one field binding inside a [`Pat::Struct`]: project `field` off the scrutinee
/// and bind it to `local` (the local's name is the field name, or the rename).
#[derive(Debug)]
pub struct StructPatBinding {
    pub field: Text,
    pub local: LocalId,
}

#[derive(Debug)]
pub enum Expr {
    Missing,
    Literal(Literal),
    /// resolved local, function, or unknown name. resolution result is stored
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
        args: ThinVec<ExprId>,
    },
    /// `[a, b, c]` array literal.
    ArrayLit(ThinVec<ExprId>),
    /// `[value; N]` array repeat: `value` copied `N` times. the element is
    /// evaluated once; `count` is a resolved const length.
    ArrayRepeat {
        value: ExprId,
        count: u64,
    },
    /// `base[index]` - element access on an array or pointer.
    Index {
        base: ExprId,
        index: ExprId,
    },
    StructLit {
        ty: TypeRef,
        fields: ThinVec<StructLitField>,
    },
    Field {
        base: ExprId,
        name: Text,
    },
    Assign {
        op: AssignOp,
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
    /// `return expr;` / `return;`. diverges, so it carries no value type. valid
    /// only in statement position (like `Break`/`Continue`); MIR lowers it to a
    /// `Return` statement.
    Return(Option<ExprId>),
    Ref {
        operand: ExprId,
    },
    Deref {
        operand: ExprId,
    },
    Cast {
        operand: ExprId,
        ty: TypeRef,
    },
    Match {
        scrut: ExprId,
        arms: ThinVec<MatchArm>,
    },
    /// `sizeof(T)` kernel intrinsic: a compile-time `usize`. the value is the
    /// target layout size, which eye does not model - it leans on the c backend
    /// (`sizeof(ctype)`), so the type is carried verbatim to codegen rather than
    /// folded to an eye integer. recognized by callee name like `print`/`len`,
    /// so a user-defined `sizeof` shadows it.
    SizeOf(TypeRef),
    /// `len(arr)` kernel intrinsic: the element count of the operand's array
    /// type, a compile-time `usize`. lowering emits this node (keeping the
    /// structural arity/place + array-ness checks) and the count is folded at
    /// MIR lowering from the operand's type in `TypeckResults` - so lowering,
    /// which holds no types after the S2C cutover, need not compute it.
    Len(ExprId),
    Block(BlockId),
}

/// a visitor over [`Expr`] variants with default no-op fallthrough.
/// implementors override only the variants they care about instead of
/// writing a full enum match.
pub trait VisitExpr {
    fn visit_missing(&mut self) {}
    fn visit_literal(&mut self, _lit: &Literal) {}
    fn visit_path(&mut self, _res: &Resolution) {}
    fn visit_binary(&mut self, _op: BinOp, _lhs: ExprId, _rhs: ExprId) {}
    fn visit_unary(&mut self, _op: UnaryOp, _operand: ExprId) {}
    fn visit_call(&mut self, _callee: ExprId, _args: &[ExprId]) {}
    fn visit_array_lit(&mut self, _elems: &[ExprId]) {}
    fn visit_array_repeat(&mut self, _value: ExprId, _count: u64) {}
    fn visit_index(&mut self, _base: ExprId, _index: ExprId) {}
    fn visit_struct_lit(&mut self, _fields: &[StructLitField]) {}
    fn visit_field(&mut self, _base: ExprId, _name: &Text) {}
    fn visit_assign(&mut self, _op: AssignOp, _lhs: ExprId, _rhs: ExprId) {}
    fn visit_if(&mut self, _cond: ExprId, _then: BlockId, _else: Option<BlockId>) {}
    fn visit_loop(&mut self, _body: BlockId) {}
    fn visit_break(&mut self) {}
    fn visit_continue(&mut self) {}
    fn visit_return(&mut self, _value: Option<ExprId>) {}
    fn visit_ref(&mut self, _operand: ExprId) {}
    fn visit_deref(&mut self, _operand: ExprId) {}
    fn visit_cast(&mut self, _operand: ExprId, _ty: &TypeRef) {}
    fn visit_match(&mut self, _scrut: ExprId, _arms: &[MatchArm]) {}
    fn visit_size_of(&mut self, _ty: &TypeRef) {}
    fn visit_len(&mut self, _operand: ExprId) {}
    fn visit_block(&mut self, _block: BlockId) {}

    /// visit an expression, dispatching to the per-variant method.
    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Missing => self.visit_missing(),
            Expr::Literal(lit) => self.visit_literal(lit),
            Expr::Path(res) => self.visit_path(res),
            Expr::Binary { op, lhs, rhs } => self.visit_binary(*op, *lhs, *rhs),
            Expr::Unary { op, operand } => self.visit_unary(*op, *operand),
            Expr::Call { callee, args } => self.visit_call(*callee, args),
            Expr::ArrayLit(elems) => self.visit_array_lit(elems),
            Expr::ArrayRepeat { value, count } => self.visit_array_repeat(*value, *count),
            Expr::Index { base, index } => self.visit_index(*base, *index),
            Expr::StructLit { fields, .. } => self.visit_struct_lit(fields),
            Expr::Field { base, name } => self.visit_field(*base, name),
            Expr::Assign { op, lhs, rhs } => self.visit_assign(*op, *lhs, *rhs),
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => self.visit_if(*cond, *then_branch, *else_branch),
            Expr::Loop { body } => self.visit_loop(*body),
            Expr::Break => self.visit_break(),
            Expr::Continue => self.visit_continue(),
            Expr::Return(value) => self.visit_return(*value),
            Expr::Ref { operand } => self.visit_ref(*operand),
            Expr::Deref { operand } => self.visit_deref(*operand),
            Expr::Cast { operand, ty } => self.visit_cast(*operand, ty),
            Expr::Match { scrut, arms } => self.visit_match(*scrut, arms),
            Expr::SizeOf(ty) => self.visit_size_of(ty),
            Expr::Len(operand) => self.visit_len(*operand),
            Expr::Block(block) => self.visit_block(*block),
        }
    }
}

impl Expr {
    /// visit direct expression-id children stored on this expression. block
    /// contents live behind [`BlockId`] and are intentionally left to callers
    /// that have access to the surrounding [`Body`].
    pub fn for_each_child_expr(&self, mut f: impl FnMut(ExprId)) {
        match self {
            Expr::Missing
            | Expr::Literal(_)
            | Expr::Path(_)
            | Expr::Break
            | Expr::Continue
            // `sizeof` carries a type, not child expressions.
            | Expr::SizeOf(_)
            | Expr::Block(_) => {}
            Expr::Binary { lhs, rhs, .. } | Expr::Assign { lhs, rhs, .. } => {
                f(*lhs);
                f(*rhs);
            }
            Expr::Unary { operand, .. }
            | Expr::Ref { operand }
            | Expr::Deref { operand }
            | Expr::Cast { operand, .. }
            | Expr::Len(operand) => f(*operand),
            Expr::Return(value) => {
                if let Some(v) = value {
                    f(*v);
                }
            }
            Expr::Call { callee, args } => {
                f(*callee);
                args.iter().copied().for_each(f);
            }
            Expr::ArrayLit(elems) => elems.iter().copied().for_each(f),
            Expr::ArrayRepeat { value, .. } => f(*value),
            Expr::Index { base, index } => {
                f(*base);
                f(*index);
            }
            Expr::StructLit { fields, .. } => {
                fields.iter().map(|field| field.value).for_each(f);
            }
            Expr::Field { base, .. } => f(*base),
            Expr::If { cond, .. } => f(*cond),
            Expr::Loop { .. } => {}
            Expr::Match { scrut, arms } => {
                f(*scrut);
                for arm in arms.iter() {
                    if let Some(g) = arm.guard {
                        f(g);
                    }
                    f(arm.body);
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct MatchArm {
    pub pat: PatId,
    /// optional guard expression (`pat if expr -> body`).
    /// evaluated only when the pattern matches; the arm body runs only when
    /// the guard is also true.
    pub guard: Option<ExprId>,
    pub body: ExprId,
    /// source span of the whole arm (`pat (if guard)? -> body`). the match
    /// type-judgment diagnostics (domain, coverage, duplicate, unreachable)
    /// run in the typeck pass, which has no AST; this lets them anchor on the
    /// arm exactly as lowering once did. inert for MIR.
    pub ptr: SyntaxNodePtr,
}

#[derive(Debug)]
pub struct StructLitField {
    pub name: Text,
    /// always materialized. shorthand `Point { x }` is desugared at lowering
    /// into `Point { x: x }` where the value is a synthesized `Path` expr
    /// whose source-map entry points at the same `StructLitField` syntax node
    /// as the field name.
    pub value: ExprId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Literal {
    Int(u128),
    Float(SmolStr),
    String(SmolStr),
    Bool(bool),
    Char(char),
}

/// result of name resolution for a `NameRef`. diagnostic-friendly: unresolved
/// becomes [`Resolution::Unresolved`] (not a hard error here).
#[derive(Debug, Clone)]
pub enum Resolution {
    Local(LocalId),
    Fn(FnId),
    Struct(StructId),
    Enum(EnumId),
    /// a top-level compile-time constant. a value (inlined at MIR lowering to
    /// its folded [`ConstValue`]); usable in value position, but `&const` is
    /// illegal (it has no address) and it is not an assignable place.
    Const(ConstId),
    /// a block-scope `const` ([`Body::local_consts`]). same semantics as
    /// [`Resolution::Const`], but resolved lexically rather than at module
    /// level.
    LocalConst(LocalConstId),
    /// a top-level global: addressable static storage. unlike a const, it has an
    /// address (`&G` is legal) and is an assignable place when declared `mut`.
    /// MIR lowers a reference to `Place::Global` (a named c symbol).
    Global(GlobalId),
    /// a specific variant of an enum. produced either by qualified access
    /// (`Shape.Circle` lowers the whole `FieldExpr` to this) or by
    /// type-directed lookup in a typed context (`let Shape sh = Circle;`).
    Variant {
        enum_id: EnumId,
        idx: u32,
    },
    Unresolved(Text),
}
