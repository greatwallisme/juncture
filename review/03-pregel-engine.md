# Module 03: Pregel Engine - Conformance Review

## Summary

The Pregel Engine implementation demonstrates **strong conformance** with the design document (design/03-pregel-engine.md). The core architecture, key algorithms, and data structures are well-implemented. However, several gaps exist in advanced features and some design deviations were found.

**Overall Assessment**: Acceptable with recommendations for design doc updates and targeted feature completion.

---

## A Findings (Critical - Missing)

### [A-001] Missing RetryPolicy with exponential backoff implementation
**Design**: Section 11.1 (design/03-pregel-engine.md:1569-1650) specifies `RetryPolicy` struct with `initial_interval`, `backoff_factor`, `max_interval`, `max_attempts`, `jitter`, and `retry_on` fields, plus `execute_with_retry()` function.

**Actual**: Code: `graph/builder.rs:415-426` defines `RetryPolicy` struct but it's a simple placeholder without backoff logic or retry execution wrapper. No `execute_with_retry()` implementation exists in pregel module.

**Risk**: Node-level retry is a critical resilience feature. The current implementation provides the data structure but not the execution logic.

**Affected Files**: 
- `crates/juncture-core/src/graph/builder.rs:415-426`
- Missing: `crates/juncture-core/src/pregel/retry.rs` or integration in `runner.rs`

**Action**: Implement `execute_with_retry()` function and integrate it into the node execution path in `runner.rs`. The retry logic should wrap node calls with exponential backoff and jitter.

---

### [A-002] Missing TimeoutPolicy implementation
**Design**: Section 11.2 (design/03-pregel-engine.md:1700-1760) specifies `TimeoutPolicy` struct with `run_timeout`, `idle_timeout`, and `refresh_on` fields, plus `execute_with_timeout()` function.

**Actual**: Code: `pregel/context.rs:22-31` defines `TimeoutPolicy` struct but it's a simple placeholder with no timeout execution logic. No `execute_with_timeout()` function exists.

**Risk**: Timeout enforcement is critical for preventing hung nodes. Without implementation, nodes can block indefinitely.

**Affected Files**:
- `crates/juncture-core/src/pregel/context.rs:22-31`
- Missing: integration in `runner.rs`

**Action**: Implement `execute_with_timeout()` using `tokio::time::timeout()` and integrate into node execution path before or after retry logic.

---

### [A-003] Missing ExecutionContext and ExecutionConfig usage as designed
**Design**: Section 2.1 (design/03-pregel-engine.md:183-243) specifies `ExecutionContext<S>` struct with `state`, `field_versions`, `versions_seen`, `pending_writes` fields, and `ExecutionConfig` struct with immutable configuration fields.

**Actual**: Code: `pregel/context.rs:39-50` defines `ExecutionContext` and `ExecutionConfig` but `PregelLoop` doesn't use them. Instead, `loop_.rs:117-172` shows `PregelLoop` has flattened fields (lines 119-172). The `as_context()` and `as_config()` methods exist (lines 820-874) but create cloned views rather than using the structs as primary data holders.

**Risk**: This is more of a design doc deviation than a functional gap. The flattened structure is valid but differs from the design's stated separation of concerns.

**Affected Files**:
- `crates/juncture-core/src/pregel/loop_.rs:117-172` (flattened fields)
- `crates/juncture-core/src/pregel/context.rs:39-50` (unused struct definitions)

**Action**: Either update PregelLoop to use ExecutionContext/ExecutionConfig as primary data structures, or update design doc to reflect the flattened design with accessor methods. Implementation note D-03-1 acknowledges this deviation.

---

## B Findings (Major - Partial/Wrong)

### [B-001] apply_writes signature mismatch
**Design**: Section 5.1 (design/03-pregel-engine.md:784-828) specifies `apply_writes(state, superstep_result, nodes, field_versions)` returning `FieldsChanged`.

**Actual**: Code: `scheduler.rs:471-516` implements `apply_writes(state, task_outputs, field_versions)` - missing the `nodes` parameter. The function sorts by task trigger type rather than node registration order.

**Risk**: Minor. The path-based sorting implemented (PULL by node name, PUSH by send index) provides deterministic merge order and matches LangGraph semantics better than node registration order.

**Affected Files**: `crates/juncture-core/src/pregel/scheduler.rs:471-516`

**Action**: Update design doc to reflect that nodes parameter is not needed and path-based sorting is used instead.

---

### [B-002] Missing SyncAsyncFuture.result() error handling
**Design**: Section 13.2 (design/03-pregel-engine.md:2008-2039) specifies `SyncAsyncFuture::result()` returns `T`. Implementation note D-03-8 states it should return `Result<T, JunctureError>`.

**Actual**: Code: `pregel/types.rs` doesn't contain `SyncAsyncFuture` type at all. It's not implemented.

**Risk**: Moderate. `SyncAsyncFuture` is needed for the functional API (@task/@entrypoint decorators). Without it, cached task results cannot be properly retrieved.

**Affected Files**: Missing from `crates/juncture-core/src/pregel/types.rs`

**Action**: Implement `SyncAsyncFuture` enum with `Ready(Result<T, JunctureError>)` and `Future(BoxFuture<'static, Result<T, JunctureError>>)` variants, and `async fn result(self) -> Result<T, JunctureError>`.

---

### [B-003] Missing finish() implementation in after_tick
**Design**: Section 5.5 (design/03-pregel-engine.md:898-919) specifies calling `finish_all_channels()` when `pending_tasks` is empty (execution complete).

**Actual**: Code: `loop_.rs:718-723` implements `finish_all_channels()` call correctly when `pending_tasks.is_empty()`. However, the state trait's `finish_field()` method (trait_.rs:27) is a default no-op.

**Risk**: Minor for now. `LastValueAfterFinishChannel` won't work until channel types implement `finish_field()`.

**Affected Files**:
- `crates/juncture-core/src/pregel/loop_.rs:718-723` (correct call site)
- `crates/juncture-core/src/state/trait_.rs:27` (default no-op)
- `crates/juncture-core/src/state/channel.rs` (channel implementations)

**Action**: Implement `finish_field()` in `LastValueAfterFinishChannel` and `NamedBarrierValueAfterFinishChannel` types.

---

### [B-004] Missing error handler integration
**Design**: Section 11.5 (design/03-pregel-engine.md:1866-1942) specifies two-phase error recovery with `schedule_error_handlers()` and error handler node execution.

**Actual**: Code: `scheduler.rs:739-778` implements `schedule_error_handlers()` but returns empty Vec. Comment acknowledges missing TaskOutput error tracking (line 740). No integration point in `after_tick()` or `runner.rs` to call this function.

**Risk**: Moderate. Graph execution cannot recover from node failures via error handler nodes.

**Affected Files**:
- `crates/juncture-core/src/pregel/scheduler.rs:739-778` (stub implementation)
- Missing: integration in `loop_.rs:after_tick()` or `runner.rs`

**Action**: Extend `TaskOutput` to include error information, then implement error handler scheduling in `after_tick()` Phase 1 (scan pending_writes for ERROR_SOURCE_NODE markers).

---

### [B-005] Missing consume_triggered_channels() integration
**Design**: Section 5.4 (design/03-pregel-engine.md:872-893) specifies calling `consume_triggered_channels()` after `apply_writes()` and before `reset_ephemeral()`.

**Actual**: Code: `scheduler.rs:702-708` defines `consume_triggered_channels()` as a no-op stub. Comment says work is integrated into `reset_ephemeral()`. However, `after_tick()` (loop_.rs:553) calls `reset_ephemeral()` directly without identifying which channels were triggered.

**Risk**: Minor. Current implementation works because `reset_ephemeral()` clears all ephemeral fields. The optimization (only consume triggered channels) is not implemented.

**Affected Files**:
- `crates/juncture-core/src/pregel/scheduler.rs:702-708` (stub)
- `crates/juncture-core/src/pregel/loop_.rs:553` (calls reset_ephemeral directly)

**Action**: Either implement triggered channel tracking and selective consume, or update design doc to reflect that all ephemeral fields are reset each superstep (current behavior).

---

### [B-006] Missing TriggerToNodes usage in scheduling
**Design**: Section 6.1 (design/03-pregel-engine.md:936-980) specifies `TriggerToNodes` for O(triggered_nodes) scheduling optimization.

**Actual**: Code: `scheduler.rs:532-583` implements `TriggerToNodes` with `from_trigger_table()` and `triggered_nodes()` methods. However, `compute_next_tasks()` (lines 334-406) doesn't use it - it iterates through all completed tasks and checks trigger table directly.

**Risk**: Minor for small graphs. Performance issue for large graphs with many nodes and few triggered nodes per superstep.

**Affected Files**:
- `crates/juncture-core/src/pregel/scheduler.rs:532-583` (unused optimization)
- `crates/juncture-core/src/pregel/scheduler.rs:334-406` (compute_next_tasks doesn't use it)

**Action**: Integrate `TriggerToNodes` into `compute_next_tasks()` to achieve O(triggered_nodes) scheduling instead of O(nodes) scanning.

---

## C Findings (Minor - Naming/Docs)

### [C-001] LoopStatus variants carry data vs unit variants
**Design**: Section 2.1 (design/03-pregel-engine.md:264-287) shows `InterruptBefore` and `InterruptAfter` as unit variants. Implementation note D-03-3 states actual implementation carries `Vec<InterruptSignal>`.

**Actual**: Code: `types.rs` confirms `InterruptBefore(Vec<InterruptSignal>)` and `InterruptAfter(Vec<InterruptSignal>)` carry signal data.

**Risk**: None. This is an enhancement over the design.

**Affected Files**: `crates/juncture-core/src/pregel/types.rs`

**Action**: Update design doc to reflect that interrupt status variants carry interrupt signal data.

---

### [C-002] TaskOutput includes trigger field
**Design**: Section 2.1 (design/03-pregel-engine.md:294-300) shows `TaskOutput` without `trigger` field. Implementation note D-03-4 states actual implementation adds `trigger: TaskTrigger` field.

**Actual**: Code: `types.rs` confirms `TaskOutput` has `trigger: TaskTrigger` field for merge ordering.

**Risk**: None. This is required for deterministic merge semantics.

**Affected Files**: `crates/juncture-core/src/pregel/types.rs`

**Action**: Update design doc to include `trigger: TaskTrigger` field in `TaskOutput` struct.

---

### [C-003] BudgetTracker uses AtomicU64 with micros-USD scaling
**Design**: Section 8.1 (design/03-pregel-engine.md:1164-1175) shows `cost_usd: AtomicF64`. Implementation note D-03-5 states actual implementation uses `AtomicU64` with micros-USD scaling to avoid `atomic_float` dependency.

**Actual**: Code: `budget.rs:129` confirms `cost_usd_micros: AtomicU64` with scaling by 1M.

**Risk**: None. This is a valid optimization.

**Affected Files**: `crates/juncture-core/src/pregel/budget.rs:129`

**Action**: Update design doc to reflect micros-USD storage approach.

---

### [C-004] NodeTimeoutError has 4 variants instead of 2
**Design**: Section 11.2 (design/03-pregel-engine.md:1736-1756) shows 2 variants. Implementation note D-03-7 states actual implementation has 4 variants.

**Actual**: Code: `error.rs:125-156` confirms 4 variants: `Timeout`, `RunTimeout`, `IdleTimeout`, `DeadlineExceeded`.

**Risk**: None. More specific timeout types are beneficial.

**Affected Files**: `crates/juncture-core/src/error.rs:125-156`

**Action**: Update design doc to reflect all 4 timeout variants.

---

### [C-005] SyncAsyncFuture.result() returns Result
**Design**: Section 13.2 (design/03-pregel-engine.md:2025-2038) shows `result()` returning `T`. Implementation note D-03-8 states it should return `Result<T, JunctureError>`.

**Actual**: `SyncAsyncFuture` is not implemented yet, so this is just a design doc note.

**Risk**: N/A (not implemented).

**Action**: When implementing `SyncAsyncFuture`, ensure `result()` returns `Result<T, JunctureError>` per the implementation note.

---

## Verified Items

### Correctly Implemented

1. **PregelLoop main structure** (`loop_.rs:117-172`)
   - All required fields present: state, nodes, trigger_table, field_versions, versions_seen, step, status, pending_tasks
   - Matches design Section 2.1

2. **FieldVersionTracker with global max version** (`scheduler.rs:21-186`)
   - Correctly implements global maximum version increment strategy
   - Matches design Section 2.2 and LangGraph's `GetNextVersion` algorithm

3. **VersionsSeen tracking** (`scheduler.rs:189-297`)
   - Correctly tracks per-node field version consumption
   - `should_activate()` and `mark_consumed()` methods match design

4. **Superstep parallel execution** (`runner.rs:78-241`)
   - Correctly uses `tokio::spawn` + `JoinSet` for concurrency
   - `Semaphore` bounded concurrency with `max_parallel_tasks`
   - `CancellationToken` propagation with `select! biased`
   - Matches design Section 4.1

5. **put_writes incremental persistence** (`runner.rs:211-218`)
   - Correctly persists each task's writes immediately after completion
   - Matches design Section 1.5 put_writes timing

6. **apply_writes with path-based sorting** (`scheduler.rs:471-516`)
   - Correctly sorts PULL tasks by node name, PUSH tasks by send index
   - Provides deterministic merge order matching LangGraph semantics
   - Matches design Section 5.1

7. **Replace reducer conflict detection** (`scheduler.rs:612-639`)
   - Correctly detects multiple writers to replace fields in single superstep
   - Returns `JunctureError::MultipleWriters` on conflict
   - Matches design Section 5.2

8. **compute_next_tasks scheduling logic** (`scheduler.rs:334-406`)
   - Correctly prioritizes Command.goto over external edges
   - Handles Fixed and Conditional edges
   - Processes Send targets with state overrides
   - Matches design Section 6.1

9. **Budget tracking** (`budget.rs:120-344`)
   - Correctly implements token, cost, duration, and step tracking
   - Atomic operations for thread-safe updates
   - `check()` method returns `BudgetExceededReason`
   - Matches design Section 8.1

10. **Cancellation propagation** (`runner.rs:147-155, loop_.rs:361-364`)
    - Correctly checks `cancellation_token.is_cancelled()` in tick()
    - Correctly uses `tokio::select! biased` in task execution
    - Matches design Section 7

11. **Interrupt handling** (`loop_.rs:391-425, 663-715`)
    - Correctly checks `interrupt_before` before executing superstep
    - Correctly checks `interrupt_after` after computing next tasks
    - Uses `should_interrupt()` helper function
    - Matches design Section 3.2 tick/after_tick flow

12. **finish_all_channels call** (`loop_.rs:718-723`)
    - Correctly calls `finish_all_channels()` when `pending_tasks.is_empty()`
    - Matches design Section 5.5

13. **Stream event emission** (`loop_.rs:556-604`)
    - Correctly emits TaskStart, TaskEnd, Updates, Values events
    - Emits DebugEvent::SuperstepStart/End with timing
    - Matches design expectations

14. **RunControl graceful shutdown** (`loop_.rs:52-111, 379-382`)
    - Correctly implements `RunControl` with `drain_requested` flag
    - Checks drain in `tick()` before computing next tasks
    - Matches design Section 11.4

15. **Durability modes** (`context.rs:17-27`)
    - Defines Sync, Async, Exit modes
    - Matches design Section 11.3

16. **PendingTask structure** (`types.rs`)
    - Correctly has `id`, `node_name`, `trigger`, `state_override` fields
    - `TaskTrigger::Pull` and `TaskTrigger::Push { index }` variants
    - Matches design Section 2.1

17. **SuperstepResult structure** (`types.rs`)
    - Correctly has `task_outputs: Vec<TaskOutput>` field
    - Matches design Section 2.1

18. **CompiledGraph.invoke/entry points** (`compiled.rs:107-171`)
    - Correctly creates PregelLoop and executes tick/superstep/after_tick loop
    - Matches design Section 3.1

19. **Error types and is_* methods** (`error.rs:294-450`)
    - Comprehensive `JunctureError` variants
    - Type-checking methods (`is_graph()`, `is_execution()`, etc.)
    - Matches design Section 10.1b

20. **RunnableConfig fields** (`config.rs:18-77`)
    - All required fields present including `interrupt_before`, `interrupt_after`, `budget`, `durability`
    - Matches design expectations

---

## Out-of-Scope (Not Reviewed This Run)

The following design sections were not verified in this review:

- **Section 13**: SyncAsyncFuture (not implemented, marked as finding B-002)
- **Section 14**: Previous Result Injection (requires checkpoint integration, Phase 6)
- **Section 11.5**: Error handler node execution (partial, marked as finding B-004)
- **Checkpoint persistence** (Phase 6 implementation)
- **Subgraph execution** (Phase 7 implementation)
- **Remote graph execution** (Phase 8 implementation)

---

## Recommendations

### High Priority
1. **Implement RetryPolicy execution logic** (A-001): Add `execute_with_retry()` with exponential backoff and jitter
2. **Implement TimeoutPolicy execution logic** (A-002): Add `execute_with_timeout()` using `tokio::time::timeout()`
3. **Clarify ExecutionContext design** (A-003): Either update code to use it or update design doc to reflect flattened approach

### Medium Priority
4. **Implement SyncAsyncFuture** (B-002): Required for functional API (@task/@entrypoint)
5. **Integrate error handlers** (B-004): Extend TaskOutput to track errors, wire up schedule_error_handlers
6. **Implement channel finish() semantics** (B-003): Add finish_field() to channel types

### Low Priority
7. **Integrate TriggerToNodes optimization** (B-006): Use in compute_next_tasks for large graph performance
8. **Update design docs** (C-001 through C-005): Align documentation with actual implementation enhancements

---

## Conclusion

The Pregel Engine implementation demonstrates **solid conformance** with the core design requirements. The main execution loop, version tracking, task scheduling, and parallel execution are all correctly implemented following LangGraph semantics.

The primary gaps are in **advanced resilience features** (retry, timeout) and **error recovery**, which are structured but not fully implemented. These should be prioritized for production readiness.

The design doc deviations noted are mostly enhancements (LoopStatus carrying data, TaskOutput including trigger) or valid optimizations (AtomicU64 for cost tracking), and the design doc should be updated to reflect these.

**Verdict**: Acceptable for current development phase. Targeted feature completion needed for production readiness.
