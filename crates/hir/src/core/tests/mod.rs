use super::*;
use ast::{AstNode, SourceFile};
use lexer::{Lexer, SourceText};

mod arrays;
mod consts;
mod format;
mod functions;
mod matches;
mod naming;
mod pointers;
mod structs;

fn lower(src: &str) -> HIR {
    let source = SourceText::new(src.to_string());
    let lexed = Lexer::new(&source).tokenize();
    let parse = parser::parse(&lexed.tokens, &source);
    let file = SourceFile::cast(parse.green).expect("root is SourceFile");
    lower_source_file(file, &lexed.interner)
}

/// the concrete diagnostic kinds, for structural assertions. tests match on
/// variants (and payloads) rather than message text, so rewording a message
/// never breaks a test.
fn diags(hir: &HIR) -> Vec<&HirError> {
    hir.diagnostics.entries().iter().map(|(_, e)| e).collect()
}

const MAIN_EYE: &str = "\
structure Point {
    int32 x,
    int32 y,
};

main() {
    let int32 x = 0;
    let int32 y = 0;
    mut Point p = Point { x, y };

    println(\"{}\", p.x);
}
";

// call return-type resolution (user + extern fns) is now a typeck concern,
// covered end-to-end by the `let`-type check in `crates/typeck/tests/judgments.rs`
// (`call_return_type_resolves_*`) and the program corpus.

// ---- v0.3 match lowering ----

/// walk the HIR for the `main` body and return the first `Expr::Match`
/// it finds. tests assume exactly one per fixture.
fn first_match(hir: &HIR) -> (&Body, ExprId, &[MatchArm], ExprId) {
    let main_id = *hir.items.functions.get("main").expect("main fn");
    let body_id = hir.functions[main_id].body.expect("main body");
    let body = &hir.bodies[body_id];
    for (id, expr) in body.exprs.iter() {
        if let Expr::Match { scrut, arms } = expr {
            return (body, id, arms.as_slice(), *scrut);
        }
    }
    panic!("no Expr::Match in main body");
}

const SHAPE_DECL: &str = "enum Shape = Circle | Rectangle | Triangle ;\n";
