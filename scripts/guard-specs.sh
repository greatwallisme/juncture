#!/usr/bin/env bash
# Repo-wide agent-spec contract gate.
#
# `agent-spec guard` scans only the immediate directory it is given: a spec in
# specs/<subdir>/ is silently skipped, and the command still exits 0. Because a
# skipped contract is indistinguishable from a missing contract, this wrapper
# runs guard and then asserts that the number of specs guard actually scanned
# equals the number of .spec/.spec.md files on disk. A mismatch is a hard
# failure rather than a silent pass.
#
# A scenario with a `skip` verdict is already not-passing inside guard, so an
# unbound test selector cannot slip through this wrapper.
#
# Usage:
#   scripts/guard-specs.sh                      # gate the committed tree (CI)
#   scripts/guard-specs.sh --change-scope worktree   # gate local edits
#
# Extra arguments are forwarded to `agent-spec guard`.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SPEC_DIR="$PROJECT_ROOT/specs"

if [ ! -d "$SPEC_DIR" ]; then
    echo "guard-specs: $SPEC_DIR not found" >&2
    exit 2
fi

cd "$PROJECT_ROOT"

on_disk="$(find "$SPEC_DIR" -type f \( -name '*.spec' -o -name '*.spec.md' \) | wc -l | tr -d ' ')"
if [ "$on_disk" -eq 0 ]; then
    echo "guard-specs: no specs found under specs/" >&2
    exit 2
fi

echo "guard-specs: $on_disk spec file(s) on disk under specs/" >&2

set +e
output="$(agent-spec guard --spec-dir specs --code . --min-score 0.7 "$@" 2>&1)"
guard_status=$?
set -e

printf '%s\n' "$output"

if [ "$guard_status" -ne 0 ]; then
    echo "guard-specs: FAILED (agent-spec guard exit $guard_status)" >&2
    exit "$guard_status"
fi

scanned="$(printf '%s\n' "$output" | sed -n 's/.*guard: \([0-9][0-9]*\) spec(s) passed.*/\1/p' | tail -n 1)"

if [ -z "$scanned" ]; then
    echo "guard-specs: FAILED (could not read scanned spec count from guard output)" >&2
    exit 1
fi

if [ "$scanned" -ne "$on_disk" ]; then
    echo "guard-specs: FAILED (guard scanned $scanned of $on_disk specs)" >&2
    echo "guard-specs: a spec outside specs/ was silently skipped; keep specs flat or scan each subdirectory explicitly" >&2
    exit 1
fi

echo "guard-specs: OK ($scanned of $on_disk specs scanned)" >&2
