"""Wide state benchmark: graph with many state fields.

Port of LangGraph's bench/wide_state.py for Juncture comparison.
Produces JSON output for the comparison script.
"""

import json
import operator
import sys
import time

from collections.abc import Sequence
from dataclasses import dataclass, field
from functools import partial
from random import choice

import uvloop
from langgraph._internal._runnable import RunnableCallable
from langgraph.constants import END, START
from langgraph.graph import StateGraph


def create_wide_state(n: int) -> StateGraph:
    """Create a wide state graph with 15+ fields and various reducer types."""

    @dataclass(kw_only=True)
    class State:
        messages: list = field(default_factory=list)
        """Messages exchanged during conversation."""
        trigger_events: list = field(default_factory=list)
        """External events converted by the graph."""
        primary_issue_medium: str = field(default="email")
        """Primary medium for issue communication."""
        autoresponse: dict | None = field(default=None)
        """Auto-response configuration."""
        issue: dict | None = field(default=None)
        """Current issue details."""
        relevant_rules: list[dict] | None = field(default=None)
        """SOPs from rulebook relevant to conversation."""
        memory_docs: list[dict] | None = field(default=None)
        """Memory docs relevant to conversation."""
        categorizations: list[dict] = field(default_factory=list)
        """AI-generated issue categorizations."""
        responses: list[dict] = field(default_factory=list)
        """Draft responses recommended by AI."""
        user_info: dict | None = field(default=None)
        """Current user state by email."""
        crm_info: dict | None = field(default=None)
        """CRM info for user's organization."""
        email_thread_id: str | None = field(default=None)
        """Current email thread ID."""
        slack_participants: dict = field(default_factory=dict)
        """Growing list of Slack participants."""
        bot_id: str | None = field(default=None)
        """Bot user ID in Slack channel."""
        notified_assignees: dict = field(default_factory=dict)
        """Assignees that have been notified."""

    list_fields = {
        "messages",
        "trigger_events",
        "categorizations",
        "responses",
        "memory_docs",
        "relevant_rules",
    }

    def read_write(read: str, write: Sequence[str], input_state: State) -> dict:
        """Node function that reads one field and writes to others."""
        val = getattr(input_state, read)
        val = {val: val} if isinstance(val, str) else val
        val_single = val[-1] if isinstance(val, list) else val
        val_list = val if isinstance(val, list) else [val]
        return {
            k: val_list
            if k in list_fields
            else val_single
            if k in {"user_info", "crm_info", "slack_participants", "notified_assignees", "autoresponse", "issue"}
            else "".join(choice("abcdefghijklmnopqrstuvwxyz") for _ in range(n))
            for k in write
        }

    builder = StateGraph(State)
    builder.add_edge(START, "one")
    builder.add_node(
        "one",
        RunnableCallable(
            partial(read_write, "messages", ["trigger_events", "primary_issue_medium"]),
            partial(read_write, "messages", ["trigger_events", "primary_issue_medium"]),
        ),
    )
    builder.add_edge("one", "two")
    builder.add_node(
        "two",
        RunnableCallable(
            partial(read_write, "trigger_events", ["autoresponse", "issue"]),
            partial(read_write, "trigger_events", ["autoresponse", "issue"]),
        ),
    )
    builder.add_edge("two", "three")
    builder.add_edge("two", "four")
    builder.add_node(
        "three",
        RunnableCallable(
            partial(read_write, "autoresponse", ["relevant_rules"]),
            partial(read_write, "autoresponse", ["relevant_rules"]),
        ),
    )
    builder.add_node(
        "four",
        RunnableCallable(
            partial(
                read_write,
                "trigger_events",
                ["categorizations", "responses", "memory_docs"],
            ),
            partial(
                read_write,
                "trigger_events",
                ["categorizations", "responses", "memory_docs"],
            ),
        ),
    )
    builder.add_node(
        "five",
        RunnableCallable(
            partial(
                read_write,
                "categorizations",
                [
                    "user_info",
                    "crm_info",
                    "email_thread_id",
                    "slack_participants",
                    "bot_id",
                    "notified_assignees",
                ],
            ),
            partial(
                read_write,
                "categorizations",
                [
                    "user_info",
                    "crm_info",
                    "email_thread_id",
                    "slack_participants",
                    "bot_id",
                    "notified_assignees",
                ],
            ),
        ),
    )
    builder.add_edge(["three", "four"], "five")
    builder.add_edge("five", "six")
    builder.add_node(
        "six",
        RunnableCallable(
            partial(read_write, "responses", ["messages"]),
            partial(read_write, "responses", ["messages"]),
        ),
    )
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


async def run_benchmark(iterations: int, num_iterations: int = 20) -> dict:
    """Run the wide state benchmark for a given number of iterations."""
    graph = create_wide_state(iterations).compile()

    # Pre-generate input data matching Python structure
    input_messages = []
    for i in range(50):
        inner_map = {}
        for j in range(50):
            key = str(j) * 10
            value = ["hi?" * 10, True, 1, 6327816386138, None] * 5
            inner_map[key] = value
        input_messages.append(inner_map)

    input_data = {"messages": input_messages}
    config = {
        "configurable": {"thread_id": "bench"},
        "recursion_limit": 20_000_000_000,
    }

    # Warmup
    for _ in range(3):
        await arun(graph, input_data, config)

    # Timed runs
    times: list[float] = []
    for _ in range(num_iterations):
        start = time.perf_counter()
        await arun(graph, input_data, config)
        elapsed = time.perf_counter() - start
        times.append(elapsed)

    result = {
        "scenario": f"wide_state_{iterations}",
        "iterations": iterations,
        "num_runs": num_iterations,
        "times_ms": [t * 1000 for t in times],
        "mean_ms": sum(times) / len(times) * 1000,
        "min_ms": min(times) * 1000,
        "max_ms": max(times) * 1000,
    }
    return result


def main() -> None:
    uvloop.install()

    results = []
    for iterations in [300, 600, 1200]:
        sys.stdout.write(f"Running wide_state_{iterations}...\n")
        sys.stdout.flush()
        result = uvloop.run(run_benchmark(iterations))
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
