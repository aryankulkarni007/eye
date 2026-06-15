pub mod core;
pub mod lower;

use hir::core::{FnId, HIR};
use rustc_hash::FxHashMap;
use typeck::TypeckResults;

use crate::core::MirBody;

/// lower every defined function in `hir` to MIR, keyed by [`FnId`].
///
/// `typeck` is the per-fn type side table from `typeck::check_file` - since
/// the S2 cutover MIR reads expression types from [`TypeckResults`], not
/// from the HIR body's stamps. the whole-file convenience over
/// [`lower::lower_function`] for direct pipeline callers (tests, fuzzing,
/// benches, `eye::compile_file`). the database's `mir_map` query computes
/// the same map and memoizes it, which is what lets `--dump-mir` and c
/// generation share one lowering pass.
pub fn lower_all(hir: &HIR, typeck: &FxHashMap<FnId, TypeckResults>) -> FxHashMap<FnId, MirBody> {
    hir.functions
        .iter()
        .filter_map(|(id, f)| {
            let body_id = f.body?;
            let results = typeck.get(&id)?;
            let mir = lower::lower_function(
                hir,
                &hir.types,
                &hir.bodies[body_id],
                results,
                f.params.len(),
                f.ret,
            );
            Some((id, mir))
        })
        .collect()
}

#[cfg(test)]
mod tests;
