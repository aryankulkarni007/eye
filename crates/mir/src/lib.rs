pub mod core;
pub mod lower;

use hir::core::{FnId, HIR};
use rustc_hash::FxHashMap;

use crate::core::MirBody;

/// Lower every defined function in `hir` to MIR, keyed by [`FnId`].
///
/// The whole-file convenience over [`lower::lower_function`] for direct
/// pipeline callers (tests, fuzzing, benches, `eye::compile_file`). The
/// database's `mir_map` query computes the same map and memoizes it, which is
/// what lets `--dump-mir` and C generation share one lowering pass (the job
/// the deleted `MirCache` used to do).
pub fn lower_all(hir: &HIR) -> FxHashMap<FnId, MirBody> {
    hir.functions
        .iter()
        .filter_map(|(id, f)| {
            let body_id = f.body?;
            let mir =
                lower::lower_function(hir, &hir.types, &hir.bodies[body_id], f.params.len(), f.ret);
            Some((id, mir))
        })
        .collect()
}

#[cfg(test)]
mod tests;
