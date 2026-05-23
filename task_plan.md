# Task Plan: Strict Fix All 39 Gaps (v3)

## Goal
Fix ALL 39 gaps identified in the strict re-review. Zero deferred items.
Every fix must pass: cargo build + cargo test + cargo clippy with zero warnings.

## Phases

### Phase 1: Dead Code Cleanup (M05) - QUICK WIN
- **Status**: complete

### Phase 2: M05 Cancelled Event + MessageBatchConfig
- **Status**: complete

### Phase 3: M09 Type Deduplication
- **Status**: complete

### Phase 4: M09 Metrics Emissions (12 items)
- **Status**: complete

### Phase 5: M10 SQL Vector Search + InjectedStore
- **Status**: complete

### Phase 6: M01 Missing Channels
- **Status**: complete

### Phase 7: M01 Delta + AfterFinish Fixes
- **Status**: complete

### Phase 8: M06 HITL Audit Trail
- **Status**: complete

### Phase 9: M04 Checkpoint Gaps
- **Status**: complete
- **Items**: M04-001 (TTL cleanup), M04-002 (schema migration), M04-003 (delta recovery)

### Phase 10: M07 StateSubset + M07 add_subgraph
- **Status**: complete
- M07-001: StateSubset proc-macro was already implemented (review agent misidentified gap)
- M07-002: Added `add_subgraph_explicit()` convenience method in builder.rs

### Phase 11: M08 Event Metadata + Lifecycle + Errors
- **Status**: complete
- M08-001: Added timestamp to ToolStarted, success to ToolFinished
- M08-002: Added emit_tool_started/emit_tool_finished to ToolRuntime
- M08-003: Changed LlmError::Other to Box<dyn Error>

### Phase 12: M02 Functional API + M05 with_run_id
- **Status**: complete
- M05-005: Added with_run_id() to RunnableConfig (complete)
- M02-001: Functional API module (complete)
  - Created crates/juncture-core/src/func/mod.rs with Runtime<S>, compile_entrypoint(), compile_entrypoint_with_config()
  - Added pub mod func to lib.rs and re-exports (FuncRuntime, compile_entrypoint, compile_entrypoint_with_config)
  - Fixed 6 clippy issues: doc_markdown, type_repetition_in_bounds, option_as_ref_cloned, assertions_on_result_states, redundant_clone, unused_imports
  - Tests: test_runtime_new, test_runtime_default, test_runtime_with_previous, test_runtime_from_entrypoint_config, test_runtime_clone, test_compile_entrypoint_basic, test_compile_entrypoint_with_config

### Phase 13: Final Verification
- **Status**: complete
- **Result**: cargo build (0 errors), cargo clippy (0 warnings), cargo test (877 passed, 0 failed), cargo fmt (clean)

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| clippy assertions_on_result_states in interrupt tests | 1 | Replaced assert!(result.is_ok()) with result.unwrap() |
| M06 timestamp field missing in InterruptSignal constructions | 1 | Added timestamp: Utc::now() to all 13+ construction sites |
| M04 agent const fn with Arc::new | 1 | Removed const from with_ttl_config() |
| M04 doc backtick errors | 1 | Added backticks around code identifiers in doc comments |
| M04 significant_drop_tightening | 1 | Added #[allow] with reason |
| M02 IntoNode trait bound not satisfied for raw functions | 1 | Wrapped test functions in NodeFnUpdate() wrapper type |
| M02 compile() takes 0 args, not checkpointer | 1 | Changed to compile_with_checkpointer(checkpointer) |
| M02 clippy doc_markdown on StateGraph | 1 | Added backticks around StateGraph in doc comment |
| M02 clippy type_repetition_in_bounds on Debug impl | 1 | Combined S: State + Default + Debug into single bound |
| M02 clippy option_as_ref_cloned | 1 | Replaced .as_ref().cloned() with .clone() |
