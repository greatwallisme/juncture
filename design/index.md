# Juncture 架构设计总览

## 1. 项目定位

Juncture 是 LangGraph 的 Rust 实现，保留其核心编程模型（StateGraph + Pregel 执行引擎），同时利用 Rust 的类型系统和并发能力提供编译期安全、真多核并行、零成本抽象。

与 LangGraph Python 版的关系：**语义等价，实现不同**。用户心智模型一致（StateGraph、Node、Edge、Checkpoint、HITL、Send、Subgraph），但底层用 Rust 类型系统替代 Python 的动态 Channel 映射。

---

## 2. 架构总览

```
┌─────────────────────────────────────────────────────────────────────────┐
│                            用户代码层                                     │
│  #[derive(State)]   StateGraph::new()   graph.compile()                 │
│  create_react_agent()   app.stream()    interrupt()   Command::new()    │
└──────────────────────────────────┬──────────────────────────────────────┘
                                   │
┌──────────────────────────────────▼──────────────────────────────────────┐
│                        juncture（门面 crate）                             │
│  prelude / LLM providers / Tool trait / prebuilt agents / Command       │
│  feature flags: anthropic / openai / ollama / sqlite / postgres          │
└──────┬──────────────┬────────────────┬──────────────┬───────────────────┘
       │              │                │              │
┌──────▼──────┐ ┌─────▼────────┐ ┌────▼──────────┐ ┌▼─────────────────┐
│juncture-core│ │juncture-derive│ │juncture-      │ │juncture-tracing  │
│             │ │              │ │checkpoint     │ │                  │
│ Channel     │ │#[derive      │ │               │ │ OpenTelemetry    │
│ (typed)     │ │ (State)]     │ │MemorySaver    │ │ 节点级 span      │
│ StateGraph  │ │ proc-macro   │ │SqliteSaver    │ │ token metrics    │
│ Pregel 引擎 │ │              │ │PostgresSaver  │ │                  │
│ Node/Edge   │ │ 生成：        │ │               │ │                  │
│ Command     │ │ - Update     │ │ trait:        │ │                  │
│ HITL        │ │ - merge()    │ │  get/put/     │ │                  │
│ Send API    │ │ - versions   │ │  put_writes/  │ │                  │
│ Subgraph    │ │ - schema     │ │  list         │ │                  │
│ Topology    │ │   migration  │ │               │ │                  │
│ Validator   │ │              │ │               │ │                  │
└──────┬──────┘ └──────────────┘ └───────────────┘ └──────────────────┘
       │
┌──────▼──────────────────────────────────────────────────────────────────┐
│                         执行层（Pregel 内部）                             │
│  PregelLoop                                                              │
│    └─ superstep                                                          │
│         ├─ 确定就绪节点（基于 field_versions + versions_seen）            │
│         ├─ tokio::spawn 并发执行（真多核并行）                            │
│         ├─ JoinSet 收集结果                                              │
│         ├─ CancellationToken 传播取消信号                                │
│         ├─ put_writes 增量持久化（每个 task 完成即写）                    │
│         ├─ apply_writes: 按字段独立 merge（确定性顺序）                  │
│         ├─ 更新 field_versions                                           │
│         ├─ Checkpoint 持久化（superstep 结束）                           │
│         ├─ Streaming 事件发射                                            │
│         └─ 计算下一 superstep 节点集合                                   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Crate 结构

```
juncture/
├── Cargo.toml                         # workspace
├── crates/
│   ├── juncture-derive/               # proc-macro crate
│   │   └── src/
│   │       ├── lib.rs                 # 导出 #[derive(State)]
│   │       ├── state_derive.rs        # State derive 主逻辑
│   │       ├── reducer.rs             # reducer 属性解析与代码生成
│   │       ├── update_gen.rs          # StateUpdate 结构体生成
│   │       ├── version_gen.rs         # field_versions 追踪代码生成
│   │       └── migration_gen.rs       # schema 版本迁移代码生成
│   │
│   ├── juncture-core/                 # 核心引擎
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── state/
│   │       │   ├── trait.rs           # State trait + FieldVersions
│   │       │   ├── channel.rs         # Channel 语义抽象（Reducer trait）
│   │       │   └── messages.rs        # MessagesState 内置实现
│   │       ├── graph/
│   │       │   ├── builder.rs         # StateGraph（构建阶段）
│   │       │   ├── compiled.rs        # CompiledGraph（执行入口）
│   │       │   └── topology.rs        # 拓扑验证器
│   │       ├── node/
│   │       │   ├── trait.rs           # Node<S> trait
│   │       │   ├── blanket.rs         # async fn → Node 自动包装
│   │       │   └── prebuilt.rs        # ToolNode 等内置节点
│   │       ├── edge/
│   │       │   ├── fixed.rs           # 静态边
│   │       │   ├── conditional.rs     # 条件边（同步 + 异步路由）
│   │       │   └── barrier.rs         # NamedBarrier（wait-all）
│   │       ├── pregel/
│   │       │   ├── loop_.rs           # PregelLoop 主循环
│   │       │   ├── runner.rs          # PregelRunner（superstep 并发执行）
│   │       │   ├── scheduler.rs       # 基于 versions_seen 的节点调度
│   │       │   └── budget.rs          # 预算追踪器
│   │       ├── command.rs             # Command 类型（update + goto + resume）
│   │       ├── send.rs                # Send API（动态 fan-out）
│   │       ├── interrupt.rs           # HITL interrupt/resume
│   │       ├── subgraph.rs            # 子图挂载与命名空间隔离
│   │       ├── stream.rs              # StreamMode + StreamEvent
│   │       ├── config.rs              # RunnableConfig
│   │       └── error.rs              # 错误类型层次
│   │
│   ├── juncture-checkpoint/
│   │   └── src/
│   │       ├── trait.rs               # CheckpointSaver trait
│   │       ├── types.rs               # Checkpoint, CheckpointMetadata
│   │       ├── memory.rs              # MemorySaver（开发用）
│   │       └── serde.rs              # 序列化策略
│   │
│   ├── juncture-checkpoint-sqlite/
│   ├── juncture-checkpoint-postgres/
│   │
│   ├── juncture-tracing/
│   │   └── src/
│   │       ├── lib.rs                 # OpenTelemetry 初始化
│   │       └── instrument.rs          # 自动插桩逻辑
│   │
│   └── juncture/                      # 门面 crate
│       └── src/
│           ├── prelude.rs
│           ├── llm/
│           │   ├── trait.rs           # ChatModel trait
│           │   ├── anthropic.rs
│           │   ├── openai.rs
│           │   ├── ollama.rs
│           │   └── mock.rs
│           ├── tools/
│           │   └── trait.rs           # Tool trait
│           └── prebuilt/
│               ├── react.rs           # create_react_agent
│               └── tool_node.rs
│
├── examples/
└── tests/
```

---

## 4. 核心设计决策

### 4.1 Channel 系统的 Rust 适配

**LangGraph 的做法**：每个 State 字段对应一个独立的 Channel 对象（LastValue、BinaryOperatorAggregate、Topic 等），通过 `channel_versions` 字典追踪每个 channel 的版本号，通过 `versions_seen[node]` 追踪每个节点消费过的版本。节点触发条件：其订阅的 channel 中有版本号高于 `versions_seen` 的。

**Juncture 的做法**：用 Rust 类型系统静态化 Channel 语义。

| LangGraph Channel | Juncture 等价 | 实现方式 |
|---|---|---|
| LastValue | `#[reducer(replace)]` | proc-macro 生成 replace 逻辑 |
| BinaryOperatorAggregate | `#[reducer(append)]` / `#[reducer(custom = fn)]` | proc-macro 生成对应 merge |
| EphemeralValue | `#[reducer(ephemeral)]` | superstep 结束后自动清零 |
| Topic | `#[reducer(append)]` + 配置 | 累积模式 |
| NamedBarrierValue | `BarrierEdge` | 边类型，非字段属性 |
| DeltaChannel | checkpoint 层优化 | 增量存储，对 State 层透明 |

**关键保留**：
- `FieldVersions`：每个字段独立版本号，proc-macro 自动生成追踪代码
- `VersionsSeen`：每个节点记录已消费的字段版本，用于调度决策
- 语义等价：同一 superstep 内多个节点写同一字段时，reducer 决定合并策略

**为什么不直接用动态 Channel Map**：
- Rust 的类型系统可以在编译期保证字段存在性和类型正确性
- 动态 HashMap<String, Box<dyn Any>> 丢失所有编译期安全
- proc-macro 生成的代码等价于手写的 per-field channel 逻辑，但零运行时开销

### 4.2 Command 替代 NodeOutput

**LangGraph 的 Command**：统一的节点返回类型，组合 state update + routing + resume + parent navigation。

**Juncture 的 Command**：

```rust
pub struct Command<S: State> {
    pub update: Option<S::Update>,
    pub goto: Goto,
    pub resume: Option<serde_json::Value>,
    pub graph: GraphTarget,
}

pub enum Goto {
    None,
    Node(String),
    Nodes(Vec<String>),
    Send(Vec<SendTarget<S>>),
}

pub enum GraphTarget {
    Current,
    Parent,
}
```

节点可以返回 `S::Update`（简单情况）或 `Command<S>`（需要控制流时）。

### 4.3 边作为触发器通道的语法糖

在 LangGraph 内部，`add_edge(A, B)` 实际创建了一个隐藏的 EphemeralValue channel `branch:to:B`，A 执行完后写入该 channel，B 订阅该 channel 从而被触发。

Juncture 简化：边直接存储在图结构中，调度器在 superstep 结束后根据边关系 + field_versions 计算下一批节点。不需要为每条边创建隐式 channel——这是 Python 动态类型的实现细节，不是语义需求。

### 4.4 增量写入持久化（put_writes）

LangGraph 的关键设计：每个 task 完成后立即通过 `put_writes()` 持久化其输出，而非等到整个 superstep 结束。这保证了：
- 崩溃恢复：已完成的 task 不需要重新执行
- DeltaChannel 优化：只存增量

Juncture 保留此设计：`CheckpointSaver::put_writes()` 在每个节点完成后调用，`put_checkpoint()` 在 superstep 结束后调用。

### 4.5 真并行执行

- `tokio::spawn` + `JoinSet`：每个节点在独立 task 中执行，利用 tokio work-stealing 调度器实现真多核并行
- 每个节点获得 State 的独立克隆（`&S` 只读语义，返回 `S::Update`）
- `CancellationToken` 结构化取消传播
- Merge 顺序确定性：按节点注册顺序（IndexMap）

---

## 5. LangGraph 概念映射

<!-- Addresses finding: ALL (overview update) -->

| LangGraph Python | Juncture Rust | 说明 |
|---|---|---|
| `StateGraph` | `StateGraph<S, I=S, O=S>` | 泛型化，支持 I/O Schema 分离 |
| `UntrackedValue` | `#[reducer(untracked)]` | 跨 superstep 但不 checkpoint |
| `LastValueAfterFinish` | `#[reducer(replace_after_finish)]` | 延迟触发，finish() 后可用 |
| `NamedBarrierValueAfterFinish` | Barrier + AfterFinish | 延迟触发 Barrier |
| `Overwrite(value)` | `Overwrite<T>(value)` | 绕过 reducer 直接写入 |
| `entrypoint.final` | `Final<V, S>` | 区分返回值与保存值 |
| `bulk_update_state` | `bulk_update_state(updates)` | 批量原子更新 |
| `ManagedValues` | `Runtime.managed_values()` | IsLastStep + RemainingSteps |
| `GraphBubbleUp` | `BubbleUp<S>` enum | Interrupt/Drained/ParentCommand |
| `ErrorCode` | `ErrorCode` enum | 标准错误代码分类 |
| `tools_condition()` | `tools_condition()` | 工具条件路由 |
| `ValidationNode` | `ValidationNode` | 输入验证（deprecated but useful） |
| `ToolRuntime` | `ToolRuntime<S>` | 工具运行时上下文注入 |
| `ServerInfo` | `ServerInfo` | 服务器部署元数据 |
| `CachePolicy` | `CachePolicy` | 缓存键生成策略 |
| `HIDDEN_TAG` | `HIDDEN_TAG` | 过滤内部节点 |
| `resume_map` | `ResumeValue::ByNamespace` | 命名空间路由 resume |
| `xxh3_128` | `xxhash_rust::xxh3::xxh3_128` | 确定性中断 ID 生成 |
| `TypedDict` + `Annotated` | `#[derive(State)]` struct | proc-macro 替代运行时注解 |
| Channel (internal) | per-field reducer + FieldVersions | 静态化 channel 语义 |
| `add_node(name, fn)` | `graph.add_node(name, fn)` | blanket impl 支持多种签名 |
| `add_edge(a, b)` | `graph.add_edge(a, b)` | 一致 |
| `add_conditional_edges` | `graph.add_conditional_edges` | 一致，额外支持异步路由 |
| `compile(checkpointer=)` | `graph.compile(checkpointer)` | 额外执行拓扑验证 |
| `invoke(input, config)` | `graph.invoke(input, &config)` | 一致 |
| `stream(input, stream_mode=)` | `graph.stream(input, &config, mode)` | 9 种模式全部支持 (Values, Updates, Messages, Custom, Debug, Tools, Checkpoints, Tasks, Multi) |
| `Command(update=, goto=)` | `Command::new().update().goto()` | builder 模式 |
| `interrupt(value)` | `interrupt!(value)` | 宏实现，支持命名中断 ID |
| `Send(node, arg)` | `Send::new(node, state)` | 一致 |
| `get_state(config)` | `graph.get_state(&config)` | 一致 |
| `update_state(config, values)` | `graph.update_state(&config, update)` | 类型安全 |
| `InMemorySaver` | `MemorySaver` | 一致 |
| `SqliteSaver` | `SqliteSaver` | 一致 |
| `RunnableConfig` | `RunnableConfig` | 一致 |
| `recursion_limit` | `config.recursion_limit` | 一致 |
| `thread_id` | `config.thread_id` | 一致 |
| Store (cross-thread) | `Store` trait | 长期记忆，独立于 checkpoint |
| `Runtime(context=)` | `Runtime<C>` | 上下文注入、Store、heartbeat、ExecutionInfo |
| `@entrypoint` / `@task` | 函数式 API | `#[entrypoint]` / `#[task]` 等价 |
| `RetryPolicy` | `RetryPolicy` struct | 指数退避重试，per-node 配置 |
| `TimeoutPolicy` | `TimeoutPolicy` struct | run_timeout / idle_timeout |
| `Durability` | `Durability` enum | Sync / Async / Exit 持久化模式 |
| `RunControl` | `RunControl` struct | 优雅停止 |
| `IsLastStep` / `RemainingSteps` | Managed Values via Runtime | 感知递归限制 |
| `GraphCallbackHandler` | `GraphCallbackHandler` trait | 生命周期回调 |
| `RemoteGraph` | `RemoteGraph<S>` | 远程图调用，跨进程组合 |
| SDK Client | `JunctureClient` | 面向应用开发者的完整客户端 |
| `CowState<S>` | `CowState<S>` | 默认状态包装器，写时复制优化 |
| `SyncAsyncFuture<T>` | `SyncAsyncFuture<T>` | 函数式 API 任务结果（可能同步或异步） |
| `AnyValue` channel | `#[reducer(any)]` | 假设所有值相等的累积器 |
| `MessagePack` | `SerializationFormat::MessagePack` | 默认序列化格式（性能优先） |
| `JsonPlusSerializer` | `JsonPlusSerializer` | 增强的 JSON 序列化器 |
| `EncryptedSerializer` | `EncryptedSerializer` | AES-256-GCM 加密序列化器 |
| `max_parallel_tasks` | `max_parallel_tasks: usize` | 有界并发控制（Semaphore） |
| `CachePolicy` | `CachePolicy` struct | 缓存策略（自定义键生成） |
| `run_name` | `config.run_name: String` | 运行名称（用于标识） |
| `checkpoint_ns` | `config.checkpoint_ns: String` | 子图命名空间隔离 |
| `add_sequence` | `graph.add_sequence(nodes)` | 便捷方法：线性链 |
| `validate_keys` | `graph.validate_keys()` | 状态键验证 |
| `ToolInterceptor` | `ToolInterceptor` trait | 工具调用拦截器（pre/post） |
| `SubgraphTransformer` | `SubgraphTransformer` struct | 嵌套子图事件转换器 |
| `MetricsRegistry` | `MetricsRegistry` | 显式 metrics API |
| `TTLConfig` | `TTLConfig` | Store TTL 自动过期配置 |
| `FilterExpr` (complete) | `FilterExpr` enum | 完整的过滤表达式（$eq, $ne, $gt, $gte, $lt, $lte, $and, $or, $not） |
| EmbeddingFunc | `EmbeddingFunc` trait | 向量搜索嵌入函数 |
| ErrorKind is_xxx() | `JunctureError::is_*()` | Microsoft 风格错误类型检查 |
| REMOVE_ALL_MESSAGES | `REMOVE_ALL_MESSAGES` | 清空消息列表 sentinel |
| EmptyChannelError | `JunctureError::EmptyChannel` | 空 Channel 错误 |
| EmptyInputError | `JunctureError::EmptyInput` | 空输入错误 |
| TaskNotFound | `JunctureError::TaskNotFound` | 任务未找到错误 |

---

## 6. 模块设计文档索引

| 文档 | 内容 |
|---|---|
| [01-state-channel.md](01-state-channel.md) | State 系统与 Channel 语义适配 |
| [02-graph-builder.md](02-graph-builder.md) | 图构建、Node / Edge / Command 与编译 |
| [03-pregel-engine.md](03-pregel-engine.md) | Pregel 执行引擎、调度、取消、预算、重试、超时 |
| [04-checkpoint.md](04-checkpoint.md) | Checkpoint 持久化系统 |
| [05-streaming.md](05-streaming.md) | Streaming 系统（10 种模式） |
| [06-hitl.md](06-hitl.md) | Human-in-the-Loop（interrupt / resume / 命名中断） |
| [07-subgraph.md](07-subgraph.md) | 子图组合系统 |
| [08-llm-tools.md](08-llm-tools.md) | LLM 集成、Tool 系统、Prebuilt Agent |
| [09-observability.md](09-observability.md) | 可观测性（Tracing / OpenTelemetry / Callbacks） |
| [10-store.md](10-store.md) | Store 跨线程长期记忆 |
