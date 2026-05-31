# Core Concepts

[Chinese Version / 中文版](../zh/core-concepts.md)

This document explains the fundamental building blocks of Juncture. Understanding these concepts is essential before working with the examples.

## State

State is the central data structure that flows through a Juncture graph. Every node reads the current state and returns an update that gets merged back.

### Defining State with `#[derive(State)]`

The `#[derive(State)]` macro generates two types:
1. **The state struct** (e.g., `WorkflowState`) -- holds the actual data
2. **An update struct** (e.g., `WorkflowStateUpdate`) -- holds `Option<T>` fields for partial updates

```rust
use juncture_derive::State;

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}

// Generated automatically:
// struct WorkflowStateUpdate {
//     step: Option<String>,
//     count: Option<u32>,
// }
```

The update struct uses `Option<T>` for every field. When a field is `None`, it is not changed. When it is `Some(value)`, the value is merged according to the field's reducer.

### Reducers

Reducers control how updates are merged into the current state. Juncture supports several built-in reducers:

| Reducer | Behavior | Use Case |
|---------|----------|----------|
| `replace` (default) | Last writer wins (panics on double-write in same superstep) | Scalar values, single-owner fields |
| `append` | Extends the Vec collection | Lists that grow over time |
| `ephemeral` | Resets to Default after each superstep | Temporary computation results |
| `last_write_wins` | Last writer wins silently (no panic on double-write) | Status fields, timestamps |
| `untracked` | Not persisted across checkpoints | Ephemeral state |
| `replace_after_finish` | Available only after finish() | Post-completion updates |
| `any` | All writers should provide equal values | Consensus fields |
| `custom = path::to::func` | User-defined merge function `fn(&mut T, T)` | Complex merging strategies |

```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CounterState {
    // Default replace reducer -- last writer wins
    value: u32,

    // Append reducer -- extends the vector
    #[reducer(append)]
    items: Vec<String>,

    // Last write wins -- explicit semantics
    #[reducer(last_write_wins)]
    status: String,
}
```

### FieldsChanged Bitmask

The `#[derive(State)]` macro also generates a `FieldsChanged` bitmask (u64) that tracks which fields were modified in an update. This enables efficient change detection without comparing entire state structs.

### CowState

Juncture uses `CowState<S>` (Arc-based copy-on-write) as the default state wrapper. This avoids expensive clones when state is passed between nodes -- nodes that only read state share the same Arc, and a clone only happens when a write occurs.

## StateGraph

`StateGraph<S>` is the builder for constructing a computation graph. It is generic over the state type `S`.

### Creating a Graph

```rust
use juncture_core::StateGraph;

let mut graph = StateGraph::<MyState>::new();
```

### Adding Nodes

Nodes are the computation units. Each node receives a reference to the current state and returns an update:

```rust
use juncture_core::node::NodeFnUpdate;

graph.add_node_simple(
    "my_node",
    NodeFnUpdate(|state: &MyState| {
        async move {
            Ok(MyStateUpdate {
                field: Some("new value".to_string()),
                ..Default::default()
            })
        }
    }),
)?;
```

The node function:
- Receives `&MyState` (shared reference to current state)
- Returns `Result<MyStateUpdate, JunctureError>`
- Is async (uses `async move` block)
- Only sets fields it wants to change (others use `..Default::default()`)

### Adding Edges

Edges define the execution order between nodes:

```rust
// Simple edge: run "b" after "a"
graph.add_edge("a", "b");

// Conditional edges: route based on state
graph.add_conditional_edges(
    "router_node",
    Arc::new(my_router_fn) as Arc<dyn Router<MyState>>,
    PathMap::from(&[("path_a", "node_a"), ("path_b", "node_b")]),
);
```

### Entry and Finish Points

```rust
graph.set_entry_point("first_node");
graph.set_finish_point("last_node");
```

The entry point is where execution starts. The finish point is where execution ends. If no finish point is set, execution continues until no more nodes are reachable.

### Compiling

Before execution, the graph must be compiled:

```rust
let compiled = graph.compile()?;

// Or with a checkpointer:
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

// Or with compile config (for HITL):
let compiled = graph.compile_with_config(CompileConfig {
    interrupt_before: vec!["review".to_string()],
    interrupt_after: vec![],
})?;
```

## Conditional Routing

Conditional routing allows the graph to take different paths based on the current state. This is the primary mechanism for implementing branching logic.

### Router Function

A router function inspects the state and returns the name of the next node:

```rust
use juncture_core::edge::{PathMap, Router};

const fn grade_router(state: &ScoreState) -> &str {
    if state.score >= 90 {
        "excellent"
    } else if state.score >= 70 {
        "good"
    } else {
        "retry"
    }
}
```

### PathMap

A `PathMap` maps router return values to node names:

```rust
graph.add_conditional_edges(
    "grade",
    Arc::new(grade_router) as Arc<dyn Router<ScoreState>>,
    PathMap::from(&[
        ("excellent", "excellent"),
        ("good", "good"),
        ("retry", "retry"),
    ]),
);
```

### Router Trait

For more complex routing (async operations, error handling), implement the `Router` trait:

```rust
struct AgentRouter;

impl Router<MessagesState> for AgentRouter {
    fn route(
        &self,
        state: &MessagesState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        let target = if has_tool_calls(state) { "tools" } else { END };
        Box::pin(async move { Ok(RouteResult::One(target.to_string())) })
    }
}
```

## Pregel Engine

The Pregel engine is Juncture's execution runtime. It processes the graph in supersteps -- each superstep executes all ready nodes in parallel, then merges their updates.

### Execution Model

1. **Superstep 0**: Entry node executes
2. **Merge**: All node outputs are merged into the state (using reducers)
3. **Route**: Conditional edges determine the next set of ready nodes
4. **Superstep N**: All ready nodes execute in parallel
5. **Repeat** until the finish point is reached or no more nodes are ready

### Parallelism

Juncture uses `tokio::spawn` + `JoinSet` for true multi-core parallelism. When multiple nodes are ready in the same superstep, they execute concurrently. A `Semaphore`-based bounded concurrency control prevents resource exhaustion.

### Execution Modes

| Mode | Method | Description |
|------|--------|-------------|
| Blocking | `invoke()` | Runs the graph to completion, returns final state |
| Async | `invoke_async()` | Async version of invoke |
| Streaming | `stream()` | Returns a stream of `StreamEvent` values |

## MessagesState

For LLM applications, Juncture provides `MessagesState` -- a pre-built state type for managing conversation history:

```rust
use juncture_core::state::messages::{Message, MessagesState};
use juncture_core::state::{Content, Role};

let state = MessagesState {
    messages: vec![
        Message::system("You are a helpful assistant."),
        Message::human("Hello!"),
    ],
};
```

### Message Roles

| Role | Description |
|------|-------------|
| `Role::System` | System instructions |
| `Role::Human` | User messages |
| `Role::Ai` | AI responses (may include tool calls) |
| `Role::Tool` | Tool execution results |

### Message Constructors

```rust
Message::system("instructions".to_string())
Message::human("user input".to_string())
Message::ai("assistant response".to_string())
```

## Tool Trait

The `Tool` trait defines a callable tool that an LLM can invoke:

```rust
use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};

#[derive(Debug)]
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Adds two numbers" }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        })
    }
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let a = input["a"].as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'a'".into()))?;
        let b = input["b"].as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'b'".into()))?;
        Ok((a + b).to_string())
    }
}
```

### ToolError Variants

| Variant | Constructor | Use Case |
|---------|-------------|----------|
| `InvalidInput` | `ToolError::InvalidInput(msg)` | Bad input format |
| `ExecutionFailed` | `ToolError::ExecutionFailed(msg)` | Runtime failure |
| `Timeout` | `ToolError::Timeout` | Execution timeout |
| `ToolNotFound` | `ToolError::ToolNotFound(name)` | Unknown tool name |
| `ValidationError` | `ToolError::ValidationError(msg)` | Schema validation failure |

## ChatModel Trait

The `ChatModel` trait provides a unified interface for LLM providers:

```rust
use juncture::llm::{ChatModel, ChatOpenAI};
use futures::StreamExt;

let llm = ChatOpenAI::new("sk-...".to_string())
    .with_model("gpt-4o".to_string());

// Single invocation
let response = llm.invoke(&messages, None).await?;

// Streaming (async, returns Result<BoxStream>)
let mut stream = llm.stream(&messages, None).await?;
while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    print!("{}", chunk.content);
}

// With tools
let llm_with_tools = llm.bind_tools(vec![tool_def]);
```

## GraphOutput

When a graph finishes execution, it returns a `GraphOutput` containing:

```rust
let output = compiled.invoke(initial_state, &config)?;

output.value       // The final state (type S)
output.output      // Output extracted via FromState (type O, defaults to S)
output.interrupts  // List of InterruptInfo (for HITL)
output.metadata    // Execution metadata
```

### GraphOutputMetadata

```rust
output.metadata.steps           // Number of supersteps executed
output.metadata.run_id          // Unique run identifier
output.metadata.checkpoint_id   // Checkpoint ID (if checkpointing enabled)
output.metadata.budget_usage    // Budget usage (if budget tracking enabled)
```

## Next Steps

- [Examples Guide](examples-guide.md) -- see these concepts in action across 17 examples
- [Advanced Features](advanced-features.md) -- streaming, HITL, checkpointing, telemetry
