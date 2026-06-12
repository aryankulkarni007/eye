use std::path::PathBuf;

use clap::Parser;

/// Command-line surface for the `eye` driver. Dump flags are off by default
/// so a normal compile stays quiet; pass any subset to surface the matching
/// IR for debugging.
#[derive(Parser, Debug)]
#[command(
    name = "eye",
    about = "eye compiler driver (transpiles .eye -> c -> native via clang)"
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

    /// Print the fully-lowered HIR as a readable summary (counts, names, types).
    #[arg(long)]
    pub dump_hir: bool,

    /// Print the fully-lowered HIR in full Debug representation.
    #[arg(long)]
    pub dump_hir_raw: bool,

    /// Print the lowered MIR body for each function (readable summary).
    #[arg(long)]
    pub dump_mir: bool,

    /// Print the lowered MIR body for each function (full Debug representation).
    #[arg(long)]
    pub dump_mir_raw: bool,

    /// Print the generated C source to stdout (in addition to writing the .c
    /// file and compiling the binary).
    #[arg(long)]
    pub dump_c: bool,

    /// Stop after HIR lowering: exit 0 if the source is free of lexer,
    /// parser, and lowering diagnostics, non-zero otherwise. Skips codegen
    /// and clang, and writes no `.c` or binary.
    #[arg(long)]
    pub check: bool,

    /// Stop after lexing and parsing: exit 0 if the source is syntactically
    /// valid, non-zero otherwise. This is the parse-stage oracle the grammar
    /// parity gate (scripts/check-grammars.sh) checks the tree-sitter grammar
    /// against, so it deliberately matches what tree-sitter sees: lexer +
    /// parser only, no semantic analysis.
    #[arg(long)]
    pub parse_only: bool,

    /// Pipe the generated C through `clang-format` before writing it. Off by
    /// default: the format pass forks a process and pipes the whole source on
    /// every compile, which is pure cosmetics for the `.c` dump. Enable it when
    /// you want a readable `.c`.
    #[arg(long)]
    pub format: bool,

    /// Build the binary with `clang -O2`. Off by default: the dev/test loop
    /// uses `-O0`, which links far faster. Enable it for a shipping build.
    #[arg(long)]
    pub release: bool,
}
