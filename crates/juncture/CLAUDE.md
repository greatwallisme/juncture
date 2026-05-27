# CLAUDE.md -- juncture (facade crate)

User-facing facade that re-exports `juncture-core` and adds LLM providers, Tool infrastructure, prebuilt agent patterns, and middleware.

## Structure

```
src/
  lib.rs          -- re-exports juncture-core::*, provides prelude
  llm/            -- LLM provider implementations (feature-gated)
  tools/          -- Tool trait, ToolNode, interceptors, transformers, built-in tools
  prebuilt/       -- ReAct agent, MessagesState, agent factory, middleware, subagent delegation
  memory/         -- (reserved for memory integration)
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
| `middleware.rs` | `LlmMiddleware` trait for wrapping individual `ChatModel::invoke()` calls |
| `circuit_breaker.rs` | `CircuitBreaker` for LLM provider health tracking |

## Tools Module (`tools/`)

| File | Description |
|------|-------------|
| `trait_.rs` | `Tool` trait, `StatefulTool`, `ToolDefinition` |
| `node.rs` | `ToolNode`, `ToolNodeConfig`, `ToolExecutionTrace` -- emits `ToolsEvent::ToolStarted`/`ToolFinished` with timestamp and success flag |
| `interceptor.rs` | `ToolInterceptor`, `CompositeInterceptor`, `NopToolInterceptor` |
| `transformer.rs` | `ToolCallTransformer`, `CompositeTransformer`, `NopToolTransformer` |
| `runtime.rs` | `ToolRuntime` for execution context |
| `condition.rs` | `tools_condition`, `tools_condition_from_messages` for conditional edge routing |
| `validation.rs` | `ValidationNode` for tool input validation |
| `permission.rs` | Tool permission checking |
| `error.rs` | `ToolError` |
| `builtin/` | Built-in tool implementations |
| `builtin/think.rs` | `ThinkTool` -- strategic reflection for research agents |
| `builtin/web_fetch.rs` | `WebFetchTool` -- full webpage content fetching (feature `reqwest`) |

## Prebuilt Module (`prebuilt/`)

| File | Description |
|------|-------------|
| `messages_state.rs` | `MessagesState` with `messages: Vec<Message>` using append reducer |
| `react.rs` | `create_react_agent()`, `create_react_agent_with_config()`, `create_agent()`, `create_agent_with_config()`, `ReactAgentConfig`, `AgentNode`, `PromptSource` |
| `agent_factory.rs` | `create_agent_with_middleware()`, `AgentConfig` -- full-featured agent factory with middleware support, pre/post model hooks, model selector, store integration |
| `agent_middleware.rs` | `AgentMiddleware` trait, `AgentMiddlewareChain`, `LoopDetectionMiddleware`, `ToolErrorHandlingMiddleware`, `NopMiddleware`, `MiddlewareAction` |
| `subagent.rs` | `SubagentTool`, `AgentRegistry` trait, `InMemoryAgentRegistry`, `AgentEntry`, `IntoAgentEntry`, `SubagentError` -- multi-agent delegation system |

## Features

- `anthropic` -- Anthropic Claude provider (reqwest + SSE streaming)
- `openai` -- OpenAI GPT provider (reqwest + SSE streaming)
- `ollama` -- Ollama local model provider (reqwest)
- `structured-output` -- Structured output via schemars JSON Schema
- `store` -- Enable `juncture-store` integration
- `reqwest` -- Enable WebFetchTool and other HTTP-dependent built-in tools

## Testing

```bash
cargo test -p juncture
cargo test -p juncture --features anthropic    # Anthropic integration tests
cargo test -p juncture --features openai       # OpenAI integration tests
```

Integration tests: `tests/tools_integration.rs`, `tests/budget_tracking_integration.rs`
