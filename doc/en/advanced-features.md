# Advanced Features

[Chinese Version / 中文版](../zh/advanced-features.md)

This document covers Juncture's advanced capabilities beyond basic graph construction.

## Streaming

Streaming allows you to observe graph execution in real-time, receiving events as each superstep completes.

### Stream Modes

| Mode | Description |
|------|-------------|
| `StreamMode::Values` | Emits full state after each superstep (default) |
| `StreamMode::Updates` | Emits only the updates (deltas) from each node |
| `StreamMode::Messages` | Emits LLM token streams |
| `StreamMode::Custom` | Emits custom events from nodes |
| `StreamMode::Debug` | Emits detailed debug information |
| `StreamMode::Tools` | Emits tool execution lifecycle events |
| `StreamMode::Checkpoints` | Emits checkpoint save events |
| `StreamMode::Tasks` | Emits detailed task events |
| `StreamMode::Multi(vec)` | Combines multiple stream modes |

### StreamEvent Variants

| Variant | Description |
|---------|-------------|
| `Values { state, step }` | Complete state snapshot after a superstep |
| `FilteredValues { data, step }` | Filtered state (when `output_keys` is set) |
| `Updates { node, update, step }` | Per-node update |
| `FilteredUpdates { node, data, step }` | Filtered per-node update |
| `Messages { chunk, metadata }` | LLM token chunk |
| `Custom { node, data, ns }` | Custom event from a node |
| `TaskStart { node, task_id, step }` | Task started |
| `TaskEnd { node, task_id, step, duration_ms }` | Task completed |
| `Interrupt { node, payload, resumable, ns }` | HITL interrupt |
| `BudgetExceeded { reason, usage }` | Budget limit exceeded |
| `End { output }` | Graph execution completed |
| `Cancelled { step }` | Execution was cancelled |
| `Debug(event)` | Debug event |
| `Tools(event)` | Tool lifecycle event |
| `CheckpointSaved { checkpoint_id, metadata, step }` | Checkpoint saved |
| `TaskDetail { task_id, ... }` | Detailed task event |

### Basic Streaming

```rust
use juncture_core::stream::{StreamEvent, StreamMode};
use futures::StreamExt;

let handle = compiled
    .stream(initial_state, &config, StreamMode::Values)
    .await?;

let mut stream = handle.stream;
while let Some(result) = stream.next().await {
    match result? {
        StreamEvent::Values { state, step } => {
            println!("Step {step}: {:?}", state);
        }
        StreamEvent::End { output } => {
            println!("Final output: {:?}", output);
        }
        _ => {}
    }
}
```

### Streaming with Real LLM

For LLM applications, `ChatModel::stream()` provides token-by-token streaming:

```rust
use futures::StreamExt;

// stream() is async and returns Result<BoxStream>
let mut stream = llm.stream(&messages, None).await?;
let mut full_response = String::new();

while let Some(chunk_result) = stream.next().await {
    let chunk = chunk_result?;
    if !chunk.content.is_empty() {
        print!("{}", chunk.content);
        full_response.push_str(&chunk.content);
    }
}
```

---

## Human-in-the-Loop (HITL)

HITL allows you to pause graph execution at specific points for human review or input.

### Interrupt Configuration

Use `CompileConfig` to specify where execution should pause:

```rust
use juncture_core::graph::CompileConfig;

let config = CompileConfig {
    interrupt_before: vec!["review".to_string()],  // Pause before "review" node
    interrupt_after: vec!["propose".to_string()],   // Pause after "propose" node
};

let compiled = graph.compile_with_config(config)?;
```

### Detecting Interrupts

After execution, check `output.interrupts` to see if the graph paused:

```rust
let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

if output.interrupts.is_empty() {
    println!("Execution completed without interruption");
} else {
    println!("Interrupted at: {:?}", output.interrupts);
    println!("Current state: {:?}", output.value);

    // In a real application:
    // 1. Show the state to a human reviewer
    // 2. Wait for approval/rejection
    // 3. Resume execution with the updated state
}
```

### HITL Workflow Pattern

A typical HITL workflow follows this pattern:

```
propose -> [INTERRUPT] -> review -> execute
```

The `propose` node generates an action. Execution pauses. A human reviews the action. If approved, execution continues to `review` and then `execute`.

---

## Checkpointing

Checkpointing persists graph state to storage, enabling execution resume across sessions or processes.

### MemorySaver (In-Memory)

```rust
use juncture_checkpoint::MemorySaver;
use std::sync::Arc;

let checkpointer = MemorySaver::new();
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;
```

### Thread Identity

Use `run_id` to maintain execution continuity:

```rust
let config = RunnableConfig::default().with_run_id("session-123");
let output = compiled.invoke(state, &config)?;

// Later, in a different process or session:
let config = RunnableConfig::default().with_run_id("session-123");
let output = compiled.invoke(fresh_state, &config)?;
// The checkpointer restores the actual state from the last checkpoint
```

### Checkpoint Storage Options

| Storage | Crate | Use Case |
|---------|-------|----------|
| `MemorySaver` | juncture-checkpoint | Development, testing |
| `SqliteSaver` | juncture-checkpoint | Single-node production |
| `PostgresSaver` | juncture-checkpoint | Distributed production |

---

## Error Handling

Juncture provides structured error handling for graph execution.

### Returning Errors from Nodes

Nodes can return `Err(JunctureError)` to signal failure:

```rust
graph.add_node_simple(
    "risky_operation",
    NodeFnUpdate(|state: &MyState| async move {
        if something_wrong {
            return Err(JunctureError::execution("Operation failed"));
        }
        Ok(MyStateUpdate { .. })
    }),
)?;
```

### Error Recovery Patterns

**Retry with backoff:**
```rust
graph.add_node_simple(
    "process",
    NodeFnUpdate(|state: &MyState| {
        let retries = state.retries;
        async move {
            if retries < 3 {
                // Fail and retry
                return Err(JunctureError::execution("Transient error"));
            }
            Ok(MyStateUpdate { status: Some("done".into()), .. })
        }
    }),
)?;

graph.add_edge("process", "recovery");
graph.add_edge("recovery", "process");  // Retry loop
graph.add_edge("process", "fallback");  // Final fallback
```

**Conditional error routing:**
```rust
const fn error_router(state: &MyState) -> &str {
    if state.status == "error" { "fallback" }
    else if state.retries < 3 { "retry" }
    else { "continue" }
}
```

---

## Tools

Tools extend LLM capabilities by providing callable functions.

### Defining a Tool

```rust
use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};

#[derive(Debug)]
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "Returns current weather for a city. Input: {\"city\": \"name\"}"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "City name"
                }
            },
            "required": ["city"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let city = input["city"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'city'".into()))?;

        // Tool implementation
        Ok(format!("Weather in {city}: 22C, sunny"))
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

### Binding Tools to LLM

```rust
let tool_def = ToolDefinition {
    name: weather.name().to_string(),
    description: weather.description().to_string(),
    parameters: weather.schema(),
};

let llm_with_tools = llm.bind_tools(vec![tool_def]);
let response = llm_with_tools.invoke(&messages, None).await?;

// Check for tool calls
for tc in &response.tool_calls {
    println!("Tool: {}({})", tc.name, tc.arguments);
    let result = weather.invoke(tc.arguments.clone()).await?;
    println!("Result: {result}");
}
```

### Agent Loop Pattern

A common pattern is the ReAct agent loop:

```rust
let max_iterations = 10;
for iteration in 0..max_iterations {
    let response = llm_with_tools.invoke(&messages, None).await?;
    messages.push(response.clone());

    if response.tool_calls.is_empty() {
        break;  // No more tools to call
    }

    // Execute each tool call
    for tc in &response.tool_calls {
        let result = execute_tool(tc).await?;
        messages.push(Message {
            role: Role::Tool,
            content: Content::Text(result),
            tool_call_id: Some(tc.id.clone()),
            name: Some(tc.name.clone()),
            ..Default::default()
        });
    }
}
```

### Built-in Tools

| Tool | Description |
|------|-------------|
| `ThinkTool` | Agent self-reflection |
| `SubagentTool` | Delegate tasks to sub-agents |

---

## Structured Output

Extract structured data from LLM responses using tool-based extraction.

### Pattern

1. Define a target schema as a tool
2. Bind the tool with `ToolChoice::Required`
3. Parse the tool call arguments into a Rust struct

```rust
use juncture::llm::{CallOptions, ToolChoice};

let extraction_tool = ToolDefinition {
    name: "extract_info".to_string(),
    description: "Extract person info".to_string(),
    parameters: serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"},
            "occupation": {"type": "string"}
        },
        "required": ["name", "age", "occupation"]
    }),
};

let llm_with_tool = llm.bind_tools(vec![extraction_tool]);

let options = CallOptions {
    tool_choice: Some(ToolChoice::Required),
    ..CallOptions::default()
};

let response = llm_with_tool.invoke(&messages, Some(&options)).await?;

// Parse into struct
if let Some(tc) = response.tool_calls.first() {
    let info: ExtractedInfo = serde_json::from_value(tc.arguments.clone())?;
    println!("Name: {}, Age: {}", info.name, info.age);
}
```

---

## Middleware

Middleware intercepts agent execution for cross-cutting concerns.

### LoopDetectionMiddleware

Prevents infinite tool call loops:

```rust
use juncture::prebuilt::{AgentMiddlewareChain, LoopDetectionMiddleware};

let middleware = AgentMiddlewareChain::new()
    .with(LoopDetectionMiddleware::new(3));  // Max 3 repetitions
```

### ToolErrorHandlingMiddleware

Provides graceful error recovery for tool failures:

```rust
let middleware = AgentMiddlewareChain::new()
    .with(ToolErrorHandlingMiddleware::new());
```

### Using Middleware with Agents

```rust
use juncture::prebuilt::{AgentConfig, create_agent_with_middleware};

let config = AgentConfig {
    system_message: Some("You are a helpful assistant.".into()),
    middleware,
    ..Default::default()
};

let graph = create_agent_with_middleware(model, tools, config)?;
```

---

## Telemetry (OpenTelemetry)

Juncture provides full OpenTelemetry integration for observability.

### Setup

```rust
use juncture_tracing::init;

let metrics_registry = init()
    .with_service_name("my-service")
    .with_otlp_endpoint("http://127.0.0.1:4318")
    .with_metrics(true)
    .install()?
    .expect("metrics enabled");
```

### Metrics Collector

```rust
use juncture_core::observability::MetricsCollector;
use juncture_tracing::RegistryMetricsCollector;

let collector: Arc<dyn MetricsCollector> =
    Arc::new(RegistryMetricsCollector::new(metrics_registry));

let config = RunnableConfig::new()
    .with_metrics_collector(collector);
```

### Callback Handler

```rust
use juncture_core::observability::GraphLifecycleCallback;
use juncture_tracing::callback::{CallbackHandlerAdapter, GraphCallbackHandler};

struct MyCallback;

impl GraphCallbackHandler for MyCallback {
    fn on_node_start(&self, node: &str, task_id: &str) {
        println!("Node {node} started (task {task_id})");
    }
    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        println!("Node {node} finished in {duration_ms}ms");
    }
    fn on_node_error(&self, node: &str, error: &JunctureError) {
        println!("Node {node} failed: {error}");
    }
    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        println!("Graph finished: {:?}", result.is_ok());
    }
}

let handler: Arc<dyn GraphLifecycleCallback> =
    Arc::new(CallbackHandlerAdapter::new(Arc::new(MyCallback)));

let config = RunnableConfig::default()
    .with_callback_handler(handler);
```

### Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `juncture_graph_invocations_total` | Counter | Total graph invocations |
| `juncture_graph_errors_total` | Counter | Total graph errors |
| `juncture_node_duration_ms` | Histogram | Per-node execution duration |
| `juncture_graph_duration_ms` | Histogram | Per-graph execution duration |
| `juncture_llm_calls` | Counter | Total LLM API calls |
| `juncture_tokens_input` | Counter | Input tokens consumed |
| `juncture_tokens_output` | Counter | Output tokens generated |

### Available Spans

| Span | Description |
|------|-------------|
| `juncture.graph.invoke` | Graph execution |
| `juncture.node.execute` | Node execution |
| `juncture.llm.call` | LLM API call |
| `juncture.tool.call` | Tool execution |

---

## Sub-agents

Juncture supports delegating tasks to sub-agents for multi-agent architectures.

### Registering Sub-agents

```rust
use juncture::prebuilt::{InMemoryAgentRegistry, AgentEntry};

let mut registry = InMemoryAgentRegistry::new();
registry.register(
    "researcher".to_string(),
    AgentEntry::from_graph(researcher_graph),
);
```

### Using SubagentTool

```rust
use juncture::prebuilt::SubagentTool;

let tools: Vec<Box<dyn Tool>> = vec![
    Box::new(SubagentTool::new(registry)),
    // ... other tools
];
```

The orchestrator agent can then delegate tasks to sub-agents by calling the `task` tool with a description of the work to be done.

---

## Store (Persistent Key-Value Storage)

Juncture provides a `Store` trait for cross-thread persistent storage.

### MemoryStore

```rust
use juncture_core::store::Store;

let store = MemoryStore::new();
store.put("namespace", "key", json_value, None).await?;
let value = store.get("namespace", "key").await?;
```

### FactStore (Example Pattern)

`FactStore` is an example pattern (in `examples/deep-research`) that wraps `Store` with fact-specific operations for research applications. You can implement a similar pattern for your own domain:

```rust
// FactStore is not part of the juncture crate -- it's an example pattern.
// See examples/deep-research/src/memory/store.rs for the full implementation.

use juncture_core::store::Store;

// Your custom store wrapper
struct MyStore {
    store: Arc<dyn Store>,
    namespace: String,
}

impl MyStore {
    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError> {
        self.store.put(&self.namespace, key, value, None).await
    }

    async fn get(&self, key: &str) -> Result<Option<Item>, StoreError> {
        self.store.get(&self.namespace, key).await
    }
}
```

---

## Next Steps

- [Getting Started](getting-started.md) -- installation and first graph
- [Core Concepts](core-concepts.md) -- framework fundamentals
- [Examples Guide](examples-guide.md) -- all 17 examples walkthrough
