# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Juncture is a Rust implementation of LangGraph. The programming model is semantically equivalent to LangGraph Python (StateGraph + Pregel execution engine), but uses Rust's type system instead of Python's dynamic Channel mapping.

## Architecture

6-crate workspace:

```
juncture/              # facade crate - prelude, LLM providers, Tool trait, prebuilt agents
juncture-core/         # Channel system, StateGraph, Pregel engine, Node/Edge, Command, HITL, Subgraph
juncture-derive/       # #[derive(State)] proc-macro generating Update structs, merge(), field_versions
juncture-checkpoint/   # CheckpointSaver trait, MemorySaver, SqliteSaver, PostgresSaver
juncture-tracing/      # OpenTelemetry integration, node-level spans, token metrics
juncture-store/        # Cross-thread persistent key-value storage (Store trait, MemoryStore)
```

Each crate has its own `CLAUDE.md` with module-level details.

Key design: `#[derive(State)]` generates typed State/Update pairs with per-field `Reducer<T>` semantics at compile time. `FieldsChanged` is a u64 bitmask tracking which fields changed. `CowState<S>` (Arc-based copy-on-write) is the default State wrapper to avoid expensive clones. Pregel engine uses `tokio::spawn` + `JoinSet` for true multi-core parallelism with `Semaphore`-based bounded concurrency.

## Build & Test

```bash
cargo build --workspace --all-features
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run single test: `cargo test -p juncture-core -- test_name --exact`

## Design Documents (Authoritative)

All implementation MUST follow `design/index.md` and the 10 module design docs (`design/01-*.md` through `design/10-*.md`).

### Design-to-Code Verification

`scripts/verify-design-coverage.py` mechanically checks 214 checklist items (from `design/checklists/*.json`) against Rust source code:

```bash
python3 scripts/verify-design-coverage.py              # full report
python3 scripts/verify-design-coverage.py --summary-only # coverage %
python3 scripts/verify-design-coverage.py --by-finding  # grouped by finding ID
```

Hooks in `.claude/settings.local.json` auto-run verification on `.rs` file writes and session stop.

## Required Skills

- `planning-with-files` -- mandatory for all multi-step tasks
- `rust-guidelines` -- mandatory for ANY Rust code creation or modification (zero-tolerance quality gates)
- `rust-concurrency` -- mandatory when implementing concurrent/async code

## Reference Projects

| Project | Commit ID | Notes |
|---------|-----------|-------|
| **langgraph** (Python) | `076e2a3627206f5a1aef573aaca4a01e5af897ca` | Official LangGraph Python source - Channel architecture reference |
| **langgraph-doc** | N/A (static docs) | LangGraph documentation - design reference |
| **rust-langgraph** | `7828e62edeafb5b8e5b043fd988e3557b2536c95` | Community Rust port - alternative implementation reference |
| **oxidizedgraph** | `2eadb5b56c265122d21f28187bd3feb2bca8ada4` | Rust implementation - reference for Rust patterns |
| **cognis** | `f7a9406db69bbe587d379c95e55c45b6c02d1f9b` | Most complete Rust reference (7+ crates workspace) |

**Note**: Reference projects are active and may have updates since review date. Use commit IDs above for reproducibility.

## Constraints

- All Rust code must pass with zero warnings and zero errors (clippy pedantic/nursery/cargo/restriction)
- Never use `unwrap()`, `todo!()`, `unimplemented!()` in committed code
- Never write placeholder/mock code
- Never use file-level `#![allow(...)]` or `#![expect(...)]` -- always apply `#[allow(...)]` or `#[expect(...)]` at the item level (function, struct, field, impl block) with a reason
- Hookify rules in `.claude/hookify.*.local.md` block simplification patterns in `.rs` files
