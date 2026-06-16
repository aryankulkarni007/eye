/// Minimal flamegraph target — no CLI parsing, no rendering, no dumps.
/// Just `eye::compile_file` so the flamegraph is uncontaminated by argument
/// parsing or conditional branches. Takes one argument: path to `.eye` file.
use std::path::PathBuf;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("eyesrc/programs/raytracer.eye"));

    if let Err(e) = eye::compile_file(&PathBuf::from(&path)) {
        eprintln!("flamebench: {e}");
        std::process::exit(1);
    }
}
