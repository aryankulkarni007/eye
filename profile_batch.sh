#!/usr/bin/env bash
# DEPRECATED — use `cargo xtask flamegraph` instead.
#
# This script is kept for reference but no longer actively maintained.
# Run: cargo xtask flamegraph [--iterations N] [--output path]
#
# Differences from this script:
#   - Generates a *syntactically valid* stress program (this script concatenates
#     files, producing an invalid program that only exercises lexer/parser).
#   - Uses the `flamebench` binary (no CLI overhead) instead of a temp project.
#   - Supports configurable function count with --iterations.
#   - Uses release profile for representative profiles.

exec cargo xtask flamegraph "$@"
