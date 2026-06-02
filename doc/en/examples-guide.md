# Examples Guide

[Chinese Version / 中文版](../zh/examples-guide.md)

This guide walks through all 17 Juncture examples, explaining what each demonstrates and how to run them.

## Example Progression

The examples are organized in order of complexity:

| Phase | Examples | Key Concepts | API Key |
|-------|----------|--------------|---------|
| Core Patterns | 01-03 | State, Reducers, Routing | No |
| LLM Basics | 04-05 | Chat, Tool Calling (mock) | No |
| Advanced Features | 06-09 | Streaming, HITL, Checkpoints, Errors | No |
| Real LLM | 10-15 | Chat, Streaming, Tools, Agents | Yes |
| Production | deep-research, telemetry | Multi-agent, OTel | Yes |

---

## Phase 1: Core Patterns

### Example 01: Basic State Machine

**File:** `examples/src/01_state_machine.rs`

The simplest possible Juncture graph: a linear flow with two nodes.

```bash
cargo run -p juncture-simple-example --bin 01_state_machine
```

**What it demonstrates:**
- `#[derive(State)]` to generate state/update pairs
- `StateGraph::new()` to create a graph builder
- `add_node_simple()` with `NodeFnUpdate` closures
- `add_edge()` for linear flow
- `set_entry_point()` and `set_finish_point()`
- `compile()` and `invoke()`
- Reading `output.value` and `output.metadata.steps`

**Flow:** `START -> greet -> finish -> END`

**Key code pattern:**
```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}
```

---

### Example 02: Counter with Different Reducers

**File:** `examples/src/02_counter_reducers.rs`

Demonstrates how different reducer types handle state merges.

```bash
cargo run -p juncture-simple-example --bin 02_counter_reducers
```

**What it demonstrates:**
- Default `replace` reducer (last writer wins for scalars)
- `#[reducer(append)]` for vector accumulation
- `#[reducer(last_write_wins)]` for explicit semantics
- Custom merge functions for `HashMap`
- Cyclic graph execution (increment -> set_status -> increment -> collect)

**Flow:** `START -> increment -> set_status -> increment -> collect -> END` (with cycle)

**Key code pattern:**
```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CounterState {
    value: u32,                                    // replace (default)
    #[reducer(append)]
    items: Vec<String>,                            // append
    #[reducer(last_write_wins)]
    status: String,                                // last_write_wins
}
```

---

### Example 03: Conditional Routing

**File:** `examples/src/03_conditional_routing.rs`

Shows how to route execution based on state values.

```bash
cargo run -p juncture-simple-example --bin 03_conditional_routing
```

**What it demonstrates:**
- Router functions that inspect state and return target node names
- `PathMap` to map router outputs to node names
- `add_conditional_edges()` for dynamic routing
- Testing with multiple initial states (score=95, 75, 50)

**Flow:** `START -> grade -> {excellent|good|retry} -> END`

**Key code pattern:**
```rust
const fn grade_router(state: &ScoreState) -> &str {
    if state.score >= 90 { "excellent" }
    else if state.score >= 70 { "good" }
    else { "retry" }
}

graph.add_conditional_edges(
    "grade",
    Arc::new(grade_router) as Arc<dyn Router<ScoreState>>,
    PathMap::from(&[("excellent", "excellent"), ("good", "good"), ("retry", "retry")]),
);
```

---

## Phase 2: LLM Basics

### Example 04: Basic Chat with MessagesState

**File:** `examples/src/04_chat_basic.rs`

A simple chatbot using `MessagesState` without a real LLM.

```bash
cargo run -p juncture-simple-example --bin 04_chat_basic
```

**What it demonstrates:**
- `MessagesState` for conversation history
- `Message` constructors (`Message::human()`, `Message::ai()`)
- `Role` enum (`Human`, `Ai`, `System`, `Tool`)
- `Content` enum (`Text`, `MultiPart`)
- Single-node graph that processes messages

**Key code pattern:**
```rust
let initial_state = MessagesState {
    messages: vec![Message::human("Hi there!".to_string())],
};
```

---

### Example 05: Tool Calling (Manual)

**File:** `examples/src/05_tool_calling.rs`

Demonstrates tool definition and execution without a real LLM.

```bash
cargo run -p juncture-simple-example --bin 05_tool_calling
```

**What it demonstrates:**
- `Tool` trait implementation (`name`, `description`, `schema`, `invoke`)
- `ToolError` variants (`InvalidInput`, `ExecutionFailed`, `Timeout`, `ToolNotFound`, `ValidationError`)
- Direct tool invocation
- Building a graph with tool-aware agent node

**Key code pattern:**
```rust
#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Adds two numbers" }
    fn schema(&self) -> serde_json::Value { json!({...}) }
    async fn invoke(&self, input: Value) -> Result<String, ToolError> { ... }
}
```

---

## Phase 3: Advanced Features

### Example 06: Streaming Execution

**File:** `examples/src/06_streaming.rs`

Shows how to stream graph execution events.

```bash
cargo run -p juncture-simple-example --bin 06_streaming
```

**What it demonstrates:**
- `stream()` instead of `invoke()`
- `StreamMode::Values` for streaming state after each superstep
- `StreamEvent` variants (`Values`, `End`)
- Using `futures::StreamExt` to consume the stream
- Three sequential nodes with real-time progress

**Key code pattern:**
```rust
let handle = compiled.stream(initial_state, &config, StreamMode::Values).await?;
let mut stream = handle.stream;
while let Some(result) = stream.next().await {
    match result? {
        StreamEvent::Values { state, step } => { /* process */ }
        StreamEvent::End { output } => { /* done */ }
        _ => {}
    }
}
```

---

### Example 07: Human-in-the-Loop (HITL)

**File:** `examples/src/07_human_in_the_loop.rs`

Demonstrates interrupting execution for human approval.

```bash
cargo run -p juncture-simple-example --bin 07_human_in_the_loop
```

**What it demonstrates:**
- `CompileConfig` with `interrupt_before` and `interrupt_after`
- `compile_with_config()` for HITL workflows
- Checking `output.interrupts` to detect interruptions
- Approval workflow pattern (propose -> review -> execute)

**Flow:** `START -> propose -> [INTERRUPT] -> review -> execute -> END`

**Key code pattern:**
```rust
let config = CompileConfig {
    interrupt_before: vec!["review".to_string()],
    interrupt_after: vec![],
};
let compiled = graph.compile_with_config(config)?;

let output = compiled.invoke(initial_state, &config)?;
if !output.interrupts.is_empty() {
    // Execution paused -- wait for human approval
}
```

---

### Example 08: Checkpoint and Resume

**File:** `examples/src/08_checkpoint_resume.rs`

Shows state persistence across executions.

```bash
cargo run -p juncture-simple-example --bin 08_checkpoint_resume
```

**What it demonstrates:**
- `MemorySaver` for in-memory checkpoint storage
- `compile_with_checkpointer()` for persistence
- `RunnableConfig::with_run_id()` for thread identity
- Resuming execution from saved state
- Cyclic graph with checkpoint-based continuity

**Key code pattern:**
```rust
let checkpointer = MemorySaver::new();
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

let config = RunnableConfig::default().with_run_id("my-session");
let output = compiled.invoke(state, &config)?;
// Later, resume with same run_id to continue from saved state
```

---

### Example 09: Error Recovery

**File:** `examples/src/09_error_recovery.rs`

Demonstrates error handling patterns in graphs.

```bash
cargo run -p juncture-simple-example --bin 09_error_recovery
```

**What it demonstrates:**
- Returning `Err(JunctureError::execution(...))` from nodes
- Error propagation with `?` operator
- Retry logic pattern (process -> recovery -> process -> fallback)
- Graceful degradation with fallback nodes

**Flow:** `START -> process -> recovery -> process -> fallback -> END` (with retry cycle)

---

## Phase 4: Real LLM Applications

These examples require a real LLM API key. Configure your `.env` file first:

```bash
cp examples/.env.example examples/.env
# Set OPENAI_API_KEY, optionally OPENAI_BASE_URL and OPENAI_MODEL
```

### Example 10: Basic Chat with Real LLM

**File:** `examples/src/10_basic_chat.rs`

Single-turn and multi-turn conversation with a real LLM.

```bash
cargo run -p juncture-simple-example --bin 10_basic_chat
```

**What it demonstrates:**
- `ChatOpenAI` client construction
- `ChatModel::invoke()` for single-turn
- Multi-turn by accumulating `Message` history
- System prompts

---

### Example 11: Streaming Chat

**File:** `examples/src/11_streaming_chat.rs`

Token-by-token streaming from a real LLM.

```bash
cargo run -p juncture-simple-example --bin 11_streaming_chat
```

**What it demonstrates:**
- `ChatModel::stream()` for chunk-by-chunk streaming
- Processing `StreamChunk` values in real-time
- Accumulating the full response

---

### Example 12: Tool Calling with Real LLM

**File:** `examples/src/12_tool_calling.rs`

LLM-driven tool selection and execution.

```bash
cargo run -p juncture-simple-example --bin 12_tool_calling
```

**What it demonstrates:**
- `bind_tools()` to attach tools to an LLM
- LLM deciding when to call tools based on user input
- `ToolCall` struct (`name`, `arguments`, `id`)
- Sending tool results back to the LLM
- Multi-step tool execution flow

---

### Example 13: ReAct Agent Loop

**File:** `examples/src/13_react_agent.rs`

A manual agent loop with weather and math tools.

```bash
cargo run -p juncture-simple-example --bin 13_react_agent
```

**What it demonstrates:**
- Manual agent loop (LLM -> tools -> LLM -> ... until no more tool calls)
- Multiple tools (`WeatherTool`, `MathTool`)
- `ToolDefinition` for binding tools to the LLM
- `Role::Tool` messages for tool results
- Max iteration safety limit

---

### Example 14: Multi-turn Conversation

**File:** `examples/src/14_multi_turn.rs`

Conversation history accumulation with a cooking assistant.

```bash
cargo run -p juncture-simple-example --bin 14_multi_turn
```

**What it demonstrates:**
- Accumulating `Vec<Message>` across multiple LLM calls
- System prompt for persona
- Multi-turn context building

---

### Example 15: Structured Output Extraction

**File:** `examples/src/15_structured_output.rs`

Extracting structured JSON from LLM responses.

```bash
cargo run -p juncture-simple-example --bin 15_structured_output
```

**What it demonstrates:**
- Defining a target schema for structured data
- `ToolChoice::Required` to force tool usage
- `CallOptions` for controlling LLM behavior
- Parsing tool call arguments into Rust structs with `serde`
- Entity extraction pattern (name, profession, facts, sentiment)

---

## Phase 5: Production Examples

### Deep Research

**Package:** `examples/deep-research`

A multi-agent research assistant with LLM-driven orchestration.

```bash
cargo run -p deep-research -- "What is the current state of quantum computing?"
cargo run -p deep-research -- --model gpt-4o-mini "Topic"
cargo run -p deep-research -- --verbose "Topic"
```

**What it demonstrates:**
- `create_agent_with_middleware()` for agent with middleware chain
- `SubagentTool` for delegating tasks to sub-agents
- `InMemoryAgentRegistry` for managing sub-agent graphs
- `ThinkTool` for agent self-reflection
- `LoopDetectionMiddleware` to prevent infinite loops
- `ToolErrorHandlingMiddleware` for graceful error recovery
- `FactStore` for persistent cross-session memory
- `clap` CLI argument parsing

**Architecture:**
```
Orchestrator (ReAct agent)
  -> SubagentTool -> Researcher sub-agents (WebSearch + ThinkTool)
  -> ThinkTool (reflection after each delegation)
  -> WebSearch (Tavily API)
  -> Calculator
  -> ReadFile
```

---

### Telemetry Demo

**File:** `examples/src/telemetry_demo.rs`

End-to-end OpenTelemetry pipeline with real LLM and tools.

```bash
# Start telemetry stack
docker compose -f docker/telemetry/docker-compose.yml up -d

# Run demo
cargo run -p juncture-simple-example --bin telemetry_demo

# Verify
# Jaeger UI: http://localhost:16686
# Prometheus: http://localhost:9090
```

**What it demonstrates:**
- `juncture_tracing::init()` for OTel pipeline setup
- `RegistryMetricsCollector` for Prometheus metrics
- `GraphCallbackHandler` for lifecycle callbacks
- `CallbackHandlerAdapter` bridging callbacks to OTel spans
- Real LLM + tool execution with full telemetry
- Error path graph for error metrics verification

**Telemetry coverage:**

| Dimension | Metric/Span |
|-----------|-------------|
| LLM calls | `juncture.llm.call` span |
| LLM tokens | `juncture.tokens.input/output` |
| Tool calls | `juncture.tool.call` span |
| Graph lifecycle | `juncture.graph.invocations`, `duration_ms` |
| Node execution | `juncture.node.duration_ms` |
| Error path | `juncture.graph.errors` |
| Callbacks | `on_node_start/end`, `on_graph_end` |

---

### Example 16: Juncture Telemetry (Langfuse-compatible Dashboard)

**File:** `examples/src/16_juncture_telemetry.rs`

Real LLM agent with tool calling, instrumented with `juncture-telemetry` using the `init()` one-liner builder.

```bash
# Requires .env with OPENAI_API_KEY
cargo run -p juncture-simple-example --bin 16_juncture_telemetry

# Open dashboard
open http://127.0.0.1:8123

# With Langfuse cloud export (add to .env):
# LANGFUSE_PUBLIC_KEY=pk-lf-...
# LANGFUSE_SECRET_KEY=sk-lf-...
# LANGFUSE_BASE_URL=https://cloud.langfuse.com

# Public access
BIND_PUBLIC=1 cargo run -p juncture-simple-example --bin 16_juncture_telemetry
```

**What it demonstrates:**
- `init()` builder for one-line telemetry setup
- `with_langfuse_from_env()` auto-reads `LANGFUSE_*` env vars
- `with_dashboard(8123)` starts embedded web server
- `TelemetryHandle` with RAII auto-flush on drop
- Real agent loop with `bind_tools` and tool execution
- Multi-agent tracing via nested observation trees (`parent_observation_id`)
- Token usage and cost tracking per observation
- Session tracking across multiple traces

**Agent flow:**
```
trace (react_agent)
  ├── span: iteration_1
  │   ├── generation: llm_call (decides to use tools)
  │   ├── tool_call: get_weather({"city":"Tokyo"})
  │   └── tool_call: calculator({"expression":"42 * 17"})
  └── span: iteration_2
      └── generation: llm_call (synthesizes final answer)
```

**Dashboard features:**
- Overview: stat cards, traces-over-time chart, model cost bars, latency percentiles
- Traces: name/user/date filters, token flow notation (`input -> output (total)`)
- Trace detail: two-panel tree + detail, type filters (All/Gen/Tool/Span), observation search
- Sessions: enriched cards with aggregated stats

---

## Next Steps

- [Advanced Features](advanced-features.md) -- deep dive into streaming, HITL, checkpointing, tools, and telemetry
- [Core Concepts](core-concepts.md) -- understand the framework fundamentals
