//! codegen: HIR -> MIR -> c source.
//!
//! codegen makes no semantic decisions. it lowers the HIR to MIR
//! ([`mir::lower`]), which flattens control flow and three-addresses every
//! expression, then mechanically prints the MIR to c ([`mir_emit`]). the
//! supporting modules are pure rendering helpers shared with that emitter:
//! - [`types`]: type/declarator rendering and the printf specifier map.
//! - [`arrays`]: the fixed-array struct-wrap representation and the program-wide wrapper-typedef collection.
//!
//! the public entry point is [`gen_mir`].

mod arrays;
mod mir_emit;
mod types;

pub use mir_emit::gen_mir;
