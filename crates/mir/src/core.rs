//! MIR: mid-level IR. a target-neutral, flattened form of a function body.
//!
//! MIR sits between HIR and codegen (`docs/design/REDESIGN.md`, `docs/features/MIR.md`). HIR is
//! the resolved, structured semantic tree; codegen is a mechanical printer. MIR
//! is where control flow is made explicit and value-producing expressions are
//! linearized into temps, so codegen makes no decisions.
//!
//! the defining invariant of the value model: an [`RValue`]'s arguments are
//! always [`Operand`]s, and an [`Operand`] is always a constant or a [`Place`].
//! no `RValue` nests another `RValue`. that single rule is what makes codegen a
//! mechanical walk. three-address form: `a + b * c` becomes
//! `t0 = b * c; t1 = a + t0`.
//!
//! control flow is represented as structured statements with explicit temps
//! (nested-block [`If`]/[`Loop`]/[`Switch`]), not a basic-block CFG. this is the
//! locked v1 representation (`docs/features/MIR.md`): going from structured MIR to a CFG
//! later is a mechanical lossless pass, so it does not trap a future backend.
//!
//! [`If`]: mirstmt::if
//! [`Loop`]: mirstmt::loop
//! [`Switch`]: mirstmt::switch

use ast::{BinOp, UnaryOp};
use hir::core::{EnumId, FnId, Literal, Text, TypeRef, TypedArena};
use thin_vec::ThinVec;

/// MIR reuses the HIR (unresolved) type representation rather than maintaining a
/// parallel type system. every MIR local and temp carries one.
pub type Type = TypeRef;

// EXPERIMENTAL(typed-arena): newtype wrapping idx<mirlocal>.
hir::arena_id!(LocalId, MirLocal);

#[derive(Debug)]
pub struct MirLocal {
    pub ty: Type,
    pub name: Option<Text>,
    /// whether the c declaration omits `const`. source `mut` bindings and every
    /// generated temp (assigned across branches) are mutable; a plain `let` is
    /// not.
    pub mutable: bool,
}

/// one lowered function body. `locals` is the type table for every local,
/// parameter, and temp; declarations of `let`/temp locals happen at their point
/// of use through [`MirStmt::Let`], so the emitter never iterates this arena to
/// declare. parameters live here too (so places that reference them resolve to a
/// name) but are declared by the function signature, not by a `Let`.
///
/// EXPERIMENTAL(typed-arena): `locals` uses [`TypedArena`] so every index
/// carries [`LocalId`] at the type level.
#[derive(Debug)]
pub struct MirBody {
    pub locals: TypedArena<MirLocal, LocalId>,
    /// locals that are parameters, in declaration order. the emitter skips
    /// declaring these; the function signature already does.
    pub params: ThinVec<LocalId>,
    pub body: MirBlock,
}

/// a sequence of statements. unlike an HIR block it has no tail expression: a
/// value-producing tail has already been rewritten into an assignment to an
/// enclosing temp during lowering.
#[derive(Debug, Default)]
pub struct MirBlock {
    pub stmts: ThinVec<MirStmt>,
}

#[derive(Debug)]
pub enum MirStmt {
    /// declare a local, optionally initialized. generated temps and source
    /// `let` bindings both appear here at their point of declaration.
    Let {
        local: LocalId,
        init: Option<RValue>,
    },
    /// store an rvalue into an existing place.
    Assign {
        place: Place,
        value: RValue,
    },
    /// evaluate an rvalue for its effect and discard the result (e.g. a call or
    /// a [`RValue::Println`]).
    Eval(RValue),
    If {
        cond: Operand,
        then_block: MirBlock,
        else_block: Option<MirBlock>,
    },
    Loop {
        body: MirBlock,
    },
    /// an ordered test-chain over `scrut`. each arm fires when its [`ArmTest`]
    /// holds; `default` covers a wildcard arm when present. despite the name it
    /// is not a c `switch` - codegen renders it as an `if`/`else-if` chain so a
    /// match-arm `break` binds to the enclosing loop, not a switch.
    Switch {
        scrut: Operand,
        arms: ThinVec<SwitchArm>,
        default: Option<MirBlock>,
    },
    Break,
    Continue,
    Return(Option<Operand>),
}

/// one arm of a [`MirStmt::Switch`] test-chain: fire `body` when `test` holds
/// against the scrutinee (and `guard`, if present, also evaluates to true).
/// a guard-free switch is an `if`/`else-if` chain. a switch with any guard is a
/// flag-gated chain (`gen_switch`): a false guard must fall through to the next
/// arm, which an `if`/`else-if` cannot express once the guard needs temp
/// statements that an `&&` cannot hold.
#[derive(Debug)]
pub struct SwitchArm {
    pub test: ArmTest,
    /// optional guard (`pat if expr -> body`); see [`Guard`].
    pub guard: Option<Guard>,
    pub body: MirBlock,
}

/// a match-arm guard. `stmts` computes the guard's prerequisite temps (empty for
/// a simple guard such as a bare local); `cond` is the final boolean. codegen
/// emits `stmts` inside the arm's matched block, then `if (cond) { body }`, so a
/// false guard leaves the arm's effect unrun and the flag unset (fall-through).
#[derive(Debug)]
pub struct Guard {
    pub stmts: ThinVec<MirStmt>,
    pub cond: Operand,
}

/// what a [`SwitchArm`] tests the scrutinee against. extensible by design: S1
/// adds a `Const` (int / char / bool literal), S4 adds `Range` and `Or`.
#[derive(Debug)]
pub enum ArmTest {
    /// the scrutinee's enum tag equals this variant.
    Variant(VariantRef),
    /// the scrutinee equals this int / char / bool literal. codegen emits
    /// `scrut == <const>` - an enum tag is a c int, so this and `Variant` share
    /// the same comparison shape.
    Const(Literal),
    /// always matches the scrutinee - a guarded catch-all (`_ if c` or `x if c`).
    /// only ever carries a `Some(guard)`: an unguarded catch-all is the switch's
    /// `default` slot, not an arm. lives in the ordered arm list (not `default`)
    /// so a false guard falls through to the next arm in source order.
    Always,
}

/// identifies an enum variant by its enum and index, the same shape HIR uses in
/// `Pat::Variant`. the emitter resolves it to the c variant label.
#[derive(Debug, Clone, Copy)]
pub struct VariantRef {
    pub enum_id: EnumId,
    pub idx: u32,
}

#[derive(Debug)]
pub enum RValue {
    Use(Operand),
    /// arithmetic and comparison only. the short-circuit operators `&&` and `||`
    /// are NOT represented here: their operands would be evaluated eagerly.
    /// lowering rewrites them to control flow (see `docs/features/MIR.md` I5).
    Binary(BinOp, Operand, Operand),
    Unary(UnaryOp, Operand),
    /// a direct call to a named function (defined or `extern`), the callee
    /// resolved to a [`FnId`]. the `print` intrinsic has its own node; an
    /// indirect call through a function-pointer value is [`RValue::CallIndirect`].
    Call {
        func: FnId,
        args: ThinVec<Operand>,
    },
    /// an indirect call through a function-pointer value. `callee` is the
    /// pointer operand (a local, field, or other value of function type); the
    /// result type comes from that value's `Fn` type, not from a resolved
    /// [`FnId`].
    CallIndirect {
        callee: Operand,
        args: ThinVec<Operand>,
    },
    /// a function used as a value: its address. emits the bare c function name,
    /// which decays to a function pointer in value context. a dedicated rvalue
    /// (not an [`Operand`]) so the trivial-operand invariant stays "a constant
    /// or a place"; used where an operand is wanted, it spills to a temp.
    Func(FnId),
    /// the `println` intrinsic. carried as a dedicated node because it is sniffed
    /// today by an unresolved callee name, which a plain [`RValue::Call`] cannot
    /// represent. `args[0]` is the format constant; the rest are the values.
    /// deliberately a thin pass-through so removing the intrinsic later (compose
    /// `printf` in the stdlib, `docs/planning/ISSUE.md`) is a clean deletion.
    Println {
        args: ThinVec<Operand>,
    },
    /// an enum-variant constant (e.g. `Color.Red`), a compile-time value. kept
    /// as an rvalue rather than an [`Operand`] so the trivial-operand invariant
    /// ("a constant or a place") stays intact; a variant used where an operand
    /// is wanted spills to a temp like any other rvalue.
    Variant(VariantRef),
    Ref(Place),
    Deref(Operand),
    Cast(Operand, Type),
    /// `sizeof(T)` intrinsic. the value is target-defined layout, which eye does
    /// not model, so the type is carried verbatim and emitted as c `sizeof(ctype)`
    /// (the c backend is the layout authority). a dedicated rvalue, not an
    /// [`Operand`], so the trivial-operand invariant ("a constant or a place")
    /// holds; used where an operand is wanted, it spills to a temp.
    SizeOf(Type),
    /// an array literal. `ty` is the array type; the emitter needs the element
    /// type and length to name the c value-wrapper struct, so a bare operand
    /// list would not be enough.
    ArrayLit {
        ty: Type,
        elems: ThinVec<Operand>,
    },
    /// an array repeat `[value; count]`: `value` is evaluated once (already
    /// spilled to a trivial operand) and copied `count` times. `ty` is the array
    /// type (element + length name the c wrapper). kept distinct from `ArrayLit`
    /// so a future backend can emit a fill loop / `memset` rather than `count`
    /// copies; the c backend emits `count` copies of the wrapper.
    ArrayRepeat {
        ty: Type,
        value: Operand,
        count: u64,
    },
    StructLit {
        ty: Type,
        fields: ThinVec<(Text, Operand)>,
    },
}

/// a trivial value: never nested, always a constant or a place.
#[derive(Debug, Clone)]
pub enum Operand {
    Const(Literal),
    Copy(Place),
}

impl PartialEq for Operand {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Const(a), Self::Const(b)) => a == b,
            (Self::Copy(a), Self::Copy(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Operand {}

impl std::hash::Hash for Operand {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Const(lit) => {
                0u8.hash(state);
                lit.hash(state);
            }
            Self::Copy(place) => {
                1u8.hash(state);
                place.hash(state);
            }
        }
    }
}

/// a memory location. projections (`field`, `index`, `deref`) nest a base place.
#[derive(Debug, Clone)]
pub enum Place {
    Local(LocalId),
    /// a top-level global, addressed by its c symbol name. addressable static
    /// storage (HORIZON0 C3): readable, writable when `mut`, and `&G` is legal.
    Global(Text),
    Field(Box<Place>, Text),
    Index(Box<Place>, Box<Operand>),
    Deref(Box<Place>),
}

impl PartialEq for Place {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Local(a), Self::Local(b)) => a == b,
            (Self::Global(a), Self::Global(b)) => a == b,
            (Self::Field(a, an), Self::Field(b, bn)) => a == b && an == bn,
            (Self::Index(a, ai), Self::Index(b, bi)) => a == b && ai == bi,
            (Self::Deref(a), Self::Deref(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Place {}

impl std::hash::Hash for Place {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Local(id) => {
                0u8.hash(state);
                id.hash(state);
            }
            Self::Global(name) => {
                1u8.hash(state);
                name.hash(state);
            }
            Self::Field(base, name) => {
                2u8.hash(state);
                base.hash(state);
                name.hash(state);
            }
            Self::Index(base, idx) => {
                3u8.hash(state);
                base.hash(state);
                idx.hash(state);
            }
            Self::Deref(base) => {
                4u8.hash(state);
                base.hash(state);
            }
        }
    }
}
