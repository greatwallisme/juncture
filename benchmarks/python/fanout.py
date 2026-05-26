"""Fanout benchmark: parallel subgraph execution via Send.

Port of LangGraph's bench/fanout_to_subgraph.py for Juncture comparison.
Produces JSON output for the comparison script.
"""

import operator
import json
import sys
import time
from typing import Annotated, TypedDict

import uvloop
from bench_utils import get_cpu_time_ms, get_peak_rss_mb
from langgraph._internal._runnable import RunnableCallable
from langgraph.constants import END, START
from langgraph.graph import StateGraph
from langgraph.types import Send


def fanout_to_subgraph() -> StateGraph:
    """Create a fanout graph with subgraph."""

    class OverallState(TypedDict):
        subjects: list[str]
        jokes: Annotated[list[str], operator.add]

    async def continue_to_jokes(state: OverallState):
        """Router that fans out to multiple subgraph instances."""
        return [Send("generate_joke", {"subject": s}) for s in state["subjects"]]

    class JokeInput(TypedDict):
        subject: str

    class JokeOutput(TypedDict):
        jokes: list[str]

    class JokeState(JokeInput, JokeOutput):
        """Combined subgraph state (input + output)."""
        ...

    async def bump(state: JokeOutput):
        """Bump node: append ' a' to the first joke."""
        return {"jokes": [state["jokes"][0] + " a"]}

    async def generate(state: JokeInput):
        """Generate node: create a joke about the subject."""
        return {"jokes": [f"Joke about {state['subject']}"]}

    async def edit(state: JokeInput):
        """Edit node: modify the subject."""
        subject = state["subject"]
        return {"subject": f"{subject} - hohoho"}

    async def bump_loop(state: JokeOutput):
        """Router for the bump loop: continue until joke ends with ' a' * 10."""
        return END if state["jokes"][0].endswith(" a" * 10) else "bump"

    # subgraph
    subgraph = StateGraph(JokeState, input_schema=JokeInput, output_schema=JokeOutput)
    subgraph.add_node("edit", RunnableCallable(edit, edit))
    subgraph.add_node("generate", RunnableCallable(generate, generate))
    subgraph.add_node("bump", RunnableCallable(bump, bump))
    subgraph.set_entry_point("edit")
    subgraph.add_edge("edit", "generate")
    subgraph.add_edge("generate", "bump")
    subgraph.add_conditional_edges("bump", bump_loop)
    subgraph.set_finish_point("generate")
    subgraphc = subgraph.compile()

    # parent graph
    builder = StateGraph(OverallState)
    builder.add_node("generate_joke", subgraphc)
    builder.add_conditional_edges(START, continue_to_jokes)
    builder.add_edge("generate_joke", END)

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


def run_benchmark(num_subjects: int, num_iterations: int = 20) -> dict:
    """Run the fanout benchmark for a given number of subjects."""
    graph = fanout_to_subgraph().compile()
    input_data = {
        "subjects": [f"subject_{i}" for i in range(num_subjects)],
    }
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
    node_count = num_subjects * 12  # each subject: edit+generate+bump*10

    result = {
        "scenario": f"fanout_{num_subjects}",
        "num_subjects": num_subjects,
        "node_count": node_count,
        "iterations": num_iterations,
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
    for num_subjects in [10, 100]:
        sys.stdout.write(f"Running fanout_{num_subjects}...\n")
        sys.stdout.flush()
        result = run_benchmark(num_subjects)
        results.append(result)
        sys.stdout.write(
            f"  {result['scenario']}: {result['mean_ms']:.2f} ms "
            f"(min={result['min_ms']:.2f}, max={result['max_ms']:.2f})\n"
        )
        sys.stdout.flush()

    # Write JSON for comparison script
    with open("results_python_fanout.json", "w") as f:
        json.dump({"benchmarks": results}, f, indent=2)

    sys.stdout.write("\nResults written to results_python_fanout.json\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
