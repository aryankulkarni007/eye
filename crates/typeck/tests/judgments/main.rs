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
mod lints;
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

/// error-severity diagnostics only: the accepted-case assertions mean "no
/// errors"; the T1 lints (warning severity) fire on fixtures freely (an
/// unused binding in a test program is not a rejection). lint assertions use
/// [`warnings`].
fn diags(hir: &HIR) -> Vec<&HirError> {
    use diagnostics::Diagnostic as _;
    hir.diagnostics
        .entries()
        .iter()
        .filter(|(_, e)| e.severity() == diagnostics::Severity::Error)
        .map(|(_, e)| e)
        .collect()
}

/// warning-severity diagnostics (the T1 lints).
fn warnings(hir: &HIR) -> Vec<&HirError> {
    use diagnostics::Diagnostic as _;
    hir.diagnostics
        .entries()
        .iter()
        .filter(|(_, e)| e.severity() == diagnostics::Severity::Warning)
        .map(|(_, e)| e)
        .collect()
}
