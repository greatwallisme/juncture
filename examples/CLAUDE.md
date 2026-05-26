# CLAUDE.md -- examples

9 self-contained binaries demonstrating Juncture graph execution patterns, from basic state machines to advanced features.

## Run Commands

```bash
# Run any example
cargo run -p juncture-simple-example --bin 01_state_machine
cargo run -p juncture-simple-example --bin 07_human_in_the_loop
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

## Progression

01-03: Core graph patterns (state, reducers, routing)
04-05: LLM integration (chat model, tool calling)
06-09: Advanced features (streaming, HITL, checkpointing, errors)

## Package Name

The package is named `juncture-simple-example` in Cargo.toml (not `juncture-examples`).
