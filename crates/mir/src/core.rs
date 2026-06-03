//! MIR: mid-level IR. A target-neutral, flattened form of a function body.
//!
//! MIR sits between HIR and codegen (`docs/REDESIGN.md`, `docs/MIR.md`). HIR is
//! the resolved, structured semantic tree; codegen is a mechanical printer. MIR
//! is where control flow is made explicit and value-producing expressions are
//! linearized into temps, so codegen makes no decisions.
//!
//! The defining invariant of the value model: an [`RValue`]'s arguments are
//! always [`Operand`]s, and an [`Operand`] is always a constant or a [`Place`].
//! No `RValue` nests another `RValue`. That single rule is what makes codegen a
//! mechanical walk. Three-address form: `a + b * c` becomes
//! `t0 = b * c; t1 = a + t0`.
//!
//! Control flow is represented as structured statements with explicit temps
//! (nested-block [`If`]/[`Loop`]/[`Switch`]), not a basic-block CFG. This is the
//! locked v1 representation (`docs/MIR.md`): going from structured MIR to a CFG
//! later is a mechanical lossless pass, so it does not trap a future backend.
//!
//! [`If`]: MirStmt::If
//! [`Loop`]: MirStmt::Loop
//! [`Switch`]: MirStmt::Switch

use ast::{BinOp, UnaryOp};
use hir::core::{EnumId, FnId, Literal, Text, TypeRef};
use la_arena::{Arena, Idx};
use thin_vec::ThinVec;

/// MIR reuses the HIR (unresolved) type representation rather than maintaining a
/// parallel type system. Every MIR local and temp carries one.
pub type Type = TypeRef;

/// A MIR local: a source local, a parameter, or a generated temp. Each is typed.
/// `name` is the source name for parameters and `let` bindings; generated temps
/// leave it `None` and the emitter derives a name from the [`LocalId`].
pub type LocalId = Idx<MirLocal>;

#[derive(Debug)]
pub struct MirLocal {
    pub ty: Type,
    pub name: Option<Text>,
    /// Whether the C declaration omits `const`. Source `mut` bindings and every
    /// generated temp (assigned across branches) are mutable; a plain `let` is
    /// not.
    pub mutable: bool,
}

/// One lowered function body. `locals` is the type table for every local,
/// parameter, and temp; declarations of `let`/temp locals happen at their point
/// of use through [`MirStmt::Let`], so the emitter never iterates this arena to
/// declare. Parameters live here too (so places that reference them resolve to a
/// name) but are declared by the function signature, not by a `Let`.
#[derive(Debug)]
pub struct MirBody {
    pub locals: Arena<MirLocal>,
    /// Locals that are parameters, in declaration order. The emitter skips
    /// declaring these; the function signature already does.
    pub params: ThinVec<LocalId>,
    pub body: MirBlock,
}

/// A sequence of statements. Unlike an HIR block it has no tail expression: a
/// value-producing tail has already been rewritten into an assignment to an
/// enclosing temp during lowering.
#[derive(Debug, Default)]
pub struct MirBlock {
    pub stmts: ThinVec<MirStmt>,
}

#[derive(Debug)]
pub enum MirStmt {
    /// Declare a local, optionally initialized. Generated temps and source
    /// `let` bindings both appear here at their point of declaration.
    Let {
        local: LocalId,
        init: Option<RValue>,
    },
    /// Store an rvalue into an existing place.
    Assign {
        place: Place,
        value: RValue,
    },
    /// Evaluate an rvalue for its effect and discard the result (e.g. a call or
    /// a [`RValue::Print`]).
    Eval(RValue),
    If {
        cond: Operand,
        then_block: MirBlock,
        else_block: Option<MirBlock>,
    },
    Loop {
        body: MirBlock,
    },
    /// `switch` over an enum tag. `arms` are variant cases; `default` covers a
    /// wildcard arm when present.
    Switch {
        scrut: Operand,
        arms: ThinVec<SwitchArm>,
        default: Option<MirBlock>,
    },
    Break,
    Continue,
    Return(Option<Operand>),
}

#[derive(Debug)]
pub struct SwitchArm {
    pub variant: VariantRef,
    pub body: MirBlock,
}

/// Identifies an enum variant by its enum and index, the same shape HIR uses in
/// `Pat::Variant`. The emitter resolves it to the C variant label.
#[derive(Debug, Clone, Copy)]
pub struct VariantRef {
    pub enum_id: EnumId,
    pub idx: u32,
}

#[derive(Debug)]
pub enum RValue {
    Use(Operand),
    /// Arithmetic and comparison only. The short-circuit operators `&&` and `||`
    /// are NOT represented here: their operands would be evaluated eagerly.
    /// Lowering rewrites them to control flow (see `docs/MIR.md` I5).
    Binary(BinOp, Operand, Operand),
    Unary(UnaryOp, Operand),
    /// A direct call to a named function (defined or `extern`). Carries the
    /// resolved [`FnId`] rather than the speculative `callee: Operand` of the
    /// `docs/MIR.md` sketch: every Eye call resolves to a function (the only
    /// unresolved callee is the `print` intrinsic, which has its own node), so a
    /// function reference never needs to be a trivial operand. An indirect call
    /// (through a function-pointer value) would add a separate variant; Eye has
    /// no function-pointer type today.
    Call {
        func: FnId,
        args: ThinVec<Operand>,
    },
    /// The `print` intrinsic. Carried as a dedicated node because it is sniffed
    /// today by an unresolved callee name, which a plain [`RValue::Call`] cannot
    /// represent. `args[0]` is the format constant; the rest are the values.
    /// Deliberately a thin pass-through so removing the intrinsic later (compose
    /// `printf` in the stdlib, `docs/ISSUE.md`) is a clean deletion.
    Print {
        args: ThinVec<Operand>,
    },
    /// An enum-variant constant (e.g. `Color.Red`), a compile-time value. Kept
    /// as an rvalue rather than an [`Operand`] so the trivial-operand invariant
    /// ("a constant or a place") stays intact; a variant used where an operand
    /// is wanted spills to a temp like any other rvalue.
    Variant(VariantRef),
    Ref(Place),
    Deref(Operand),
    Cast(Operand, Type),
    /// An array literal. `ty` is the array type; the emitter needs the element
    /// type and length to name the C value-wrapper struct, so a bare operand
    /// list would not be enough.
    ArrayLit {
        ty: Type,
        elems: ThinVec<Operand>,
    },
    StructLit {
        ty: Type,
        fields: ThinVec<(Text, Operand)>,
    },
}

/// A trivial value: never nested, always a constant or a place.
#[derive(Debug, Clone)]
pub enum Operand {
    Const(Literal),
    Copy(Place),
}

/// A memory location. Projections (`field`, `index`, `deref`) nest a base place.
#[derive(Debug, Clone)]
pub enum Place {
    Local(LocalId),
    Field(Box<Place>, Text),
    Index(Box<Place>, Box<Operand>),
    Deref(Box<Place>),
}
