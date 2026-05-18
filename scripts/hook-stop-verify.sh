#!/usr/bin/env bash
# Stop hook: runs full design coverage verification before session ends.
# Skips if no Rust source directory exists yet.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$PROJECT_ROOT/src"

if [ ! -d "$SRC_DIR" ]; then
    echo "No src/ directory found - skipping design coverage verification." 1>&2
    exit 0
fi

cd "$PROJECT_ROOT"

# Count Rust source files
RS_COUNT=$(find "$SRC_DIR" -name "*.rs" 2>/dev/null | wc -l)
if [ "$RS_COUNT" -eq 0 ]; then
    echo "No .rs files found - skipping design coverage verification." 1>&2
    exit 0
fi

# Run full verification report
echo "Running design coverage verification ($RS_COUNT source files)..." 1>&2
python3 scripts/verify-design-coverage.py 1>&2

EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
    echo "" 1>&2
    echo "WARNING: Design coverage verification found missing items." 1>&2
    echo "Run 'python3 scripts/verify-design-coverage.py' for details." 1>&2
fi

exit 0
