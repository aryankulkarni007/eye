//! EXPERIMENTAL(typed-arena): Arena index newtypes.
//!
//! Every HIR node is addressed by a typed newtype rather than a raw [`Idx`],
//! so `StructId` and `FnId` are distinct types that the compiler refuses to
//! mix up.

use crate::arena_id;

use super::body::*;
use super::items::*;

arena_id!(StructId, Struct);
arena_id!(UnionId, Union);
arena_id!(ConstId, Const);
arena_id!(GlobalId, Global);
arena_id!(EnumId, Enum);
arena_id!(OpaqueId, OpaqueType);
arena_id!(FnId, Function);
arena_id!(FieldId, Field);
arena_id!(ExprId, Expr);
arena_id!(StmtId, Stmt);
arena_id!(PatId, Pat);
arena_id!(LocalId, Local);
arena_id!(LocalConstId, LocalConst);
arena_id!(BlockId, Block);
arena_id!(BodyId, Body);
