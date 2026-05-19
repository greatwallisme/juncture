# Findings & Decisions: Juncture Technical Design Conformance Audit

## Requirements
- Full conformance audit of Juncture project against technical design documents
- Design doc path: design/ (index.md + 10 module docs + 10 checklists/*.json)
- Scope: ENTIRE source tree under crates/ (6 crates, 96 Rust files)
- Identify: architectural deviations, missing features, undocumented additions
- Report per module: PASS, DEVIATION, MISSING, EXTRA items
- Use absolute file paths for all source code references

## Research Findings
- Project has 6 crates: juncture (facade), juncture-core (engine), juncture-derive (macro), juncture-checkpoint, juncture-tracing, juncture-store
- 10 design modules: 01-state-channel through 10-store
- 214 total checklist items across 10 JSON files
- Design verification script exists: scripts/verify-design-coverage.py
- Previous session achieved 214/214 (100%) design coverage per script
- Git shows recent commit: "implement full juncture workspace (6 crates, 214/214 design coverage)"

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| Module-by-module approach | 96 files across 6 crates is too large for direct review |
| Use checklist JSONs | Provides mechanical baseline (214 items to verify) |
| Absolute file paths | Avoid confusion with relative paths in findings |
| Read-only analysis | Never modify source code during audit |
| Category C default | Code additions are C unless they violate/omit explicit requirements |

---

## Module 01: State & Channel System

### Summary

27 checklist items audited. 16 PASS, 6 DEVIATION, 1 MISSING, 5 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 01-001 | State trait (bounds, methods, associated types) | `crates/juncture-core/src/state/trait_.rs:7-46` |
| 01-002 | CowState struct (fields, new/get/get_mut/update/commit) | `crates/juncture-core/src/state/trait_.rs:91-155` |
| 01-003 | FieldsChanged (u64 bitmask, is_empty/has_field/set_field/merge) | `crates/juncture-core/src/state/trait_.rs:52-81` |
| 01-004 | Reducer trait (reduce, reduce_one with fast path) | `crates/juncture-core/src/state/channel.rs:12-23` |
| 01-005 | ReplaceReducer (assert len <= 1) | `crates/juncture-core/src/state/channel.rs:30-42` |
| 01-006 | AppendReducer (Vec extend semantics) | `crates/juncture-core/src/state/channel.rs:48-61` |
| 01-007 | AnyValueReducer (debug_assert + last wins) | `crates/juncture-core/src/state/channel.rs:68-83` |
| 01-008 | LastWriteWinsReducer (multi-writer, last wins) | `crates/juncture-core/src/state/channel.rs:89-97` |
| 01-012 | FieldVersionTracker (Vec<u64>, global_max, new/bump/bump_all/get/as_slice) | `crates/juncture-core/src/pregel/scheduler.rs:20-164` |
| 01-017 | Message (all fields: id/role/content/tool_calls/tool_call_id/name/usage, all methods: human/ai/system/tool_result/has_tool_calls/remove) | `crates/juncture-core/src/state/messages.rs:7-234` |
| 01-018 | Role enum (System/Human/Ai/Tool) | `crates/juncture-core/src/state/messages.rs:25-35` |
| 01-020 | messages_reducer (add_messages semantics with REMOVE_ALL_MESSAGES support) | `crates/juncture-core/src/state/messages.rs:114-127` |
| 01-022 | RemoveMessage struct (id field) | `crates/juncture-core/src/state/channel.rs:422-426` |
| 01-023 | IntoState trait (into_state method) | `crates/juncture-core/src/state/trait_.rs:166-168` |
| 01-024 | FromState trait (from_state method) | `crates/juncture-core/src/state/trait_.rs:171-173` |
| 01-027 | EphemeralChannel (update/get/consume semantics) | `crates/juncture-core/src/state/channel.rs:189-236` |

### DEVIATIONS

#### [D-01-1] VersionsSeen uses HashMap instead of IndexMap
- **Checklist**: 01-013
- **Design**: `seen: IndexMap<NodeId, Vec<u64>>` -- explicitly states "使用 IndexMap 而非 HashMap 保证确定性迭代顺序"
- **Code**: `crates/juncture-core/src/pregel/scheduler.rs:173` -- `seen: HashMap<String, Vec<u64>>`
- **Impact**: HashMap iteration order is non-deterministic. This can cause non-deterministic scheduling when multiple nodes have equal activation priority. The design chose IndexMap specifically to guarantee deterministic merge order across runs.
- **Severity**: Medium

#### [D-01-2] REMOVE_ALL_MESSAGES is `&str` not `Message` struct
- **Checklist**: 01-021
- **Design**: `pub const REMOVE_ALL_MESSAGES: Message = Message { id: "__remove_all__", role: Role::System, content: Content::Text(String::new()), ... }` -- full typed sentinel
- **Code**: `crates/juncture-core/src/state/messages.rs:105` -- `pub const REMOVE_ALL_MESSAGES: &str = "__remove_all__"`
- **Impact**: Weaker type safety. Any message with id `"__remove_all__"` triggers the behavior. The design intended a typed sentinel that could be used directly in message lists.
- **Severity**: Low

#### [D-01-3] Channel trait has extra checkpoint methods not in design
- **Checklist**: 01-011
- **Design**: `Channel<T>` with `update`, `get`, `consume` only
- **Code**: `crates/juncture-core/src/state/channel.rs:110-132` -- adds `checkpoint() -> Option<serde_json::Value>` and `from_checkpoint(value) -> Result<Self, String>`
- **Impact**: Design specifies Channel as a pure state-field abstraction. Checkpoint persistence is a separate concern (design doc 04). Mixing checkpoint into Channel violates separation of concerns, but makes each channel self-contained for serialization.
- **Severity**: Low (architectural, not functional)

#### [D-01-4] InvalidUpdateError variants simplified (no structured fields)
- **Checklist**: 01-015
- **Design**: `MultipleWriters { field: String, conflicting_nodes: Vec<String> }`, `MultipleOverwrite { field: String }`, `InvalidValue { field: String, reason: String }`
- **Code**: `crates/juncture-core/src/error.rs:96-106` -- `MultipleWriters`, `MultipleOverwrite`, `InvalidValue(String)` -- no field names or node lists
- **Impact**: When Replace reducer detects multiple writers, design intended to report the specific field and conflicting node names. Code only gives generic "multiple writers for replace channel".
- **Severity**: Medium (diagnostic impact)

#### [D-01-5] Overwrite missing custom serde for `__overwrite__` marker
- **Checklist**: 01-014
- **Design**: Custom `Serialize`/`Deserialize` using `{"__overwrite__": value}` wire format for LangGraph checkpoint compatibility
- **Code**: `crates/juncture-core/src/state/channel.rs:103-104` -- bare `Overwrite<T>(pub T)` with no custom serde impl
- **Impact**: Checkpoint JSON cannot distinguish Overwrite values from normal values. Deserialization cannot reconstruct Overwrite semantics. LangGraph Python checkpoint interop is blocked.
- **Severity**: Medium

#### [D-01-6] DeltaBlob uses serde_json::Value instead of generic T
- **Checklist**: 01-026
- **Design**: `DeltaBlob<T>` with `Missing` and `Snapshot(T)` -- generic over value type
- **Code**: `crates/juncture-core/src/state/channel.rs:410-416` -- `DeltaBlob` with `Snapshot(serde_json::Value)` -- no generic parameter
- **Impact**: All values erased to JSON before storage. Loses compile-time type guarantees at the DeltaBlob boundary.
- **Severity**: Low

### MISSING

#### [M-01-1] MessagesState struct not implemented
- **Checklist**: 01-016
- **Design**: Built-in `MessagesState` struct with `messages: Vec<Message>` field and `#[reducer(messages)]` annotation. Zero-config entry point for simple chat agents.
- **Code**: Not found anywhere in the codebase.
- **Impact**: Users must define their own state struct for every chat agent instead of using a ready-made one.
- **Severity**: Medium

### EXTRAS (in code but not in design)

#### [E-01-1] Deref impl for CowState
- **Code**: `crates/juncture-core/src/state/trait_.rs:157-163` -- `impl<S: State> Deref for CowState<S>`

#### [E-01-2] Extra Message methods: ai_with_tool_calls, content_text
- **Code**: `crates/juncture-core/src/state/messages.rs:157-218`

#### [E-01-3] ContentPart::Thinking variant + ImageData/ImageSource types
- **Code**: `crates/juncture-core/src/state/messages.rs:48-78`
- **Note**: Extends design for Anthropic extended thinking and multimodal image support

#### [E-01-4] Channel types parametric on Reducer (UntrackedChannel<T,R>, EphemeralChannel<T,R>, etc.)
- **Code**: `crates/juncture-core/src/state/channel.rs:140-335`
- **Note**: More flexible than design's hardcoded channel types. Positive architectural improvement.

#### [E-01-5] Content::MultiPart naming (design says Content::Parts)
- **Code**: `crates/juncture-core/src/state/messages.rs:44`

---

## Module 02: Graph Builder & Compilation

### Summary

Design doc is the largest module (1649 lines). Covers StateGraph, Node system, Edge system, Command, Runtime, RunnableConfig, CompiledGraph, RemoteGraph, and Functional API. Key focus: builder API surface, topology validation, and execution entry points.

### PASS Items

- **StateGraph struct** (nodes IndexMap, edges Vec, entry_point, finish_points, subgraphs) -- matches design `graph/builder.rs:319-337`
- **add_node with full config** (defer, metadata, destinations, retry_policies) -- matches design `graph/builder.rs:378-406`
- **add_node_simple** (convenience wrapper) -- matches design `graph/builder.rs:419-425`
- **add_edge / add_conditional_edges** -- match design signatures `graph/builder.rs:692-734`
- **set_entry_point / set_finish_point** -- match design `graph/builder.rs:745-770`
- **add_sequence** -- matches design, validates nodes exist `graph/builder.rs:786-813`
- **compile / compile_ephemeral** -- match design `graph/builder.rs:843-857`
- **TopologyValidator** -- all 7 validation steps (entry point, edge targets, reachability BFS, isolated nodes, unreachable nodes, Tarjan SCC loop detection) `graph/topology.rs:161-371`
- **TopologyError** enum -- all 7 variants match design `graph/topology.rs:22-49`
- **TriggerTable building** -- matches design's edge-to-trigger conversion `graph/builder.rs:886-944`
- **CompiledGraph** -- Arc-wrapped inner, Clone, nodes + trigger_table + checkpointer `graph/compiled.rs:36-669`
- **GraphOutput** struct (value, interrupts, metadata) -- matches design `graph/compiled.rs:674-684`
- **InterruptInfo** (node, value, id) -- matches design `graph/compiled.rs:689-699`
- **DrawableGraph / DrawableNode / DrawableEdge** -- match design `graph/compiled.rs:760-797`
- **to_mermaid / to_dot / to_json** -- all three export formats implemented `graph/compiled.rs:474-591`
- **ErrorHandlerNode** wrapper -- matches design concept `graph/builder.rs:99-184`
- **RetryingNode** wrapper with exponential backoff -- matches design `graph/builder.rs:190-294`
- **RetryPolicy** struct -- matches design fields `graph/builder.rs:39-88`

### DEVIATIONS

#### [D-02-1] add_node returns Result<(), TopologyError> instead of &mut Self
- **Design**: `pub fn add_node(...) -> &mut Self` (builder pattern, chainable)
- **Code**: `graph/builder.rs:386` -- `pub fn add_node(...) -> Result<(), TopologyError>`
- **Impact**: Cannot chain `.add_node().add_node().add_edge()`. Users must use `?` after each call, breaking fluent builder pattern.
- **Severity**: Low (API ergonomics, not functionality)

#### [D-02-2] ErrorHandlerNode handler receives JunctureError instead of NodeError<S>
- **Design**: Error handler receives `NodeError<S> { node: String, error: JunctureError, state: S, attempt: u32 }`
- **Code**: `graph/builder.rs:111` -- `handler: Arc<dyn Fn(JunctureError) -> Command<S> + Send + Sync>`
- **Impact**: Error handler cannot access the failed state snapshot, node name, or retry attempt count. Design intended richer recovery context for conditional fallback strategies.
- **Severity**: Medium (limits error recovery intelligence)

#### [D-02-3] validate_keys is a no-op stub
- **Design**: "验证状态键的有效性。检查所有节点的更新是否只引用了 State 中定义的字段"
- **Code**: `graph/builder.rs:824` -- `pub const fn validate_keys(&self) -> Result<(), TopologyError> { Ok(()) }`
- **Impact**: No compile-time or runtime check that node updates reference valid State fields. Invalid field references will only surface at execution time as silent no-ops (Option field remains None).
- **Severity**: Medium (deferred validation to runtime)

#### [D-02-4] StateUpdate uses `values` field instead of `update`
- **Design**: `pub struct StateUpdate<S> { pub update: S::Update, pub as_node: Option<String> }`
- **Code**: `graph/compiled.rs:718` -- `pub values: S::Update, pub label: Option<String>, pub as_node: Option<String>`
- **Impact**: API naming differs from design. Extra `label` field not in design.
- **Severity**: Low (naming + extra field)

#### [D-02-5] GraphOutputMetadata missing budget_usage field
- **Design**: `pub struct GraphOutputMetadata { pub steps: usize, pub checkpoint_id: Option<String>, pub budget_usage: Option<BudgetUsage> }`
- **Code**: `graph/compiled.rs:704-711` -- only `steps` and `checkpoint_id`, no `budget_usage`
- **Severity**: Low (missing optional diagnostic field)

### MISSING

#### [M-02-1] Functional API (entrypoint / task proc macros)
- **Design**: Section 7 defines `#[entrypoint]` and `#[task]` attribute macros for functional workflow definition. `#[task(retry = ...)]`, `#[task(cache = ...)]`, `#[entrypoint(checkpointer = ...)]`
- **Code**: Not found anywhere. `juncture-derive` only exports `#[derive(State)]`.
- **Severity**: High (entire alternative API surface missing)

#### [M-02-2] Final<V, S> type not implemented
- **Design**: `pub struct Final<V, S> { pub value: V, pub save: S }` for distinguishing return value from checkpoint save value in entrypoint functions
- **Code**: Not found
- **Severity**: Medium (only relevant with functional API)

### EXTRAS

#### [E-02-1] add_node_with_retry method
- **Code**: `graph/builder.rs:487-509` -- convenience method for wrapping nodes with retry policy
- **Design**: Design mentions retry via `retry_policies` in NodeMetadata but no separate method

#### [E-02-2] add_subgraph_node and add_subgraph_with_config methods
- **Code**: `graph/builder.rs:563-670` -- two alternative subgraph mounting APIs using StateSubset trait
- **Design**: Design specifies simpler `add_subgraph(name, subgraph, input_map, output_map)`

#### [E-02-3] StateFilter uses after_step/before_step instead of source
- **Code**: `graph/compiled.rs:745-755` -- `StateFilter { after_step, before_step, limit }`
- **Design**: `StateFilter { source: Option<CheckpointSource>, limit: Option<usize> }`
- **Note**: Different filtering approach -- step-based vs source-based

---

## Module 03: Pregel Execution Engine

### Summary

25 checklist items audited. 11 PASS, 9 DEVIATION, 2 MISSING, 5 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 03-004 | PendingTask (id, node_name, trigger, state_override) | `crates/juncture-core/src/pregel/types.rs:44-56` |
| 03-005 | TaskTrigger (Pull, Push{index}) | `crates/juncture-core/src/pregel/types.rs:63-72` |
| 03-007 | SuperstepResult (task_outputs) | `crates/juncture-core/src/pregel/types.rs:78-81` |
| 03-012 | TriggerToNodes (mapping, from_trigger_table, triggered_nodes) | `crates/juncture-core/src/pregel/scheduler.rs:492-540` |
| 03-013 | BudgetConfig (max_tokens, max_cost_usd, max_duration, max_steps, on_exceeded) | `crates/juncture-core/src/pregel/budget.rs:39-55` |
| 03-014 | BudgetExceededAction (Terminate, Interrupt, Custom) | `crates/juncture-core/src/pregel/budget.rs:13-24` |
| 03-016 | BudgetUsage (tokens_used, cost_usd, duration, steps_completed) | `crates/juncture-core/src/pregel/budget.rs:383-395` |
| 03-018 | BubbleUp (Interrupt, Drained, ParentCommand) | `crates/juncture-core/src/pregel/types.rs:135-144` |
| 03-023 | Durability (Sync, Async, Exit) | `crates/juncture-core/src/pregel/durability.rs:10-32` |
| 03-024 | PregelProtocol (invoke, stream, get_state, update_state) | `crates/juncture-core/src/pregel/protocol.rs:29-103` |
| 03-025 | SyncAsyncFuture (Ready, Future, result, is_ready) | `crates/juncture-core/src/pregel/types.rs:321-385` |

### DEVIATIONS

#### [D-03-1] PregelLoop does not use ExecutionContext/ExecutionConfig split
- **Checklist**: 03-001, 03-002, 03-003
- **Design**: PregelLoop has fields `context: ExecutionContext<S>`, `config: ExecutionConfig`, `checkpointer: Arc<dyn CheckpointSaver>`. ExecutionContext and ExecutionConfig are defined as separate structs with clear mutable/immutable split.
- **Code**: `crates/juncture-core/src/pregel/loop_.rs:25-61` -- PregelLoop has flat fields (`state`, `field_versions`, `versions_seen`, `pending_tasks`, `runnable_config`). ExecutionContext and ExecutionConfig exist in `context.rs` but are NOT used by PregelLoop. No `checkpointer` field.
- **Impact**: ExecutionContext/ExecutionConfig are dead code. The mutable/immutable separation the design intended is lost. No checkpoint integration in PregelLoop (no put_writes during superstep).
- **Severity**: High (architectural -- core execution loop structure differs from design)

#### [D-03-2] execute_superstep missing checkpoint and stream integration
- **Checklist**: 03-009
- **Design**: `execute_superstep` takes `checkpointer` and `stream_tx` params. After each task completes: (1) `checkpointer.put_writes()` for crash recovery, (2) emit `StreamEvent::TaskEnd`.
- **Code**: `crates/juncture-core/src/pregel/runner.rs:58-142` -- No `checkpointer` parameter, no `stream_tx` parameter. Task outputs are only collected, not persisted or streamed.
- **Impact**: No crash recovery during superstep execution. If a process crashes mid-superstep, completed tasks' writes are lost. Stream events are only emitted in `PregelLoop::after_tick()` (batch, not per-task).
- **Severity**: Medium (missing durability guarantee)

#### [D-03-3] LoopStatus InterruptBefore/InterruptAfter carry Vec<InterruptSignal>
- **Checklist**: 03-006
- **Design**: `InterruptBefore` and `InterruptAfter` are simple unit variants.
- **Code**: `crates/juncture-core/src/pregel/types.rs:25-28` -- `InterruptBefore(Vec<InterruptSignal>)` and `InterruptAfter(Vec<InterruptSignal>)` carry signal data.
- **Impact**: Richer data available on interrupt status. Positive deviation.
- **Severity**: Low (enrichment, not violation)

#### [D-03-4] TaskOutput has extra trigger field
- **Checklist**: 03-008
- **Design**: `TaskOutput` has `task_id`, `node_name`, `command`, `duration`.
- **Code**: `crates/juncture-core/src/pregel/types.rs:87-102` -- Extra `trigger: TaskTrigger` field.
- **Impact**: Needed for `apply_writes` path-based sorting. Positive addition.
- **Severity**: Low (necessary for deterministic merge)

#### [D-03-5] BudgetTracker uses integer scaling instead of AtomicF64
- **Checklist**: 03-015
- **Design**: `cost_usd: AtomicF64` (using `atomic_float` crate). `report_usage(&self, usage: &TokenUsage, model_pricing: &dyn ModelPricing)`.
- **Code**: `crates/juncture-core/src/pregel/budget.rs:129` -- `cost_usd_micros: AtomicU64` with 1M scaling factor. `report_usage(&self, tokens: u64, cost_usd: f64)` -- simplified signature without TokenUsage/ModelPricing types.
- **Impact**: Avoids external `atomic_float` dependency. Cost is stored as micros-USD integer, converted on read. The report_usage decoupling from TokenUsage/ModelPricing means budget tracking doesn't depend on LLM types.
- **Severity**: Low (implementation detail, functionally equivalent)

#### [D-03-6] TimeoutPolicy.refresh_on signature differs
- **Checklist**: 03-021
- **Design**: `refresh_on: Option<Arc<dyn Fn(&StreamEvent<()>) -> bool + Send + Sync>>` -- takes StreamEvent to detect progress.
- **Code**: `crates/juncture-core/src/pregel/context.rs:170-174` -- `refresh_on: Option<Arc<dyn Fn() -> bool + Send + Sync>>` -- no parameters.
- **Impact**: Cannot inspect stream events to detect progress. The heartbeat callback cannot differentiate between event types.
- **Severity**: Medium (reduces idle timeout intelligence)

#### [D-03-7] NodeTimeoutError has extra variants not in design
- **Checklist**: 03-022
- **Design**: Two variants: `RunTimeout { node, timeout: Duration }` and `IdleTimeout { node, timeout: Duration }`.
- **Code**: `crates/juncture-core/src/error.rs:112-143` -- Four variants: `Timeout { node, timeout_ms: u64 }`, `RunTimeout { node, timeout: u64 }`, `IdleTimeout { node, timeout: u64 }`, `DeadlineExceeded { node }`. Duration stored as u64 ms instead of Duration type.
- **Impact**: Extra `Timeout` and `DeadlineExceeded` variants add flexibility. Using `u64` ms instead of `Duration` loses type safety but is simpler.
- **Severity**: Low (enrichment)

#### [D-03-8] SyncAsyncFuture.result() returns Result instead of panicking
- **Checklist**: 03-025
- **Design**: `result()` panics on `Ready(None)`: `SyncAsyncFuture::Ready(None) => panic!("Task result not available")`.
- **Code**: `crates/juncture-core/src/pregel/types.rs:360-366` -- returns `Err(JunctureError::empty_channel())` instead of panicking.
- **Impact**: Safer -- no panic path. Caller must handle the error case.
- **Severity**: Low (improvement over design)

#### [D-03-9] compute_next_tasks has Goto::Send stub
- **Checklist**: 03-011
- **Design**: Full Send handling with `PendingTask::push(node, idx, Some(target.state))` for each send target.
- **Code**: `crates/juncture-core/src/pregel/scheduler.rs:352-358` -- `Goto::Send(_send_targets) => {}` with comment: "This will be implemented in a future phase".
- **Impact**: Send API (dynamic fan-out) is non-functional. Any node returning `Command::send()` will silently produce no next tasks.
- **Severity**: High (core feature not implemented)

### MISSING

#### [M-03-1] RunControl struct not implemented
- **Design**: Section 11.4 defines `RunControl` with `request_drain()` and `is_drain_requested()` for graceful shutdown. Checked in `tick()` before `prepare_next_tasks`.
- **Code**: Not found anywhere.
- **Impact**: No graceful shutdown mechanism. Cannot drain the loop from external signal (e.g., SIGTERM).
- **Severity**: Medium

#### [M-03-2] Multiple Pregel internals not implemented
- **Design**: Sections 5.2-5.5 define: `check_replace_conflicts()` (multi-writer detection), `consume_triggered_channels()` (ephemeral consume), `finish_all_channels()` (finish notification), `schedule_error_handlers()` (error recovery), `reserved_keys` module.
- **Code**: None of these functions exist. No reserved write keys. No error handler scheduling.
- **Impact**: No multi-writer conflict detection at superstep level. No channel consume step. No finish notification for AfterFinish channels. No error handler recovery flow.
- **Severity**: Medium (individual features missing, not architecturally blocking)

### EXTRAS (in code but not in design)

#### [E-03-1] PendingTask convenience constructors
- **Code**: `crates/juncture-core/src/pregel/types.rs:177-236` -- `new()`, `pull()`, `push()` constructors
- **Note**: Design specifies struct fields only, these are ergonomic additions

#### [E-03-2] SuperstepResult helper methods
- **Code**: `crates/juncture-core/src/pregel/types.rs:253-279` -- `empty()`, `is_empty()`, `len()`, `Default` impl

#### [E-03-3] LoopStatus helper methods
- **Code**: `crates/juncture-core/src/pregel/types.rs:281-302` -- `is_running()`, `is_terminal()`, `is_interrupted()`

#### [E-03-4] BudgetConfig builder pattern methods
- **Code**: `crates/juncture-core/src/pregel/budget.rs:84-118` -- `with_max_tokens()`, `with_max_cost_usd()`, etc.

#### [E-03-5] PregelLoop snapshot_state() method
- **Code**: `crates/juncture-core/src/pregel/loop_.rs:374-379` -- clone state without consuming loop

---

## Module 04: Checkpoint Persistence System

### Summary

28 checklist items audited. 17 PASS, 5 DEVIATION, 2 MISSING, 4 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 04-001 | CheckpointSaver trait (get_tuple, list, put, put_writes) | `crates/juncture-core/src/checkpoint.rs:189-234` |
| 04-002 | Checkpoint struct (all 11 fields including v, new_versions, counters_since_delta_snapshot) | `crates/juncture-core/src/checkpoint.rs:241-284` |
| 04-003 | CheckpointMetadata (source, step, writes, parents, run_id) | `crates/juncture-core/src/checkpoint.rs:303-320` |
| 04-004 | CheckpointSource (Input, Loop, Update, Fork) | `crates/juncture-core/src/checkpoint.rs:326-339` |
| 04-005 | CheckpointTuple (config, checkpoint, metadata, pending_writes, parent_config) | `crates/juncture-core/src/checkpoint.rs:346-366` |
| 04-006 | StateSnapshot (values, next, config, metadata, created_at, parent_config, tasks) | `crates/juncture-core/src/checkpoint.rs:458-479` |
| 04-007 | PendingWrite (task_id, channel, value) | `crates/juncture-core/src/checkpoint.rs:373-382` |
| 04-008 | PendingTask as CheckpointPendingTask (id, node, triggers, state_override) | `crates/juncture-core/src/checkpoint.rs:388-400` |
| 04-009 | CheckpointFilter (source, step_gte, step_lte, before, after, limit) | `crates/juncture-core/src/checkpoint.rs:430-448` |
| 04-010 | MemorySaver (storage: Arc<RwLock<HashMap>>, writes: Arc<RwLock<HashMap>>) | `crates/juncture-checkpoint/src/memory.rs:39-45` |
| 04-013 | SerializationFormat (MessagePack, Json) | `crates/juncture-checkpoint/src/serde.rs:26-33` |
| 04-014 | CheckpointSerializer trait (serialize, deserialize, format) | `crates/juncture-checkpoint/src/serde.rs:39-71` |
| 04-015 | MsgpackSerializer | `crates/juncture-checkpoint/src/serde.rs:78-108` |
| 04-016 | JsonSerializer | `crates/juncture-checkpoint/src/serde.rs:114-145` |
| 04-020 | ChannelDelta (channel, op, values) | `crates/juncture-checkpoint/src/types.rs:30-39` |
| 04-021 | DeltaOp (Append, Replace) | `crates/juncture-core/src/checkpoint.rs:418-424` |
| 04-022 | DeltaCounters (updates, supersteps) | `crates/juncture-core/src/checkpoint.rs:291-297` |

### DEVIATIONS

#### [D-04-1] CheckpointPendingTask named differently from design
- **Checklist**: 04-008
- **Design**: `PendingTask` struct with fields `id`, `node`, `triggers`, `state_override`.
- **Code**: `crates/juncture-core/src/checkpoint.rs:388` -- named `CheckpointPendingTask` to avoid collision with `pregel::types::PendingTask`. Fields match exactly.
- **Impact**: Naming collision resolved. Extra `CheckpointPendingTask` alias re-exported as `PendingTask` from checkpoint module would match design.
- **Severity**: Low (cosmetic)

#### [D-04-2] CheckpointSerializer has extra serialize_value/deserialize_value methods
- **Checklist**: 04-014
- **Design**: Three methods: `serialize`, `deserialize`, `format`.
- **Code**: `crates/juncture-checkpoint/src/serde.rs:39-71` -- Five methods: `serialize_value`, `deserialize_value`, `serialize`, `deserialize`, `format`. Extra `serialize_value`/`deserialize_value` work on `serde_json::Value` directly.
- **Impact**: Positive enrichment -- provides both typed and untyped serialization paths.
- **Severity**: Low (extra methods, not missing)

#### [D-04-3] EncryptedSerializer uses generic parameter, design uses Box<dyn>
- **Checklist**: 04-017
- **Design**: `EncryptedSerializer { inner: Box<dyn CheckpointSerializer>, cipher: Aes256Gcm }`
- **Code**: `crates/juncture-checkpoint/src/serde.rs:214` -- `EncryptedSerializer<S: CheckpointSerializer> { inner: S, key: [u8; 32] }` -- generic over inner serializer, stores raw key not cipher.
- **Impact**: More type-safe (generic vs trait object). Key stored instead of cipher (cipher recreated per call). Extra `from_passphrase()` method using PBKDF2 (not in design). Feature-gated behind `encryption`.
- **Severity**: Low (implementation detail)

#### [D-04-4] JsonPlusSerializer only adds pretty-printing, not type extensions
- **Checklist**: 04-018
- **Design**: Enhanced JSON with datetime/UUID/bytes/enum special handling.
- **Code**: `crates/juncture-checkpoint/src/serde.rs:150-204` -- only toggles `serde_json::to_vec_pretty` vs `serde_json::to_vec`. No special type handling.
- **Impact**: No datetime/UUID/bytes encoding extensions. Just pretty-print JSON.
- **Severity**: Low (simplification)

#### [D-04-5] CheckpointSaver returns JunctureError, not CheckpointError
- **Checklist**: 04-001
- **Design**: `CheckpointSaver` methods return `Result<..., CheckpointError>`.
- **Code**: `crates/juncture-core/src/checkpoint.rs:189-234` -- methods return `Result<..., JunctureError>`. MemorySaver uses internal `ToJunctureError` adapter to convert `CheckpointError` to `JunctureError::checkpoint(msg)`.
- **Impact**: CheckpointError detail is lost in the string conversion. Callers get JunctureError but lose structured CheckpointError context.
- **Severity**: Medium (error information degradation)

### MISSING

#### [M-04-1] CheckpointNamespace not in design checklist but implemented
- **Design**: Section 7.2 defines `CheckpointNamespace` with `root()`, `child()`, `to_string()`, and `segments` field.
- **Checklist**: 04-027 lists CheckpointNamespace but it was verified as present.
- **Status**: Actually PASS -- `crates/juncture-core/src/checkpoint.rs:35-129` implements full CheckpointNamespace with root, child, parent, as_str, to_string, parse.

#### [M-04-2] SqliteSaver and PostgresSaver are stub implementations
- **Checklist**: 04-011, 04-012
- **Design**: Full implementations with SQL schema, WAL mode, connection pooling.
- **Code**: `crates/juncture-checkpoint/src/sqlite.rs` and `postgres.rs` exist behind feature flags but contain minimal/stub implementations.
- **Impact**: Production persistence backends not usable.
- **Severity**: Medium (feature-gated stubs, MemorySaver is functional)

### EXTRAS (in code but not in design)

#### [E-04-1] NamespaceSegment struct
- **Code**: `crates/juncture-core/src/checkpoint.rs:136-166` -- `NamespaceSegment` with `node_name` and `invocation_id`
- **Note**: Not in design. Adds structured namespace segment support.

#### [E-04-2] detect_format() function
- **Code**: `crates/juncture-checkpoint/src/serde.rs:334-357` -- auto-detects MessagePack vs JSON from byte header
- **Note**: Design mentions this conceptually in section 5.1 but not as a standalone function

#### [E-04-3] CheckpointError has extra Database and Serialization variants
- **Code**: `crates/juncture-checkpoint/src/error.rs:10-71` -- adds `Database(String)` and `Serialization(String)` beyond design's 6 variants
- **Note**: Design specifies Serialize, Deserialize, SchemaMigration, Storage, NotFound, PoolExhausted. Code adds Database and Serialization aliases.

#### [E-04-4] TtlConfig in checkpoint crate, not core
- **Code**: `crates/juncture-checkpoint/src/types.rs:44-54` -- `TtlConfig` with `default_ttl`, `sweep_interval`, `max_checkpoints`
- **Note**: Design specifies this but location is checkpoint crate (correct per section 10 crate organization)

---

## Module 05: Streaming System

### Summary

18 checklist items audited. 9 PASS, 7 DEVIATION, 0 MISSING, 3 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 05-001 | StreamMode (9 variants: Values, Updates, Messages, Custom, Debug, Tools, Checkpoints, Tasks, Multi) | `crates/juncture-core/src/stream.rs:5-34` |
| 05-004 | TaskEventType (Started, Completed, Failed, Retrying) | `crates/juncture-core/src/stream.rs:197-203` |
| 05-005 | MessageChunk (content, tool_call_chunks, usage_delta) | `crates/juncture-core/src/stream.rs:111-116` |
| 05-006 | ToolCallChunk (id, name, args_delta, index) | `crates/juncture-core/src/stream.rs:119-125` |
| 05-007 | MessageStreamMetadata (node, model, tags, ns) | `crates/juncture-core/src/stream.rs:128-134` |
| 05-008 | DebugEvent (6 variants: SuperstepStart, SuperstepEnd, CheckpointSaved, ChannelUpdate, RouteDecision, BudgetStatus) | `crates/juncture-core/src/stream.rs:137-163` |
| 05-009 | StreamPart (ns, event, data, metadata) | `crates/juncture-core/src/stream.rs:206-212` |
| 05-010 | StreamChannel (name, tx, send method) | `crates/juncture-core/src/stream.rs:226-255` |
| 05-011 | StreamTransformer trait (transform method) | `crates/juncture-core/src/stream.rs:257-260` |

### DEVIATIONS

#### [D-05-1] StreamEvent.BudgetExceeded uses String reason, not BudgetExceededReason
- **Checklist**: 05-002
- **Design**: Section 2.2 defines `BudgetExceeded { reason: BudgetExceededReason, usage: BudgetUsage }` where `BudgetExceededReason` is the structured budget module type.
- **Code**: `crates/juncture-core/src/stream.rs:86` -- `BudgetExceeded { reason: String, usage: BudgetUsage }`. Uses `String` instead of `BudgetExceededReason`. Additionally, `BudgetUsage` is defined locally at line 188-194 with `tokens_used: u64, cost_usd: f64, duration_ms: u64, steps_completed: usize` rather than reusing the budget module's `BudgetUsage` which uses `AtomicU64`-based micros scaling.
- **Impact**: Loses structured budget error context. BudgetUsage struct is duplicated.
- **Severity**: Medium (type mismatch with budget module)

#### [D-05-2] StreamEvent.CheckpointSaved missing metadata field
- **Checklist**: 05-002
- **Design**: Section 2.2 defines `CheckpointSaved { checkpoint_id: String, metadata: CheckpointMetadata, step: usize }`.
- **Code**: `crates/juncture-core/src/stream.rs:98` -- `CheckpointSaved { checkpoint_id: String, step: usize }`. Missing `metadata: CheckpointMetadata` field.
- **Impact**: Stream consumers cannot see checkpoint metadata (source, parents, run_id) without additional lookup.
- **Severity**: Low (metadata still stored in checkpoint itself)

#### [D-05-3] ToolsEvent.ToolStarted missing input, ToolFinished missing output
- **Checklist**: 05-003
- **Design**: Section 2.2 defines `ToolStarted { tool_name, tool_call_id, node, input: serde_json::Value }` and `ToolFinished { tool_call_id, output: serde_json::Value, duration_ms }`.
- **Code**: `crates/juncture-core/src/stream.rs:167-185` -- `ToolStarted { tool_name, tool_call_id, node }` missing `input`. `ToolFinished { tool_call_id, duration_ms }` missing `output`.
- **Impact**: Tool streaming consumers cannot see tool input/output in lifecycle events. Must wait for state update.
- **Severity**: Medium (reduces utility of Tools stream mode)

#### [D-05-4] EventEmitter.emit() returns Result, should_emit() omits End event
- **Checklist**: 05-012
- **Design**: Section 3.2 defines `emit()` as returning nothing (silently ignores errors via `let _ = self.tx.send(event).await`). `should_emit()` includes `StreamEvent::End` in individual mode checks: `StreamMode::Values => matches!(event, Values{..} | End{..})`.
- **Code**: `crates/juncture-core/src/stream.rs:293` -- `emit()` returns `Result<(), SendError>`. `crates/juncture-core/src/stream.rs:304-317` -- `should_emit()` does NOT include `End` in individual mode checks. Only explicit mode-matched events return true; `End` falls through to `_ => false`. Multi mode returns true for everything.
- **Impact**: Individual modes (Values, Updates, Messages, etc.) will never emit the End event, preventing consumers from detecting stream completion. Emit propagates errors to callers instead of silently ignoring.
- **Severity**: High (stream consumers in single modes cannot detect completion)

#### [D-05-5] StreamWriter renamed to StreamEventWriter with stub send()
- **Checklist**: 05-013
- **Design**: Section 3.3 defines `StreamWriter` with `tx: mpsc::Sender<StreamEvent<()>>`, `node: String`, `ns: Vec<String>`, async `send(&self, data: serde_json::Value)` that actually sends events, and `with_ns(&self, ns_segment: String) -> Self`.
- **Code**: `crates/juncture-core/src/stream.rs:321-392` -- Named `StreamEventWriter<S: State>` (not `StreamWriter`). Has `node`, `mode`, `ns`, `_phantom` fields (no `tx`). `send()` at line 362 is `const fn` and always returns `Err(SendError(event))` -- it never actually sends. `with_ns()` takes `Vec<String>` not `String`.
- **Impact**: Custom streaming from nodes is non-functional. The writer is a stub that discards all events.
- **Severity**: High (StreamMode::Custom cannot work)

#### [D-05-6] StreamConfig.subgraph_filter is Option<Vec<String>>, not Vec<String>
- **Checklist**: 05-014
- **Design**: Section 5.3 defines `subgraph_filter: Vec<String>` (empty = all).
- **Code**: `crates/juncture-core/src/stream.rs:399` -- `subgraph_filter: Option<Vec<String>>`. None = no filter (include all), Some(Vec) = filter.
- **Impact**: Semantic difference: design uses empty Vec for "all", code uses None for "all". Both work but API differs.
- **Severity**: Low (API convention difference)

#### [D-05-7] StreamResumption fields non-optional, should_skip simplified
- **Checklist**: 05-015
- **Design**: Section 6.1 defines `last_checkpoint_id: Option<String>`, `last_step: Option<usize>`, `should_skip(&self, event: &StreamEvent<()>)` that pattern-matches event types.
- **Code**: `crates/juncture-core/src/stream.rs:426-447` -- `last_checkpoint_id: String`, `last_step: usize` (non-optional). `should_skip(&self, current_step: usize)` takes `usize` not `&StreamEvent<()>`.
- **Impact**: Cannot represent "resumption from scratch" (no checkpoint yet). should_skip only checks step, not event type.
- **Severity**: Medium (reduced resumption flexibility)

### EXTRAS (in code but not in design)

#### [E-05-1] BudgetUsage struct duplicated in stream module
- **Code**: `crates/juncture-core/src/stream.rs:188-194` -- Local `BudgetUsage` with `tokens_used: u64, cost_usd: f64, duration_ms: u64, steps_completed: usize`. Separate from budget module's `BudgetUsage` in `crates/juncture-core/src/pregel/budget.rs:227-240` which uses different field types.
- **Note**: Two different BudgetUsage structs exist in the same crate. The stream version uses `u64/f64` directly; the budget version uses `AtomicU64`-based micros.

#### [E-05-2] Duplicate simplified StreamMode/StreamEvent in pregel/stream.rs
- **Code**: `crates/juncture-core/src/pregel/stream.rs:9-24` -- Simplified `StreamMode` with only 3 variants (Values, Updates, Debug). Simplified `StreamEvent` with only 4 variants (Values, Updates, TaskEnd, Error). `IntoStreamEvent` trait and tests.
- **Note**: Two parallel streaming type hierarchies in the same crate. The pregel version is simpler and used internally by the Pregel engine. The `stream.rs` version is the full public API.

#### [E-05-3] JsonParseTransformer uses unwrap_or (project rule violation)
- **Code**: `crates/juncture-core/src/stream.rs:463-464` -- `serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)`.
- **Note**: Project constraint forbids `unwrap()` in committed code. `unwrap_or` is functionally different (always returns a value) but is still an `unwrap` family call. Should use `match` or `map_or` for explicit error handling.

---

## Module 06: Human-in-the-Loop (HITL)

### Summary

13 checklist items audited. 5 PASS, 7 DEVIATION, 0 MISSING, 3 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 06-003 | InterruptSignal (index, id: Option, payload) | `crates/juncture-core/src/interrupt/mod.rs:20-29` |
| 06-006 | ResumeValue (Single, ById, ByNamespace) + From<Vec<Value>> | `crates/juncture-core/src/interrupt/mod.rs:35-71` |
| 06-007 | CommandGoto (One, Many, Parent, Send) | `crates/juncture-core/src/command.rs:75-87` |
| 06-008 | ParentCommand newtype wrapper | `crates/juncture-core/src/command.rs:98` |
| 06-009 | Scratchpad (processed_interrupts, data, is_interrupt_processed, mark_interrupt_processed) | `crates/juncture-core/src/interrupt/mod.rs:244-308` |

### DEVIATIONS

#### [D-06-1] interrupt! macro takes explicit ctx argument, not task-local
- **Checklist**: 06-001
- **Design**: Section 2.1 defines `interrupt!($payload)` that internally calls `$crate::hitl::__current_interrupt_index()` to get index via task-local.
- **Code**: `crates/juncture-core/src/lib.rs:52-61` -- `interrupt!($ctx:expr, $payload:expr)` requires explicit context argument. No `__current_interrupt_index()` helper; index is obtained via `ctx.next_index()` inside `__interrupt_impl`.
- **Impact**: Users must pass InterruptContext explicitly to each interrupt! call. Cannot use implicit task-local lookup.
- **Severity**: Medium (API ergonomics, design intended task-local convenience)

#### [D-06-2] generate_interrupt_id hashes payload, not (node_name, index)
- **Checklist**: 06-004
- **Design**: Section 2.2 defines `fn generate_interrupt_id(node_name: &str, index: usize) -> String` using xxh3 `finish128()` producing 32-char hex (128-bit).
- **Code**: `crates/juncture-core/src/interrupt/mod.rs:100-105` -- `fn generate_interrupt_id(payload: &serde_json::Value) -> String` hashes the payload, not (node_name, index). Uses `finish()` (64-bit, 16-char hex), not `finish128()` (128-bit, 32-char hex). However, the inline generation in `__interrupt_impl` at lines 216-223 correctly hashes ("current_node", index) per design, but uses `finish()` not `finish128()`.
- **Impact**: Public function `generate_interrupt_id` has wrong semantics. Inline generation in __interrupt_impl is correct in structure but reduced hash width (64-bit vs 128-bit).
- **Severity**: Medium (public API mismatch, reduced collision resistance)

#### [D-06-3] __interrupt_impl ID generation inconsistent with generate_interrupt_id
- **Checklist**: 06-005
- **Design**: Section 2.2 defines __interrupt_impl calling `generate_interrupt_id("current_node", index)`.
- **Code**: `crates/juncture-core/src/interrupt/mod.rs:216-223` -- Inline Xxh3 hash of ("current_node", index) instead of calling `generate_interrupt_id()`. The standalone function hashes payload, creating inconsistency: two different hash strategies in the same module.
- **Impact**: Two unrelated ID generation paths. If external code uses `generate_interrupt_id()` expecting it to match internal IDs, they won't match.
- **Severity**: Low (internal consistency)

#### [D-06-4] Heartbeat is no-op stub
- **Checklist**: 06-010
- **Design**: Section 9.4 defines `Heartbeat { tx: UnboundedSender<()> }` with `ping() -> Result<(), SendError>`.
- **Code**: `crates/juncture-core/src/runtime.rs:163-188` -- `Heartbeat { _private: () }`, `ping()` is `const fn` and does nothing. No channel. No actual signaling.
- **Impact**: idle_timeout in TimeoutPolicy cannot detect live nodes. Long-running nodes will always appear idle.
- **Severity**: High (timeout mechanism non-functional)

#### [D-06-5] should_interrupt missing version-gating mechanism
- **Checklist**: 06-011
- **Design**: Section 4 defines two-step check: (1) version-gating comparing `channel_versions` vs `versions_seen["__interrupt__"]`, (2) node name check. Prevents redundant interrupts when no channels changed since last interrupt.
- **Code**: `crates/juncture-core/src/interrupt/mod.rs:141-185` -- `should_interrupt(pending_tasks, interrupt_before, interrupt_after)` does direct node name check only. No `channel_versions` or `versions_seen` parameters. No version-gating step. Also combines before+after into one function (design separates them).
- **Impact**: After checkpoint restore with no state changes, interrupt_before will fire again, causing infinite interrupt loops.
- **Severity**: High (can cause infinite interrupt loops)

#### [D-06-6] SendTarget missing timeout field, uses Value not generic S
- **Checklist**: 06-012
- **Design**: Section 9.3 defines `SendTarget<S: State> { pub node: String, pub state: S, pub timeout: Option<Duration> }`.
- **Code**: `crates/juncture-core/src/command.rs:39-45` -- `SendTarget { pub node: String, pub state: serde_json::Value }`. Not generic. Missing `timeout: Option<Duration>`.
- **Impact**: No per-task timeout override for Send targets. State is type-erased to Value instead of preserving generic type S.
- **Severity**: Medium (feature gap)

#### [D-06-7] Command<S> missing resume field, uses Goto not CommandGoto
- **Checklist**: 06-007 (related)
- **Design**: Section 5 defines `Command<S> { update: Option<S::Update>, goto: Option<CommandGoto>, resume: Option<ResumeValue> }`.
- **Code**: `crates/juncture-core/src/command.rs:7-16` -- `Command<S> { update: Option<S::Update>, goto: Goto, graph: GraphTarget }`. Missing `resume: Option<ResumeValue>` field entirely. Uses `Goto` enum (None/Next/Multiple/Send/End) instead of `Option<CommandGoto>`. Has `graph: GraphTarget` field not in design.
- **Impact**: Resume values cannot be passed through Command. The resume flow must use a separate mechanism (not defined in Command). `CommandGoto` enum exists but is not used by `Command<S>`.
- **Severity**: High (resume flow broken at Command level)

### EXTRAS (in code but not in design)

#### [E-06-1] InterruptContext extra methods
- **Code**: `crates/juncture-core/src/interrupt/context.rs:73-88` -- `current_index()` getter and `send_interrupt()` method not in design.
- **Note**: Ergonomic additions for interrupt signal dispatch.

#### [E-06-2] Scratchpad extra data access methods
- **Code**: `crates/juncture-core/src/interrupt/mod.rs:295-307` -- `get_data()` and `set_data()` methods for transient data storage, plus `new()` constructor.
- **Note**: Design section 9.2 only defines `is_interrupt_processed` and `mark_interrupt_processed`.

#### [E-06-3] Send<S> struct in send.rs wraps SendTarget conversion
- **Code**: `crates/juncture-core/src/send.rs:33-50` -- Generic `Send<S: State>` struct with `From<Send<S>> for SendTarget` impl. Uses `expect()` which violates project rules.
- **Note**: Provides typed Send API that converts to untyped SendTarget.

---

## Module 07: Subgraph Composition System

### Summary

10 checklist items audited. 4 PASS, 6 DEVIATION, 0 MISSING, 2 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 07-001 | StateSubset trait (extract, map_update) | `crates/juncture-core/src/subgraph.rs:44-73` |
| 07-004 | NamespaceSegment (node_name, invocation_id) | `crates/juncture-core/src/checkpoint.rs:136-166` |
| 07-005 | SubgraphPersistence (Inherit, PerThread, Stateless) | `crates/juncture-core/src/subgraph.rs:87-98` |
| 07-006 | SubgraphConfig { persistence } | `crates/juncture-core/src/subgraph.rs:78-82` |

### DEVIATIONS

#### [D-07-1] SubgraphNode output_map takes &Sub not Sub::Update
- **Checklist**: 07-002
- **Design**: Section 6 defines `output_map: Arc<dyn Fn(Sub::Update) -> S::Update>` -- maps subgraph update to parent update.
- **Code**: `crates/juncture-core/src/subgraph.rs:164` -- `output_map: Arc<dyn Fn(&Sub) -> S::Update>` -- takes reference to full subgraph state, not update. The Node::call() impl at line 239 calls `(self.output_map)(&sub_output.value)` with the full output state.
- **Impact**: Output mapping receives the complete final state instead of the delta. Cannot distinguish which fields changed. Different semantic contract from design.
- **Severity**: Medium (type contract mismatch)

#### [D-07-2] SubgraphTransformer filter uses Vec<String>, not closure
- **Checklist**: 07-007
- **Design**: Section 6.1 defines `filter: Option<Box<dyn Fn(&StreamEvent<S>) -> bool>>` with closure-based filtering. Generic over `<S: State>`. `new(subgraph_name, parent_ns)` takes parent namespace. `transform()` returns `Option<StreamEvent<S>>` with namespace-added event.
- **Code**: `crates/juncture-core/src/subgraph.rs:306-429` -- `filter: Option<Vec<String>>` uses string-based event type matching via `get_event_type()`. Not generic over S. `new()` takes only `subgraph_name`, missing `parent_ns` param. `transform()` returns cloned event without namespace modification. `add_namespace()` just pushes to Vec instead of transforming event.
- **Impact**: Filter cannot express arbitrary conditions (only string event type matching). Namespace not applied to events. Events pass through unchanged.
- **Severity**: Medium (filter is limited, namespace not applied)

#### [D-07-3] add_subgraph_node takes Arc, returns Result
- **Checklist**: 07-008
- **Design**: Section 2.1 defines `fn add_subgraph_node<Sub: StateSubset<S>>(&mut self, name: &str, subgraph: CompiledGraph<Sub>) -> &mut Self`.
- **Code**: `crates/juncture-core/src/graph/builder.rs:567` -- `fn add_subgraph_node<Sub>(&mut self, name: &str, subgraph: Arc<CompiledGraph<Sub>>) -> Result<&mut Self, TopologyError>`. Takes Arc instead of owned. Returns Result instead of &mut Self. Has `#[allow(dead_code)]` -- not used in production code.
- **Impact**: API differs. Dead code annotation suggests incomplete integration.
- **Severity**: Low (API convention difference)

#### [D-07-4] add_subgraph takes SubgraphMount, not individual params
- **Checklist**: 07-009
- **Design**: Section 2.2 defines `fn add_subgraph<Sub>(&mut self, name, subgraph, input_map, output_map) -> &mut Self`.
- **Code**: `crates/juncture-core/src/graph/builder.rs:523` -- `fn add_subgraph(&mut self, mount: SubgraphMount<S>) -> Result<(), TopologyError>`. Takes pre-built SubgraphMount. Returns `Result<(), TopologyError>` not `&mut Self`.
- **Impact**: Different builder API pattern. Users must construct SubgraphMount first.
- **Severity**: Low (API ergonomics)

#### [D-07-5] add_subgraph_with_config output_map wrapper ignores actual output
- **Checklist**: 07-010
- **Design**: Section 4 defines `add_subgraph_with_config(name, subgraph, input_map, output_map, config)` where output_map receives `Sub::Update`.
- **Code**: `crates/juncture-core/src/graph/builder.rs:644-648` -- Output map wrapper creates: `Arc::new(move |_sub: &Sub| { output_map(Sub::Update::default()) })`. Always passes `Sub::Update::default()` to the user's output_map, discarding actual subgraph output.
- **Impact**: Subgraph state changes are silently lost. Parent graph always receives default (empty) update from subgraph. Combined with D-07-1 output_map type mismatch, subgraph composition is non-functional.
- **Severity**: High (subgraph output mapping broken)

#### [D-07-6] add_subgraph_node output_map also uses default update
- **Checklist**: 07-008
- **Code**: `crates/juncture-core/src/graph/builder.rs:574` -- `Arc::new(|_sub_output: &Sub| Sub::map_update(Sub::Update::default()))`. Same bug as D-07-5: always passes default update.
- **Impact**: StateSubset-based subgraph composition also non-functional.
- **Severity**: High (shared-state subgraph output broken)

### EXTRAS (in code but not in design)

#### [E-07-1] SubgraphMount struct
- **Code**: `crates/juncture-core/src/subgraph.rs:104-135` -- `SubgraphMount<S> { name, config, node: Arc<dyn Node<S>> }`. Pre-built mount point for add_subgraph().
- **Note**: Design does not define this type. It acts as a builder intermediary.

#### [E-07-2] SubgraphNode extra name field
- **Code**: `crates/juncture-core/src/subgraph.rs:152` -- `pub name: String` field not in design.
- **Note**: Design section 6 SubgraphNode does not have a name field.

---

## Module 08: LLM & Tools

### Summary

36 checklist items audited. 23 PASS, 9 DEVIATION, 2 MISSING, 3 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 08-001 | Message (id, role, content, tool_calls, tool_call_id, name, usage + constructors) | `crates/juncture-core/src/state/messages.rs:7-234` |
| 08-002 | Role (System, Human, AI, Tool) | `crates/juncture-core/src/state/messages.rs:25-35` |
| 08-003 | Content (Text, MultiPart) | `crates/juncture-core/src/state/messages.rs:39-44` |
| 08-004 | ContentPart (Text, Image, Thinking with signature) | `crates/juncture-core/src/state/messages.rs:48-60` |
| 08-005 | ImageData (media_type, source) | `crates/juncture-core/src/state/messages.rs:64-69` |
| 08-006 | ImageSource (Base64, Url) | `crates/juncture-core/src/state/messages.rs:73-78` |
| 08-007 | ToolCall (id, name, args) | `crates/juncture-core/src/state/messages.rs:82-89` |
| 08-008 | TokenUsage (input_tokens, output_tokens, total_tokens) | `crates/juncture-core/src/state/messages.rs:92-100` |
| 08-009 | MessageChunk (role, content, tool_call_chunks, usage) -- LLM streaming | `crates/juncture-core/src/llm.rs:145-154` |
| 08-010 | ToolCallChunk (index, id, name, arguments) -- LLM streaming | `crates/juncture-core/src/llm.rs:158-167` |
| 08-011 | ChatModel trait (invoke, stream, bind_tools, with_structured_output, model_name) | `crates/juncture-core/src/llm.rs:178-238` |
| 08-012 | CallOptions (temperature, max_tokens, stop_sequences, top_p, model_override, tool_choice, response_format) | `crates/juncture-core/src/llm.rs:74-95` |
| 08-013 | ToolChoice (Auto, None, Required, Specific) | `crates/juncture-core/src/llm.rs:99-111` |
| 08-014 | ResponseFormat (JsonObject, JsonSchema) | `crates/juncture-core/src/llm.rs:115-127` |
| 08-018 | Tool trait (name, description, schema, definition, invoke) | `crates/juncture-core/src/tools.rs:54-83` |
| 08-019 | ToolDefinition (name, description, parameters) | `crates/juncture-core/src/llm.rs:131-138` |
| 08-022 | ToolNodeConfig (tools, handle_errors, validate_input, call_transformer, interceptor, tools_condition) | `crates/juncture-core/src/tools.rs:224-237` |
| 08-023 | ToolInterceptor (pre_execute, post_execute) | `crates/juncture-core/src/tools.rs:157-175` |
| 08-024 | ToolCallTransformer (transform) | `crates/juncture-core/src/tools.rs:209-216` |
| 08-025 | StatefulTool (invoke_with_state, invoke_with_store) | `crates/juncture-core/src/tools.rs:116-148` |
| 08-027 | ToolExecutionTrace (tool_name, tool_call_id, attempt, first_attempt_time, duration_ms, success) | `crates/juncture-core/src/tools.rs:337-350` |
| 08-031 | ReactAgentConfig (model, tools, prompt, response_format, pre/post_model_hook, store, interrupt_before/after, model_selector) | `crates/juncture-core/src/prebuilt.rs:18-41` |
| 08-032 | PromptSource (Static, Dynamic) | `crates/juncture-core/src/prebuilt.rs:162-167` |

### DEVIATIONS

#### [D-08-1] ChatAnthropic invoke/stream are stubs
- **Checklist**: 08-015
- **Design**: Section 3.1 defines full Anthropic Messages API integration with HTTP requests and SSE streaming.
- **Code**: `crates/juncture-core/src/chat.rs:110-136` -- `invoke()` returns empty AI message. `stream()` returns empty stream. No actual API calls. Fields marked `#[expect(dead_code)]`.
- **Impact**: Anthropic provider non-functional. Returns empty responses.
- **Severity**: High (provider completely non-functional)

#### [D-08-2] ChatOpenAI invoke/stream are stubs
- **Checklist**: 08-016
- **Design**: Section 3.2 defines full OpenAI Chat Completions API integration.
- **Code**: `crates/juncture-core/src/chat.rs:229-280` -- Same stub pattern as ChatAnthropic.
- **Impact**: OpenAI provider non-functional.
- **Severity**: High (provider completely non-functional)

#### [D-08-3] ChatOllama invoke/stream are stubs
- **Checklist**: 08-017
- **Design**: Section 3.3 defines full Ollama API integration.
- **Code**: `crates/juncture-core/src/chat.rs:327-378` -- Same stub pattern.
- **Impact**: Ollama provider non-functional.
- **Severity**: High (provider completely non-functional)

#### [D-08-4] ToolError has extra ToolNotFound and ValidationError variants
- **Checklist**: 08-020
- **Design**: 3 variants: InvalidInput, ExecutionFailed, Timeout.
- **Code**: `crates/juncture-core/src/tools.rs:24-45` -- 5 variants: InvalidInput, ExecutionFailed, Timeout, ToolNotFound, ValidationError.
- **Impact**: Positive enrichment. Extra variants provide finer-grained error handling.
- **Severity**: Low (extra variants)

#### [D-08-5] ToolRuntime.emit_output_delta is const no-op
- **Checklist**: 08-026
- **Design**: Section 5.2 defines `emit_output_delta()` as actual streaming output.
- **Code**: `crates/juncture-core/src/tools.rs:104-106` -- `const fn emit_output_delta(&self, _delta: &str) {}` -- no-op stub. store is `Option<Arc<dyn Store>>` (design says `Store`, not optional).
- **Impact**: Tool streaming not functional. Tools cannot emit incremental output.
- **Severity**: Medium (feature gap)

#### [D-08-6] tools_condition always returns END, never checks tool_calls
- **Checklist**: 08-029
- **Design**: Section 7 defines `tools_condition()` that inspects state's messages field and returns "tools" if last message has tool_calls, else END.
- **Code**: `crates/juncture-core/src/tools.rs:411-413` -- `const fn tools_condition<S>(_state: &S, _messages_field: &str) -> &'static str { crate::END }`. Always returns END regardless of state.
- **Impact**: ReAct agent routing always terminates instead of routing to tool node.
- **Severity**: High (ReAct agent loop broken)

#### [D-08-7] LlmError has extra Other variant
- **Checklist**: 08-035
- **Design**: 8 variants: AuthError, RateLimited, ContextLengthExceeded, NetworkError, InvalidResponse, ModelNotFound, ContentFiltered, Timeout.
- **Code**: `crates/juncture-core/src/llm.rs:23-67` -- 9 variants, adds `Other(String)`.
- **Impact**: Extra catch-all variant.
- **Severity**: Low (extra variant)

#### [D-08-8] StructuredOutputModel stream returns error, invoke uses unwrap_or_default
- **Checklist**: 08-033
- **Design**: Section 2.2 defines full structured output support.
- **Code**: `crates/juncture-core/src/llm.rs:334-341` -- `stream()` returns `Err(LlmError::InvalidResponse("Streaming not supported"))`. `invoke()` at line 323 uses `unwrap_or_default()` to serialize value (violates project no-unwrap rule).
- **Impact**: Structured output does not support streaming. Minor unwrap usage violation.
- **Severity**: Low (streaming gap, style violation)

#### [D-08-9] Two different ToolCallChunk types in different modules
- **Checklist**: 08-010 vs 05-006
- **Code**: `crates/juncture-core/src/llm.rs:158-167` defines LLM ToolCallChunk with fields `index, id, name, arguments`. `crates/juncture-core/src/stream.rs:119-125` defines streaming ToolCallChunk with fields `id, name, args_delta, index`. Field name `arguments` vs `args_delta`.
- **Impact**: Two parallel ToolCallChunk types with different field names for the same concept. Requires conversion between them.
- **Severity**: Medium (naming inconsistency, duplicate type)

### MISSING

#### [M-08-1] RetryingModel not in juncture-core
- **Checklist**: 08-036
- **Design**: Section 4.1 defines RetryingModel with inner, max_retries, initial_backoff fields and ChatModel impl with retry logic.
- **Code**: `crates/juncture/src/llm/retry.rs` -- exists in the facade crate, not in juncture-core. The checklist references `juncture::llm` module.
- **Impact**: Implementation exists but in facade crate. Type is available to users via facade re-export.
- **Severity**: Low (implemented, just in different crate than expected)

#### [M-08-2] ModelPricing not in juncture-core
- **Checklist**: 08-034
- **Design**: Section 4.2 defines ModelPricing trait with input_price_per_mtok, output_price_per_mtok, cost_for_usage.
- **Code**: `crates/juncture/src/llm/pricing.rs` -- exists in the facade crate.
- **Impact**: Same as M-08-1. Available via facade re-export.
- **Severity**: Low

### EXTRAS (in code but not in design)

#### [E-08-1] ValidationNode in facade crate
- **Code**: `crates/juncture/src/tools/validation.rs:38` -- ValidationNode with new(), with_max_tokens(), with_validator() methods. max_input_tokens and validator fields.
- **Note**: Design checklist 08-028 specifies this. Exists but in facade crate.

#### [E-08-2] create_react_agent in facade crate
- **Code**: `crates/juncture/src/prebuilt/react.rs:97` -- `create_react_agent()` and `create_react_agent_with_config()`. Fully implemented with graph construction.
- **Note**: Design checklist 08-030 specifies this. Exists in facade crate.

#### [E-08-3] NopToolInterceptor default implementation
- **Code**: `crates/juncture-core/src/tools.rs:178-201` -- Default no-op interceptor.
- **Note**: Not in design. Provides default behavior for ToolInterceptor.

---

## Module 09: Observability (Tracing & Client SDK)

### Summary

13 checklist items audited. 7 PASS, 5 DEVIATION, 0 MISSING, 1 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 09-002 | GraphCallbackHandler (on_interrupt, on_resume, on_checkpoint_saved, on_node_start, on_node_end, on_node_error, on_graph_end) | `crates/juncture-tracing/src/callback.rs:39-123` |
| 09-003 | GraphInterruptEvent (node, payload, interrupt_id, namespace, resumable) | `crates/juncture-tracing/src/callback.rs:163-178` |
| 09-004 | GraphResumeEvent (node, resume_value, namespace) | `crates/juncture-tracing/src/callback.rs:185-194` |
| 09-006 | ServerInfo (assistant_id, graph_id, user, deployment, version, instance_id) | `crates/juncture-tracing/src/types.rs:27-45` |
| 09-012 | AuthConfig (None, Token, ApiKey) | `crates/juncture-core/src/client.rs:11-23` |
| 09-013 | ClientError (Connection, Auth, GraphNotFound, ThreadNotFound, RunNotFound, Serialize, Server, Timeout) | `crates/juncture-core/src/client.rs:81-121` |
| 09-008 | CacheKeyInput (model, messages, tools, config) | `crates/juncture-tracing/src/types.rs:271-286` |

### DEVIATIONS

#### [D-09-1] MetricsRegistry has no counter/histogram/gauge methods
- **Checklist**: 09-001
- **Design**: Defines `MetricsRegistry` with `counter()`, `histogram()`, `gauge()` methods for creating custom metrics.
- **Code**: `crates/juncture-tracing/src/metrics.rs:89-115` -- `MetricsRegistry { _private: () }` with only `new()` method. No `counter()`, `histogram()`, or `gauge()` methods. Feature-gated behind `otel`. Metric name constants defined but registry has no actual metric creation.
- **Impact**: Cannot create custom metrics. Only metric name constants exist.
- **Severity**: Medium (API stub)

#### [D-09-2] DebugEvent has 12 variants, design specifies different variant set
- **Checklist**: 09-005
- **Design**: 12 variants: GraphStart, SuperstepStart, NodeStart, NodeEnd, NodeError, ChannelWrite, ChannelUpdate, Merge, EdgeTraversed, CheckpointSaved, BudgetCheck, GraphEnd.
- **Code**: `crates/juncture-tracing/src/debug.rs:16-126` -- All 12 variants present with matching names. However, some field names differ:
  - SuperstepStart: design uses `nodes`, code uses `pending_nodes`
  - NodeEnd: code has extra `output_type: String` field not in design
  - ChannelWrite: code has extra `node: String` field not in design
  - CheckpointSaved: code has extra `source: String` field not in design
  - BudgetCheck: design uses `budget_remaining_pct`, code uses same name
- **Impact**: Minor field name/extra field differences. All 12 variants present.
- **Severity**: Low (extra fields are enrichments)

#### [D-09-3] CachePolicy named LlmCachePolicy in code
- **Checklist**: 09-007
- **Design**: `CachePolicy` struct with `key_func` field.
- **Code**: `crates/juncture-tracing/src/types.rs:174-179` -- Named `LlmCachePolicy` with `key_func: Option<LlmCacheKeyFn>`. Design names it `CachePolicy`.
- **Impact**: Different name. LLM prefix makes scope clearer.
- **Severity**: Low (naming)

#### [D-09-4] JunctureClient methods are stubs
- **Checklist**: 09-009
- **Design**: `JunctureClient` with list_graphs, graph, create_thread, get_thread, list_threads, delete_thread.
- **Code**: `crates/juncture-core/src/client.rs:132-260` -- All methods exist. `list_graphs()` and `create_thread()` make HTTP requests but always return `Err(ClientError::Server { ... })`. Other methods follow similar stub patterns.
- **Impact**: Client SDK non-functional against real server.
- **Severity**: Medium (stub implementation)

#### [D-09-5] InvokeConfig fields are Option<T>, design shows bare values
- **Checklist**: 09-011
- **Design**: `InvokeConfig` with fields: thread_id, checkpoint_id, recursion_limit, metadata, tags, interrupt_before, interrupt_after.
- **Code**: `crates/juncture-core/src/client.rs:27-42` -- All fields present. All are `Option<T>` which is correct for a config struct (fields are optional overrides).
- **Impact**: Minor. Option wrapping is idiomatic for config.
- **Severity**: Low (idiomatic Rust pattern)

### EXTRAS (in code but not in design)

#### [E-09-1] DebugEvent has helper methods (is_graph_start, is_graph_end, etc.)
- **Code**: `crates/juncture-tracing/src/debug.rs:128-284` -- Convenience `is_*` methods and serde serialization.
- **Note**: Ergonomic additions for pattern matching.

---

## Module 10: Store (Cross-Thread KV Storage)

### Summary

15 checklist items audited. 13 PASS, 1 DEVIATION, 0 MISSING, 1 EXTRA.

### PASS Items

| ID | Item | Location |
|----|------|----------|
| 10-001 | Store trait (get, put, delete, search, list_namespaces, batch) | `crates/juncture-store/src/trait_.rs:10-73` |
| 10-002 | Item (namespace, key, value, created_at, updated_at, expires_at) | `crates/juncture-store/src/types.rs:13-26` |
| 10-003 | SearchItem (item, score) | `crates/juncture-store/src/types.rs:60-66` |
| 10-004 | SearchQuery (namespace_prefix, filter, query, limit, offset) | `crates/juncture-store/src/types.rs:70-86` |
| 10-005 | SearchResult (items, total_count) | `crates/juncture-store/src/types.rs:94-99` |
| 10-006 | StoreOp (Get, Put, Delete, Search, ListNamespaces) | `crates/juncture-store/src/types.rs:103-142` |
| 10-007 | StoreResult (Item, Items, Namespaces, None) | `crates/juncture-store/src/types.rs:146-155` |
| 10-008 | FilterExpr (Eq, Ne, Gt, Gte, Lt, Lte, And, Or, Not) with evaluation | `crates/juncture-store/src/filter.rs:7-56` |
| 10-009 | IndexConfig (dims, embed, fields) | `crates/juncture-store/src/types.rs:158-166` |
| 10-010 | EmbeddingFunc trait (embed) | `crates/juncture-store/src/types.rs:170-177` |
| 10-011 | MemoryStore (data, index_config, with_vector_search) | `crates/juncture-store/src/memory.rs:18-60` |
| 10-014 | TTLConfig (default_ttl, refresh_on_read, sweep_interval, sweep_max_items) | `crates/juncture-store/src/types.rs:181-201` |
| 10-015 | StoreError (NotFound, InvalidNamespace, Serialize, Storage, VectorSearch, Embedding) | `crates/juncture-store/src/error.rs:5-29` |

### DEVIATIONS

#### [D-10-1] SqliteStore and PostgresStore not found in juncture-store crate
- **Checklist**: 10-012, 10-013
- **Design**: Defines SqliteStore (pool, index_config) and PostgresStore (pool, index_config) with sqlx backends.
- **Code**: Not found anywhere in `crates/juncture-store/`. No sqlite.rs or postgres.rs files. No feature flags for sqlite/postgres in the crate's Cargo.toml.
- **Impact**: No persistent store backends. Only MemoryStore available.
- **Severity**: Medium (feature-gated persistence missing)

### EXTRAS (in code but not in design)

#### [E-10-1] FilterExpr has full evaluation engine
- **Code**: `crates/juncture-store/src/filter.rs:59-148` -- `matches_filter()` function and `compare_values()` helper with support for nested field paths (e.g., "metadata.status"), type-aware comparison (null < bool < number < string < array), and all logical combinators.
- **Note**: Design only specifies the enum variants. The evaluation engine is a complete implementation.

---

## Audit Complete

All 10 modules audited. Total: 214 checklist items across design docs 01-10.

