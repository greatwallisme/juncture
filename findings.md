# Findings: Documentation Task

## Example Categories

### Core Patterns (01-03) -- No API key needed
- **01_state_machine**: `#[derive(State)]`, linear graph, `invoke()`, `GraphOutput`
- **02_counter_reducers**: `#[reducer(append)]`, `#[reducer(last_write_wins)]`, custom merge functions
- **03_conditional_routing**: `Router` trait, `PathMap`, `add_conditional_edges`

### LLM Integration (04-05) -- Mock/Manual
- **04_chat_basic**: `MessagesState`, `Message`, `Role`, `Content`, single-node chatbot
- **05_tool_calling**: `Tool` trait, `ToolNode`, manual agent graph with tool execution

### Advanced Features (06-09) -- No API key needed
- **06_streaming**: `stream()`, `StreamMode`, `StreamEvent`, `futures::StreamExt`
- **07_human_in_the_loop**: `CompileConfig`, `interrupt_before`, `interrupt_after`, `output.interrupts`
- **08_checkpoint_resume**: `MemorySaver`, `compile_with_checkpointer()`, `thread_id`
- **09_error_recovery**: `JunctureError::execution()`, Result propagation, retry patterns

### Real LLM Applications (10-15) -- Requires API key
- **10_basic_chat**: `ChatOpenAI`, single/multi-turn with real LLM
- **11_streaming_chat**: `ChatModel::stream`, token-by-token display
- **12_tool_calling**: `bind_tools`, tool execution loop with real LLM
- **13_react_agent**: Manual agent loop, WeatherTool + MathTool, tool_call iteration
- **14_multi_turn**: Conversation history accumulation, system prompts
- **15_structured_output**: `ToolChoice::Required`, JSON entity extraction

### Production Examples
- **deep-research**: Multi-agent orchestrator with SubagentTool, middleware chain, FactStore
- **telemetry_demo**: Full OTel pipeline (traces + metrics + callbacks), real LLM + tools

## Key API Patterns

### State Definition
```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct MyState {
    field: String,                          // default: replace reducer
    #[reducer(append)]
    items: Vec<String>,                     // append reducer
    #[reducer(last_write_wins)]
    status: String,                         // last-write-wins
}
```

### Graph Construction
```rust
let mut graph = StateGraph::<MyState>::new();
graph.add_node_simple("name", NodeFnUpdate(|state| async { Ok(Update) }))?;
graph.add_edge("a", "b");
graph.add_conditional_edges("router", router_fn, path_map);
graph.set_entry_point("start");
graph.set_finish_point("end");
let compiled = graph.compile()?;
```

### Execution Modes
- `compiled.invoke(state, &config)` -- blocking single execution
- `compiled.stream(state, &config, StreamMode::Values)` -- async streaming
- `compiled.invoke_async(state, &config)` -- async single execution

### Tool Definition
```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str { "tool_name" }
    fn description(&self) -> &'static str { "description" }
    fn schema(&self) -> serde_json::Value { json!({...}) }
    async fn invoke(&self, input: Value) -> Result<String, ToolError> { ... }
}
```

### LLM Integration
```rust
let llm = ChatOpenAI::new(api_key).with_base_url(url).with_model(model);
let llm_with_tools = llm.bind_tools(vec![tool_def]);
let response = llm_with_tools.invoke(&messages, None).await?;
```

## Environment Configuration
```bash
OPENAI_API_KEY=sk-your-key          # Required for examples 10-15
OPENAI_BASE_URL=https://...         # Optional, for OpenAI-compatible APIs
OPENAI_MODEL=gpt-4o                 # Optional, defaults to gpt-4o
TAVILY_API_KEY=tvily-your-key       # Optional, for web search in deep-research
```
