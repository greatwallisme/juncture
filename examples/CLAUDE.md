# CLAUDE.md -- examples

15 self-contained binaries (01-15) plus a standalone multi-agent deep-research application, demonstrating Juncture graph execution patterns from basic state machines to production-grade LLM pipelines.

## Run Commands

```bash
# Run any mock example (no API key needed)
cargo run -p juncture-simple-example --bin 01_state_machine
cargo run -p juncture-simple-example --bin 07_human_in_the_loop

# Run any real LLM example (requires .env configuration)
cp .env.example .env  # then fill in your API key
cargo run -p juncture-simple-example --bin 10_basic_chat
cargo run -p juncture-simple-example --bin 13_react_agent

# Run deep-research (separate package, requires .env with OPENAI_* and optionally TAVILY_API_KEY)
cargo run -p deep-research -- "What is the current state of quantum computing?"
cargo run -p deep-research -- --model gpt-4o-mini "Explain recent AI breakthroughs"
cargo run -p deep-research -- --verbose "Research topic here"
```

## Examples Overview

| # | Binary | Key Concepts |
|---|--------|--------------|
| 01 | `01_state_machine` | `#[derive(State)]`, linear graph, `invoke()` |
| 02 | `02_counter_reducers` | `#[reducer(append)]`, `#[reducer(last_write_wins)]` |
| 03 | `03_conditional_routing` | `Router` trait, `PathMap`, `add_conditional_edges` |
| 04 | `04_chat_basic` | `MessagesState`, `Message`, `MockChatModel` |
| 05 | `05_tool_calling` | `Tool` trait, `ToolNode`, manual agent graph |
| 06 | `06_streaming` | `stream()`, `StreamMode`, `StreamEvent` |
| 07 | `07_human_in_the_loop` | `CompileConfig` interrupts, `interrupt_before` |
| 08 | `08_checkpoint_resume` | `MemorySaver`, `compile_with_checkpointer()`, thread_id |
| 09 | `09_error_recovery` | Result propagation, error handling with `?` |
| 10 | `10_basic_chat` | `ChatOpenAI`, single/multi-turn with real LLM |
| 11 | `11_streaming_chat` | `ChatModel::stream`, token-by-token display |
| 12 | `12_tool_calling` | `bind_tools`, tool execution loop with real LLM |
| 13 | `13_react_agent` | `create_react_agent`, weather + math tools |
| 14 | `14_multi_turn` | Conversation history accumulation, system prompts |
| 15 | `15_structured_output` | `ToolChoice::Required`, JSON entity extraction |

## Progression

01-03: Core graph patterns (state, reducers, routing)
04-05: LLM integration patterns (chat model, tool calling)
06-09: Advanced features (streaming, HITL, checkpointing, errors)
10-15: Real LLM applications (requires `.env` with API key)
deep-research: Multi-agent research assistant (separate package, see below)

## Deep-Research Example

A standalone multi-agent research application in its own package (`deep-research`). Demonstrates production-grade patterns: LLM-driven orchestration via ReAct agent, subagent delegation, middleware chain (loop detection + error handling), fact extraction, and session persistence.

```bash
# Build and test
cargo build -p deep-research
cargo test -p deep-research

# Run with default model (reads OPENAI_MODEL from .env, falls back to gpt-4o)
cargo run -p deep-research -- "Your research question"

# Options
cargo run -p deep-research -- --model gpt-4o-mini "Topic"   # override model
cargo run -p deep-research -- --verbose "Topic"              # verbose logging
cargo run -p deep-research -- --thread-id session-1 "Topic"  # checkpoint persistence
```

Requires `.env` with `OPENAI_API_KEY`, `OPENAI_BASE_URL` (for OpenAI-compatible APIs), and optionally `TAVILY_API_KEY` for web search. See `examples/deep-research/CLAUDE.md` for full architecture details.

## Environment Configuration (Examples 10-15)

Real LLM examples load configuration from `.env` via `dotenvy`:

```bash
OPENAI_API_KEY=sk-your-key          # Required
OPENAI_BASE_URL=https://...         # Optional, for OpenAI-compatible APIs
OPENAI_MODEL=gpt-4o                 # Optional, defaults to gpt-4o
```

Shared env loading is in `src/common.rs` (loaded via `#[path = "common.rs"] mod common;`).

## Package Name

The package is named `juncture-simple-example` in Cargo.toml (not `juncture-examples`).
