#!/bin/bash
# Regenerate insta snapshot files across the workspace.
#
# Usage:  ./eyesrc/update_snapshots.sh

set -euo pipefail

cd "$(dirname "$0")/.."
INSTA_UPDATE=always cargo test --workspace 2>&1
