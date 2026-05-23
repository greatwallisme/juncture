# CLAUDE.md -- juncture-core

Core types and engine for Juncture. This is the largest crate; all other crates depend on it.

## Module Map

| Module | Responsibility |
|--------|---------------|
| `state/` | `State` trait, `CowState<S>` (Arc copy-on-write), `FieldsChanged` (u64 bitmask), `Reducer<T>` trait, channel types (`UntrackedChannel`, `EphemeralChannel`, `DeltaChannel`, `LastValueAfterFinishChannel`, `NamedBarrierChannel`, `TopicChannel`), `Overwrite<T>` serde wrapper |
| `graph/` | `StateGraph<S,I,O>` builder (3 type params), `CompiledGraph<S,I,O>`, topology validation, `RemoteGraph`, `RetryPolicy`, `TimeoutPolicy`, `CompileConfig` |
| `pregel/` | Pregel execution engine: `PregelLoop<S>`, `execute_superstep()`, task scheduling (`compute_next_tasks`, `apply_writes`), `BudgetConfig`, `BudgetTracker`, `Durability` modes, streaming |
| `node/` | `Node<S>` trait, `IntoNode` conversions via wrapper types (`NodeFnUpdate`, `NodeFnCommand`, `NodeFnUpdateWithRuntime`, etc.) |
| `edge/` | `Edge`, `Router`, `PathMap`, `TriggerTable<S>`, `START`/`END` sentinels |
| `interrupt/` | HITL: `InterruptSignal` (with timestamp), `ResumeValue` (single/ID-based/namespace-based), `Scratchpad`, `interrupt!` macro, `InterruptContext`, `validate_resume_coverage()` |
| `command/` | `Command<S>`, `Goto`, `SendTarget`, `GraphTarget`, `ParentCommand`, `Final<V,S>` for node return routing |
| `subgraph/` | `StateSubset<Parent>` trait (proc-macro generated), `SubgraphConfig`, `SubgraphNode`, `SubgraphMount`, `SubgraphTransformer` |
| `runtime/` | `Runtime<C>` (context, store, stream, heartbeat, previous value, execution info) |
| `stream/` | `StreamEvent`, `StreamMode`, `StreamTransformer`, `EventEmitter`, `ToolsEvent` (with timestamp/success), `MessageBatchConfig`, `StreamConfig`, `StreamResumption`, transformers (`JsonParse`, `FilterFields`, `Batch`) |
| `checkpoint/` | `CheckpointSaver` trait, `Checkpoint`, `CheckpointMetadata`, `CheckpointNamespace`, `PendingWrite` |
| `config/` | `RunnableConfig` (with `with_run_id()`), `CacheConfig`, `CachePolicy`, `TaskConfig`, `EntrypointConfig` |
| `func/` | Functional API: `Runtime<S>`, `compile_entrypoint()`, `compile_entrypoint_with_config()` -- lightweight wrapper around StateGraph for function-based workflow definition |
| `store/` | `Store` trait, `MemoryStore`, `SqliteStore`, `PostgresStore`, `FilterExpr`, `SearchQuery`, `TTLConfig`, `IndexConfig`, `EmbeddingFunc` (cross-thread KV storage with optional vector search) |
| `llm/` | `ChatModel` trait, `ToolDefinition`, `CallOptions`, `LlmError` (`Other` variant holds `Box<dyn Error + Send + Sync>`) |
| `tools/` | `Tool<S>` trait, `ToolRuntime<S>` (with `emit_tool_started`/`emit_tool_finished`), `ToolNode`, `ToolNodeConfig`, `ToolExecutionTrace`, `tools_condition` |
| `prebuilt/` | `PromptSource`, `ReactAgentConfig` |
| `observability/` | `MetricsCollector` trait, `GraphLifecycleCallback` trait, `CacheKeyInput`, `LlmCachePolicy`, `ServerInfo` |
| `error/` | `JunctureError`, `ErrorCode`, `NodeTimeoutError`, `InvalidUpdateError` |
| `chat/` | `ChatAnthropic`, `ChatOpenAI`, `ChatOllama` (thin re-exports; real impls in facade crate) |
| `send/` | `Send` for dynamic fan-out |
| `client/` | `GraphClient`, `JunctureClient`, `StateSnapshot`, `Thread` for remote graph access |

## Key Design Patterns

### State + Update pair
Every state struct `S` has an associated `S::Update` (all fields `Option<T>`). `#[derive(State)]` (in juncture-derive) generates the Update struct and `apply()` with per-field reducer semantics. `FieldsChanged` is a u64 bitmask tracking which fields changed in a superstep.

### IntoNode wrapper types
Raw async functions don't implement `IntoNode<S>` directly. Wrap them: `NodeFnUpdate(func)` for `Fn(S) -> Result<S::Update>`, `NodeFnCommand(func)` for `Fn(S) -> Result<Command<S>>`, etc. Forms with `RunnableConfig` and `Runtime<C>` parameters also exist.

### Pregel execution
1. `PregelLoop::tick()` checks for pending tasks
2. `execute_superstep()` spawns all tasks via `tokio::spawn` + `JoinSet` with `Semaphore`-bounded concurrency
3. `PregelLoop::after_tick()` calls `apply_writes()` to merge results, then `compute_next_tasks()` using field version tracking (`versions_seen` per node)
4. Repeat until no tasks or termination

### CowState
`CowState<S>` wraps `Arc<S>` for copy-on-write. The default state wrapper -- avoids cloning entire state per node spawn. Call `update()` to stage changes, `commit()` to apply.

### Channels and Reducers
Fields use `Reducer<T>` to define merge semantics: `ReplaceReducer` (one writer, panics on double-write), `AppendReducer` (Vec extend), `LastWriteWinsReducer`, `AnyValueReducer`. Channel types add checkpoint/ephemeral/delta behavior on top. `NamedBarrierChannel` provides keyed barrier synchronization. `TopicChannel` provides pub/sub semantics.

### StateGraph compilation
`StateGraph::compile()` produces `CompiledGraph<S,I,O>` (no checkpointer). Use `compile_with_checkpointer()` or `compile_with_config()` for persistence. `add_node()` takes 7 args: name, node (impl IntoNode), defer, metadata, destinations, retry_policies, timeout_policies.

## Features

- `otel` -- OpenTelemetry integration
- `sqlite` -- sqlx SQLite support
- `postgres` -- sqlx Postgres support

## Tests

Integration tests in `tests/` cover: `derive_state`, `edge_tests`, `interrupt_tests`, `node_tests`, `runtime_tests`.
