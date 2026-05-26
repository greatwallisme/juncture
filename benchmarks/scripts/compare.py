"""Comparison script for Juncture (Rust) vs LangGraph (Python) benchmarks.

Reads JSON results from both sides and produces a side-by-side comparison table.

Usage:
    python compare.py [--rust FILE] [--python-dir DIR]
    python compare.py  # defaults: results_rust.json, benchmarks/python/
"""

import json
import sys
from pathlib import Path


def load_results(path: str) -> dict:
    """Load benchmark results from a JSON file."""
    with open(path) as f:
        return json.load(f)


def consolidate_python_results(python_dir: str) -> dict:
    """Load and merge all Python result JSON files from a directory."""
    python_path = Path(python_dir)
    all_benchmarks = []

    for json_file in sorted(python_path.glob("results_python_*.json")):
        data = load_results(str(json_file))
        for bench in data.get("benchmarks", []):
            # Normalize: add mean_ms for comparison
            if "mean_ms" not in bench and "wall_ms" in bench:
                bench["mean_ms"] = bench["wall_ms"]
            all_benchmarks.append(bench)

    return {"benchmarks": all_benchmarks}


def format_ms(value: float) -> str:
    """Format milliseconds with appropriate precision."""
    if value < 1:
        return f"{value:.4f}"
    if value < 100:
        return f"{value:.2f}"
    return f"{value:.1f}"


def print_comparison(rust_data: dict, python_data: dict) -> None:
    """Print a formatted comparison table."""
    rust_by_scenario = {}
    for b in rust_data.get("benchmarks", []):
        # Rust uses wall_ms
        if "wall_ms" in b and "mean_ms" not in b:
            b["mean_ms"] = b["wall_ms"]
        rust_by_scenario[b["scenario"]] = b

    python_by_scenario = {}
    for b in python_data.get("benchmarks", []):
        if "mean_ms" not in b and "wall_ms" in b:
            b["mean_ms"] = b["wall_ms"]
        python_by_scenario[b["scenario"]] = b

    all_scenarios = sorted(
        set(rust_by_scenario.keys()) | set(python_by_scenario.keys())
    )

    if not all_scenarios:
        sys.stdout.write("No benchmark results found.\n")
        return

    # Header
    sys.stdout.write("\n")
    sys.stdout.write(
        f"{'Scenario':<30} {'Rust (ms)':>12} {'Python (ms)':>12} {'Speedup':>10}\n"
    )
    sys.stdout.write("=" * 66 + "\n")

    for scenario in all_scenarios:
        rust = rust_by_scenario.get(scenario)
        python = python_by_scenario.get(scenario)

        rust_mean = rust.get("mean_ms") if rust else None
        python_mean = python.get("mean_ms") if python else None

        rust_str = format_ms(rust_mean) if rust_mean is not None else "N/A"
        python_str = format_ms(python_mean) if python_mean is not None else "N/A"

        if rust_mean is not None and python_mean is not None and rust_mean > 0:
            speedup = python_mean / rust_mean
            speedup_str = f"{speedup:.1f}x"
        else:
            speedup_str = "N/A"

        marker = ""
        if rust and not python:
            marker = " (Python N/A)"
        elif python and not rust:
            marker = " (Rust N/A)"

        sys.stdout.write(
            f"{scenario:<30} {rust_str:>12} {python_str:>12} {speedup_str:>10}{marker}\n"
        )

    sys.stdout.write("\n")

    # CPU/Memory side-by-side comparison
    sys.stdout.write("CPU/Memory Comparison:\n")
    sys.stdout.write(
        f"{'Scenario':<26} "
        f"{'Rust':>14} {'Python':>14} "
        f"{'Rust':>14} {'Python':>14} "
        f"{'Rust':>12} {'Python':>12}\n"
    )
    sys.stdout.write(
        f"{'':26} "
        f"{'CPU(ms)':>14} {'CPU(ms)':>14} "
        f"{'RSS(MB)':>14} {'RSS(MB)':>14} "
        f"{'PerNode':>12} {'PerNode':>12}\n"
    )
    sys.stdout.write("=" * 112 + "\n")

    for scenario in all_scenarios:
        rust = rust_by_scenario.get(scenario)
        python = python_by_scenario.get(scenario)
        if not rust and not python:
            continue

        rust_cpu = rust.get("cpu_ms", 0) if rust else None
        python_cpu = python.get("cpu_ms") if python else None
        rust_rss = rust.get("peak_rss_mb", 0) if rust else None
        python_rss = python.get("peak_rss_mb") if python else None
        rust_pn = rust.get("per_node_wall_us", 0) if rust else None
        python_pn = python.get("per_node_wall_us") if python else None

        def fmt(v):
            if v is None:
                return "N/A"
            if abs(v) >= 1000:
                return f"{v:,.0f}"
            return f"{v:.1f}"

        sys.stdout.write(
            f"{scenario:<26} "
            f"{fmt(rust_cpu):>14} {fmt(python_cpu):>14} "
            f"{fmt(rust_rss):>14} {fmt(python_rss):>14} "
            f"{fmt(rust_pn):>12} {fmt(python_pn):>12}\n"
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
    sys.stdout.write(
        "  - Wide state: Python benchmark uses reduced input/iterations due to speed\n"
    )
    sys.stdout.write("\n")


def main() -> None:
    args = sys.argv[1:]

    rust_file = "benchmarks/results_rust.json"
    python_dir = "benchmarks/python"

    i = 0
    while i < len(args):
        if args[i] == "--rust" and i + 1 < len(args):
            rust_file = args[i + 1]
            i += 2
        elif args[i] == "--python-dir" and i + 1 < len(args):
            python_dir = args[i + 1]
            i += 2
        else:
            sys.stderr.write(f"Unknown argument: {args[i]}\n")
            sys.exit(1)

    rust_path = Path(rust_file)

    if not rust_path.exists():
        sys.stderr.write(f"Rust results not found: {rust_file}\n")
        sys.exit(1)

    rust_data = load_results(str(rust_path))
    python_data = consolidate_python_results(python_dir)

    print_comparison(rust_data, python_data)


if __name__ == "__main__":
    main()
