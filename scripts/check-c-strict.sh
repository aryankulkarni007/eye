#!/bin/bash
# Strict-C gate: compile every .eye file under eyesrc/, then syntax-check the
# generated C under pedantic clang flags. Catches the "Eye accepts, C is
# wrong" bug class (missing coercions, illegal identifiers, nonstandard
# constructs, printf/varargs type mismatches) that the default -O0 build
# silently tolerates.
#
# Suppressed warnings (deliberate, each a known non-bug):
#   -Wno-unused-parameter       Eye has no unused-binding lint yet; user code
#                               with unused params is legal Eye.
#   -Wno-unused-variable        same, for locals.
#   -Wno-unused-const-variable  string statics are emitted per unique literal
#                               even when println inlines the literal into the
#                               format string (todo: emit only referenced ones).
#
# Usage:  ./scripts/check-c-strict.sh [-v]

set -euo pipefail

cd "$(dirname "$0")/.."
DRIVER="${CARGO_TARGET_DIR:-target}/debug/eye"

if [ ! -x "$DRIVER" ]; then
    echo "Building eye driver first..."
    cargo build -q
fi

verbose=false
if [ "${1:-}" = "-v" ]; then verbose=true; fi

FLAGS=(-fsyntax-only -std=c11 -pedantic-errors -Wall -Wextra -Werror
       -Wno-unused-parameter -Wno-unused-variable -Wno-unused-const-variable)

errors=0
total=0

while IFS= read -r -d '' f; do
    # Generate (or refresh) the C next to the source. A file that fails Eye
    # compilation is check_all.sh's concern, not this gate's; skip it.
    if ! "$DRIVER" "$f" >/dev/null 2>&1; then
        continue
    fi
    c="${f%.eye}.c"
    [ -f "$c" ] || continue
    total=$((total + 1))
    if $verbose; then echo -n "  $c ... "; fi
    if ! output=$(clang "${FLAGS[@]}" "$c" 2>&1); then
        echo "STRICT-FAIL: $c"
        echo "$output" | head -6
        errors=$((errors + 1))
    elif $verbose; then
        echo "ok"
    fi
done < <(find eyesrc -name '*.eye' -print0)

if [ "$errors" -eq 0 ]; then
    echo "Strict-C gate: all $total generated C files pass."
else
    echo "Strict-C gate: $errors / $total generated C files FAILED."
    exit 1
fi
