# Juncture Core

[![Crates.io](https://img.shields.io/crates/v/juncture-core.svg)](https://crates.io/crates/juncture-core)
[![Documentation](https://docs.rs/juncture-core/badge.svg)](https://docs.rs/juncture-core)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Core types and execution engine for the Juncture state machine framework. This crate provides the fundamental building blocks: State trait, Channel system, Pregel execution engine, and graph primitives.

## Key Components

### State System
- `State` trait with `#[derive(State)]` proc-macro
- `CowState<S>` for copy-on-write state management
- `FieldsChanged` bitmask for tracking state changes
- Reducer semantics: Replace, Append, LastWriteWins, Any, Custom

### Pregel Engine
- Multi-core parallel execution via `tokio::spawn` + `JoinSet`
- Bounded concurrency with `Semaphore`
- Field version tracking for deterministic execution

### Graph Primitives
- `StateGraph<S,I,O>` builder
- `CompiledGraph<S,I,O>` execution
- `Node<S>` trait and `IntoNode` conversions
- `Edge`, `Router`, `PathMap` for graph topology

### HITL Support
- `interrupt!()` and `interrupt_with_ctx!()` macros
- `ResumeValue` for single/ID-based/namespace-based resumes
- `Scratchpad` for persistent interrupt context

## Usage

```rust
use juncture_core::prelude::*;

#[derive(State)]
struct MyState {
    #[reducer(append)]
    messages: Vec<String>,
    #[reducer(replace)]
    count: usize,
}

// Build and compile a graph
let graph = StateGraph::<MyState, _, _>::new()
    .add_node("process", process_node)
    .add_edge(START, "process")
    .add_edge("process", END)
    .compile();
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `multi-thread` | Tokio multi-thread runtime |
| `wasm` | WebAssembly support |
| `otel` | OpenTelemetry integration |
| `sqlite` | SQLite checkpoint storage |
| `postgres` | PostgreSQL checkpoint storage |
| `chat` | LLM provider support (reqwest) |

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
