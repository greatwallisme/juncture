# CLAUDE.md -- benchmarks

Performance comparison suite: Juncture (Rust) vs LangGraph (Python).

## Run Commands

```bash
# Rust profiling (all scenarios)
cargo run --release -p juncture-benchmarks --bin profile

# Single scenario
cargo run --release -p juncture-benchmarks --bin profile -- sequential

# Criterion statistical benchmarks
cargo bench -p juncture-benchmarks
cargo bench -p juncture-benchmarks --bench fanout

# Python benchmarks (from benchmarks/python/)
cd benchmarks/python && uv run python sequential.py

# Full comparison
python3 benchmarks/scripts/compare.py
```

## Architecture

Three measurement layers:

- `benches/*.rs` -- Criterion benchmarks (statistical wall-clock timing, 100+ samples)
- `src/bin/profile.rs` -- Standalone profiler: wall-clock, CPU time (`/proc/self/stat`), peak RSS (`/proc/self/status`)
- `src/profiling.rs` -- `ProfileResult` struct, `profile_execution()` helper, JSON/CSV report generators

Python equivalents in `python/*.py` mirror each Rust scenario using `langgraph` + `uvloop`. Metrics: wall-clock via `time.perf_counter()`, CPU via `resource.getrusage()`, RSS via `ru_maxrss`, per-node derived as `mean_ms * 1000 / node_count`.

Shared utility: `python/bench_utils.py` provides `get_cpu_time_ms()`, `get_peak_rss_mb()`, `measure_run()`.

## 6 Scenarios

| Scenario | Rust File | Python File | Measures |
|----------|-----------|-------------|----------|
| Sequential | `benches/sequential.rs` | `python/sequential.py` | Per-node overhead (10/100/1000/3000 nodes) |
| Wide State | `benches/wide_state.rs` | `python/wide_state.py` | State clone/reduce cost (15+ fields, 300/600/1200 iter) |
| Fanout | `benches/fanout.rs` | `python/fanout.py` | Parallel subgraph scheduling (10/100 subjects) |
| Checkpoint | `benches/checkpoint.rs` | `python/checkpoint.py` | MemorySaver on/off (100-node chain) |
| Conditional Routing | `benches/conditional_routing.rs` | `python/conditional_routing.py` | Dynamic edge routing (3/10/50 branches) |
| Streaming | `benches/streaming.rs` | `python/streaming.py` | Event emission throughput (100/1000/10000 nodes) |

## Key Design: No-op Nodes

All node functions do minimal work (return empty state update). This isolates framework overhead, not language speed. See `benchmarks/README.md` for fairness limitations.

## Output

- Rust: `benchmarks/results_rust.json` (set `JUNCTURE_BENCH_OUTPUT` env var for custom path)
- Python: `benchmarks/python/results_python_*.json` (one file per scenario, merged by `compare.py`)
- `compare.py` reads both sides and produces unified wall-clock + CPU/RSS/PerNode comparison table
