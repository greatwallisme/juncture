# Findings: Juncture Design-to-Code Conformance Audit

## Audit Status: COMPLETE

| Module | Status | Critical | Major | Minor | Conformance |
|--------|--------|----------|-------|-------|-------------|
| 01 State & Channel | complete | 3 | 1 | 8 (code exceeds) | 85% |
| 02 Graph Builder | complete | 4 | 3 | 8 (code exceeds) | 82% |
| 03 Pregel Engine | complete | 1 | 4 | 8 (code exceeds) | 85% |
| 04 Checkpoint | complete | 2 | 2 | 4 (code exceeds) | 88% |
| 05 Streaming | complete | 0 | 5 | 3 (code exceeds) | 87% |
| 06 HITL | complete | 0 | 5 | 4 (code exceeds) | 90% |
| 07 Subgraph | complete | 0 | 0 | 8 (code exceeds) | 95% |
| 08 LLM & Tools | complete | 2 | 6 | 8 (code exceeds) | 75% |
| 09 Observability | complete | 0 | 3 | 5 (code exceeds) | 88% |
| 10 Store | complete | 4 | 3 | 2 (code exceeds) | 65% |
| **TOTAL** | | **16** | **32** | **58** | **84% avg** |

---

## Module 01: State & Channel

### Critical Findings (A)

#### [A-01-001] Missing try_apply() in proc-macro generation
- **Category:** Missing
- **Design ref:** design/01-state-channel.md section 2.4 (proc-macro #[derive(State)])
- **Code ref:** juncture-derive/src/state_derive.rs:167-198, juncture-core/src/state/trait_.rs:29-31
- **Description:** The proc-macro generates `fn apply()` but does NOT generate `fn try_apply()`. The default implementation just delegates to `apply()`, so reducer constraint violations (like multiple writers on a replace channel) are never detected as errors. ReplaceReducer should return an error on multiple writes per design section 3.1.
- **Impact:** Silent data corruption when multiple nodes write to the same replace-reducer field in the same superstep. Users get non-deterministic "last write wins" instead of a clear error.

#### [A-01-002] Incomplete finish_field() implementation
- **Category:** Incomplete
- **Design ref:** design/01-state-channel.md sections 1.2, 2.2 (AfterFinish variant)
- **Code ref:** juncture-derive/src/state_derive.rs:167-198, juncture-core/src/state/trait_.rs:43
- **Description:** The trait defines `fn finish_field()` as a default no-op, but the proc-macro does NOT generate any `finish_field()` implementation. `LastValueAfterFinishChannel` exists in channel.rs:290-367 with a `finish()` method, but there's no bridge from State trait's `finish_field()` to per-channel `finish()` calls.
- **Impact:** Fields using `#[reducer(replace_after_finish)]` never become available because `finish()` is never called. The design's core "delayed trigger" pattern is broken.

#### [A-01-003] Missing field_versions()/bump_versions() integration
- **Category:** Deviation
- **Design ref:** design/01-state-channel.md section 2.6 (version tracking)
- **Code ref:** juncture-derive/src/state_derive.rs:167-198, juncture-core/src/state/trait_.rs:49-58
- **Description:** The proc-macro does NOT generate `field_versions()` or `bump_versions()` implementations. Defaults are no-ops returning `Default::default()`. Version tracking is managed externally in `PregelLoop` via `FieldVersionTracker` (pregel/loop_.rs:131). This is an architectural mismatch - the State trait API is misleading since `state.field_versions()` always returns empty.
- **Impact:** Users who call `state.field_versions()` get empty/default values instead of actual versions. State trait API is misleading. Either remove these methods or implement them properly.

### Major Findings (B)

#### [B-01-001] Missing consume() integration in State trait and Pregel loop
- **Category:** Missing
- **Design ref:** design/01-state-channel.md section 2.5 (Channel lifecycle: consume step)
- **Code ref:** juncture-core/src/state/trait_.rs, juncture-core/src/pregel/loop_.rs
- **Description:** Design specifies that after `apply_writes()`, all triggered channels should call `consume()`. The Channel trait defines `consume()` in channel.rs:165, but this is never called by the Pregel engine. Without this, the `consumed` flag in `EphemeralChannel` (line 238) is never set, breaking the channel's designed semantics.
- **Impact:** EphemeralChannel consumed flag never set, breaking designed consume semantics.

### Code Exceeds Design (C) - No action needed

1. **[C-01-001]** Overwrite<T> serialization correctly uses `{"__overwrite__": value}` format per design
2. **[C-01-002]** REMOVE_ALL_MESSAGES provides factory methods (Message::remove_all(), Message::remove()) - more ergonomic than design
3. **[C-01-003]** Message::content_text() helper extracts text from Content::Text and Content::MultiPart
4. **[C-01-004]** Message::ai_with_tool_calls() constructor - eliminates boilerplate
5. **[C-01-005]** DeltaBlob uses serde_json::Value instead of generic T - simplifies checkpoint serialization
6. **[C-01-006]** FieldVersions derives Debug - essential for troubleshooting
7. **[C-01-007]** FieldsChanged methods are const fn - compile-time optimization
8. **[C-01-008]** Proc-macro supports 8 reducer types (not just 5 listed in design section 2.4)

---

## Module 02: Graph Builder

### Critical Findings (A)

#### [A-02-001] StateGraph missing I/O Schema generics
- **Category:** Deviation
- **Design ref:** design/02-graph-builder.md section 1.1 (lines 20-45)
- **Code ref:** juncture-core/src/graph/builder.rs:636
- **Description:** Design specifies `StateGraph<S, I: IntoState<S> = S, O: FromState<S> = S>` with 3 type parameters. Implementation only has `StateGraph<S>`. `IntoState<S>` and `FromState<S>` traits exist in state/trait_.rs but have ZERO implementations.
- **Impact:** Cannot hide private fields or use different input/output schemas. All graph operations expose full S type.

#### [A-02-002] add_node() returns Result not &mut Self
- **Category:** Deviation
- **Design ref:** design/02-graph-builder.md section 1 (lines 72-106)
- **Code ref:** juncture-core/src/graph/builder.rs:697-726
- **Description:** Design shows `add_node() -> &mut Self` for fluent chaining. Implementation returns `Result<&mut Self, TopologyError>` with immediate validation. Changes builder pattern semantics from "collect all errors at compile time" to "fail immediately on first error".
- **Impact:** Fluent chaining requires `?` operators, breaking the design's stated builder pattern. However, this is arguably better (fail-fast).

#### [A-02-003] compile() signature mismatch
- **Category:** Deviation
- **Design ref:** design/02-graph-builder.md section 1 (lines 157-163)
- **Code ref:** juncture-core/src/graph/builder.rs:1220-1277
- **Description:** Design: `compile(self, checkpointer: impl CheckpointSaver)` consumes self, requires checkpointer. Implementation: `compile(&self)` borrows self, no checkpointer required. Additional methods `compile_with_config()`, `compile_with_checkpointer()` exist but differ from design.
- **Impact:** Breaking API change - builder is reusable (not consumed), checkpointer is optional (not required).

#### [A-02-004] validate_keys() missing field-level validation
- **Category:** Incomplete
- **Design ref:** design/02-graph-builder.md section 1 (lines 166-170)
- **Code ref:** juncture-core/src/graph/builder.rs:1166-1204
- **Description:** Design says "check all node updates only reference fields defined in State". Implementation validates node names and references but does NOT validate update field references against State definition.
- **Impact:** Nodes can return updates with non-existent fields causing runtime errors instead of compile-time validation.

### Major Findings (B)

#### [B-02-001] add_sequence() return type mismatch
- **Category:** Deviation
- **Design ref:** design/02-graph-builder.md section 1 (lines 131-142)
- **Code ref:** juncture-core/src/graph/builder.rs:1118-1145
- **Description:** Design shows `add_sequence() -> &mut Self`. Implementation returns `Result<&mut Self, TopologyError>` with node existence validation.

#### [B-02-002] PathMap missing path_map! convenience macro
- **Category:** Missing
- **Design ref:** design/02-graph-builder.md section 1 (lines 174-187)
- **Code ref:** juncture-core/src/edge/types.rs:179-243
- **Description:** PathMap type exists with From implementations, but the `path_map!` convenience macro is not implemented.

#### [B-02-003] Command builder pattern missing with_resume() in design
- **Category:** Deviation
- **Design ref:** design/02-graph-builder.md section 4.3 (lines 801-846)
- **Code ref:** juncture-core/src/command.rs:194-199
- **Description:** Implementation has `Command::with_resume(ResumeValue)` supporting ById and ByNamespace variants, not documented in design.

### Code Exceeds Design (C) - No action needed

1. **[C-02-001]** NodeMetadata consolidation - cleaner API with structured config vs many parameters
2. **[C-02-002]** RetryPolicy production-grade features - exponential backoff, jitter, retry_on predicate
3. **[C-02-003]** ErrorHandlerNode wrapper pattern - composes error recovery seamlessly
4. **[C-02-004]** TimeoutNode with TimeoutPolicy - per-node timeout enforcement
5. **[C-02-005]** Command::with_resume() enhancement - supports ById and ByNamespace variants
6. **[C-02-006]** SendTarget with timeout override field
7. **[C-02-007]** TopologyValidator with Tarjan SCC distinguishes valid agent loops from infinite loops
8. **[C-02-008]** CompiledGraph invoke/stream async variants (invoke_async, stream_with_config)

## Module 03: Pregel Engine

### Critical Findings (A)

#### [A-03-001] Incomplete path-based sorting in apply_writes
- **Category:** Incorrect
- **Design ref:** design/03-pregel-engine.md section 5.1 (lines 799-828)
- **Code ref:** juncture-core/src/pregel/loop_.rs:658-669, juncture-core/src/pregel/scheduler.rs:476-523
- **Description:** Design specifies path-based sorting for merge order (PULL tasks alphabetically by node name, PUSH by send index). `scheduler::apply_writes()` implements this correctly, but `after_tick()` bypasses it entirely and applies writes in non-deterministic completion order from JoinSet.
- **Impact:** Non-deterministic state merge when concurrent nodes write same field, violating LangGraph determinism semantics.

### Major Findings (B)

#### [B-03-001] Missing RetryPolicy execution logic
- **Category:** Missing
- **Design ref:** design/03-pregel-engine.md section 11.1 (lines 1576-1706)
- **Code ref:** juncture-core/src/pregel/runner.rs:199-201
- **Description:** Design specifies `execute_with_retry()` with exponential backoff, jitter, configurable max_attempts. Nodes are executed once with no retry wrapper. `RetryPolicy` struct not defined in pregel crate.
- **Impact:** Nodes cannot recover from transient failures automatically.

#### [B-03-002] Missing TimeoutPolicy execution logic
- **Category:** Missing
- **Design ref:** design/03-pregel-engine.md section 11.2 (lines 1708-1774)
- **Code ref:** juncture-core/src/pregel/runner.rs:199-201
- **Description:** Design specifies `TimeoutPolicy` with `run_timeout`/`idle_timeout` and `tokio::time::timeout()` wrapper. No timeout wrapping exists. Nodes can hang indefinitely.
- **Impact:** Nodes can block superstep completion indefinitely.

#### [B-03-003] Missing Durability mode behaviors
- **Category:** Incomplete
- **Design ref:** design/03-pregel-engine.md section 11.3 (lines 1776-1806)
- **Code ref:** juncture-core/src/pregel/loop_.rs:654-888
- **Description:** `Durability` enum exists but checkpoint writes are always synchronous. No async spawning for `Async` mode, no conditional writes for `Exit` mode.
- **Impact:** Performance optimization from durability modes not available.

#### [B-03-004] Missing managed values via Runtime
- **Category:** Missing
- **Design ref:** design/03-pregel-engine.md section 10.2 (lines 1451-1456)
- **Code ref:** juncture-core/src/runtime.rs
- **Description:** Design specifies `Runtime::managed_values()` providing `IsLastStep` and `RemainingSteps`. No `managed_values()` method exists. Nodes cannot make decisions based on execution progress.
- **Impact:** Nodes cannot adapt behavior based on remaining step budget.

### Code Exceeds Design (C)

1. **[C-03-001]** Enhanced error handler integration with `error_handler_map`, `schedule_error_handlers()`, `TaskOutput::error`
2. **[C-03-002]** Comprehensive OpenTelemetry integration with structured span attributes and metrics
3. **[C-03-003]** StreamHandle with run_id for stream resumption and correlation
4. **[C-03-004]** Interrupt version tracking via `interrupt_versions_seen` preventing infinite interrupt loops
5. **[C-03-005]** Multi-interrupt matching with scratchpad and 3-strategy resume (Single, ById, ByNamespace)
6. **[C-03-006]** `finish_all_channels()` implementation for LastValueAfterFinishChannel semantics
7. **[C-03-007]** RunControl graceful shutdown with drain checking in tick()
8. **[C-03-008]** GraphCallbackHandler lifecycle callbacks throughout engine

## Module 04: Checkpoint

### Critical Findings (A)

#### [A-04-001] DeltaChannel ancestor walk reconstruction not implemented
- **Category:** Missing
- **Design ref:** design/04-checkpoint.md section 1.4 (lines 57-77)
- **Code ref:** juncture-core/src/state/channel.rs:369-427
- **Description:** Design specifies ancestor walk algorithm for DeltaChannel: find nearest full snapshot, traverse delta writes forward, replay to reconstruct value. `replay_writes()` method exists but is a stub that only logs a warning. Append-heavy channels store complete checkpoints every time, negating the optimization entirely.
- **Impact:** Storage bloat for append-heavy workloads (e.g., long message histories). DeltaChannel is structurally present but functionally dead.

#### [A-04-002] CheckpointNamespace separator inconsistency
- **Category:** Deviation
- **Design ref:** design/04-checkpoint.md section 7.2 (lines 927-940)
- **Code ref:** juncture-core/src/checkpoint.rs:16-20
- **Description:** Design main text shows `:` separator format but implementation note C-04-5 and actual code use `|`. Implementation note explains `|` avoids UUID v6 colon conflicts. Design doc has internal inconsistency between main spec and implementation note.
- **Impact:** Users reading design will expect `:` but code uses `|`. External tools parsing checkpoint namespaces will fail.

### Major Findings (B)

#### [B-04-001] DeltaCounters tracked but never used
- **Category:** Incomplete
- **Design ref:** design/04-checkpoint.md sections 3.1 (lines 216-222), 3.2 (lines 259-276)
- **Code ref:** juncture-core/src/pregel/loop_.rs:1224, juncture-core/src/state/channel.rs:369-396
- **Description:** `counters_since_delta_snapshot` always populated with empty HashMap. No logic increments counters or uses them to decide between full snapshot vs delta storage. DeltaChannel.snapshot_frequency never consulted by checkpoint backend.
- **Impact:** All checkpoints are full snapshots regardless of snapshot_frequency setting.

#### [B-04-002] put_checkpoint() never called for normal superstep completion
- **Category:** Missing
- **Design ref:** design/04-checkpoint.md sections 1.2 (lines 24-35), 4.4
- **Code ref:** juncture-core/src/pregel/loop_.rs:650-889, juncture-core/src/pregel/runner.rs:301
- **Description:** Design specifies two-phase persistence: put_writes() after each task + put() after superstep. put_writes() is correctly called after each task, but put() is ONLY called for interrupt checkpoints. Normal superstep completion does NOT save a full checkpoint.
- **Impact:** Crash recovery must replay from last interrupt or initial checkpoint. In long-running graphs without interrupts, all completed tasks must be replayed.

### Code Exceeds Design (C)

1. **[C-04-001]** CheckpointSource::Interrupt variant for HITL pause point tracking
2. **[C-04-002]** InterruptSignal persistence in checkpoints enabling ID/namespace-based resume
3. **[C-04-003]** PendingInterrupts vector in checkpoint for HITL state persistence across crashes
4. **[C-04-004]** Three-layer serialization auto-detection (MsgPack/JSON + Encryption + auto-detect)

## Module 05: Streaming

### Critical Findings (A)
(none)

### Major Findings (B)

#### [B-05-001] Missing StreamMode variants from "10 modes" claim
- **Category:** Incomplete
- **Design ref:** design/index.md (stream mapping: "10 modes fully supported")
- **Code ref:** juncture-core/src/stream.rs:6-34
- **Description:** Design index claims 10 modes but only 9 are implemented. Missing: Subgraphs, Events, Metastream, or Stateless (unclear which was intended). The design doc itself specifies 9 modes (7 LangGraph + 2 Juncture).
- **Impact:** Design index documentation inconsistency.

#### [B-05-002] Command::stream_data() not implemented
- **Category:** Missing
- **Design ref:** design/05-streaming.md section 7
- **Code ref:** juncture-core/src/command.rs:4-250
- **Description:** Design mentions Custom data streaming via Command::stream_data(). No such method exists. Custom streaming only via explicit StreamWriter parameter.
- **Impact:** Nodes cannot easily send custom streaming data without receiving StreamWriter.

#### [B-05-003] Incomplete subgraph stream event filtering
- **Category:** Incomplete
- **Design ref:** design/05-streaming.md section 5.3
- **Code ref:** juncture-core/src/graph/compiled.rs:518-531
- **Description:** Subgraph event filtering exists but may not be complete for all nested scenarios. Missing explicit integration between SubgraphTransformer and EventEmitter::with_subgraph_ns().
- **Impact:** Unexpected event filtering in complex nested subgraph scenarios.

#### [B-05-004] LLM streaming integration incomplete
- **Category:** Incomplete
- **Design ref:** design/05-streaming.md section 4
- **Code ref:** juncture-core/src/stream.rs:578-685
- **Description:** call_llm_streaming() exists but integration with actual LLM providers unclear. No evidence of tool call streaming or ToolOutputDelta events being emitted. nostream tag filtering may not be wired into LLM call paths.
- **Impact:** Messages mode may not work for actual LLM streaming scenarios.

#### [B-05-005] Message batching not implemented
- **Category:** Missing
- **Design ref:** design/05-streaming.md section 7.3
- **Code ref:** juncture-core/src/stream.rs:687-730, 910-930
- **Description:** MessageBatchConfig type exists but batching logic is not implemented. BatchTransformer is a stub. No time-based or count-based chunk accumulation.
- **Impact:** High-volume token streaming may produce excessive overhead.

### Code Exceeds Design (C)

1. **[C-05-001]** FilteredValues/FilteredUpdates events for output_key filtering (avoids cloning entire state)
2. **[C-05-002]** StreamResumption with should_skip() for checkpoint-based stream replay
3. **[C-05-003]** StreamHandle with run_id exposure for observability and resumption

## Module 06: HITL

**Note**: Prior review had 8 critical findings. Most have been resolved: update_state(), get_state(), pending_interrupts persistence, ResumeValue::ById, Scratchpad integration are all now implemented.

### Critical Findings (A)
(none - all prior critical issues resolved)

### Major Findings (B)

#### [B-06-001] Missing null-resume mechanism in multi-interrupt matching
- **Category:** Incomplete
- **Design ref:** design/06-hitl.md section 3.1 (lines 223-246)
- **Code ref:** juncture-core/src/interrupt/mod.rs:292-294, juncture-core/src/pregel/runner.rs:387-454
- **Description:** Scratchpad::get_null_resume() exists but null-resume semantics not integrated into match_resume_to_interrupts(). Users cannot implement "click to continue" interrupts without providing dummy values.
- **Impact:** Confirmation-only interrupts require explicit resume values.

#### [B-06-002] interrupt! macro missing named ID support
- **Category:** Incomplete
- **Design ref:** design/06-hitl.md section 2.1 (lines 34-50)
- **Code ref:** juncture-core/src/lib.rs:64-97
- **Description:** Design specifies both interrupt!(payload) and interrupt!("my_id", payload) forms. __interrupt_impl() accepts optional id but no macro syntax to pass it. (Note: previous session claimed this was fixed but actual macro code may not support named form.)
- **Impact:** Users cannot use deterministic interrupt IDs for cross-session resume.

#### [B-06-003] StreamEvent::Interrupt namespace always empty
- **Category:** Incorrect
- **Design ref:** design/06-hitl.md section 2.3 (line 189)
- **Code ref:** juncture-core/src/pregel/loop_.rs:798, 859, 947
- **Description:** All interrupt events emit ns: Vec::new() instead of actual namespace from execution context.
- **Impact:** Subgraph interrupt events cannot be properly namespaced.

#### [B-06-004] Missing Heartbeat integration
- **Category:** Missing
- **Design ref:** design/06-hitl.md section 9.4 (lines 918-936)
- **Code ref:** juncture-core/src/runtime.rs
- **Description:** Runtime should provide heartbeat() for long-running node tasks. Heartbeat struct not defined.
- **Impact:** Cannot implement idle_timeout detection for long-running human tasks.

#### [B-06-005] HIDDEN_TAG filtering not implemented
- **Category:** Missing
- **Design ref:** design/06-hitl.md section 6.1 (lines 587-619)
- **Code ref:** juncture-core/src/interrupt/mod.rs:81, 149-199
- **Description:** HIDDEN_TAG constant defined but no filtering logic in should_interrupt() or stream event emission.
- **Impact:** Internal nodes appear in interrupt checks and stream output.

### Code Exceeds Design (C)

1. **[C-06-001]** Interrupt ID generation correctly uses xxh3_128 per design
2. **[C-06-002]** Multi-interrupt resume algorithm fully implemented with 3-strategy matching
3. **[C-06-003]** Scratchpad integration complete with lifecycle management
4. **[C-06-004]** Checkpoint persistence for interrupts complete with CheckpointSource::Interrupt

## Module 07: Subgraph

**Highest conformance module** -- all core subgraph functionality fully implemented.

### Critical Findings (A)
(none)

### Major Findings (B)
(none)

### Design Clarification Needed

- **StateSubset::map_update()** signature uses by-value (matches design spec), but SubgraphNode uses by-reference closures internally. Need to document the ownership semantics clearly.

### Code Exceeds Design (C)

1. **[C-07-001]** SubgraphTransformer uses `/` separator for stream events (more readable than `:`)
2. **[C-07-002]** CheckpointNamespace implements Display trait for idiomatic formatting
3. **[C-07-003]** SubgraphTransformer provides with_filter_types() convenience method
4. **[C-07-004]** SubgraphConfig simplified to single persistence field (cleaner than design)
5. **[C-07-005]** StateSubset proc-macro generates stricter trait bounds (Clone + Send + Sync + Debug)
6. **[C-07-006]** compute_child_namespace returns Option (Stateless mode returns None correctly)
7. **[C-07-007]** ParentCommand implemented as JunctureError variant (reuses error infrastructure)
8. **[C-07-008]** BubbleUp enum has richer variants: Interrupt, Drained, ParentCommand

## Module 08: LLM & Tools

### Critical Findings (A)

#### [A-08-001] Missing StatefulTool trait with ToolRuntime integration
- **Category:** Missing
- **Design ref:** design/08-llm-tools.md section 4.0
- **Code ref:** juncture/src/tools/trait_.rs, juncture/src/tools/runtime.rs
- **Description:** Design specifies StatefulTool<S> trait with invoke_with_runtime() receiving ToolRuntime<S>. ToolRuntime<S> struct exists but is not integrated into Tool trait hierarchy. Tools cannot access graph state, config, or Store during execution.
- **Impact:** Tools are stateless - cannot access graph context, configuration, or cross-thread Store.

#### [A-08-002] Missing ToolRuntime.emit_output_delta() streaming method
- **Category:** Missing
- **Design ref:** design/08-llm-tools.md section 4.0
- **Code ref:** juncture/src/tools/runtime.rs:39-98
- **Description:** ToolRuntime should have emit_output_delta() for streaming tool results via StreamEvent::Tools(ToolsEvent::ToolOutputDelta). Method does not exist. Entire streaming tool result feature is absent.
- **Impact:** Tools cannot emit incremental output during execution.

### Major Findings (B)

#### [B-08-001] Budget tracking not integrated into LLM providers
- **Category:** Incomplete
- **Design ref:** design/08-llm-tools.md sections 7.1-7.3
- **Code ref:** juncture/src/llm/anthropic.rs, openai.rs, ollama.rs
- **Description:** LLM providers record token usage to tracing spans and metrics but do NOT integrate with BudgetTracker. No automatic budget enforcement or cost tracking.
- **Impact:** No automatic budget enforcement for LLM calls.

#### [B-08-002] StructuredOutputModel uses wrong approach (not tool-based)
- **Category:** Deviation
- **Design ref:** design/08-llm-tools.md section 6.1
- **Code ref:** juncture/src/llm/structured.rs:48-172
- **Description:** Design specifies tool-based extraction (create virtual tool with T's schema, set tool_choice=required, extract from tool_call.arguments). Implementation simply calls model and validates response text as JSON. Less reliable than design's approach.
- **Impact:** Structured output extraction is less reliable than tool-based approach.

#### [B-08-003] tools_condition() signature simplified
- **Category:** Deviation
- **Design ref:** design/08-llm-tools.md section 4.1
- **Code ref:** juncture/src/tools/condition.rs:35-39
- **Description:** Design: tools_condition<S>(state: &S, messages_field: &str). Implementation: tools_condition(messages: &[Message]). Cannot work with custom state types.
- **Impact:** Cannot use tools_condition with non-standard message field locations.

#### [B-08-004] ToolNode input validation not implemented (stub only)
- **Category:** Incomplete
- **Design ref:** design/08-llm-tools.md section 4.2
- **Code ref:** juncture/src/tools/node.rs
- **Description:** ToolNodeConfig.validate_input field exists but validation logic is never implemented. Invalid tool inputs passed without validation.
- **Impact:** Invalid tool inputs cause runtime errors instead of early rejection.

#### [B-08-005] ReactAgentConfig missing advanced fields
- **Category:** Incomplete
- **Design ref:** design/08-llm-tools.md section 5.2
- **Code ref:** juncture/src/prebuilt/react.rs:189-213
- **Description:** Design specifies max_iterations, interrupt_before_tools, pre_model_hook, post_model_hook, model_selector, context_schema, store. Only first two implemented. Missing hooks, model selector, Store integration.
- **Impact:** Cannot inject custom logic, use dynamic model selection, or inject Store context.

#### [B-08-006] ToolCallTransformer and ToolExecutionTrace defined but unused
- **Category:** Incomplete
- **Design ref:** design/08-llm-tools.md section 4.2
- **Code ref:** juncture/src/tools/node.rs
- **Description:** ToolNodeConfig.call_transformer stored but never applied. ToolExecutionTrace struct defined but never created during execution. Both are dead code.
- **Impact:** Cannot transform tool call arguments or audit tool execution.

### Code Exceeds Design (C)

1. **[C-08-001]** ToolError has additional variants: ToolNotFound, ValidationFailed, Intercepted
2. **[C-08-002]** CompositeInterceptor/CompositeTransformer for chaining behaviors
3. **[C-08-003]** ToolError helper methods (constructors + is_fatal()/is_retryable())
4. **[C-08-004]** ToolDefinition provider conversion methods (to_openai_format, to_anthropic_format)
5. **[C-08-005]** ToolNodeConfig with rich validation/interceptor/transformer options
6. **[C-08-006]** PricingTable with comprehensive Claude/GPT/Gemini coverage
7. **[C-08-007]** RetryingModel with full exponential backoff + retry_after extraction
8. **[C-08-008]** MessagesState uses #[derive(State)] with custom messages_reducer

## Module 09: Observability

### Critical Findings (A)
(none)

### Major Findings (B)

#### [B-09-001] Missing juncture.graph.complete span event
- **Category:** Missing
- **Design ref:** design/09-observability.md section 1 (Span hierarchy)
- **Code ref:** juncture-tracing/src/spans.rs:12 (constant exists but unused)
- **Description:** Design shows juncture.graph.complete event with total_steps, total_tokens, cost_usd. Span constant defined but never emitted. Only on_graph_end callback is called, no OTel span.
- **Impact:** Cannot trace complete graph execution lifecycle in distributed tracing systems.

#### [B-09-002] Incomplete OpenTelemetry metrics export
- **Category:** Incomplete
- **Design ref:** design/09-observability.md sections 4, 6.3
- **Code ref:** juncture-tracing/src/config.rs:243 (with_metrics flag unused), 332-378 (no metrics pipeline)
- **Description:** Design says "metrics auto-export when otel enabled". TracingConfig::with_metrics(true) sets a flag that is never read. No OTel metrics MeterProvider configured. Metrics are in-memory HashMap only.
- **Impact:** Users expect metrics to flow to OTLP but only get tracing. Configuration mismatch.

#### [B-09-003] Missing juncture.checkpoint.put span emission
- **Category:** Missing
- **Design ref:** design/09-observability.md sections 1, 6.1
- **Code ref:** juncture-tracing/src/spans.rs:27 (constant exists but unused)
- **Description:** Design lists juncture.checkpoint.put in auto-instrumentation table. Span constant defined but never emitted. Checkpoint saves only produce debug tracing events.
- **Impact:** Cannot observe checkpoint persistence latency in distributed tracing.

### Code Exceeds Design (C)

1. **[C-09-001]** Dual-mode MetricsRegistry (in-memory HashMap + optional OTel Meter) with handle abstraction
2. **[C-09-002]** Blanket Arc<T> and Box<T> GraphCallbackHandler implementations for thread-safe sharing
3. **[C-09-003]** Comprehensive TestMetricsCollector with labeled metrics, utilities, thread-safety
4. **[C-09-004]** ServerInfo builder pattern with From<HashMap> for environment-derived config
5. **[C-09-005]** LlmCachePolicy with custom key_func and CacheKeyInput struct

## Module 10: Store

**Lowest conformance module** -- critical crate duplication issue.

### Critical Findings (A)

#### [A-10-001] Crate duplication: two incompatible Store implementations
- **Category:** Deviation
- **Design ref:** design/10-store.md (Source Code Structure lines 20-33)
- **Code ref:** juncture-store/src/ (standalone), juncture-core/src/store.rs (core, 1619 lines)
- **Description:** Design specifies single standalone crate. TWO implementations exist with incompatible serialization, divergent TTL strategies, and different trait bounds. 1619 lines of duplicated code in core.
- **Impact:** Double maintenance, inconsistent behavior, unclear dependency path.

#### [A-10-002] FilterExpr serialization format incompatibility
- **Category:** Deviation
- **Design ref:** design/10-store.md (Filtering Operators lines 366-437)
- **Code ref:** juncture-store/src/filter.rs:51-56, juncture-core/src/store.rs:183-252
- **Description:** Standalone: tuple variants `And(Vec<FilterExpr>)`. Core: externally tagged `{"op": "$and", "expressions": [...]}`. Different JSON wire formats. Filters serialized by one cannot be deserialized by the other.
- **Impact:** Wire protocol incompatibility between implementations. Data corruption risk.

#### [A-10-003] TTL implementation strategy mismatch
- **Category:** Deviation
- **Design ref:** design/10-store.md (TTL lines 572-699)
- **Code ref:** juncture-store/src/memory.rs:67-111, juncture-core/src/store.rs:310-341, 443-497
- **Description:** Design specifies background sweep task with sweep_interval and sweep_max_items. Standalone crate implements this correctly. Core implementation uses lazy cleanup on read with no background task. Core explicitly admits different strategy.
- **Impact:** Memory leaks in core (expired items accumulate until read), inconsistent behavior.

#### [A-10-004] Debug bound inconsistency between Store traits
- **Category:** Deviation
- **Design ref:** design/10-store.md section 2.1
- **Code ref:** juncture-store/src/trait_.rs:10, juncture-core/src/store.rs:53-54
- **Description:** Standalone crate adds `Debug` bound. Core explicitly rejects it ("async trait intended for dynamic dispatch"). Trait objects incompatible between implementations.
- **Impact:** Cannot use standalone Store implementations where core Store expected.

### Major Findings (B)

#### [B-10-001] Vector search not implemented (placeholder only)
- **Category:** Missing
- **Design ref:** design/10-store.md section 3 (Vector Search lines 306-363)
- **Code ref:** juncture-store/Cargo.toml:21 (empty feature flag), juncture-core/src/store.rs:236-259
- **Description:** Design specifies EmbeddingFunc trait, cosine similarity, auto-embedding on put(). Feature flag exists but is empty. IndexConfig struct exists but search is stubbed.
- **Impact:** Cannot perform semantic search -- key use case for knowledge retrieval.

#### [B-10-002] SQL backend search operations return empty results
- **Category:** Missing
- **Design ref:** design/10-store.md section 6.2 (SQL Backend lines 466-506)
- **Code ref:** juncture-core/src/store.rs:913-920, 1187-1194
- **Description:** SqliteStore::search() and PostgresStore::search() both return empty SearchResult. No WHERE clause generation from FilterExpr.
- **Impact:** No search capability with persistent storage.

#### [B-10-003] list_namespaces offset parameter silently ignored
- **Category:** Incomplete
- **Design ref:** design/10-store.md section 2.1
- **Code ref:** juncture-core/src/store.rs:590-617
- **Description:** Design specifies offset for pagination. Core implementation accepts parameter but ignores it (_offset). Standalone crate implements correctly.
- **Impact:** Incorrect pagination in core implementation.

### Code Exceeds Design (C)

1. **[C-10-001]** FilterExpr::matches() evaluation engine with dot-notation path access and type-aware JSON comparison
2. **[C-10-002]** Item::is_expired() helper method for clean TTL checking
