# Getting Started with Juncture

[Chinese Version / 中文版](../zh/getting-started.md)

## What is Juncture?

Juncture is a Rust implementation of LangGraph's state machine framework for building LLM agent applications. It preserves the core programming model -- `StateGraph` + Pregel execution engine -- while leveraging Rust's type system for compile-time safety and true multi-core parallelism.

## Prerequisites

- Rust 1.85+ (edition 2024)
- For real LLM examples: an OpenAI API key (or any OpenAI-compatible endpoint)

## Installation

Add Juncture to your `Cargo.toml`:

```toml
[dependencies]
juncture = "0.1"
juncture-core = "0.1"
juncture-derive = "0.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
```

## Your First Graph

Here is a minimal Juncture graph with two nodes that run sequentially:

```rust
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;

// Define state with #[derive(State)]
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create graph builder
    let mut graph = StateGraph::<WorkflowState>::new();

    // Add nodes
    graph.add_node_simple(
        "greet",
        NodeFnUpdate(|state: &WorkflowState| {
            let count = state.count;
            async move {
                Ok(WorkflowStateUpdate {
                    step: Some("greeted".to_string()),
                    count: Some(count + 1),
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "finish",
        NodeFnUpdate(|state: &WorkflowState| {
            let count = state.count;
            async move {
                Ok(WorkflowStateUpdate {
                    step: Some("done".to_string()),
                    count: Some(count + 1),
                })
            }
        }),
    )?;

    // Define flow: greet -> finish
    graph.add_edge("greet", "finish");
    graph.set_entry_point("greet");
    graph.set_finish_point("finish");

    // Compile and execute
    let compiled = graph.compile()?;
    let initial_state = WorkflowState {
        step: "initialized".to_string(),
        count: 0,
    };

    let output = compiled.invoke(initial_state, &RunnableConfig::default())?;
    println!("Final state: step={}, count={}", output.value.step, output.value.count);
    println!("Steps executed: {}", output.metadata.steps);

    Ok(())
}
```

## Running the Examples

Juncture ships with 17 examples that demonstrate progressively more complex patterns.

### Mock Examples (No API Key)

These examples use simulated data and run without any external dependencies:

```bash
# Basic state machine
cargo run -p juncture-simple-example --bin 01_state_machine

# Counter with different reducers
cargo run -p juncture-simple-example --bin 02_counter_reducers

# Conditional routing
cargo run -p juncture-simple-example --bin 03_conditional_routing

# Chat with mock model
cargo run -p juncture-simple-example --bin 04_chat_basic

# Tool calling (manual)
cargo run -p juncture-simple-example --bin 05_tool_calling

# Streaming execution
cargo run -p juncture-simple-example --bin 06_streaming

# Human-in-the-loop
cargo run -p juncture-simple-example --bin 07_human_in_the_loop

# Checkpoint and resume
cargo run -p juncture-simple-example --bin 08_checkpoint_resume

# Error recovery
cargo run -p juncture-simple-example --bin 09_error_recovery
```

### Real LLM Examples (API Key Required)

These examples call a real LLM API. First, configure your environment:

```bash
cp examples/.env.example examples/.env
# Edit .env and set your OPENAI_API_KEY
```

```bash
# Basic chat with real LLM
cargo run -p juncture-simple-example --bin 10_basic_chat

# Streaming chat
cargo run -p juncture-simple-example --bin 11_streaming_chat

# Tool calling with real LLM
cargo run -p juncture-simple-example --bin 12_tool_calling

# ReAct agent loop
cargo run -p juncture-simple-example --bin 13_react_agent

# Multi-turn conversation
cargo run -p juncture-simple-example --bin 14_multi_turn

# Structured output extraction
cargo run -p juncture-simple-example --bin 15_structured_output
```

### Deep Research (Separate Package)

A multi-agent research assistant with web search, subagent delegation, and middleware:

```bash
cargo run -p deep-research -- "What is the current state of quantum computing?"
cargo run -p deep-research -- --model gpt-4o-mini "Explain recent AI breakthroughs"
cargo run -p deep-research -- --verbose "Research topic here"
```

### Telemetry Demo

End-to-end OpenTelemetry pipeline with Jaeger and Prometheus:

```bash
# Start telemetry stack
docker compose -f docker/telemetry/docker-compose.yml up -d

# Run demo
cargo run -p juncture-simple-example --bin telemetry_demo

# Verify: Jaeger UI at http://localhost:16686
```

## Environment Configuration

Real LLM examples load configuration from `.env` via `dotenvy`:

```bash
OPENAI_API_KEY=sk-your-key          # Required
OPENAI_BASE_URL=https://...         # Optional, for OpenAI-compatible APIs
OPENAI_MODEL=gpt-4o                 # Optional, defaults to gpt-4o
TAVILY_API_KEY=tvily-your-key       # Optional, for web search in deep-research
```

## Build & Test

```bash
# Build all crates
cargo build --workspace --all-features

# Run all tests
cargo test --workspace --all-targets --all-features

# Run clippy (zero warnings enforced)
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Check formatting
cargo fmt --all -- --check

# Run a single test
cargo test -p juncture-core -- test_name --exact
```

## Workspace Structure

```
juncture/              -- facade crate (prelude, LLM providers, Tool trait, prebuilt agents)
juncture-core/         -- Channel system, StateGraph, Pregel engine, Node/Edge
juncture-derive/       -- #[derive(State)] proc-macro
juncture-checkpoint/   -- CheckpointSaver (MemorySaver, SqliteSaver, PostgresSaver)
juncture-tracing/      -- OpenTelemetry integration
juncture-store/        -- Cross-thread persistent key-value storage
benchmarks/            -- Performance comparison (Juncture vs LangGraph)
examples/              -- 15 examples + deep-research + telemetry demo
```

## Next Steps

- [Core Concepts](core-concepts.md) -- understand State, StateGraph, Reducers, and the Pregel engine
- [Examples Guide](examples-guide.md) -- detailed walkthrough of every example
- [Advanced Features](advanced-features.md) -- streaming, HITL, checkpointing, tools, telemetry
