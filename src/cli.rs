use std::path::PathBuf;

use clap::Parser;

/// Command-line surface for the `eye` driver. Dump flags are off by default
/// so a normal compile stays quiet; pass any subset to surface the matching
/// IR for debugging.
#[derive(Parser, Debug)]
#[command(
    name = "eye",
    about = "Eye compiler driver (transpiles .eye -> C -> native via clang)"
)]
pub struct Cli {
    /// Source file to compile. Must have a `.eye` extension.
    pub input: PathBuf,

    /// Print the lossless rowan CST before parsing diagnostics are checked.
    #[arg(long)]
    pub dump_cst: bool,

    /// Print the typed AST as a structured summary.
    #[arg(long)]
    pub dump_ast: bool,

    /// Print the interner contents (every identifier and string literal).
    #[arg(long)]
    pub dump_symbols: bool,

    /// Print the fully-lowered HIR (items, bodies, expr arenas, types).
    #[arg(long)]
    pub dump_hir: bool,

    /// Stop after lexing and parsing: exit 0 if the source is syntactically
    /// valid, non-zero otherwise. Skips HIR lowering, codegen and clang, and
    /// writes no `.c` or binary. This is the parse-stage oracle the grammar
    /// parity gate (scripts/check-grammars.sh) checks the tree-sitter grammar
    /// against, so it deliberately matches what tree-sitter sees: lexer +
    /// parser only, no semantic analysis.
    #[arg(long)]
    pub check: bool,
}
