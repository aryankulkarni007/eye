#!/usr/bin/env bash
set -euo pipefail

# ── configuration ──────────────────────────────────────────────
EYE_SRC_DIR="eyesrc"
FLAMEGRAPH_OUTPUT="flamegraph.svg"
FUNC_CALL="eye::compile_file"
# Number of times to repeat your entire codebase in the stress file
ITERATIONS=100
# ────────────────────────────────────────────────────────────────

if ! command -v cargo-flamegraph &>/dev/null; then
    echo "cargo-flamegraph not found. install with: cargo install flamegraph"
    exit 1
fi

PROFILE_DIR="$(mktemp -d flamebatch.XXXX)"
# Ensure we clean up the temp dir AND the generated stress file
STRESS_FILE="$EYE_SRC_DIR/STRESS_TEST_GENERATED.eye"
trap 'rm -rf "$PROFILE_DIR" "$STRESS_FILE"' EXIT

echo "generating stress test file: $STRESS_FILE"
echo "// AUTO-GENERATED STRESS TEST" > "$STRESS_FILE"

# concatenate all files in the source dir multiple times
for i in $(seq 1 $ITERATIONS); do
    # find all .eye files, excluding the one we are currently writing to
    find "$EYE_SRC_DIR" -type f -name "*.eye" ! -name "STRESS_TEST_GENERATED.eye" -exec cat {} + >> "$STRESS_FILE"
    echo -e "\n" >> "$STRESS_FILE"
done

echo "creating profiling harness in $PROFILE_DIR"

cat > "$PROFILE_DIR/Cargo.toml" <<EOF
[workspace]
[package]
name = "flamebatch"
version = "0.1.0"
edition = "2021"

[dependencies]
eye = { path = ".." }
EOF

mkdir -p "$PROFILE_DIR/src"

# Main now only compiles the single massive file
cat > "$PROFILE_DIR/src/main.rs" <<EOF
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from("../$STRESS_FILE");
    if !path.exists() {
        panic!("stress file not found at {:?}", path);
    }

    eprintln!("profiling compilation of aggregated stress file...");
    $FUNC_CALL(&path).expect("compilation failed");
}
EOF

echo "running cargo flamegraph..."
(cd "$PROFILE_DIR" && cargo flamegraph --bin flamebatch)

mv "$PROFILE_DIR/flamegraph.svg" "$FLAMEGRAPH_OUTPUT"
echo "done. aggregated file was $(du -h "$STRESS_FILE" | cut -f1). open $FLAMEGRAPH_OUTPUT in a browser."
