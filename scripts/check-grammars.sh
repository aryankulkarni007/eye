#!/usr/bin/env bash
#
# Grammar parity gate.
#
# The tree-sitter grammar (~/dev/eye-tools/treesitter/grammar.js) is a hand port
# of the canonical Rust parser (crates/parser/src/grammar.rs). Hand ports drift:
# add an operator to the Rust grammar, forget grammar.js, and the editor keeps
# parsing the old language until you happen to notice ERROR nodes days later.
#
# This makes that drift loud and immediate, using the `eye` compiler as the
# source of truth. `eye --check` is a parse-stage oracle (lexer + parser only,
# no HIR/codegen), which is exactly what tree-sitter can see. The invariant:
#
#     compiler accepts a file  =>  tree-sitter must parse it with no ERROR node
#
# We only gate on that one direction. The reverse (tree-sitter accepts what the
# compiler rejects) is expected -- tree-sitter does error recovery and is
# designed to be permissive -- so it is not a failure.
#
# Exit non-zero on any drift.
set -euo pipefail

REPO="$HOME/dev/eye"
TS_DIR="$HOME/dev/eye-tools/treesitter"
CORPUS="$REPO/eyesrc"

cd "$REPO"

echo "==> building eye compiler (parse-stage oracle)"
cargo build -q -p eye
EYE="$REPO/target/debug/eye"

echo "==> regenerating tree-sitter parser from grammar.js"
( cd "$TS_DIR" && tree-sitter generate >/dev/null )

echo "==> checking corpus parity"
fail=0
for f in "$CORPUS"/*.eye; do
  [[ -e "$f" ]] || continue
  name="$(basename "$f")"

  # Only files the canonical compiler accepts constrain the tree-sitter grammar.
  "$EYE" --check "$f" >/dev/null 2>&1 || continue

  if ( cd "$TS_DIR" && tree-sitter parse "$f" 2>/dev/null | grep -qE 'ERROR|MISSING' ); then
    echo "    DRIFT: $name compiles but tree-sitter has ERROR/MISSING"
    fail=1
  fi
done

if [[ "$fail" -ne 0 ]]; then
  echo "==> FAIL: tree-sitter grammar drifted from the compiler." >&2
  echo "    Update $TS_DIR/grammar.js to match crates/parser/src/grammar.rs," >&2
  echo "    then run $TS_DIR/rebuild.sh" >&2
  exit 1
fi

echo "==> OK: tree-sitter grammar matches the compiler on all accepted corpus files."
