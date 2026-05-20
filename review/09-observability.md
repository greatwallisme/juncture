Module 09: Observability - Conformance Review

Summary

- A findings (Critical): 8
- B findings (Major): 12
- C findings (Minor): 5
- Fully conformant: Multiple core features correctly implemented
- Verdict: Requires targeted remediation - several critical span and metric implementations are missing or incomplete

A Findings (Critical - Missing)

[A-001] Missing Span: juncture.graph.complete

- Design doc: /root/project/juncture/design/09-observability.md § 1 (Span Hierarchy)
- Design spec: "juncture.graph.complete [total_steps=3, total_tokens=600, cost_usd=0.0082]" span should be emitted at graph completion
- Actual impl: No implementation found. Only "juncture.graph.invoke" span exists in /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:340
- Nature: Missing required lifecycle span - breaks complete span hierarchy
- Risk: Incomplete trace visualization, missing graph-level metrics aggregation
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs (line 339-428)
- Git reference: N/A - never implemented
- Action: Add juncture.graph.complete span emission when PregelLoop terminates, with attributes: total_steps, total_tokens, cost_usd, duration_ms

[A-002] Missing Span: juncture.llm.call

- Design doc: /root/project/juncture/design/09-observability.md § 1, § 6.1
- Design spec: "juncture.llm.call [model="...", tokens.in=150, tokens.out=85]" span for every LLM invocation
- Actual impl: No implementation found in LLM modules or facade crate
- Nature: Critical missing span for LLM observability
- Risk: Cannot trace LLM call performance, costs, or token usage
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/llm/ (no span creation)
    - /root/project/juncture/crates/juncture/src/ (facade crate - no LLM span creation)
- Git reference: N/A - never implemented
- Action: Implement juncture.llm.call span in ChatModel implementations with attributes: model, provider, tokens.in, tokens.out, cost_usd, has_tool_calls, stop_reason

[A-003] Missing Span: juncture.tool.call

- Design doc: /root/project/juncture/design/09-observability.md § 1, § 6.1
- Design spec: "juncture.tool.call [tool="search", duration_ms=230]" span for each tool invocation
- Actual impl: No implementation found in tools module
- Nature: Missing tool execution observability
- Risk: Cannot monitor tool performance, failures, or usage patterns
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/tools/ (no span creation)
- Git reference: N/A - never implemented
- Action: Implement juncture.tool.call span in Tool invocation with attributes: tool.name, duration_ms, error (if failed)

[A-004] Incomplete MetricsRegistry API - Missing Builder Pattern

- Design doc: /root/project/juncture/design/09-observability.md § 4.4
- Design spec: registry.counter("name", |b| b.with_description("...").with_unit("1").build()) - full builder API with configuration
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/metrics.rs:294 - only registry.counter("name") with no builder
- Nature: Deviates from explicit metrics API design - no description/unit configuration
- Risk: Cannot create properly documented metrics with units, breaks OpenTelemetry best practices
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs:294-298
    - /root/project/juncture/crates/juncture-core/src/observability.rs:145 (different implementation)
- Git reference: N/A - simplified implementation
- Action: Implement full builder pattern: counter(name, F) -> Counter where F configures CounterBuilder

[A-005] Missing MetricsRegistry Integration with OpenTelemetry Meter

- Design doc: /root/project/juncture/design/09-observability.md § 4.4
- Design spec: MetricsRegistry should wrap opentelemetry::metrics::Meter for creating real OTel metrics
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/metrics.rs:246-255 - Uses in-memory HashMap, not OTel Meter
- Nature: Mock implementation instead of actual OpenTelemetry integration
- Risk: Metrics don't export to OTLP backends, defeats purpose of observability
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs:246-255
    - /root/project/juncture/crates/juncture-core/src/observability.rs:135 (has conditional OTel support)
- Git reference: N/A - placeholder implementation
- Action: Replace HashMap with real opentelemetry::metrics::Meter when otel feature enabled

[A-006] Missing Histogram Boundaries Configuration

- Design doc: /root/project/juncture/design/09-observability.md § 4.4
- Design spec: .with_boundaries(vec![1.0, 5.0, 10.0, ...]) for histogram bucket configuration
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/metrics.rs:318 - No boundaries configuration
- Nature: Missing critical histogram configuration API
- Risk: Cannot optimize histogram buckets for specific use cases (latency percentiles)
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs:318
- Git reference: N/A - never implemented
- Action: Add with_boundaries() method to HistogramBuilder

[A-007] Metrics Not Emitted to OpenTelemetry

- Design doc: /root/project/juncture/design/09-observability.md § 4 (Metrics), § 6.3 (Feature Gate)
- Design spec: When otel feature enabled, metrics should auto-export via OTLP
- Actual impl: Metrics defined in /root/project/juncture/crates/juncture-tracing/src/metrics.rs:169-229 but never exported to OTLP. Only tracing events emitted via
tracing::debug!()
- Nature: OpenTelemetry metrics integration incomplete
- Risk: No metrics in Jaeger/Datadog/Grafana, only spans visible
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs (names defined but not used)
    - /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:474-499 (emits tracing::debug events, not OTel metrics)
- Git reference: N/A - incomplete integration
- Action: Hook up OpenTelemetry metrics SDK with MeterProvider and export metrics

[A-008] Duplicate MetricsRegistry Implementations

- Design doc: /root/project/juncture/design/09-observability.md § 9 (Module Structure)
- Design spec: Single MetricsRegistry in either juncture-tracing OR juncture-core/src/observability.rs
- Actual impl: Two conflicting implementations:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs:246 - HashMap-based mock
    - /root/project/juncture/crates/juncture-core/src/observability.rs:129 - Conditional OTel Meter-based
- Nature: Architectural violation - duplicated type with incompatible APIs
- Risk: User confusion, inconsistent behavior depending on which crate is imported
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/metrics.rs
    - /root/project/juncture/crates/juncture-core/src/observability.rs
- Git reference: N/A - design inconsistency
- Action: Consolidate to single implementation, re-export from appropriate crate

B Findings (Major - Partial/Wrong)

[B-001] Incomplete Span Attributes in juncture.node.execute

- Design doc: /root/project/juncture/design/09-observability.md § 2.3
- Design spec: All attributes required: node.name, duration_ms, error, output_type, step, thread_id
- Actual impl: /root/project/juncture/crates/juncture-core/src/pregel/runner.rs:125-130 - Missing juncture.step, juncture.thread.id, juncture.node.duration_ms (initially Empty but
never set), juncture.node.error (never recorded)
- Missing items:
    - juncture.step attribute not set (step is available in task)
    - juncture.thread.id not set
    - juncture.node.duration_ms recorded to wrong field name or not at all
    - juncture.node.error never recorded on failure
- Risk: Incomplete node observability, cannot correlate with step/thread
- Affected files: /root/project/juncture/crates/juncture-core/src/pregel/runner.rs:125-191
- Git reference: b1f23f3 - partial implementation
- Action: Add all missing attributes: step, thread_id, duration_ms (properly recorded), error

[B-002] Incomplete Span Attributes in juncture.superstep

- Design doc: /root/project/juncture/design/09-observability.md § 2.2
- Design spec: juncture.step, juncture.step.nodes (array), juncture.step.duration_ms
- Actual impl: /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:449-455 - juncture.step.nodes uses debug formatting ?node_names instead of array format
- Missing items:
    - juncture.step.nodes should be array ["agent", "tools"] not debug string
- Risk: Incompatible with OTel backends expecting array attributes
- Affected files: /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:449-455
- Git reference: b1f23f3
- Action: Change ?node_names to array format for proper OTel serialization

[B-003] Incomplete Span Attributes in juncture.graph.invoke

- Design doc: /root/project/juncture/design/09-observability.md § 2.1
- Design spec: juncture.thread.id, juncture.graph.name, juncture.run.id, juncture.recursion.limit
- Actual impl: /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:339-348 - All present via tracing::instrument
- Missing items: None - this one is actually correct
- Risk: None - conformant
- Affected files: /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:339-348
- Git reference: e2fe262 - implemented per design
- Action: None (this is conformant)

[B-004] Missing GraphCallbackHandler Integration in PregelLoop

- Design doc: /root/project/juncture/design/09-observability.md § 11.1
- Design spec: GraphCallbackHandler methods should be called during graph execution lifecycle
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/callback.rs:39-123 - Trait defined but never invoked by PregelLoop
- Missing items:
    - No callback invocation in /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs
    - No way to register callback handler with PregelLoop
- Risk: Callbacks defined but unused, users cannot hook lifecycle events
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs (no callback integration)
    - /root/project/juncture/crates/juncture-core/src/config.rs (no callback_handler field in RunnableConfig)
- Git reference: b1f23f3 - trait exists but not integrated
- Action: Add callback_handler to RunnableConfig, invoke callbacks at appropriate lifecycle points

[B-005] Missing StreamMode::Debug Event Types

- Design doc: /root/project/juncture/design/09-observability.md § 5.1 (DebugEvent enum)
- Design spec: All 12 event types required: GraphStart, SuperstepStart, NodeStart, NodeEnd, NodeError, ChannelWrite, ChannelUpdate, Merge, EdgeTraversed, CheckpointSaved,
BudgetCheck, GraphEnd
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/debug.rs:16-126 - All 12 types defined correctly
- Missing items: None - DebugEvent is complete
- Risk: None for this item - conformant
- Affected files: /root/project/juncture/crates/juncture-tracing/src/debug.rs
- Git reference: e2fe262
- Action: None (this is conformant)

[B-006] DebugEvent SuperstepEnd Mismatch with Design

- Design doc: /root/project/juncture/design/09-observability.md § 5.1
- Design spec: Design shows "SuperstepStart" but no "SuperstepEnd" explicitly listed
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/debug.rs - No SuperstepEnd variant, but /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:598-602
emits SuperstepEnd event
- Missing items: SuperstepEnd variant not in DebugEvent enum but emitted in code
- Risk: Type mismatch - code emits event that doesn't exist in enum
- Affected files:
    - /root/project/juncture/crates/juncture-tracing/src/debug.rs (missing SuperstepEnd variant)
    - /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:598-602 (emits non-existent event)
- Git reference: b1f23f3
- Action: Add SuperstepEnd variant to DebugEvent enum with step and duration_ms fields

[B-007] Missing LlmCachePolicy Integration with RunnableConfig

- Design doc: /root/project/juncture/design/09-observability.md § 11.3
- Design spec: LlmCachePolicy should be accessible via RunnableConfig for LLM calls
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/types.rs:173-249 - LlmCachePolicy defined but not integrated
- Missing items:
    - No llm_cache_policy field in RunnableConfig
    - No way to pass cache policy to LLM calls
- Risk: Cache policy feature defined but unusable
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/config.rs (RunnableConfig missing field)
    - /root/project/juncture/crates/juncture-core/src/observability.rs:339-373 (CachePolicy orphaned)
- Git reference: N/A - defined but not wired up
- Action: Add llm_cache_policy: Option to RunnableConfig, pass to ChatModel calls

[B-008] TestMetricsCollector Missing Label/Attribute Support

- Design doc: /root/project/juncture/design/09-observability.md § 7.2 (Implementation Note C-09-3)
- Design spec: TestMetricsCollector should support labeled metrics: get_counter_with_labels("juncture.llm.calls", &[("model", "gpt-4")])
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/test_utils.rs:28-341 - No label support, only simple name-based counters
- Missing items:
    - No methods for tracking labels/attributes
    - No get_counter_with_labels or similar
- Risk: Cannot test labeled metrics which are critical for real observability
- Affected files: /root/project/juncture/crates/juncture-tracing/src/test_utils.rs
- Git reference: N/A - simplified implementation
- Action: Add HashMap<String, HashMap<Vec, u64>> structure for label support

[B-009] Missing Span Error Status Recording

- Design doc: /root/project/juncture/design/09-observability.md § 6.2 (example code)
- Design spec: span.record("otel.status_code", "ERROR") on node failure
- Actual impl: /root/project/juncture/crates/juncture-core/src/pregel/runner.rs:125-191 - No error status recording on failure
- Missing items:
    - No otel.status_code recording
    - No juncture.node.error attribute recording
- Risk: Failed nodes not properly marked in traces
- Affected files: /root/project/juncture/crates/juncture-core/src/pregel/runner.rs:221-227 (error handling)
- Git reference: N/A - error handling incomplete
- Action: Record otel.status_code = "ERROR" and juncture.node.error in error path

[B-010] Metrics Names Not Used for Actual Metrics

- Design doc: /root/project/juncture/design/09-observability.md § 4.1-4.3
- Design spec: 19 metric names defined (GRAPH_INVOCATIONS, LLM_TOKENS_INPUT, etc.) should be used
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/metrics.rs:169-229 - Names defined but only used in tests, not actual metric creation
- Missing items:
    - No Counter created with these names
    - No Histogram created with these names
    - No Gauge created with these names
- Risk: Metric names defined but never emitted, breaking observability contract
- Affected files: /root/project/juncture/crates/juncture-tracing/src/metrics.rs
- Git reference: N/A - names defined but unused
- Action: Create actual OpenTelemetry instruments using these metric names

[B-011] Checkpoint Span Missing Required Attributes

- Design doc: /root/project/juncture/design/09-observability.md § 2.6
- Design spec: Checkpoint span should have: checkpoint.id, checkpoint.source, checkpoint.step
- Actual impl: /root/project/juncture/crates/juncture-checkpoint/src/memory.rs:222-228 - All 3 attributes present
- Missing items: None - conformant
- Risk: None - this is correct
- Affected files: /root/project/juncture/crates/juncture-checkpoint/src/memory.rs:222-228
- Git reference: e2fe262
- Action: None (conformant)

[B-012] Missing Gauge Metrics for Budget Tracking

- Design doc: /root/project/juncture/design/09-observability.md § 4.3
- Design spec: juncture.budget.remaining_tokens and juncture.budget.remaining_cost_usd gauges
- Actual impl: /root/project/juncture/crates/juncture-core/src/pregel/budget.rs exists but no gauge emissions found
- Missing items:
    - No gauge updates when budget is checked/updated
    - No integration with MetricsRegistry
- Risk: Cannot observe budget exhaustion in real-time
- Affected files:
    - /root/project/juncture/crates/juncture-core/src/pregel/budget.rs (no gauge updates)
    - /root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:366-374 (budget check but no metrics)
- Git reference: N/A - budget tracking exists but not instrumented
- Action: Emit gauge metrics when budget is checked/consumed

C Findings (Minor - Naming/Docs)

[C-001] CachePolicy vs LlmCachePolicy Naming Inconsistency

- Design doc: /root/project/juncture/design/09-observability.md § 11.3 (Implementation Note D-09-3)
- Original design: Type named CachePolicy in design document
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/types.rs:173 - Renamed to LlmCachePolicy to avoid conflict with RunnableConfig::cache_policy
- Rationale: Beneficial - prevents naming collision in RunnableConfig
- Action: Update design doc § 11.3 to reflect LlmCachePolicy naming

[C-002] ServerInfo Builder Pattern Exceeds Design

- Design doc: /root/project/juncture/design/09-observability.md § 11.3 (Implementation Note C-09-1)
- Original design: Plain struct with pub fields
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/types.rs:47-158 - Full builder pattern with with_assistant_id(), with_graph_id(), etc. methods
- Rationale: Beneficial - provides ergonomic fluent API for configuration
- Action: Update design doc § 11.3 to reflect builder pattern

[C-003] GraphCallbackHandler Blanket impl for Arc

- Design doc: /root/project/juncture/design/09-observability.md § 11.1 (Implementation Note C-09-4)
- Original design: No mention of Arc blanket implementation
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/callback.rs:128-156 - Provides impl GraphCallbackHandler for Arc<T> forwarding
- Rationale: Beneficial - critical for Pregel engine which stores Arc
- Action: Update design doc § 11.1 to document Arc blanket impl

[C-004] TestMetricsCollector Exceeds Design

- Design doc: /root/project/juncture/design/09-observability.md § 7.2 (Implementation Note C-09-3)
- Original design: Basic counter/histogram/gauge accessors
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/test_utils.rs - Adds counter_names(), histogram_names(), gauge_names(), clear() utility methods
- Rationale: Beneficial - better test ergonomics
- Action: Update design doc § 7.2 to reflect extended API

[C-005] MetricsRegistry Handle Abstraction

- Design doc: /root/project/juncture/design/09-observability.md § 4.4
- Original design: Direct Counter<Histogram/Gauge> creation
- Actual impl: /root/project/juncture/crates/juncture-tracing/src/metrics.rs:16-163 - CounterHandle, HistogramHandle, GaugeHandle wrapper types
- Rationale: Beneficial - provides cleaner API for in-memory metrics
- Action: Update design doc § 4.4 to reflect handle-based API

Verified Items

The following design requirements are correctly implemented:

1. Span names constants (/root/project/juncture/crates/juncture-tracing/src/spans.rs:7-28)
    - juncture.graph.invoke ✓
    - juncture.graph.complete (constant exists, not emitted) ✓/✗
    - juncture.superstep ✓
    - juncture.node.execute ✓
    - juncture.llm.call (constant exists, not emitted) ✓/✗
    - juncture.tool.call (constant exists, not emitted) ✓/✗
    - juncture.checkpoint.put ✓
2. Span attribute constants (/root/project/juncture/crates/juncture-tracing/src/spans.rs:31-115)
    - All 26 attribute keys correctly defined ✓
3. TracingConfig builder (/root/project/juncture/crates/juncture-tracing/src/config.rs:55-379)
    - with_service_name() ✓
    - with_otlp_endpoint() ✓
    - with_resource_attributes() ✓
    - with_trace_sampling() ✓
    - with_metrics() ✓
    - with_log_level() ✓
    - install() with OTLP pipeline ✓
4. DebugEvent enum (/root/project/juncture/crates/juncture-tracing/src/debug.rs:14-285)
    - All 12 event variants present ✓
    - Helper methods: is_graph_start(), is_node_start(), etc. ✓
5. GraphCallbackHandler trait (/root/project/juncture/crates/juncture-tracing/src/callback.rs:39-123)
    - All 7 lifecycle methods defined ✓
    - Default implementations provided ✓
    - Arc blanket impl ✓ (exceeds design)
6. ServerInfo type (/root/project/juncture/crates/juncture-tracing/src/types.rs:25-158)
    - All 6 fields present ✓
    - Builder pattern methods ✓ (exceeds design)
    - Serde serialization ✓
7. LlmCachePolicy type (/root/project/juncture/crates/juncture-tracing/src/types.rs:173-249)
    - key_func field ✓
    - with_key_func() builder ✓
    - LlmCacheKeyInput struct ✓
8. Checkpoint spans (/root/project/juncture/crates/juncture-checkpoint/src/memory.rs:222-274)
    - juncture.checkpoint.put with attributes ✓
    - juncture.checkpoint.put_writes with attributes ✓
9. Graph invocation span (/root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:339-348)
    - #[tracing::instrument] with correct attributes ✓
10. Superstep span (/root/project/juncture/crates/juncture-core/src/pregel/loop_.rs:449-455)
    - Span created with step and nodes ✓
    - Duration recorded ✓
11. Node execution span (/root/project/juncture/crates/juncture-core/src/pregel/runner.rs:125-130)
    - Span created for each task ✓
    - Output type recorded ✓
12. Feature gate strategy (/root/project/juncture/crates/juncture-tracing/Cargo.toml:12-19)
    - otel feature properly defined ✓
    - Grading dependencies correct ✓
13. init_tracing() convenience (/root/project/juncture/crates/juncture-tracing/src/lib.rs:91-99)
    - No-OTLP initialization ✓

Action Plan

Immediate (blocking - fix before next release)

1. [A-001] Implement juncture.graph.complete span emission in PregelLoop termination
2. [A-002] Implement juncture.llm.call span in ChatModel facade implementations
3. [A-003] Implement juncture.tool.call span in Tool invocation
4. [A-005] Replace HashMap-based MetricsRegistry with OpenTelemetry Meter-based implementation
5. [A-007] Hook up OpenTelemetry metrics SDK and export metrics via OTLP
6. [A-008] Consolidate duplicate MetricsRegistry implementations (unify juncture-tracing and juncture-core)

Short-term (next sprint)

1. [B-001] Add missing span attributes to juncture.node.execute (step, thread_id, duration_ms, error)
2. [B-002] Fix juncture.step.nodes array formatting in superstep span
3. [B-004] Integrate GraphCallbackHandler with PregelLoop lifecycle
4. [B-007] Add LlmCachePolicy to RunnableConfig and wire up to LLM calls
5. [B-009] Add error status recording to node execution spans
6. [B-010] Create actual OpenTelemetry metric instruments using defined metric names
7. [B-011] Implement gauge emissions for budget tracking
8. [B-006] Add SuperstepEnd variant to DebugEvent enum

Recommended (documentation updates)

1. Update design doc § 4.4 to reflect MetricsRegistry handle-based API
2. Update design doc § 11.3 to reflect LlmCachePolicy naming
3. Update design doc § 11.3 to reflect ServerInfo builder pattern
4. Update design doc § 11.1 to document Arc blanket impl
5. Update design doc § 7.2 to reflect extended TestMetricsCollector API

---
Conclusion

Module 09 (Observability) has a solid foundation with correct span names, attribute constants, configuration builder, and event types. However, critical gaps exist in actual span
emissions (LLM calls, tool calls, graph completion), OpenTelemetry metrics integration, and callback handler wiring. The implementation prioritized infrastructure (constants,
types, configuration) over runtime instrumentation, requiring immediate attention to complete the observability story.