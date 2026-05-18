#!/usr/bin/env bash
# PostToolUse hook: runs design coverage verification after .rs file writes/edits.
# Only activates when the modified file is a Rust source file.
# Outputs summary to stderr for inline feedback.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC_DIR="$PROJECT_ROOT/src"

if [ ! -d "$SRC_DIR" ]; then
    exit 0
fi

# Read hook input from stdin (JSON with tool_name, tool_input, tool_output)
INPUT=$(cat)

# Extract file_path from tool_input
FILE_PATH=$(echo "$INPUT" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    print(data.get('tool_input', {}).get('file_path', ''))
except Exception:
    print('')
" 2>/dev/null || echo "")

# Only run for .rs files
if [[ "$FILE_PATH" != *.rs ]]; then
    exit 0
fi

# Run verification in summary-only mode
cd "$PROJECT_ROOT"
python3 scripts/verify-design-coverage.py --summary-only 1>&2
