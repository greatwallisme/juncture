"""Wide state benchmark: graph with many state fields.

Port of LangGraph's bench/wide_state.py for Juncture comparison.
Produces JSON output for the comparison script.

Node behavior mirrors the Rust benchmark exactly: each node reads one field
(to simulate state access cost) and writes fixed, small values to other fields.
This avoids exponential state growth and ensures fair comparison.
"""

import json
import operator
import sys
import time

from dataclasses import dataclass, field
from typing import Annotated

import uvloop
from bench_utils import get_cpu_time_ms, get_peak_rss_mb
from langgraph._internal._runnable import RunnableCallable
from langgraph.constants import END, START
from langgraph.graph import StateGraph


def create_wide_state(n: int) -> StateGraph:
    """Create a wide state graph with 15+ fields and various reducer types."""

    @dataclass(kw_only=True)
    class State:
        messages: Annotated[list, operator.add] = field(default_factory=list)
        trigger_events: Annotated[list, operator.add] = field(default_factory=list)
        primary_issue_medium: Annotated[str, lambda x, y: y or x] = field(
            default="email"
        )
        autoresponse: Annotated[dict | None, lambda _, y: y] = field(default=None)
        issue: Annotated[dict | None, lambda x, y: y if y else x] = field(default=None)
        relevant_rules: list[dict] | None = field(default=None)
        memory_docs: list[dict] | None = field(default=None)
        categorizations: Annotated[list[dict], operator.add] = field(
            default_factory=list
        )
        responses: Annotated[list[dict], operator.add] = field(default_factory=list)
        user_info: Annotated[dict | None, lambda x, y: y if y is not None else x] = (
            field(default=None)
        )
        crm_info: Annotated[dict | None, lambda x, y: y if y is not None else x] = (
            field(default=None)
        )
        email_thread_id: Annotated[str | None, lambda x, y: y if y is not None else x] = (
            field(default=None)
        )
        slack_participants: Annotated[dict, operator.or_] = field(default_factory=dict)
        bot_id: str | None = field(default=None)
        notified_assignees: Annotated[dict, operator.or_] = field(default_factory=dict)

    def node_one(state: State) -> dict:
        _ = state.messages[-1:]  # read access (no crash on empty)
        return {
            "trigger_events": [{"event": "triggered"}],
            "primary_issue_medium": "email",
        }

    def node_two(state: State) -> dict:
        _ = state.trigger_events[-1:]  # read access
        return {"autoresponse": {"enabled": True}}

    def node_three(state: State) -> dict:
        _ = state.autoresponse  # read access
        return {"relevant_rules": []}

    def node_four(state: State) -> dict:
        _ = state.trigger_events[-1:]  # read access
        return {
            "categorizations": [],
            "responses": [],
            "memory_docs": None,
        }

    def node_five(state: State) -> dict:
        _ = state.categorizations[-1:]  # read access
        return {
            "user_info": {},
            "crm_info": {},
            "email_thread_id": "t",
            "slack_participants": {},
            "bot_id": "b",
            "notified_assignees": {},
        }

    def node_six(state: State) -> dict:
        _ = state.responses[-1:]  # read access
        return {"messages": [{"message": "completed"}]}

    builder = StateGraph(State)
    builder.add_edge(START, "one")
    builder.add_node("one", RunnableCallable(node_one))
    builder.add_edge("one", "two")
    builder.add_node("two", RunnableCallable(node_two))
    builder.add_edge("two", "three")
    builder.add_edge("two", "four")
    builder.add_node("three", RunnableCallable(node_three))
    builder.add_node("four", RunnableCallable(node_four))
    builder.add_node("five", RunnableCallable(node_five))
    builder.add_edge(["three", "four"], "five")
    builder.add_edge("five", "six")
    builder.add_node("six", RunnableCallable(node_six))
    builder.add_conditional_edges(
        "six", lambda state: END if len(state.messages) > n else "one"
    )

    return builder


async def arun(graph, input_data: dict, config: dict) -> int:
    """Run the graph and return the number of events."""
    return len(
        [
            c
            async for c in graph.astream(
                input_data,
                config,
                durability="exit",
            )
        ]
    )


async def run_benchmark(iterations: int, num_iterations: int = 3) -> dict:
    """Run the wide state benchmark for a given number of iterations."""
    graph = create_wide_state(iterations).compile()

    input_data = {"messages": []}
    config = {
        "configurable": {"thread_id": "bench"},
        "recursion_limit": 20_000_000_000,
    }

    # Warmup (1 run)
    await arun(graph, input_data, config)

    # Timed runs
    times: list[float] = []
    cpu_before = get_cpu_time_ms()
    for _ in range(num_iterations):
        start = time.perf_counter()
        await arun(graph, input_data, config)
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    cpu_after = get_cpu_time_ms()
    rss = get_peak_rss_mb()

    mean_ms = sum(times) / len(times) * 1000
    node_count = iterations * 6  # 6 nodes per iteration

    result = {
        "scenario": f"wide_state_{iterations}",
        "iterations": iterations,
        "node_count": node_count,
        "num_runs": num_iterations,
        "times_ms": [t * 1000 for t in times],
        "mean_ms": mean_ms,
        "min_ms": min(times) * 1000,
        "max_ms": max(times) * 1000,
        "cpu_ms": cpu_after - cpu_before,
        "peak_rss_mb": rss,
        "per_node_wall_us": mean_ms * 1000 / node_count,
    }
    return result


def main() -> None:
    uvloop.install()

    results = []
    for iterations in [300, 600, 1200]:
        sys.stdout.write(f"Running wide_state_{iterations}...\n")
        sys.stdout.flush()
        result = uvloop.run(run_benchmark(iterations, num_iterations=3))
        results.append(result)
        sys.stdout.write(
            f"  {result['scenario']}: {result['mean_ms']:.2f} ms "
            f"(min={result['min_ms']:.2f}, max={result['max_ms']:.2f})\n"
        )
        sys.stdout.flush()

    # Write JSON for comparison script
    with open("results_python_wide_state.json", "w") as f:
        json.dump({"benchmarks": results}, f, indent=2)

    sys.stdout.write("\nResults written to results_python_wide_state.json\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
