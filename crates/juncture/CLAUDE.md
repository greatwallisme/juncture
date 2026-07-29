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
| `trait_.rs` | Re-exports the canonical `juncture-core` LLM surface (`ChatModel`, `LlmError`, `CallOptions`, `ToolDefinition`, `ToolChoice`, `ResponseFormat`, `StructuredOutputModel`, `BoxStream`, `MessageChunk`, `ToolCallChunk`) -- single source of truth (Phase M consolidation) |
| `message.rs` | `TokenUsage` alias |
| `anthropic.rs` | `ChatAnthropic` (feature `anthropic`) -- Anthropic Claude API |
| `openai.rs` | `ChatOpenAI` (feature `openai`) -- OpenAI GPT API |
| `ollama.rs` | `ChatOllama` (feature `ollama`) -- Ollama local model API |
| `mock.rs` | `MockChatModel` for testing (uses `MockError` custom error type); module gated by `#[cfg(any(test, feature = "test-util"))]` (feature `test-util`, off in release builds) |
| `retry.rs` | `RetryingModel` wrapper with configurable retry policy (`RetryExhaustedError` custom error type) |
| `pricing.rs` | `ModelPricing`, `PricingTable` for cost tracking |
| `middleware.rs` | `LlmMiddleware` trait for wrapping individual `ChatModel::invoke()` calls |
| `circuit_breaker.rs` | `CircuitBreaker` for LLM provider health tracking |

## Provider Constructors (fallible)

Since the breaking API change, all provider constructors return `Result<Self, LlmError>` (they validate the API key / config). Propagate with `?`; never chain `.with_*` off a bare `new(...)`:

```rust
use juncture::llm::{ChatOpenAI, LlmError};

fn build_model(api_key: &str) -> Result<ChatOpenAI, LlmError> {
    let model = ChatOpenAI::new(api_key)?.with_model("gpt-4o");
    Ok(model)
}
```

`ChatAnthropic` and `ChatOllama` follow the same fallible `new()` pattern.

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

- `multi-thread` -- (default) true multi-core parallelism via `tokio::spawn` + `JoinSet`; turning it off serializes execution
- `anthropic` -- Anthropic Claude provider (reqwest + SSE streaming)
- `openai` -- OpenAI GPT provider (reqwest + SSE streaming)
- `ollama` -- Ollama local model provider (reqwest)
- `reqwest` -- shared HTTP enablement pulled in by the provider features; also gates `WebFetchTool` and other HTTP-dependent built-in tools
- `store` -- enable `juncture-store` integration
- `wasm` -- forward to `juncture-core/wasm` for WASM targets
- `test-util` -- (default-off) test-only utilities such as `MockChatModel`; the `mock` module is gated by `#[cfg(any(test, feature = "test-util"))]` so test doubles never ship in release builds

## Testing

```bash
cargo test -p juncture
cargo test -p juncture --features anthropic    # Anthropic integration tests
cargo test -p juncture --features openai       # OpenAI integration tests
```

Integration tests: `tests/tools_integration.rs`, `tests/budget_tracking_integration.rs`
