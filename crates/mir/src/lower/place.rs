//! place lowering: `lower_place` resolves a place expression (`a`, `a[i]`,
//! `s.f`, `*p`) to a [`Place`], and `place_for_value` yields a place holding an
//! arbitrary value, spilling a value-position control-flow expression to a temp.

use hir::core::{Expr, ExprId, Resolution};
use thin_vec::ThinVec;

use super::Lower;
use crate::core::*;

impl<'a> Lower<'a> {
    /// get a [`Place`] holding the value of `e`, spilling to a temp when needed.
    /// a value-position control-flow expression (`if`/`match`) is not handled by
    /// [`Lower::lower_place`] (it is not an rvalue), so route it through
    /// [`Lower::lower_operand`], which spills it into a temp and yields a place.
    pub(crate) fn place_for_value(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Place {
        if self.is_value_control_flow(e) {
            match self.lower_operand(e, buf) {
                Operand::Copy(p) => p,
                // a control-flow value is never a constant, but stay total: park a
                // constant in a temp so the caller still gets a place.
                op @ Operand::Const(_) => {
                    let ty = self.mir_type_of(e);
                    let mid = self.locals.alloc(MirLocal {
                        ty,
                        name: None,
                        mutable: false,
                    });
                    buf.push(MirStmt::Let {
                        local: mid,
                        init: Some(RValue::Use(op)),
                    });
                    Place::Local(mid)
                }
            }
        } else {
            self.lower_place(e, buf)
        }
    }

    /// lower a place expression (`a`, `a[i]`, `s.f`, `*p`) to a [`Place`],
    /// emitting any prerequisite temps into `buf`. used for an assign target,
    /// the operand of `&`, and reading a projection as a trivial operand. a
    /// non-trivial index spills to a temp; a base that is not itself a place (a
    /// call or literal returning an aggregate) is evaluated into a temp and
    /// projected from that local, preserving evaluation order.
    pub(crate) fn lower_place(&mut self, e: ExprId, buf: &mut ThinVec<MirStmt>) -> Place {
        let body = self.body;
        match &body.exprs[e] {
            Expr::Path(Resolution::Local(hid)) => Place::Local(self.map_local(*hid)),
            Expr::Path(Resolution::Global(gid)) => {
                Place::Global(self.hir.globals[*gid].name.clone())
            }
            Expr::Index { base, index } => {
                let (base, index) = (*base, *index);
                let base_place = self.lower_place(base, buf);
                let idx = self.lower_operand(index, buf);
                Place::Index(Box::new(base_place), Box::new(idx))
            }
            Expr::Field { base, name } => {
                let (base, name) = (*base, name.clone());
                let base_place = self.lower_place(base, buf);
                Place::Field(Box::new(base_place), name)
            }
            Expr::Deref { operand } => {
                let base_place = self.lower_place(*operand, buf);
                Place::Deref(Box::new(base_place))
            }
            // the base is not a place (e.g. a call returning an aggregate):
            // evaluate it into a temp and treat that local as the place.
            _ => {
                let rv = self.lower_rvalue(e, buf);
                let ty = self.mir_type_of(e);
                let mid = self.locals.alloc(MirLocal {
                    ty,
                    name: None,
                    mutable: true,
                });
                buf.push(MirStmt::Let {
                    local: mid,
                    init: Some(rv),
                });
                Place::Local(mid)
            }
        }
    }
}
