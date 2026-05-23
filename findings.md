# Findings: Fresh Design-to-Code Sync Review

## Review Status

| Module | Status | Critical | Major | Minor | Notes |
|--------|--------|----------|-------|-------|-------|
| 01-state-channel | complete | 0 | 0 | 8 (code exceeds) | HIGHLY CONFORMANT |
| 02-graph-builder | complete | 0 | 4 | 8 (code exceeds) | HIGHLY CONFORMANT |
| 03-pregel-engine | complete | 0 | 3 | 8 (code exceeds) | HIGHLY CONFORMANT |
| 04-checkpoint | complete | 0 | 0 | 5 (code exceeds) | EXCELLENT CONFORMANCE |
| 05-streaming | complete | 0 | 1 | 3 (code exceeds) | HIGHLY CONFORMANT |
| 06-hitl | complete | 0 | 0 | 2 (code exceeds) | FULLY CONFORMANT |
| 07-subgraph | complete | 0 | 0 | 3 (code exceeds) | FULLY CONFORMANT |
| 08-llm-tools | complete | 0 | 3 | 8 (code exceeds) | HIGHLY CONFORMANT |
| 09-observability | complete | 2 | 3 | 2 (code exceeds) | REQUIRES REMEDIATION |
| 10-store | complete | 3 | 3 | 1 (code exceeds) | REQUIRES REMEDIATION |

---

## Module 01: State & Channel

**Verdict: HIGHLY CONFORMANT** -- 0 critical, 0 major findings. All 8 findings are code-exceeds-design enhancements.

### Code Exceeds Design (C) - No action needed

1. **[C-01-001]** Structured Error Propagation -- Reducer::reduce()/reduce_one() return `Result<(), InvalidUpdateError>` instead of panicking. Better Rust practice than design specified.
2. **[C-01-002]** State Trait Extension Methods -- Additional methods: try_apply(), finish_field(), consume_field(), consume_field_indices(), replace_field_indices(), replace_after_finish_field_indices(), field_is_set(), field_count(), field_names(), delta_channel_specs(). Enable Pregel engine integration.
3. **[C-01-003]** CowState Production Implementation -- Fully implemented with Arc::make_mut() semantics, get(), get_mut(), update(), commit(), try_commit(). No more todo!() placeholders.
4. **[C-01-004]** FieldVersions as Standalone Type -- Unified FieldVersions(pub Vec<u64>) used by all states instead of per-state generated types. Reduces code generation.
5. **[C-01-005]** FieldsChanged Const Functions -- is_empty() and has_field() as const fn for zero-cost field change tracking in Pregel scheduler hot paths.
6. **[C-01-006]** Message Factory Methods -- Message::remove(), Message::remove_all(), content_text() helper replace direct const value approach. Better API ergonomics.
7. **[C-01-007]** DeltaBlob Simplification -- Uses serde_json::Value instead of generic T. Simplifies checkpoint serialization at cost of type safety.
8. **[C-01-008]** Channel Trait Extensions -- Comprehensive type bounds, error propagation, serde integration beyond basic design spec.

---

## Module 02: Graph Builder

**Verdict: HIGHLY CONFORMANT** -- 0 critical, 4 major (B-level), 8 code-exceeds-design.

### Major Findings (B)

1. **[B-02-001]** add_sequence() return type -- Design comment was misleading; implementation actually matches spec. No action needed.
2. **[B-02-002]** Missing IntoNode blanket impl for Runtime injection -- Design specifies forms E/F (async fn with Runtime parameter). Only forms A-D implemented. Users cannot inject Runtime via simple function signatures.
3. **[B-02-003]** validate_keys() field-level validation missing -- Design says "check all node updates only reference fields defined in State". Implementation validates node names/topology but doesn't inspect node logic for field access validation.
4. **[B-02-004]** Edge module file structure differs from design -- Design suggests fixed.rs, conditional.rs, barrier.rs files. Actual: functionality consolidated in types.rs. No functional impact.

### Code Exceeds Design (C)

1. **[C-02-001]** NodeMetadata consolidation -- cleaner API with structured config vs many parameters. Adds error_handler, timeout_policies.
2. **[C-02-002]** Enhanced RetryPolicy -- production-grade with jitter (0.75-1.25x), retry_on predicate, configurable backoff.
3. **[C-02-003]** ErrorHandlerNode wrapper pattern -- composes error recovery seamlessly with NodeError context.
4. **[C-02-004]** TimeoutNode with TimeoutPolicy -- per-node timeout enforcement via tokio::time::timeout.
5. **[C-02-005]** Enhanced TopologyError variants -- InvalidNodeName, InvalidFieldReference with detailed context.
6. **[C-02-006]** CompileConfig for interrupt defaults -- interrupt_before/after at compile time.
7. **[C-02-007]** SendTarget with per-target timeout override field.
8. **[C-02-008]** Command.stream_data field -- custom JSON payloads for streaming events.

---

## Module 03: Pregel Engine

**Verdict: HIGHLY CONFORMANT** -- 0 critical, 3 major (B-level), 8 code-exceeds-design.

### Major Findings (B)

1. **[B-03-001]** TriggerToNodes optimization not integrated -- `from_trigger_table()` and `triggered_nodes()` defined but `compute_next_tasks()` iterates all completed tasks instead of using reverse mapping. O(nodes) vs O(triggered_nodes) in large graphs.
2. **[B-03-002]** finish_all_channels() condition incomplete -- Only checks `pending_tasks.is_empty()`. Design specifies calling when `compute_next_tasks()` returns empty (broader completion condition). May miss LastValueAfterFinishChannel activation.
3. **[B-03-003]** consume_triggered_channels() called with ALL indices instead of only triggered ones -- Design specifies selective channel consumption per superstep. Current code passes all `consume_field_indices()` rather than only actually triggered channels.

### Code Exceeds Design (C)

1. **[C-03-001]** Multi-interrupt matching algorithm -- 3 strategies (Single, ById, ByNamespace) with scratchpad null-resume handling.
2. **[C-03-002]** Error handler recovery system -- two-phase scheduling (scan ERROR_SOURCE_NODE, create recovery tasks), error_handler_map, TaskOutput.error field.
3. **[C-03-003]** Callback handler integration -- node start/end/error event notifications throughout task execution.
4. **[C-03-004]** Delta counters for checkpointing -- HashMap<String, DeltaCounters> tracking updates/supersteps since last full snapshot.
5. **[C-03-005]** Per-node timeout with heartbeat idle detection -- layered timeout (cancellation -> timeout -> idle -> retry -> interrupt -> node.call).
6. **[C-03-006]** Scratchpad for multi-interrupt null-resume -- tracks processed interrupts, get_null_resume() for partial resume.
7. **[C-03-007]** StreamData in Command -- Vec<serde_json::Value> emitted as StreamEvent::Custom per entry.
8. **[C-03-008]** Interrupt version tracking -- interrupt_versions_seen HashMap stores channel versions at interrupt time for deduplication.

---

## Module 04: Checkpoint

**Verdict: EXCELLENT CONFORMANCE** -- 0 critical, 0 major, 5 code-exceeds-design.

### Code Exceeds Design (C)

1. **[C-04-001]** CheckpointSource::Interrupt variant -- adds HITL pause point tracking for get_state_history filtering by interrupt events.
2. **[C-04-002]** Auto-detection of serialization formats -- `detect_format()` with magic byte inspection + `deserialize_auto()` fallback. Enables seamless JSON-to-MsgPack migration.
3. **[C-04-003]** Dual error type system -- core CheckpointError in juncture-core + storage-specific CheckpointError in checkpoint crate. Finer-grained retry/recovery.
4. **[C-04-004]** Enhanced EncryptedSerializer -- generic inner serializer (monomorphization optimization), PBKDF2 key derivation, composable with any inner format.
5. **[C-04-005]** CheckpointNamespace type system -- structured NamespaceSegment, hierarchical child()/parent()/is_root() operations beyond simple wire format.

---

## Module 05: Streaming

**Verdict: HIGHLY CONFORMANT** -- 0 critical, 1 major (B-level), 3 code-exceeds-design.

### Major Findings (B)

1. **[B-05-001]** Missing CheckpointSaved event emission -- StreamEvent::CheckpointSaved and DebugEvent::CheckpointSaved types defined but never emitted in PregelLoop. StreamMode::Checkpoints is non-functional. Design specifies emission after successful checkpoint persistence.

### Code Exceeds Design (C)

1. **[C-05-001]** FilteredValues/FilteredUpdates events -- output_keys filtering to avoid cloning entire state. Significant performance optimization for large state objects.
2. **[C-05-002]** MessageBatchConfig fully implemented -- configurable max_chunks (10) and flush_interval_ms (100ms) for batching optimization. Addresses design's §7.3 throughput concern.
3. **[C-05-003]** Comprehensive nostream tag filtering -- EventEmitter::has_nostream_tag() integrated with CallOptions, per-message tag filtering in should_emit().

---

## Module 06: HITL

**Verdict: FULLY CONFORMANT** -- 0 critical, 0 major, 2 code-exceeds-design.

### Code Exceeds Design (C)

1. **[C-06-001]** Enhanced interrupt_before/after payloads -- Structured JSON with node name and reason ("interrupt_before"/"interrupt_after") instead of empty/minimal payloads. Better debugging and client handling.
2. **[C-06-002]** HIDDEN_TAG filtering fully implemented -- Design note said "not yet implemented" but is_complete: is_hidden_node() checks __ prefix+suffix, filtering active in should_interrupt(). Note was outdated.

---

## Module 07: Subgraph

**Verdict: FULLY CONFORMANT** -- 0 critical, 0 major, 3 code-exceeds-design.

### Code Exceeds Design (C)

1. **[C-07-001]** CheckpointNamespace type safety -- Struct-based CheckpointNamespace with NamespaceSegment components instead of string manipulation. Compile-time validation, child() method, better serialization.
2. **[C-07-002]** SubgraphTransformer with_filter_types() -- Type-based event filtering in addition to closure-based. Reduces boilerplate for standard event type filtering.
3. **[C-07-003]** SubgraphMount builder pattern -- Fluent API builder with with_name(), with_config(), with_persistence(). Cleaner than direct add_subgraph() parameters.

---

## Module 08: LLM & Tools

**Verdict: HIGHLY CONFORMANT** -- 0 critical, 3 major (B-level), 8 code-exceeds-design.

### Major Findings (B)

1. **[B-08-001]** ChatModel::stream() return type syntax -- Design uses BoxStream shorthand, implementation uses explicit Pin<Box<dyn Stream>>. Functionally equivalent, cosmetic difference.
2. **[B-08-002]** ToolError::ValidationFailed simplified -- Design specifies Vec<String> for multiple errors, implementation uses single String. Limits detailed validation feedback.
3. **[B-08-003]** StructuredOutputModel streaming unsupported -- stream() returns error. Design implies both invoke and stream support. Limits real-time structured extraction.

### Code Exceeds Design (C)

1. **[C-08-001]** Enhanced ToolNode validation -- JSON Schema validation, type checking, required field verification, property-level validation.
2. **[C-08-002]** Tool lifecycle streaming events -- ToolStarted/ToolFinished events with timing metadata via with_tools_event_tx().
3. **[C-08-003]** ToolInterceptor async interface with CompositeInterceptor chaining.
4. **[C-08-004]** StatefulTool<S> trait with ToolRuntime<S> -- state, config, store, emit_output_delta() integration.
5. **[C-08-005]** ReactAgentConfig hooks -- pre_model_hook, post_model_hook, model_selector, store fields.
6. **[C-08-006]** ValidationNode complete implementation -- token limit checking, custom validators, Node<MessagesState>.
7. **[C-08-007]** CallOptions tags field for streaming metadata filtering.
8. **[C-08-008]** StructuredOutputModel hybrid extraction -- use_tool_based flag with automatic text fallback.

---

## Module 09: Observability

**Verdict: REQUIRES REMEDIATION** -- 2 critical (A-level), 3 major (B-level), 2 code-exceeds-design.

### Critical Findings (A)

1. **[A-09-001]** Missing automatic span hierarchy -- Design specifies `juncture.graph.invoke` -> `juncture.superstep` -> `juncture.node.execute` span tree. Only `juncture.node.execute` spans exist. No parent-child correlation in distributed traces. Observability systems see isolated node spans without graph/superstep context.
2. **[A-09-002]** Missing checkpoint span -- Design specifies `juncture.checkpoint.put` span with id/source/step attributes. Implementation uses `juncture.checkpoint.put_writes` instead. External monitoring dashboards filtering for `juncture.checkpoint.put` show no data.

### Major Findings (B)

1. **[B-09-001]** Incomplete metrics collection -- Design specifies 20 metrics (10 counters, 7 histograms, 3 gauges). Only 4 actually emitted: graph.invocations, graph.errors, superstep.duration_ms, budget.remaining_tokens. Missing: all LLM token/cost/duration metrics, tool metrics, checkpoint writes.
2. **[B-09-002]** Missing span attributes -- Many designed attributes never populated: juncture.graph.name, juncture.run.id, juncture.recursion.limit, juncture.step.nodes, juncture.llm.has_tool_calls (hardcoded false), juncture.llm.stop_reason (left Empty).
3. **[B-09-003]** Incomplete OTel metrics pipeline -- TracingConfig::with_metrics(true) sets flag but doesn't wire MetricsRegistry to OTel MeterProvider. Users expect "just works" but must manually wire up pipeline.

### Code Exceeds Design (C)

1. **[C-09-001]** Enhanced callback system -- GraphCallbackHandler trait with on_interrupt/resume/checkpoint_saved/node_start/end/error/graph_end lifecycle methods. Exceeds debug-only design.
2. **[C-09-002]** Labeled metrics support -- TestMetricsCollector with increment_counter_with_labels() and multi-dimensional metric breakdown.

---

## Module 10: Store

**Verdict: REQUIRES REMEDIATION** -- 3 critical (A-level), 3 major (B-level), 1 code-exceeds-design.

### Critical Findings (A)

1. **[A-10-001]** TTL cleanup strategy deviation -- Design specifies background sweep task (start_sweep_task, sweep_expired_items with tokio::time::interval). Implementation uses lazy cleanup on read only. Expired items accumulate indefinitely if never accessed, causing memory leaks.
2. **[A-10-002]** TTLConfig structure violation -- Design requires `sweep_interval: Duration` (required) and `sweep_max_items: usize` (required). Implementation has `sweep_max_items: Option<usize>` (optional) and completely missing `sweep_interval` field. Breaking API change.
3. **[A-10-003]** SQL backend vector search missing -- Design specifies pgvector table structure and vector search for SQL backends. SqliteStore/PostgresStore accept index_config but completely ignore it (prefixed `_index`). No vector table creation, no similarity search in SQL queries. Misleading API.

### Major Findings (B)

1. **[B-10-001]** max_depth parameter silently ignored -- list_namespaces() accepts max_depth but prefixes it `_max_depth` in all three implementations. No depth limiting logic exists. API exposes non-functional parameter.
2. **[B-10-002]** FilterExpr::matches() method missing -- Design specifies method on FilterExpr. Implementation uses standalone evaluate_filter() function instead. Less ergonomic API.
3. **[B-10-003]** Item.embedding field always None for SQL backends -- MemoryStore correctly computes embeddings. SqliteStore/PostgresStore return None. No embedding column in SQL schema. Inconsistent API.

### Code Exceeds Design (C)

1. **[C-10-001]** FilterExpr serde tag correct -- Design note C-10-1b incorrectly claims `#[serde(tag = "op")]` is missing. Implementation correctly has it with tagged serialization format. Design note is outdated.

---

## Overall Summary

### Totals
- **Critical (A-level)**: 5 (Module 09: 2, Module 10: 3)
- **Major (B-level)**: 17 (Module 02: 4, Module 03: 3, Module 05: 1, Module 08: 3, Module 09: 3, Module 10: 3)
- **Code Exceeds (C-level)**: 46 across all modules

### Modules by Conformance

| Tier | Modules |
|------|---------|
| EXCELLENT (0 A, 0 B) | 04-checkpoint, 06-hitl, 07-subgraph |
| HIGHLY CONFORMANT (0 A, few B) | 01-state-channel, 02-graph-builder, 03-pregel-engine, 05-streaming, 08-llm-tools |
| REQUIRES REMEDIATION (has A-level) | 09-observability, 10-store |

### Top 5 Priority Fixes

1. **[A-09-001]** Missing span hierarchy -- No juncture.graph.invoke or juncture.superstep spans. Observability broken for distributed tracing.
2. **[A-10-001]** TTL background sweep missing -- Memory leaks from accumulated expired items.
3. **[A-10-002]** TTLConfig structure violation -- Missing sweep_interval, wrong sweep_max_items type.
4. **[A-10-003]** SQL vector search absent -- Misleading API accepts config but ignores it.
5. **[A-09-002]** Checkpoint span naming wrong -- put_writes instead of put, breaks monitoring.
