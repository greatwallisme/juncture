"""Streaming benchmark: stream events from a chain of N nodes.

Port of LangGraph's streaming for Juncture comparison.
Produces JSON output for the comparison script.
"""

import json
import sys
import time

import uvloop
from bench_utils import get_cpu_time_ms, get_peak_rss_mb
from langgraph._internal._runnable import RunnableCallable
from langgraph.graph import MessagesState, StateGraph


def create_streaming(number_nodes: int) -> StateGraph:
    """Create a streaming graph consisting of N nodes."""

    def append_message(state: MessagesState) -> dict:
        """Append a message to the state."""
        return {"messages": []}

    async def aappend_message(state: MessagesState) -> dict:
        """Append a message to the state."""
        return {"messages": []}

    builder = StateGraph(MessagesState)
    prev_node = "__start__"

    for i in range(number_nodes):
        name = f"node_{i}"
        builder.add_node(name, RunnableCallable(append_message, aappend_message))
        builder.add_edge(prev_node, name)
        prev_node = name

    builder.add_edge(prev_node, "__end__")
    return builder


async def arun(graph, input_data: dict, config: dict) -> int:
    """Run the graph and return the number of events."""
    count = 0
    async for _ in graph.astream(
        input_data,
        config,
    ):
        count += 1
    return count


def run_benchmark(num_nodes: int, num_iterations: int = 20) -> dict:
    """Run the streaming benchmark for a given number of nodes."""
    graph = create_streaming(num_nodes).compile()
    input_data = {"messages": []}
    config = {
        "configurable": {"thread_id": "bench"},
        "recursion_limit": 20_000_000_000,
    }

    # Warmup
    for _ in range(3):
        uvloop.run(arun(graph, input_data, config))

    # Timed runs
    times: list[float] = []
    cpu_before = get_cpu_time_ms()
    for _ in range(num_iterations):
        start = time.perf_counter()
        uvloop.run(arun(graph, input_data, config))
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    cpu_after = get_cpu_time_ms()
    rss = get_peak_rss_mb()

    mean_ms = sum(times) / len(times) * 1000

    result = {
        "scenario": f"streaming_{num_nodes}",
        "num_nodes": num_nodes,
        "node_count": num_nodes,
        "iterations": num_iterations,
        "times_ms": [t * 1000 for t in times],
        "mean_ms": mean_ms,
        "min_ms": min(times) * 1000,
        "max_ms": max(times) * 1000,
        "cpu_ms": cpu_after - cpu_before,
        "peak_rss_mb": rss,
        "per_node_wall_us": mean_ms * 1000 / num_nodes,
    }
    return result


def main() -> None:
    uvloop.install()

    results = []
    for num_nodes in [100, 1000, 10000]:
        sys.stdout.write(f"Running streaming_{num_nodes}...\n")
        sys.stdout.flush()
        result = run_benchmark(num_nodes)
        results.append(result)
        sys.stdout.write(
            f"  {result['scenario']}: {result['mean_ms']:.2f} ms "
            f"(min={result['min_ms']:.2f}, max={result['max_ms']:.2f})\n"
        )
        sys.stdout.flush()

    # Write JSON for comparison script
    with open("results_python_streaming.json", "w") as f:
        json.dump({"benchmarks": results}, f, indent=2)

    sys.stdout.write("\nResults written to results_python_streaming.json\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
