# Juncture (Rust) vs LangGraph (Python) Benchmark Suite

This directory contains a benchmark suite for comparing the performance of
[Juncture](https://github.com/user/juncture) (a Rust implementation of
LangGraph) against the original [LangGraph](https://github.com/langchain-ai/langgraph)
Python library.

## Directory Structure

```
benchmarks/
  benches/                  # Rust criterion benchmarks (6 scenarios)
    sequential.rs           # Linear chain of N no-op nodes
    wide_state.rs           # 15+ field state with read/write nodes
    fanout.rs               # Parallel subgraph fanout via Send
    checkpoint.rs           # Persistence overhead (MemorySaver on/off)
    conditional_routing.rs  # Dynamic edge routing with N branches
    streaming.rs            # Stream event collection throughput
  src/
    profiling.rs            # CPU time, wall-clock, peak RSS measurement
    bin/profile.rs          # Standalone profiling runner (all scenarios)
  python/                   # LangGraph Python equivalents
    sequential.py ...       # One script per scenario
    pyproject.toml          # uv-managed environment
  scripts/
    run_all.sh              # Runs both Rust + Python benchmarks
    compare.py              # Produces side-by-side comparison table
```

## Quick Start

### Rust Benchmarks (Criterion)

```bash
# Run all Rust benchmarks
cargo bench -p juncture-benchmarks

# Run a specific benchmark
cargo bench -p juncture-benchmarks --bench sequential

# Run CPU/memory profiling (all scenarios)
cargo run --release -p juncture-benchmarks --bin profile

# Run profiling for a single scenario
cargo run --release -p juncture-benchmarks --bin profile -- sequential
```

### Python Benchmarks (LangGraph)

```bash
cd benchmarks/python
uv sync                    # One-time setup
uv run python sequential.py
```

### Full Comparison

```bash
bash benchmarks/scripts/run_all.sh
python benchmarks/scripts/compare.py

# Or with custom paths:
python benchmarks/scripts/compare.py --rust benchmarks/results_rust.json --python-dir benchmarks/python
```

## Benchmark Scenarios

All scenarios are ported from LangGraph's `bench/` directory with equivalent
graph topologies and node function complexity.

| Scenario | Description | Key Parameter | What It Measures |
|----------|-------------|---------------|------------------|
| Sequential | Linear chain of N no-op nodes | 10, 100, 1000, 3000 nodes | Per-node framework overhead |
| Wide State | 15+ field state with 6 node types | 300, 600, 1200 iterations | State clone/reduce overhead |
| Fanout | Parallel subgraph via Send | 10, 100 subjects | Task scheduling parallelism |
| Checkpoint | Same graph with/without MemorySaver | 100-node chain | Persistence overhead |
| Conditional Routing | Dynamic edge routing | 3, 10, 50 branches | Edge evaluation overhead |
| Streaming | Stream event collection | 100, 1000, 10000 nodes | Event emission throughput |

## How the Comparison Works

1. **Identical graph topologies**: Both sides construct the same graph shape
   (same node count, same edge structure, same state fields).

2. **No-op nodes**: Node functions do minimal work (return empty state update).
   This isolates framework overhead rather than measuring language speed.

3. **Pre-generated input**: All input data is constructed before timing starts.
   The timed region contains only `invoke`/`stream` calls.

4. **Statistical rigor**: Rust uses Criterion (100+ samples, outlier detection);
   Python uses repeated iterations with warmup.

5. **Two measurement tools**:
   - `cargo bench` -- Criterion timing (wall-clock, statistical analysis)
   - `cargo run --bin profile` -- CPU time, wall-clock, peak RSS, per-node breakdown

---

## Critical Limitations: Why This Comparison Is NOT Fair

This section is the most important part of this document. The numbers produced
by these benchmarks **do not** represent a fair or comprehensive comparison
between Juncture and LangGraph. Anyone interpreting these results must
understand the following limitations.

### 1. Different Execution Models

**Python (LangGraph)**: Single-threaded cooperative multitasking via asyncio
(and optionally uvloop). All "parallel" tasks are actually interleaved on one
thread. The GIL prevents true multi-core parallelism.

**Rust (Juncture)**: True multi-core parallelism via `tokio::spawn` + `JoinSet`.
Tasks are distributed across all available CPU cores.

**Impact**: The fanout scenario is fundamentally unfair. Juncture's parallel
speedup comes from hardware parallelism that LangGraph physically cannot use.
A fanout-100 comparison shows Juncture faster by a large margin, but this
measures language runtime capability, not framework design quality.

### 2. Node Function Baseline Cost Differs

Even "no-op" nodes carry language-specific overhead:

- **Python**: A `def noop(state): return None` still invokes Python's function
  call protocol, creates a dict-based state update, and triggers GC bookkeeping.
- **Rust**: `Ok(MessagesStateUpdate { messages: None })` is a stack-allocated
  struct with zero allocation overhead in release mode.

**Impact**: Per-node overhead includes both framework cost and language baseline
cost. The ratio conflates two factors that cannot be separated by this
methodology. A 10x speedup might be 3x framework improvement + 7x language
advantage.

### 3. Memory Measurement Incomparability

- **Rust**: Reads `/proc/self/status` VmHWM (peak RSS), which includes all
  allocations including the allocator's internal bookkeeping.
- **Python**: Would use `tracemalloc` which tracks Python objects only and
  excludes interpreter overhead, JIT buffers, and native extensions.

**Impact**: Memory numbers are reported separately per language and should
**never** be compared as a ratio.

### 4. Serialization Differences (Checkpoint Scenario)

- **Python**: Uses `pickle` or `json` (stdlib, highly optimized C implementation).
- **Rust**: Uses `serde_json` or `rmp-serde` (Rust-native serialization).

**Impact**: Checkpoint overhead reflects each ecosystem's idiomatic approach,
not framework design. A faster Rust checkpoint says more about serde than about
Juncture's checkpoint architecture.

### 5. Different Maturity Levels

- **LangGraph**: Production-grade library with years of optimization, millions
  of users, and dedicated performance work.
- **Juncture**: Early-stage implementation that has not yet undergone
  performance-focused iteration.

**Impact**: Performance gaps may shrink or grow as both projects evolve. These
benchmarks are a point-in-time snapshot, not a definitive statement.

### 6. Benchmark Methodology Gaps

| What We Measure | What We Do NOT Measure |
|------------------|----------------------|
| Wall-clock time | Memory allocator fragmentation |
| CPU time (user+system) | Cache miss rates / L1-L3 behavior |
| Peak RSS | Latency percentiles (p99, p999) |
| Per-node overhead | Concurrent request throughput |
| Single-invocation cost | Sustained multi-invocation load |
| In-memory checkpoint | Disk/network checkpoint I/O |

The profiling infrastructure reads coarse-grained metrics from `/proc/`. This
is sufficient for identifying O(N^2) bottlenecks but inadequate for
fine-grained performance analysis. Real-world workload profiling requires
tools like `perf`, `flamegraph`, `heaptrack`, or `valgrind`.

### 7. Compiler Optimization Variance

Rust benchmarks run in `--release` mode with LLVM optimizations. Python has no
equivalent compilation step (even PyPy or Cython would be different comparisons).
This means Rust benefits from aggressive inlining, monomorphization, and
dead code elimination that have no Python counterpart.

### 8. Graph Construction vs. Execution

These benchmarks pre-compile graphs before timing. In practice, graph
construction (topology validation, trigger table building, edge compilation)
also contributes to real-world latency. This cost is excluded from measurements.

---

## What These Benchmarks ARE Useful For

Despite the limitations above, this suite provides genuine value:

1. **Detecting Juncture regressions**: Compare Juncture's performance across
   commits to catch performance degradations early.

2. **Identifying algorithmic issues**: The O(N^2) TriggerToNodes bottleneck,
   the Send dedup bug, the state_json deserialization bug, and the O(N) state
   clone per node were all discovered and fixed through these benchmarks.

3. **Rough scaling characteristics**: Understanding how Juncture behaves as
   graph size grows (linear? quadratic? constant overhead?).

4. **Cross-framework scaling comparison**: Whether Juncture scales linearly
   with node count (yes, after the TriggerToNodes fix) vs. how LangGraph
   scales -- this is a meaningful comparison even if absolute numbers differ.

## Interpreting Results Responsibly

When presenting these results:

- **Always** state the limitations alongside the numbers.
- **Never** claim "Juncture is X times faster than LangGraph" without the
  caveats above.
- **Do** use these benchmarks to track Juncture's own performance over time.
- **Do** use the profiling data to identify and fix bottlenecks.
- **Do** acknowledge that LangGraph operates in a fundamentally different
  runtime environment with different constraints and trade-offs.

## Sample Results (2026-05-26, release mode)

### Wall-Clock Speedup

| Scenario | Rust (ms) | Python (ms) | Speedup |
|----------|-----------|-------------|---------|
| sequential_3000 | 16.9 | 7652 | **452x** |
| streaming_10000 | 142.7 | 78085 | **547x** |
| fanout_100 | 1.35 | 566 | **420x** |
| wide_state_1200 | 95.4 | 3593 | **38x** |
| checkpoint_on_100 | 1.0 | 47.7 | **48x** |
| conditional_routing_50 | 0.7 | 3.9 | **5.6x** |

### CPU/Memory Comparison

| Scenario | Rust CPU(ms) | Python CPU(ms) | Rust RSS(MB) | Python RSS(MB) | Rust PerNode(us) | Python PerNode(us) |
|----------|-------------|----------------|-------------|----------------|-----------------|-------------------|
| sequential_3000 | 100 | 153,213 | 14.4 | 80.9 | 5.6 | 2,551 |
| streaming_10000 | 650 | 1,561,866 | 41.3 | 120.7 | 14.3 | 7,809 |
| fanout_100 | 10 | 11,579 | 41.3 | 71.0 | 1.1 | 471 |
| wide_state_1200 | 330 | 10,771 | 14.4 | 70.1 | 13.2 | 499 |

### Per-node Scaling (Juncture, release mode)

| Nodes | Per-node (us) | Scaling |
|-------|--------------|---------|
| 10    | 94           | constant base |
| 100   | 11           | amortized |
| 1000  | 5.5          | near-constant |
| 3000  | 5.6          | near-constant |

Per-node overhead is approximately constant, confirming linear total scaling.
The `call_arc(Arc<S>)` optimization avoids O(N) state clone for wide states.
