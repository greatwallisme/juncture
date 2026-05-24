#!/usr/bin/env bash
# Run all benchmarks (Rust + Python) and produce comparison report.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Running Rust benchmarks ==="
cd "$BENCH_DIR"
cargo bench -p juncture-benchmarks -- --output-format bencher | tee target/criterion_results.txt

echo ""
echo "=== Running Python benchmarks ==="
cd "$BENCH_DIR/python"

for script in sequential.py wide_state.py fanout.py checkpoint.py conditional_routing.py streaming.py; do
    if [ -f "$script" ]; then
        echo "--- Running $script ---"
        uv run python "$script"
    fi
done

echo ""
echo "=== Comparison report ==="
cd "$BENCH_DIR"
python3 scripts/compare.py --rust target/criterion_results.json --python python/results_python.json || echo "Comparison requires aggregated JSON results."
