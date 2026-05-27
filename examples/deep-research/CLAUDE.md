# CLAUDE.md -- deep-research

Multi-agent research assistant built with Juncture framework using LLM-driven orchestration.

## Architecture

```
Orchestrator (ReAct agent via create_agent_with_middleware)
  -> SubagentTool -> Researcher sub-agents (WebSearch + ThinkTool)
  -> ThinkTool (reflection after each delegation)
  -> WebSearch (Tavily API)
  -> Calculator
  -> ReadFile

Middleware:
  -> LoopDetectionMiddleware (max 3 repetitions)
  -> ToolErrorHandlingMiddleware (graceful error recovery)
```

## Source Layout

```
src/
  main.rs         -- CLI entry point with clap derive
  lib.rs          -- library root, module declarations
  config.rs       -- ResearchConfig from env vars and CLI args
  state.rs        -- ResearchState using #[derive(State)]
  llm.rs          -- LLM model builder with middleware chain (logging + circuit breaker)
  orchestrator.rs -- LLM-driven orchestrator using create_agent_with_middleware
  agents/
    mod.rs        -- (empty, orchestrator handles all agent logic)
  memory/
    mod.rs        -- Memory module re-exports
    store.rs      -- FactStore for persistent cross-session facts
  tools/
    mod.rs        -- Tool module re-exports
    web_search.rs -- Tavily search API integration
    calculator.rs -- Arithmetic expression evaluation
    file_io.rs    -- Safe file reading from CWD
```

## Run Commands

```bash
# Build
cargo build -p deep-research

# Run with default model (gpt-4o)
cargo run -p deep-research -- "What is the current state of quantum computing?"

# Run with custom model
cargo run -p deep-research -- --model gpt-4o-mini "Explain recent AI breakthroughs"

# Run with verbose logging
cargo run -p deep-research -- --verbose "Research topic here"

# Run with session persistence (checkpointing)
cargo run -p deep-research -- --thread-id my-research-session "Research topic here"
```

## Environment Configuration

Create `.env` in the project root:

```bash
OPENAI_API_KEY=sk-your-key          # Required
OPENAI_BASE_URL=https://...         # Optional, for OpenAI-compatible APIs
TAVILY_API_KEY=tvily-your-key       # Optional, for web search tool
```

## Key Patterns

### Multi-Agent Graph Flow

```
START -> orchestrator (ReAct agent) -> END
```

The orchestrator is a `ReAct` agent that:
1. Analyzes the research query
2. Delegates sub-tasks to researcher sub-agents via `SubagentTool`
3. Uses `ThinkTool` to reflect after each delegation
4. Iterates until sufficient information is gathered
5. Synthesizes findings into a comprehensive report with inline citations [1], [2], ...

### Agent Middleware

The orchestrator uses agent-level middleware:

- **LoopDetectionMiddleware** -- Prevents infinite tool loops (max 3 repetitions)
- **ToolErrorHandlingMiddleware** -- Graceful tool error recovery

### Memory Integration

**FactStore** -- Persists research facts across sessions using `juncture_core::store::Store`

## Tool Schemas

| Tool | Input | Notes |
|------|-------|-------|
| `web_search` | `{"query": "search string"}` | Tavily Search API |
| `calculator` | `{"expression": "2 + 3 * 4"}` | Supports `+`, `-`, `*`, `/` |
| `read_file` | `{"path": "relative/path"}` | Rejects paths outside CWD |

## Testing

```bash
# Run all integration tests
cargo test -p deep-research

# Run specific test
cargo test -p deep-research -- test_calculator_tool

# Quality checks
cargo clippy -p deep-research --all-targets -- -D warnings
```
