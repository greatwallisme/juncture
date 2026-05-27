# CLAUDE.md -- juncture-core

Core types and engine for Juncture. This is the largest crate; all other crates depend on it.

## Module Map

### Directory modules (multi-file)

| Module | Files | Responsibility |
|--------|-------|---------------|
| `state/` | `mod.rs`, `trait_.rs`, `channel.rs`, `messages.rs` | `State` trait, `CowState<S>` (Arc copy-on-write), `FieldsChanged` (u64 bitmask), `Reducer<T>` trait, channel types (`UntrackedChannel`, `EphemeralChannel`, `DeltaChannel`, `LastValueAfterFinishChannel`, `NamedBarrierChannel`, `TopicChannel`), `Overwrite<T>` serde wrapper, `MessagesState`/`MessagesStateUpdate` (pre-built chat state with append reducer), `Message`, `Content`, `Role`, `ToolCall` |
| `graph/` | `mod.rs`, `builder.rs`, `compiled.rs`, `remote.rs`, `topology.rs` | `StateGraph<S,I,O>` builder (3 type params), `CompiledGraph<S,I,O>`, topology validation, `RemoteGraph`, `RetryPolicy`, `TimeoutPolicy`, `CompileConfig`, `GraphOutput`, `DrawableGraph` |
| `pregel/` | `mod.rs`, `loop_.rs`, `runner.rs`, `scheduler.rs`, `protocol.rs`, `budget.rs`, `context.rs`, `durability.rs`, `types.rs` | Pregel execution engine: `PregelLoop<S>`, `execute_superstep()`, task scheduling (`compute_next_tasks`, `apply_writes`), `BudgetConfig`, `BudgetTracker`, `Durability` modes, streaming, `PregelProtocol` |
| `node/` | `mod.rs`, `trait.rs`, `into_node.rs` | `Node<S>` trait, `IntoNode` conversions via wrapper types (`NodeFnUpdate`, `NodeFnCommand`, `NodeFnUpdateWithRuntime`, etc.) |
| `edge/` | `mod.rs`, `types.rs`, `compiled.rs` | `Edge`, `Router`, `PathMap`, `TriggerTable<S>`, `START`/`END` sentinels |
| `interrupt/` | `mod.rs`, `context.rs` | HITL: `InterruptSignal` (with timestamp), `ResumeValue` (single/ID-based/namespace-based), `Scratchpad`, `interrupt!` macro, `interrupt_with_ctx!` macro, `InterruptContext`, `validate_resume_coverage()` |
| `func/` | `mod.rs` | Functional API: `Runtime<S>`, `compile_entrypoint()`, `compile_entrypoint_with_config()` -- lightweight wrapper around StateGraph for function-based workflow definition |

### Flat file modules

| Module | Responsibility |
|--------|---------------|
| `command.rs` | `Command<S>`, `Goto`, `SendTarget`, `GraphTarget`, `ParentCommand`, `Final<V,S>` for node return routing |
| `subgraph.rs` | `StateSubset<Parent>` trait (proc-macro generated), `SubgraphConfig`, `SubgraphNode`, `SubgraphMount`, `SubgraphTransformer`, `SubgraphPersistence` |
| `runtime.rs` | `Runtime<C>` (context, store, stream, heartbeat, previous value, execution info), `Heartbeat`, `HeartbeatWatcher`, `RunControl` |
| `stream.rs` | `StreamEvent`, `StreamMode`, `StreamTransformer`, `EventEmitter`, `ToolsEvent` (with timestamp/success), `MessageBatchConfig`, `StreamConfig`, `StreamResumption`, transformers (`JsonParse`, `FilterFields`, `Batch`), `StreamWriter` |
| `checkpoint.rs` | `CheckpointSaver` trait, `Checkpoint`, `CheckpointMetadata`, `CheckpointNamespace`, `PendingWrite`, `CHECKPOINT_NS_SEPARATOR` |
| `config.rs` | `RunnableConfig` (with `with_run_id()`), `CacheConfig`, `CachePolicy`, `TaskConfig`, `EntrypointConfig` |
| `store.rs` | `Store` trait, `MemoryStore`, `FilterExpr`, `SearchQuery`, `TTLConfig`, `IndexConfig`, `EmbeddingFunc` (cross-thread KV storage with optional vector search) |
| `llm.rs` | `ChatModel` trait, `ToolDefinition`, `CallOptions`, `LlmError` (`Other` variant holds `Box<dyn Error + Send + Sync>`) |
| `tools.rs` | `Tool<S>` trait, `ToolRuntime<S>` (with `emit_tool_started`/`emit_tool_finished`), `ToolNode`, `ToolNodeConfig`, `ToolExecutionTrace`, `tools_condition` |
| `prebuilt.rs` | `PromptSource`, `ReactAgentConfig` |
| `observability.rs` | `MetricsCollector` trait, `GraphLifecycleCallback` trait, `CacheKeyInput`, `LlmCachePolicy`, `ServerInfo` |
| `error.rs` | `JunctureError`, `ErrorCode`, `NodeTimeoutError`, `InvalidUpdateError` |
| `chat.rs` | `ChatAnthropic`, `ChatOpenAI`, `ChatOllama` (thin re-exports; real impls in facade crate) |
| `send.rs` | `Send` for dynamic fan-out |
| `client.rs` | `GraphClient`, `JunctureClient`, `StateSnapshot`, `Thread` for remote graph access |

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

### Macros
- `interrupt!(payload)` / `interrupt!("id", payload)` -- HITL interrupt using task-local context
- `interrupt_with_ctx!(ctx, payload)` -- HITL interrupt with explicit context
- `parent_command!("target")` -- subgraph-to-parent routing

## Features

- `otel` -- OpenTelemetry integration
- `sqlite` -- sqlx SQLite support
- `postgres` -- sqlx Postgres support

## Tests

Integration tests in `tests/` cover: `derive_state`, `edge_tests`, `interrupt_tests`, `node_tests`, `runtime_tests`, `fanout_perf_test`.
