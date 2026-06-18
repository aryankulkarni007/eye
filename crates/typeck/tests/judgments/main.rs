//! type-judgment diagnostics owned by the typeck pass. tests migrate here
//! from `crates/hir/src/core/tests.rs` as S2 step b moves each check
//! cluster out of lowering.

use ast::{AstNode, SourceFile};
use hir::core::{ConstError, HIR, HirError, PatternError, ResolveError, TypeError};
use lexer::{Lexer, SourceText};

mod branches;
mod calls;
mod casts;
mod let_init;
mod matches;
mod range_arith;
mod returns;

/// lower + typeck, returning the HIR with lowering diagnostics and the
/// typeck diagnostics merged into one stream (fn order, like the driver).
fn lower(src: &str) -> HIR {
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parser::parse(&lexed.tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    let mut hir = hir::core::lower_source_file(file, &lexed.interner);
    let typeck = typeck::check_file(&hir);
    let mut fn_ids: Vec<_> = typeck.keys().copied().collect();
    fn_ids.sort_by_key(|id| id.raw_idx().into_u32());
    for fn_id in fn_ids {
        hir.diagnostics.extend(typeck[&fn_id].diagnostics.clone());
    }
    hir
}

fn diags(hir: &HIR) -> Vec<&HirError> {
    hir.diagnostics.entries().iter().map(|(_, e)| e).collect()
}
