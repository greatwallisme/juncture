"""Comparison script for Juncture (Rust) vs LangGraph (Python) benchmarks.

Reads JSON results from both sides and produces a side-by-side comparison table.

Usage:
    python compare.py [--rust FILE] [--python FILE]
"""

import json
import sys
from pathlib import Path


def load_results(path: str) -> dict:
    """Load benchmark results from a JSON file."""
    with open(path) as f:
        return json.load(f)


def format_ms(value: float) -> str:
    """Format milliseconds with appropriate precision."""
    if value < 1:
        return f"{value:.4f}"
    if value < 100:
        return f"{value:.2f}"
    return f"{value:.1f}"


def print_comparison(rust_data: dict, python_data: dict) -> None:
    """Print a formatted comparison table."""
    rust_by_scenario = {
        b["scenario"]: b for b in rust_data.get("benchmarks", [])
    }
    python_by_scenario = {
        b["scenario"]: b for b in python_data.get("benchmarks", [])
    }

    all_scenarios = sorted(
        set(rust_by_scenario.keys()) | set(python_by_scenario.keys())
    )

    if not all_scenarios:
        sys.stdout.write("No benchmark results found.\n")
        return

    # Header
    sys.stdout.write("\n")
    sys.stdout.write(
        f"{'Scenario':<35} {'Rust (ms)':<18} {'Python (ms)':<18} {'Speedup':<10}\n"
    )
    sys.stdout.write("-" * 81 + "\n")

    for scenario in all_scenarios:
        rust = rust_by_scenario.get(scenario)
        python = python_by_scenario.get(scenario)

        rust_mean = rust["mean_ms"] if rust else None
        python_mean = python["mean_ms"] if python else None

        rust_str = format_ms(rust_mean) if rust_mean is not None else "N/A"
        python_str = format_ms(python_mean) if python_mean is not None else "N/A"

        if rust_mean is not None and python_mean is not None and rust_mean > 0:
            speedup = python_mean / rust_mean
            speedup_str = f"{speedup:.1f}x"
        else:
            speedup_str = "N/A"

        sys.stdout.write(
            f"{scenario:<35} {rust_str:<18} {python_str:<18} {speedup_str:<10}\n"
        )

    sys.stdout.write("\n")

    # Known limitations
    sys.stdout.write("Known limitations:\n")
    sys.stdout.write(
        "  - Fanout: Python asyncio (single-threaded) vs Rust tokio (multi-core)\n"
    )
    sys.stdout.write(
        "  - No-op nodes include minimal language overhead, not pure framework cost\n"
    )
    sys.stdout.write(
        "  - Memory measurements are not directly comparable across languages\n"
    )
    sys.stdout.write(
        "  - Checkpoint serialization uses idiomatic per-language methods\n"
    )
    sys.stdout.write("\n")


def main() -> None:
    args = sys.argv[1:]

    rust_file = "results_rust.json"
    python_file = "results_python.json"

    i = 0
    while i < len(args):
        if args[i] == "--rust" and i + 1 < len(args):
            rust_file = args[i + 1]
            i += 2
        elif args[i] == "--python" and i + 1 < len(args):
            python_file = args[i + 1]
            i += 2
        else:
            sys.stderr.write(f"Unknown argument: {args[i]}\n")
            sys.exit(1)

    rust_path = Path(rust_file)
    python_path = Path(python_file)

    if not rust_path.exists():
        sys.stderr.write(f"Rust results not found: {rust_file}\n")
        sys.exit(1)

    if not python_path.exists():
        sys.stderr.write(f"Python results not found: {python_file}\n")
        sys.exit(1)

    rust_data = load_results(str(rust_path))
    python_data = load_results(str(python_path))

    print_comparison(rust_data, python_data)


if __name__ == "__main__":
    main()
