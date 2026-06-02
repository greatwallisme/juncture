# Juncture

[![Crates.io](https://img.shields.io/crates/v/juncture.svg)](https://crates.io/crates/juncture)
[![Documentation](https://docs.rs/juncture/badge.svg)](https://docs.rs/juncture)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Typed state machine framework for building LLM agent applications in Rust. Juncture is a Rust implementation of [LangGraph](https://github.com/langchain-ai/langgraph), providing the same programming model with Rust's type safety and performance.

## Features

- **Typed State Machine**: Compile-time state validation with `#[derive(State)]`
- **Pregel Execution Engine**: Multi-core parallel execution with bounded concurrency
- **Checkpoint Persistence**: Save/restore execution state for time-travel debugging and crash recovery
- **HITL Workflows**: Human-in-the-loop interrupts with `interrupt!()` macro
- **LLM Providers**: Built-in support for Anthropic, OpenAI, and Ollama
- **Tool Infrastructure**: Tool trait, ToolNode, interceptors, and transformers
- **Prebuilt Agents**: ReAct agent pattern, MessagesState, agent factory with middleware
- **Observability**: OpenTelemetry integration and Langfuse-compatible telemetry

## Quick Start

```rust
use juncture::prelude::*;
use juncture::prebuilt::{create_react_agent, MessagesState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a simple ReAct agent
    let agent = create_react_agent::<MessagesState>(
        ChatAnthropic::new("claude-3-5-sonnet-20241022")?,
        vec![/* your tools */],
    );

    // Run the agent
    let result = agent.invoke(MessagesState::new("Hello!")).await?;
    println!("Response: {:?}", result.messages.last());
    Ok(())
}
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `anthropic` | Anthropic Claude provider |
| `openai` | OpenAI GPT provider |
| `ollama` | Ollama local model provider |
| `structured-output` | Structured output via schemars JSON Schema |
| `store` | Cross-thread persistent key-value storage |
| `multi-thread` | Tokio multi-thread runtime (default) |
| `wasm` | WebAssembly support |

## Examples

See the [examples](https://github.com/greatwallisme/juncture/tree/main/examples) directory for complete working examples.

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
