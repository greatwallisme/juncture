#!/bin/bash
# Simple contention analysis script for Rust concurrent programs

if [ $# -eq 0 ]; then
    echo "Usage: $0 <binary_path> [args...]"
    exit 1
fi

BINARY="$1"
shift

# Check if perf is available
if ! command -v perf &> /dev/null; then
    echo "Error: perf is not available. Install linux-tools for your distribution."
    exit 1
fi

echo "Analyzing contention for: $BINARY"

# Run perf to record contention events
perf record -e sched:sched_switch -e lock:lock_acquire -e lock:lock_release \
           --call-graph dwarf \
           "$@" &

PERF_PID=$!

# Wait for a bit or until process finishes
sleep 2

if kill -0 $PERF_PID 2>/dev/null; then
    echo "Press Ctrl+C to stop analysis..."
    wait $PERF_PID
fi

echo "Generating contention report..."
perf script > contention_report.txt

echo "Contention report saved to contention_report.txt"
echo ""
echo "Top contention events:"
perf report --sort=dso,symbol --stdio | head -20

echo ""
echo "Lock contention patterns:"
grep -E "(lock_acquire|lock_release)" contention_report.txt | \
    awk '{print $1, $2, $NF}' | sort | uniq -c | sort -nr | head -10

rm contention_report.txt