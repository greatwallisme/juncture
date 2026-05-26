"""Conditional routing benchmark: route based on state value to N branches.

Port of LangGraph's conditional routing for Juncture comparison.
Produces JSON output for the comparison script.
"""

import json
import sys
import time

import uvloop
from bench_utils import get_cpu_time_ms, get_peak_rss_mb
from langgraph._internal._runnable import RunnableCallable
from langgraph.graph import StateGraph


class RoutingState(dict):
    """State for conditional routing benchmark."""

    value: int
    result: str


def create_conditional_routing(num_branches: int) -> StateGraph:
    """Create a conditional routing graph with N branches."""

    def route_node(state: RoutingState) -> dict:
        """Route node that sets the initial value."""
        return {"value": state["value"]}

    async def aroute_node(state: RoutingState) -> dict:
        """Async route node that sets the initial value."""
        return {"value": state["value"]}

    def create_branch_node(branch_index: int):
        """Create a branch node function."""

        def branch_node(state: RoutingState) -> dict:
            return {"result": f"branch_{branch_index}_visited"}

        async def abranch_node(state: RoutingState) -> dict:
            return {"result": f"branch_{branch_index}_visited"}

        return RunnableCallable(branch_node, abranch_node)

    def collect_node(state: RoutingState) -> dict:
        """Collect node that marks completion."""
        return {"result": f'{state["result"]}_complete'}

    async def acollect_node(state: RoutingState) -> dict:
        """Async collect node that marks completion."""
        return {"result": f'{state["result"]}_complete'}

    def route_function(state: RoutingState) -> str:
        """Route based on value modulo num_branches."""
        return f"branch_{state['value'] % num_branches}"

    builder = StateGraph(RoutingState)

    # Add route node
    builder.add_node("route", RunnableCallable(route_node, aroute_node))

    # Add branch nodes
    for i in range(num_branches):
        builder.add_node(f"branch_{i}", create_branch_node(i))

    # Add collect node
    builder.add_node("collect", RunnableCallable(collect_node, acollect_node))

    # Set entry point
    builder.set_entry_point("route")

    # Add conditional edges from route
    builder.add_conditional_edges(
        "route", route_function, {f"branch_{i}": f"branch_{i}" for i in range(num_branches)}
    )

    # All branches converge to collect
    for i in range(num_branches):
        builder.add_edge(f"branch_{i}", "collect")

    # Set finish point
    builder.set_finish_point("collect")

    return builder


async def arun(graph, input_data: dict, config: dict) -> dict:
    """Run the graph and return the output state."""
    return await graph.ainvoke(input_data, config)


def run_benchmark(num_branches: int, num_iterations: int = 20) -> dict:
    """Run the conditional routing benchmark for a given number of branches."""
    graph = create_conditional_routing(num_branches).compile()
    input_data = {"value": 42, "result": ""}
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
        "scenario": f"conditional_routing_{num_branches}",
        "num_branches": num_branches,
        "node_count": 3,  # route -> branch -> collect
        "iterations": num_iterations,
        "times_ms": [t * 1000 for t in times],
        "mean_ms": mean_ms,
        "min_ms": min(times) * 1000,
        "max_ms": max(times) * 1000,
        "cpu_ms": cpu_after - cpu_before,
        "peak_rss_mb": rss,
        "per_node_wall_us": mean_ms * 1000 / 3,
    }
    return result


def main() -> None:
    uvloop.install()

    results = []
    for num_branches in [3, 10, 50]:
        sys.stdout.write(f"Running conditional_routing_{num_branches}...\n")
        sys.stdout.flush()
        result = run_benchmark(num_branches)
        results.append(result)
        sys.stdout.write(
            f"  {result['scenario']}: {result['mean_ms']:.2f} ms "
            f"(min={result['min_ms']:.2f}, max={result['max_ms']:.2f})\n"
        )
        sys.stdout.flush()

    # Write JSON for comparison script
    with open("results_python_conditional_routing.json", "w") as f:
        json.dump({"benchmarks": results}, f, indent=2)

    sys.stdout.write("\nResults written to results_python_conditional_routing.json\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
