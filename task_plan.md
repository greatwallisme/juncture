# Task Plan: Fix Re-Audit Findings Item-by-Item (2026-07-28)

## Goal
Fix every finding from the 2026-07-28 deep conformance re-audit (13 sync-reviewer findings B-001..B-013 + Sync-mode checkpoint propagation + Debug capacity + dead-code + chat.rs duplicate). NO code simplification — each fix must be a correct, complete implementation. Verify zero warnings/errors after each cluster.

## Constraint (per rust-guidelines 00_quality_gates + project CLAUDE.md)
- NEVER use unwrap/expect/todo/unimplemented in committed code (unreachable! + justification allowed per quality_gates #4).
- All `#[allow]`/`#[expect]` item-level with reason.
- Propagate errors via `?`; no silent swallowing of Result.
- No Cargo.toml lint relaxation. No file-level allows.
- Comments in American English.
- Verify after each cluster: fmt/clippy/test --all-targets/test --doc/doc(-D warnings)/build.

## Fix Phases — STATUS

| # | Phase | Status | Items |
|---|-------|--------|-------|
| A | Isolated quick wins | COMPLETE | Debug capacity 256; Sync-mode checkpoint Err propagation (+FailingCheckpointer tests); scheduler dead_code integrated; B-013 Ollama token usage |
| B | B-001 BLOCKER bulk_update_state | COMPLETE | Real atomic impl (single checkpoint, design sig) + get_state_history (audit-missed stub) + 2 success tests |
| C | B-008 reserved_keys + error markers | COMPLETE | reserved_keys module; runner persists ERROR/ERROR_SOURCE_NODE markers; schedule_error_handlers_from_writes; after_tick wiring; 2 tests |
| D | B-006 Previous Result Injection | COMPLETE | CheckpointMetadata.return_value; RunnableConfig.previous; PREVIOUS task-local+runner scope; func::Runtime fallback; invoke load/save __return__; 2 tests |
| E | B-005 LLM cache wired | COMPLETE | observability::CachePolicy ttl+store; LLM_CACHE_POLICY task-local+runner scope; try_llm_cache_lookup/store; to_core_cache_key_input bridge; openai/anthropic/ollama wired; 5 tests |
| F | B-012 compile_entrypoint cache_policy+timeout | COMPLETE | config::CachePolicy (node-result) store+generate_key+get/put; CompileConfig.cache_policy; timeout→TimeoutPolicy; invoke node-result cache check/store; cache-hit test |
| G | B-007 DebugEvent variants emitted | COMPLETE | All 10 dead variants now emitted (GraphStart/NodeStart/NodeEnd/NodeError/ChannelWrite/ChannelUpdate/Merge/CheckpointSaved/BudgetCheck/GraphEnd) |
| H | B-009 on_interrupt/on_resume invoked | COMPLETE | Core trait + engine calls (emit_interrupt_events, resume/resume_stream) + ResumeValue::to_json_value + tracing adapter forward |
| I | B-002 RemoteGraph operations | COMPLETE | invoke/get_state/get_state_history/update_state/resume delegating to GraphClient + From<ClientError> for JunctureError |
| J | B-003 PregelProtocol implementors | COMPLETE | impl PregelProtocol for CompiledGraph<S,S,S> (full); RemoteGraph documented as client-typed (no SSE/snapshot mismatch) |
| K | B-010 Ollama tool calling | COMPLETE | tools field + bind_tools + OllamaTool conversion + tool_calls parsing (invoke+stream) + 7 tests |
| B-011 | get_graph(xray) | COMPLETE | Node::drawable_subgraph + SubgraphNode override + get_graph expansion + test |
| N | Docs/checklist reconciliation + StreamChannel | COMPLETE | 100% design coverage (214/214); StreamChannel implemented; 6 stale doc/checklist items reconciled |
| L | B-004 #[entrypoint]/#[task] macros | COMPLETE | juncture-derive: `#[task]` (SyncAsyncFuture + cache/retry/timeout via func::run_task + per-task OnceLock) + `#[entrypoint]` (compile() accessor, node-compatible &S, clone adapter); core func::run_task + task_cache_key; facade re-export; integration tests (5); design 03 §13.3 note + checklist 03-026/03-027; 216/216 |
| M | chat.rs duplicate consolidation | COMPLETE | Core = single source of truth; facade re-exports core's ChatModel/LlmError/leaf-types; chat.rs dead duplicate DELETED; facade providers adopt async-stream ChatModel; design 08 §3 + checklist reconciled to facade API; 214/214 |
| Z | Final verification | COMPLETE | fmt/clippy/test --all-targets/test --doc/doc(-D warnings)/build all green; 216/216 |

## Verified quality gates (final, 2026-07-29)
- cargo fmt --all -- --check: PASS
- cargo clippy --workspace --all-targets --all-features -- -D warnings: PASS
- cargo test --workspace --all-targets --all-features: PASS (0 failures, incl. 5 new func_macros tests)
- cargo test --workspace --doc --all-features: PASS
- RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps: PASS
- cargo build --workspace --all-features: PASS
- Design coverage: 216/216 (100.0%)

## Architectural notes (design doc is the spec)
- B-003: RemoteGraph deliberately does NOT impl PregelProtocol — HTTP GraphClient has no SSE stream endpoint and client::StateSnapshot shape differs from checkpoint's; implementing would fabricate data (a simplification). RemoteGraph offers equivalent ops via its own client-typed API (B-002).
- B-005/B-012: TWO distinct CachePolicy types — observability::CachePolicy (LLM response cache, B-005) and config::CachePolicy (node-result cache, B-012). Both wired.
- chat.rs duplicate: facade llm types (CallOptions/ToolDefinition/etc.) structurally duplicate core's. B-005 bridged via to_core_cache_key_input; full consolidation deferred (see M).

## Remaining (2 large items, not started — require dedicated sessions)
- **B-004 #[entrypoint]/#[task] attribute macros** (design 03 §13.3): new proc-macro. Approach: extend juncture-derive with `#[entrypoint]`/`#[task]` attribute macros that expand to `compile_entrypoint`/`compile_entrypoint_with_config` calls, parsing `cache`/`retry`/`timeout`/`name` args into TaskConfig. Est. large.
- **chat.rs duplicate consolidation**: facade `crates/juncture/src/llm/{trait_,message}.rs` defines CallOptions/ToolDefinition/ToolChoice/ResponseFormat/Message duplicates of `juncture-core/src/llm.rs` + `state/messages.rs`. Consolidation: make facade re-export core types (verify no behavioral impls differ), update all facade providers/prelude. Est. large refactor.

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| rust-analyzer stale diagnostics after edits | many | Verified with `cargo check` (authoritative) — diagnostics were stale |
| `assertions_on_result_states` (is_ok no-message) | 1 | Added messages to no-message asserts |
| facade llm types distinct from core (B-005 block) | 1 | to_core_cache_key_input field-by-field bridge (identical types) |
| config::CachePolicy vs observability::CachePolicy (B-012) | 2 | Initial observability approach wrong; reworked to config node-result |
| inline `use` in generate_key (hook) | 1 | Moved to file top |
| rustdoc broken `ClientError` links (remote.rs) | 1 | Full path `(crate::client::ClientError)` |
