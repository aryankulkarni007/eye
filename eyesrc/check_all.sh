#!/bin/bash
# Compile every .eye file under eyesrc/ to verify it parses, lowers, and
# generates C without errors.  Does NOT run the produced binaries.
#
# Usage:  ./eyesrc/check_all.sh          # check every .eye file
#         ./eyesrc/check_all.sh -v       # verbose (show each file)

set -euo pipefail

cd "$(dirname "$0")/.."
DRIVER="${CARGO_TARGET_DIR:-target}/debug/eye"

if [ ! -x "$DRIVER" ]; then
    echo "Building eye driver first..."
    cargo build -q
fi

verbose=false
if [ "${1:-}" = "-v" ]; then verbose=true; fi

# Files expected NOT to compile, each with a documented reason. An XFAIL that
# starts passing is reported as stale so the list cannot rot.
#   linkedlist.eye  intentionally broken: self-referential initializer needs
#                   null / two-phase init (documented in the file header).
#   lang.eye        uses `const [char*; 24]` - an aggregate const, beyond the
#                   scalar-only const floor (docs/planning/DEFER.md). Its
#                   original blocker (string decay at struct-literal fields,
#                   CLEAK L1) is FIXED. Remove when const aggregates land.
XFAIL=(
    "eyesrc/programs/linkedlist.eye"
    "eyesrc/programs/lang.eye"
)

is_xfail() {
    local needle="$1"
    for x in "${XFAIL[@]}"; do
        if [ "$x" = "$needle" ]; then return 0; fi
    done
    return 1
}

errors=0
stale=0
total=0

while IFS= read -r -d '' f; do
    total=$((total + 1))
    rel="${f#./}"
    if $verbose; then echo -n "  $rel ... "; fi
    if ! output=$("$DRIVER" "$f" 2>&1); then
        if is_xfail "$rel"; then
            if $verbose; then echo "xfail (expected)"; fi
            continue
        fi
        echo "FAIL: $rel"
        echo "$output" | head -10
        errors=$((errors + 1))
    elif is_xfail "$rel"; then
        echo "STALE XFAIL: $rel now compiles; remove it from the XFAIL list."
        stale=$((stale + 1))
    elif $verbose; then
        echo "ok"
    fi
done < <(find eyesrc -name '*.eye' -print0)

if [ "$errors" -eq 0 ] && [ "$stale" -eq 0 ]; then
    echo "All $total files behave as expected (${#XFAIL[@]} expected failures)."
else
    echo "$errors / $total files FAILED to compile; $stale stale XFAIL entries."
    exit 1
fi
