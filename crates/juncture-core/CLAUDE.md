# CLAUDE.md -- juncture-core

Core types and engine for Juncture. This is the largest crate; all other crates depend on it.

## Module Map

| Module | Responsibility |
|--------|---------------|
| `state/` | `State` trait, `CowState<S>` (Arc copy-on-write), `FieldsChanged` (u64 bitmask), `Reducer<T>` trait, channel types (`UntrackedChannel`, `EphemeralChannel`, `DeltaChannel`, `LastValueAfterFinishChannel`) |
| `graph/` | `StateGraph<S>` builder, `CompiledGraph<S>`, topology validation, `RemoteGraph` |
| `pregel/` | Pregel execution engine: `PregelLoop<S>`, `execute_superstep()`, task scheduling (`compute_next_tasks`, `apply_writes`), budget tracking, durability, streaming |
| `node/` | `Node<S>` trait, `IntoNode` conversions from async functions |
| `edge/` | `Edge`, `Router`, `PathMap`, `TriggerTable<S>`, `START`/`END` sentinels |
| `interrupt/` | HITL: `InterruptSignal`, `ResumeValue`, `Scratchpad`, `interrupt!` macro, `InterruptContext` |
| `command/` | `Command<S>`, `Goto`, `SendTarget`, `GraphTarget`, `Final<V,S>` for node return routing |
| `subgraph/` | `StateSubset<Parent>` trait, `SubgraphConfig`, `SubgraphNode`, `SubgraphMount` |
| `runtime/` | `Runtime<C>` (context, store, stream, heartbeat, execution info) |
| `stream/` | `StreamEvent`, `StreamMode`, `StreamTransformer`, `EventEmitter`, `DebugEvent` |
| `checkpoint/` | `CheckpointSaver` trait, `Checkpoint`, `CheckpointMetadata`, `PendingWrite` |
| `config/` | `RunnableConfig`, `CacheConfig`, `TaskConfig`, `EntrypointConfig` |
| `store/` | `Store` trait, `MemoryStore`, `FilterExpr`, `SearchQuery` (cross-thread KV storage) |
| `llm/` | `ChatModel` trait, `ToolDefinition`, `CallOptions`, `LlmError` |
| `tools/` | `Tool<S>` trait, `ToolNode<S>`, `ToolNodeConfig`, `tools_condition` |
| `prebuilt/` | `PromptSource`, `ReactAgentConfig` |
| `observability/` | `MetricsRegistry`, `CacheKeyInput`, `ServerInfo` |
| `error/` | `JunctureError`, `ErrorCode`, `NodeTimeoutError`, `InvalidUpdateError` |
| `chat/` | `ChatAnthropic`, `ChatOpenAI`, `ChatOllama` (thin re-exports; real impls in facade crate) |
| `send/` | `Send` for dynamic fan-out |
| `client/` | `GraphClient`, `JunctureClient`, `StateSnapshot`, `Thread` for remote graph access |

## Key Design Patterns

### State + Update pair
Every state struct `S` has an associated `S::Update` (all fields `Option<T>`). `#[derive(State)]` (in juncture-derive) generates the Update struct and `apply()` with per-field reducer semantics. `FieldsChanged` is a u64 bitmask tracking which fields changed in a superstep.

### Pregel execution
1. `PregelLoop::tick()` checks for pending tasks
2. `execute_superstep()` spawns all tasks via `tokio::spawn` + `JoinSet` with `Semaphore`-bounded concurrency
3. `PregelLoop::after_tick()` calls `apply_writes()` to merge results, then `compute_next_tasks()` using field version tracking (`versions_seen` per node)
4. Repeat until no tasks or termination

### CowState
`CowState<S>` wraps `Arc<S>` for copy-on-write. The default state wrapper -- avoids cloning entire state per node spawn. Call `update()` to stage changes, `commit()` to apply.

### Channels and Reducers
Fields use `Reducer<T>` to define merge semantics: `ReplaceReducer` (one writer, panics on double-write), `AppendReducer` (Vec extend), `LastWriteWinsReducer`, `AnyValueReducer`. Channel types add checkpoint/ephemeral/delta behavior on top.

## Features

- `otel` -- OpenTelemetry integration
- `sqlite` -- sqlx SQLite support
- `postgres` -- sqlx Postgres support

## Tests

Integration tests in `tests/` cover: `derive_state`, `edge_tests`, `interrupt_tests`, `node_tests`, `runtime_tests`.
