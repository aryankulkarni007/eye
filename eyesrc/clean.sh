#!/bin/bash
# clean generated C files and binaries from the eyesrc directory tree.

cd "$(dirname "$0")"

echo "cleaning generated files in: $(pwd)"

# Remove C files at every level
find . -name '*.c' -exec rm -f {} \;

# Remove binaries that shadow .eye files (same stem, no extension)
find . -name '*.eye' | while read -r eye_file; do
    base_name="${eye_file%.eye}"
    if [ -f "$base_name" ] && [ "$(basename "$base_name")" != "clean" ]; then
        echo "removing binary: $base_name"
        rm -f "$base_name"
    fi
done

echo "cleanup complete"
