//! Arena index aliases. Every HIR node is addressed by a typed [`Idx`].

use la_arena::Idx;

use super::*;

pub type StructId = Idx<Struct>;
pub type UnionId = Idx<Union>;
pub type ConstId = Idx<Const>;
pub type GlobalId = Idx<Global>;
pub type EnumId = Idx<Enum>;
pub type OpaqueId = Idx<OpaqueType>;
pub type FnId = Idx<Function>;
pub type FieldId = Idx<Field>;
pub type ExprId = Idx<Expr>;
pub type StmtId = Idx<Stmt>;
pub type PatId = Idx<Pat>;
pub type LocalId = Idx<Local>;
pub type LocalConstId = Idx<LocalConst>;
pub type BlockId = Idx<Block>;
pub type BodyId = Idx<Body>;
