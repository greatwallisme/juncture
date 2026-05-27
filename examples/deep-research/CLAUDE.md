# deep-research

Multi-agent research assistant built with Juncture framework with production-ready features.

## Overview

This application demonstrates building a sophisticated multi-agent research system that uses:
- **Planner agent**: Decomposes research queries into sub-tasks using LLM
- **Researcher agents**: Execute sub-tasks in parallel using web search tool
- **Writer agent**: Synthesizes findings into a comprehensive report
- **Memory integration**: Persists facts across sessions using Store trait
- **LLM middleware**: Observability (logging) and resilience (circuit breaker) for all LLM calls
- **Checkpointing**: Session persistence via MemorySaver for resumable conversations
- **Permissions**: Configurable approval requirements for dangerous operations (file access)

## Architecture

```
src/
  main.rs         -- CLI entry point with clap derive
  config.rs       -- ResearchConfig from env vars and CLI args
  state.rs        -- ResearchState using #[derive(State)] with Finding, SubTask, TaskStatus
  llm.rs          -- LLM model builder with middleware chain (logging + circuit breaker)
  permissions.rs  -- PermissionGuard for tool access control
  agents/
    mod.rs        -- Agent module re-exports
    planner.rs    -- Planner node that decomposes queries into sub-tasks
    researcher.rs -- Research function that executes individual sub-tasks
    writer.rs     -- Writer function that synthesizes findings into reports
  memory/
    mod.rs        -- Memory module re-exports
    store.rs      -- FactStore for persistent cross-session facts
    extractor.rs  -- ResearchFactExtractor wrapping LlmFactExtractor
    conversation.rs -- ConversationTracker wrapping ConversationMemory
  tools/
    mod.rs        -- Tool module re-exports
    web_search.rs -- Tavily search API integration
    calculator.rs -- Arithmetic expression evaluation
    file_io.rs    -- Safe file reading from CWD
    memory_search.rs -- Tool for searching past research facts
  orchestrator.rs -- Multi-agent StateGraph with planner -> coordinator -> writer
```

## Multi-Agent Graph Flow

```
START -> planner -> research_coordinator -> writer -> END
```

1. **Planner node**: Decomposes query into 3-5 sub-tasks using LLM
2. **Research coordinator**: Executes researcher functions in parallel using `tokio::join_all`
3. **Writer node**: Synthesizes all findings into a coherent report using LLM

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

# Run with file access approval required
cargo run -p deep-research -- --require-approval "Research topic here"

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

## Production Features

### LLM Middleware Chain

All LLM calls (planner, researcher, writer) are wrapped with middleware for observability and resilience:

**LoggingMiddleware** (`src/llm.rs`):
- Logs all LLM invocations with model name
- Captures request/response for debugging
- Provides traceability for production monitoring

**CircuitBreaker** (`src/llm.rs`):
- Prevents cascading failures from unhealthy LLM providers
- Configuration:
  - `failure_threshold: 3` -- Opens circuit after 3 consecutive failures
  - `recovery_timeout: 60s` -- Waits 60 seconds before half-open transition
  - `half_open_max_calls: 1` -- Allows 1 test call in half-open state
- Uses lock-free atomic operations for thread-safe state management

### Checkpointing with MemorySaver

Session persistence via `juncture-checkpoint`:
- `MemorySaver` stores checkpoints in memory
- `--thread-id` flag enables session resumption
- Checkpoints saved after each node execution
- Supports conversation history continuation across runs

Example workflow:
```bash
# Start a research session
cargo run -p deep-research -- --thread-id session-123 "Research quantum computing"

# Resume from previous session (continues where it left off)
cargo run -p deep-research -- --thread-id session-123 "Continue research"
```

### Permission System

Tool access control via `PermissionGuard` (`src/permissions.rs`):

**Default permissions (without --require-approval)**:
- `web_search` -- Always allowed
- `calculator` -- Always allowed
- `memory_search` -- Always allowed
- `read_file` -- Always allowed

**With --require-approval flag**:
- `web_search` -- Always allowed
- `calculator` -- Always allowed
- `memory_search` -- Always allowed
- `read_file` -- Requires approval (asks with reason about file access risks)

The permission guard integrates with tool execution to enforce access policies before tool invocation.

## Tool Design

### web_search
- **Name**: `web_search`
- **API**: Tavily Search (POST https://api.tavily.com/search)
- **Input**: `{"query": "search string"}`
- **Output**: Formatted search results with titles, URLs, and content snippets
- **Error handling**: Returns clear error if `TAVILY_API_KEY` not configured

### calculator
- **Name**: `calculator`
- **Input**: `{"expression": "2 + 3 * 4"}`
- **Operators supported**: `+`, `-`, `*`, `/`
- **Error handling**: Returns descriptive error for division by zero or invalid expressions

### read_file
- **Name**: `read_file`
- **Input**: `{"path": "relative/path/to/file.txt"}`
- **Security**: Rejects paths outside current working directory
- **Error handling**: Returns clear error for missing files or invalid paths

### memory_search
- **Name**: `memory_search`
- **Input**: `{"query": "search string", "limit": 5}`
- **Output**: Relevant facts from previous research sessions
- **Error handling**: Returns helpful error if store not configured

## State Management

Uses custom `ResearchState` with `#[derive(State)]`:
- `messages: Vec<Message>` -- append reducer for conversation history
- `query: String` -- original research question (last_write_wins)
- `plan: Option<Vec<SubTask>>` -- research plan with sub-tasks (last_write_wins)
- `findings: Vec<Finding>` -- research findings with append reducer
- `report: Option<String>` -- final research result (last_write_wins)

### Supporting Types

- **Finding**: `{ sub_task: String, content: String, sources: Vec<String> }`
- **SubTask**: `{ id: usize, description: String, status: TaskStatus }`
- **TaskStatus**: `Pending`, `InProgress`, `Completed`

## Memory Integration

### FactStore
Persists research facts across sessions using `juncture_core::store::Store`:
- `save_fact(fact)` -- Save a fact with topic, claim, source, confidence
- `search_facts(query, limit)` -- Search for relevant facts by topic

### ResearchFactExtractor
Wraps `juncture::memory::LlmFactExtractor` to extract facts from `Finding` structs:
- `extract_from_finding(finding)` -- Extract structured facts from research content

### ConversationTracker
Wraps `juncture::memory::ConversationMemory` for conversation management:
- `add_message(message)` -- Add message to conversation
- `get_summary()` -- Get conversation summary with auto-summarization

## Multi-Agent Pattern

Uses `StateGraph` with custom nodes:
1. **Planner node**: Uses `NodeFnUpdate` wrapper with async closure capturing config
2. **Research coordinator**: Uses `tokio::join_all` for parallel sub-task execution
3. **Writer node**: Uses `NodeFnUpdate` wrapper with async closure capturing config

### Node Pattern

All nodes follow the pattern:
```rust
let node = NodeFnUpdate(move |state: &ResearchState| {
    let config = config.clone();
    Box::pin(async move {
        // ... node logic
        Ok(ResearchStateUpdate { ... })
    })
});
graph.add_node("node_name", node)?;
```

## Parallel Execution

The research coordinator uses `tokio::join_all` for parallel execution:
- Spawns multiple `research_sub_task` calls concurrently
- Each researcher creates a temporary `create_react_agent` with web_search tool
- Collects all results into findings vector
- Updates plan with completed status

## Testing

### Integration Tests

The `tests/integration_tests.rs` file contains comprehensive integration tests using `MockChatModel`:

```bash
# Run all integration tests
cargo test -p deep-research

# Run specific test
cargo test -p deep-research -- test_calculator_tool
cargo test -p deep-research -- test_read_file_rejects_traversal

# Run with output
cargo test -p deep-research -- --nocapture
```

### Test Coverage

Integration tests cover:

**Tool Functionality**:
- `test_calculator_tool` -- Basic arithmetic evaluation
- `test_calculator_complex_expression` -- Multi-operator expressions
- `test_calculator_division_by_zero` -- Error handling for invalid operations
- `test_calculator_invalid_expression` -- Parse error handling
- `test_calculator_missing_parameter` -- Required parameter validation

**Security Validations**:
- `test_read_file_rejects_traversal` -- Path traversal attack prevention (`../../../etc/passwd`)
- `test_read_file_rejects_absolute_path` -- Absolute path blocking (`/etc/passwd`)
- `test_read_file_missing_parameter` -- Required parameter validation

**Error Handling**:
- `test_web_search_without_api_key` -- API key configuration validation
- `test_web_search_missing_query` -- Required parameter validation
- `test_memory_search_without_store` -- Store configuration validation
- `test_memory_search_with_empty_store` -- Empty store handling

**State Initialization**:
- `test_research_state_default` -- Default state field initialization

**MockChatModel Integration**:
- `test_mock_chat_model_basic` -- Fixed text response
- `test_mock_chat_model_error` -- Error scenario simulation
- `test_mock_chat_model_tool_calls` -- Tool call response simulation

### Quality Checks

```bash
# Build check
cargo build -p deep-research

# Lint checks (zero warnings required)
cargo clippy -p deep-research --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check

# Full workspace verification
cargo test --workspace --all-targets --all-features
```

// Rust guideline compliant 2026-05-27
