# CLAUDE.md -- juncture (facade crate)

User-facing facade that re-exports `juncture-core` and adds LLM providers, Tool infrastructure, and prebuilt agent patterns.

## Structure

```
src/
  lib.rs          -- re-exports juncture-core::*, provides prelude
  llm/            -- LLM provider implementations (feature-gated)
  tools/          -- Tool trait, ToolNode, interceptors, transformers
  prebuilt/       -- ReAct agent, MessagesState
```

## LLM Module (`llm/`)

| File | Description |
|------|-------------|
| `trait_.rs` | `ChatModel` trait, `StructuredOutputModel`, `CallOptions`, `LlmError` (`Other` holds `Box<dyn Error + Send + Sync>`) |
| `message.rs` | Message builder helpers |
| `anthropic.rs` | `ChatAnthropic` (feature `anthropic`) -- Anthropic Claude API |
| `openai.rs` | `ChatOpenAI` (feature `openai`) -- OpenAI GPT API |
| `ollama.rs` | `ChatOllama` (feature `ollama`) -- Ollama local model API |
| `mock.rs` | `MockChatModel` for testing (uses `MockError` custom error type) |
| `retry.rs` | `RetryingModel` wrapper with configurable retry policy (`RetryExhaustedError` custom error type) |
| `pricing.rs` | `ModelPricing`, `PricingTable` for cost tracking |
| `structured.rs` | Structured output extraction (feature `structured-output`) |

## Tools Module (`tools/`)

| File | Description |
|------|-------------|
| `trait_.rs` | `Tool` trait, `ToolDefinition` |
| `node.rs` | `ToolNode`, `ToolNodeConfig`, `ToolExecutionTrace` -- emits `ToolsEvent::ToolStarted`/`ToolFinished` with timestamp and success flag |
| `interceptor.rs` | `ToolInterceptor`, `CompositeInterceptor`, `NopToolInterceptor` |
| `transformer.rs` | `ToolCallTransformer`, `CompositeTransformer` |
| `runtime.rs` | `ToolRuntime` for execution context |
| `condition.rs` | `tools_condition` for conditional edge routing |
| `validation.rs` | `ValidationNode` for tool input validation |
| `error.rs` | `ToolError` |

## Prebuilt Module (`prebuilt/`)

- `messages_state.rs` -- `MessagesState` with `messages: Vec<Message>` using append reducer
- `react.rs` -- `create_react_agent()`, `create_react_agent_with_config()`, `ReactAgentConfig`, `AgentNode`

## Features

- `anthropic` -- Anthropic Claude provider (reqwest + SSE streaming)
- `openai` -- OpenAI GPT provider (reqwest + SSE streaming)
- `ollama` -- Ollama local model provider (reqwest)
- `structured-output` -- Structured output via schemars JSON Schema
- `store` -- Enable `juncture-store` integration

## Testing

```bash
cargo test -p juncture
cargo test -p juncture --features anthropic    # Anthropic integration tests
cargo test -p juncture --features openai       # OpenAI integration tests
```

Integration test: `tests/tools_integration.rs`
