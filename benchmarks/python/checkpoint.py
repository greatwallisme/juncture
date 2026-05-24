"""Checkpoint benchmark: measure persistence cost of InMemorySaver.

Compares graph execution with and without checkpointing to isolate
the performance overhead of state persistence. Uses sequential graph
pattern (100-node linear chain) with no-op nodes.
"""

import json
import sys
import time
import uuid

import uvloop
from langgraph._internal._runnable import RunnableCallable
from langgraph.checkpoint.memory import InMemorySaver
from langgraph.graph import MessagesState, StateGraph


def create_sequential(number_nodes: int) -> StateGraph:
    """Create a sequential no-op graph consisting of N nodes."""
    builder = StateGraph(MessagesState)

    def noop(state: MessagesState) -> None:
        """Intentional no-op: node does zero work to isolate framework overhead."""
        return None

    async def anoop(state: MessagesState) -> None:
        """Intentional no-op: node does zero work to isolate framework overhead."""
        return None

    prev_node = "__start__"

    for i in range(number_nodes):
        name = f"node_{i}"
        builder.add_node(name, RunnableCallable(noop, anoop))
        builder.add_edge(prev_node, name)
        prev_node = name

    builder.add_edge(prev_node, "__end__")
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


def run_benchmark(num_nodes: int, use_checkpoint: bool, num_iterations: int = 20) -> dict:
    """Run the checkpoint benchmark for a given number of nodes."""
    if use_checkpoint:
        checkpointer = InMemorySaver()
        graph = create_sequential(num_nodes).compile(checkpointer=checkpointer)
    else:
        graph = create_sequential(num_nodes).compile()

    input_data = {"messages": []}

    # Warmup
    for _ in range(3):
        config = {
            "configurable": {"thread_id": f"warmup_{uuid.uuid4()}"},
            "recursion_limit": 20_000_000_000,
        }
        uvloop.run(arun(graph, input_data, config))

    # Timed runs
    times: list[float] = []
    for _ in range(num_iterations):
        # Use unique thread_id per iteration to avoid checkpoint conflicts
        config = {
            "configurable": {"thread_id": f"bench_{uuid.uuid4()}"},
            "recursion_limit": 20_000_000_000,
        }
        start = time.perf_counter()
        uvloop.run(arun(graph, input_data, config))
        elapsed = time.perf_counter() - start
        times.append(elapsed)

    checkpoint_status = "on" if use_checkpoint else "off"
    result = {
        "scenario": f"checkpoint_{checkpoint_status}_{num_nodes}",
        "num_nodes": num_nodes,
        "use_checkpoint": use_checkpoint,
        "iterations": num_iterations,
        "times_ms": [t * 1000 for t in times],
        "mean_ms": sum(times) / len(times) * 1000,
        "min_ms": min(times) * 1000,
        "max_ms": max(times) * 1000,
    }
    return result


def main() -> None:
    uvloop.install()

    results = []
    num_nodes = 100

    # Benchmark WITHOUT checkpointing
    sys.stdout.write(f"Running checkpoint_off_{num_nodes}...\n")
    sys.stdout.flush()
    result_off = run_benchmark(num_nodes, use_checkpoint=False)
    results.append(result_off)
    sys.stdout.write(
        f"  {result_off['scenario']}: {result_off['mean_ms']:.2f} ms "
        f"(min={result_off['min_ms']:.2f}, max={result_off['max_ms']:.2f})\n"
    )
    sys.stdout.flush()

    # Benchmark WITH checkpointing
    sys.stdout.write(f"Running checkpoint_on_{num_nodes}...\n")
    sys.stdout.flush()
    result_on = run_benchmark(num_nodes, use_checkpoint=True)
    results.append(result_on)
    sys.stdout.write(
        f"  {result_on['scenario']}: {result_on['mean_ms']:.2f} ms "
        f"(min={result_on['min_ms']:.2f}, max={result_on['max_ms']:.2f})\n"
    )
    sys.stdout.flush()

    # Write JSON for comparison script
    with open("results_python_checkpoint.json", "w") as f:
        json.dump({"benchmarks": results}, f, indent=2)

    sys.stdout.write("\nResults written to results_python_checkpoint.json\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
