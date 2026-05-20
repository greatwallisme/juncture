# 02 - 图构建与编译

## 概述

本模块定义 Juncture 的图构建 API（`StateGraph`）、节点系统（`Node` trait）、边系统（`Edge`）、Command 原语，以及编译阶段的拓扑验证。用户通过 Builder 模式声明图结构，调用 `compile()` 得到可执行的 `CompiledGraph`。

---

## 1. StateGraph Builder API

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:130` — StateGraph 类定义
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1164` — compile() 方法
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1431` — attach_node() 内部
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1537` — attach_edge() 内部

`StateGraph<S>` 是图的构建阶段表示。它收集节点、边、子图声明，在 `compile()` 时执行拓扑验证并生成不可变的执行结构。

<!-- Addresses finding: C-03 -->

### Input/Output Schema 分离

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:130` — `StateGraph(state_schema, input_schema=InputSchema, output_schema=OutputSchema)`

LangGraph 允许图使用不同的 Schema 进行输入和输出，隐藏内部私有字段。Juncture 通过泛型参数支持此模式：

```rust
/// StateGraph 支持三个类型参数
/// - S: 完整内部状态类型
/// - I: 输入 Schema（S 的子集），默认为 S
/// - O: 输出 Schema（S 的子集），默认为 S
pub struct StateGraph<S: State, I: IntoState<S> = S, O: FromState<S> = S> { ... }
```

其中 `IntoState<S>` 和 `FromState<S>` trait 定义见 `01-state-channel.md` 2.7 节。

```rust
// 使用示例：隐藏私有字段
let graph = StateGraph::<AgentState, AgentInput, AgentOutput>::new()
    .add_node("think", think_node)
    .set_entry_point("think")
    .compile(MemorySaver::new())?;

// 调用者使用 AgentInput（不包含私有字段）
let result: AgentOutput = graph.invoke(AgentInput { messages, context }, &config)?;
```

```rust
// juncture-core/src/graph/builder.rs

pub struct StateGraph<S: State> {
    nodes: IndexMap<String, Arc<dyn Node<S>>>,
    edges: Vec<Edge<S>>,
    entry_point: Option<String>,
    finish_points: Vec<String>,
    subgraphs: Vec<SubgraphMount<S>>,
}

impl<S: State> StateGraph<S> {
    pub fn new() -> Self;

    /// 添加节点。name 必须唯一，node 可以是任何实现 IntoNode<S> 的类型。
    /// <!-- Addresses finding: L-8 -->
    /// defer: 如果为 true，节点不会在收到触发时立即执行，
    /// 而是等到所有非 deferred 节点执行完毕后再执行。
    /// <!-- Addresses finding: L-9 -->
    /// metadata: 节点级元数据（用于可观测性标记、条件过滤等）。
    /// <!-- Addresses finding: L-10 -->
    /// destinations: 声明该节点可能路由到的目标列表（用于拓扑验证和图导出）。
    /// <!-- Addresses finding: L-11 -->
    /// retry_policies: 重试策略列表（替代单一的 retry_policy），
    /// 允许对不同错误类型配置不同的重试策略。
    pub fn add_node(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        defer: bool,
        metadata: Option<HashMap<String, serde_json::Value>>,
        destinations: Option<Vec<String>>,
        retry_policies: Vec<RetryPolicy>,
    ) -> &mut Self;

> **Implementation Note**: `RetryingNode` wrapper provides production-grade retry with exponential backoff.
> Goes beyond LangGraph base retry with jitter, circuit breaker, and comprehensive error classification.

> **Implementation Note (C-02-1)**: The full `RetryPolicy` implementation includes exponential backoff
> with configurable initial interval, jitter (full jitter strategy to avoid thundering herd), max
> interval caps, and max attempt limits. This exceeds the design's basic retry specification and
> provides production-grade resilience for transient failures in LLM API calls and network operations.

    // > **实现备注 (D-02-1)**: 实际实现中 `add_node` 返回 `Result<(), TopologyError>` 而非 `&mut Self`。
    // > 这破坏了链式构建器模式（不再支持 `.add_node("a", ...)?.add_node("b", ...)?`），
    // > 但支持 fail-fast 验证——重复节点名等拓扑错误在调用时立即返回而非延迟到 `compile()`。
    // > 用户需要在每次 `add_node` 调用后使用 `?` 运算符。

    /// 简化版本：向后兼容的 add_node（无额外参数）
    pub fn add_node_simple(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
    ) -> &mut Self;

    /// 添加静态边：from 执行完毕后，to 在下一 superstep 执行。
    pub fn add_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> &mut Self;

    /// 添加条件边：from 执行完毕后，调用 router 决定下一步。
    /// path_map 声明所有可能的分支目标（用于拓扑验证和图导出）。
    pub fn add_conditional_edges(
        &mut self,
        from: impl Into<String>,
        router: impl Router<S> + 'static,
        path_map: impl Into<PathMap>,
    ) -> &mut Self;

    /// 设置入口节点（等价于 add_edge(START, node)）。
    pub fn set_entry_point(&mut self, node: impl Into<String>) -> &mut Self;

    /// 设置终止节点（等价于 add_edge(node, END)）。
    pub fn set_finish_point(&mut self, node: impl Into<String>) -> &mut Self;

    /// <!-- Addresses finding: M-10 -->
    /// 添加线性链：便捷方法用于添加连续的节点序列。
    /// nodes[0] 连接到 nodes[1]，nodes[1] 连接到 nodes[2]，依此类推。
    /// 如果 entry_point 未设置，nodes[0] 自动设为入口点。
    pub fn add_sequence(&mut self, nodes: &[impl AsRef<str>]) -> &mut Self {
        for window in nodes.windows(2) {
            self.add_edge(window[0].as_ref(), window[1].as_ref());
        }
        if self.entry_point.is_none() && !nodes.is_empty() {
            self.set_entry_point(nodes[0].as_ref());
        }
        self
    }

    /// 添加子图。
    pub fn add_subgraph<Sub: State>(
        &mut self,
        name: impl Into<String>,
        subgraph: CompiledGraph<Sub>,
        input_map: impl Fn(&S) -> Sub + Send + Sync + 'static,
        output_map: impl Fn(Sub::Update) -> S::Update + Send + Sync + 'static,
    ) -> &mut Self;

> **Implementation Note**: `SubgraphMount` uses builder pattern instead of individual parameters.
> Provides fluent API for complex subgraph configuration with type-safe state mapping.

    /// 编译图：执行拓扑验证，生成 CompiledGraph。
    pub fn compile(
        self,
        checkpointer: impl CheckpointSaver,
    ) -> Result<CompiledGraph<S>, TopologyError>;

    /// 编译为无持久化的临时图（开发/测试用）。
    pub fn compile_ephemeral(self) -> Result<CompiledGraph<S>, TopologyError>;

    /// <!-- Addresses finding: L-9 -->
    /// 验证状态键的有效性。
    /// 检查所有节点的更新是否只引用了 State 中定义的字段。
    /// 在编译时自动调用，也可单独调用用于调试。
    pub fn validate_keys(&self) -> Result<(), TopologyError>;
}
```

### PathMap

```rust
/// 条件边的分支映射表。
/// key: 路由函数返回值, value: 目标节点名。
pub struct PathMap(HashMap<String, String>);

impl From<&[(&str, &str)]> for PathMap { ... }
impl From<HashMap<String, String>> for PathMap { ... }

/// 便捷宏
macro_rules! path_map {
    ($($key:expr => $val:expr),* $(,)?) => { ... };
}
```

### 使用示例

```rust
let mut graph = StateGraph::<AgentState>::new();

graph
    .add_node("agent", agent_node)
    .add_node("tools", tool_node)
    .set_entry_point("agent")
    .add_conditional_edges(
        "agent",
        |s: &AgentState| {
            if s.messages.last().map_or(false, |m| m.has_tool_calls()) {
                "tools"
            } else {
                END
            }
        },
        path_map! {
            "tools" => "tools",
            END => END,
        },
    )
    .add_edge("tools", "agent");

let app = graph.compile(MemorySaver::new())?;
```

---

## 2. Node 系统

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/_node.py` — Node 包装逻辑
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:586` — add_node() 方法（主重载）
> 源码位置: `langgraph/libs/langgraph/langgraph/pregel/_read.py` — PregelNode（编译后节点表示）

### 2.1 Node Trait

```rust
// juncture-core/src/node/mod.rs

pub trait Node<S: State>: Send + Sync + 'static {
    /// 执行节点逻辑。接收 state 的不可变快照，返回 Command。
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<Command<S>, JunctureError>>;

    /// 节点名称（用于日志、tracing、错误信息）。
    fn name(&self) -> &str;
}
```

### 2.2 IntoNode Trait 与 Blanket Impl

用户不需要手动实现 `Node` trait。通过 `IntoNode` trait 和 blanket impl，普通 async 函数自动适配为节点。

```rust
// juncture-core/src/node/into_node.rs

pub trait IntoNode<S: State> {
    fn into_node(self, name: &str) -> Arc<dyn Node<S>>;
}
```

支持四种函数签名：

```rust
// 形式 A：最简单，只关心 state，返回 partial update
async fn my_node(state: AgentState) -> Result<AgentStateUpdate> { ... }

// 形式 B：需要 config（thread_id、metadata 等）
async fn my_node(state: AgentState, config: &RunnableConfig) -> Result<AgentStateUpdate> { ... }

// 形式 C：需要控制路由（Command）
async fn my_node(state: AgentState) -> Result<Command<AgentState>> { ... }

// 形式 D：最完整形式，state + config + Command
async fn my_node(state: AgentState, config: &RunnableConfig) -> Result<Command<AgentState>> { ... }
```

Blanket impl 实现策略：

```rust
/// 形式 A: fn(S) -> Result<S::Update>
impl<S, F, Fut> IntoNode<S> for F
where
    S: State,
    F: Fn(S) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<S::Update, JunctureError>> + Send + 'static,
{
    fn into_node(self, name: &str) -> Arc<dyn Node<S>> {
        Arc::new(FnNode {
            name: name.to_string(),
            func: self,
            _phantom: PhantomData,
        })
    }
}

/// 形式 A 的 FnNode 实现将 Update 包装为 Command::update(update)
impl<S, F, Fut> Node<S> for FnNode<S, F, Fut>
where ...
{
    fn call(&self, state: S, _config: &RunnableConfig) -> BoxFuture<'_, Result<Command<S>, JunctureError>> {
        Box::pin(async move {
            let update = (self.func)(state).await?;
            Ok(Command::update(update))
        })
    }
}
```

形式 B/C/D 类似，通过不同的 trait bound 区分。Rust 编译器通过返回类型自动选择正确的 blanket impl。

### 2.3 节点执行语义

- 节点接收 state 的 **克隆**（不可变快照），不存在共享可变状态
- 同一 superstep 内的多个节点各自持有独立的 state 副本
- 节点返回 `Command<S>`，其中包含 partial update 和/或路由指令
- 节点内部可以自由使用 `&mut` 操作局部数据，不影响其他节点

### 2.4 节点错误处理器

<!-- Addresses finding: H-07 -->
<!-- Addresses finding: L-2 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:107` — `_NodeDefaults`

LangGraph 支持为每个节点注册错误处理器。当节点执行失败时，错误处理器接收错误信息并可以返回 `Command` 来恢复。

```rust
impl<S: State> StateGraph<S> {
    /// 添加带错误处理器的节点
    ///
    /// 当 node 执行失败时，error_handler 被调用。
    /// error_handler 可以返回 Command 来更新状态或路由，
    /// 从而实现优雅降级而非硬崩溃。
    pub fn add_node_with_error_handler(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        error_handler: impl Fn(NodeError<S>) -> BoxFuture<'_, Result<Command<S>, JunctureError>>
            + Send
            + Sync
            + 'static,
    ) -> &mut Self;

// > **Implementation Note (C-02-8)**: Implementation introduces a `NodeMetadata` structure that
// > consolidates per-node configuration beyond what the design specifies. This includes `defer` flag
// > (for deferred node execution ordering), `metadata` (user-defined key-value pairs for observability),
// > `destinations` (declared routing targets for topology validation), and `retry_policies` (a Vec
// > of RetryPolicy allowing different strategies per error type). This replaces individual parameters
// > in `add_node` with a single structured configuration object.
}

/// 节点错误信息
pub struct NodeError<S: State> {
    /// 失败的节点名称
    pub node: String,
    /// 原始错误
    pub error: JunctureError,
    /// 执行时的状态快照
    pub state: S,
    /// 当前执行尝试次数
    pub attempt: u32,
}
```

**使用示例**：

```rust
graph.add_node_with_error_handler(
    "call_api",
    api_node,
    |err: NodeError<AgentState>| {
        Box::pin(async move {
            // 优雅降级：记录错误并返回后备响应
            Ok(Command::update(AgentStateUpdate {
                messages: Some(vec![Message::ai(format!(
                    "API 调用失败: {}，使用缓存结果",
                    err.error
                ))]),
                use_cache: Some(true),
                ..Default::default()
            }))
        })
    },
);
```

**执行引擎集成**（见 03-pregel-engine.md）：
- 节点失败时，引擎将 `__error__` 和 `__error_source_node__` 写入保留通道
- `schedule_error_handler()` 动态创建错误处理器任务
- `_resume_error_handlers_if_applicable()` 在 tick() 循环中检查并调度

**NodeError 扩展说明**：

<!-- Addresses finding: M-5 -->

> 注意：Juncture 扩展了 LangGraph 的 NodeError，增加了 `state` 和 `attempt` 字段以支持更丰富的错误恢复。
> LangGraph 的 NodeError 仅包含 `node` 和 `error` 字段。
> Juncture 额外提供：
> - `state: S` — 执行时的状态快照，允许错误处理器基于状态做决策
> - `attempt: u32` — 当前重试尝试次数，允许错误处理器根据重试次数选择不同策略

> **Implementation Note (C-02-3)**: Implementation uses an `ErrorHandlerNode<S>` wrapper pattern
> that composes the original node with its error handler into a single `Node<S>` implementation.
> This wrapper intercepts errors from the inner node and delegates to the error handler's async
> function, seamlessly integrating error recovery into the Pregel execution pipeline without
> requiring special-case handling in the engine itself.

---

## 3. Edge 系统

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:915` — add_edge() 方法
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:969` — add_conditional_edges() 方法
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/_branch.py` — Branch 条件路由逻辑

### 3.1 内部表示

```rust
// juncture-core/src/edge/mod.rs

pub enum Edge<S: State> {
    /// 静态边：from 完成后，to 在下一 superstep 执行。
    Fixed {
        from: String,
        to: String,
    },

    /// 条件边：from 完成后，调用 router 决定目标。
    Conditional {
        from: String,
        router: Arc<dyn Router<S>>,
        path_map: PathMap,
    },
}

/// 路由器 trait，支持同步和异步两种形式。
pub trait Router<S: State>: Send + Sync + 'static {
    fn route(&self, state: &S) -> BoxFuture<'_, Result<RouteResult, JunctureError>>;
}

/// 路由结果
pub enum RouteResult {
    /// 路由到单个节点
    One(String),
    /// 路由到多个节点（并行执行）
    Multiple(Vec<String>),
}
```

### 3.2 同步路由函数的 blanket impl

```rust
/// 同步闭包自动实现 Router
impl<S, F> Router<S> for F
where
    S: State,
    F: Fn(&S) -> &str + Send + Sync + 'static,
{
    fn route(&self, state: &S) -> BoxFuture<'_, Result<RouteResult, JunctureError>> {
        Box::pin(async move {
            Ok(RouteResult::One((self)(state).to_string()))
        })
    }
}
```

### 3.3 多出边 = 并行执行

一个节点可以有多条出边。所有出边的目标节点在下一 superstep 中并行执行：

```rust
graph
    .add_edge("start", "fetch_data")
    .add_edge("start", "fetch_config");
// fetch_data 和 fetch_config 在同一 superstep 并行执行
```

### 3.4 START 和 END 哨兵

```rust
pub const START: &str = "__start__";
pub const END: &str = "__end__";
```

- `START` 是虚拟入口节点，`set_entry_point("x")` 等价于 `add_edge(START, "x")`
- `END` 是虚拟终止节点，路由到 `END` 表示该路径执行完毕
- 所有活跃路径都到达 `END` 时，图执行结束

### 3.5 边的内部触发机制

编译阶段，边被转换为触发关系表：

```rust
/// 编译后的触发关系
struct TriggerTable {
    /// node_name → 该节点完成后需要评估的边
    outgoing: HashMap<String, Vec<CompiledEdge>>,
    /// node_name → 触发该节点执行的条件
    incoming: HashMap<String, Vec<TriggerSource>>,
}

enum CompiledEdge {
    Fixed { target: String },
    Conditional { router: Arc<dyn Router<S>>, path_map: PathMap },
}

enum TriggerSource {
    Edge { from: String },
    Send { from: String },
}
```

---

## 3.5 Runtime 与上下文注入

<!-- Addresses finding: C-04 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/runtime.py:124` — `Runtime` 类
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:211` — `context_schema` 参数

LangGraph 提供 `Runtime[ContextT]` 对象，将运行时依赖（上下文、Store、流写入器等）注入到节点中，独立于状态管理。

### Runtime 结构

```rust
// juncture-core/src/runtime.rs

/// 运行时上下文，通过 context_schema 参数注入到节点
///
/// 与 State 分离的依赖注入机制。State 管理"图在做什么"，
/// Runtime 提供"节点需要什么外部资源"。
pub struct Runtime<C: Clone + Send + Sync + 'static = ()> {
    /// 不可变运行上下文（user_id, db_conn, 配置等）
    pub context: C,

    /// 跨线程持久化存储（见 10-store.md）
    pub store: Option<Arc<dyn Store>>,

    /// 自定义流写入器
    pub stream_writer: StreamWriter,

    /// 心跳信号（防止 idle timeout 误判）
    pub heartbeat: Heartbeat,
    // <!-- Addresses finding: M-06 -->

    /// 上一次执行的返回值（仅 Functional API）
    pub previous: Option<serde_json::Value>,
    // <!-- Addresses finding: M-07 -->

    /// 执行元信息
    pub execution_info: Option<ExecutionInfo>,
    // <!-- Addresses finding: H-13, L-01 -->

    /// 协作式排空控制
    pub control: Option<RunControl>,
    // <!-- Addresses finding: H-12 -->
}
```

### context_schema 参数

```rust
impl<S: State, I: IntoState<S>, O: FromState<S>> StateGraph<S, I, O> {
    /// 设置上下文 Schema 类型
    /// 节点函数签名中包含 Runtime<C> 参数时，自动注入
    pub fn with_context_schema<C: Clone + Send + Sync + 'static>(self) -> StateGraph<S, I, O, C> {
        // ...
    }
}
```

### 节点函数签名扩展

添加 Runtime 注入后，`IntoNode` blanket impl 额外支持以下签名：

```rust
// 形式 E：需要 Runtime 上下文
async fn my_node(state: S, runtime: &Runtime<MyContext>) -> Result<Command<S>> { ... }

// 形式 F：state + config + runtime
async fn my_node(state: S, config: &RunnableConfig, runtime: &Runtime<MyContext>) -> Result<Command<S>> { ... }
```

### ExecutionInfo

<!-- Addresses finding: H-13, L-01 -->

```rust
/// 只读执行元信息
///
/// 提供当前执行的 checkpoint、task、thread 等标识信息，
/// 以及节点重试计数（用于 RetryPolicy 场景）
pub struct ExecutionInfo {
    /// 当前 checkpoint ID
    pub checkpoint_id: String,
    /// 当前 checkpoint 命名空间（子图隔离）
    pub checkpoint_ns: String,
    /// 当前任务 ID
    pub task_id: String,
    /// 当前线程 ID（无 checkpointer 时为 None）
    pub thread_id: Option<String>,
    /// 当前运行 ID
    pub run_id: Option<String>,
    /// 当前节点执行尝试次数（1-indexed）
    /// <!-- Addresses finding: L-01 -->
    pub node_attempt: u32,
    /// 首次尝试的 Unix 时间戳（秒）
    /// <!-- Addresses finding: L-01 -->
    pub node_first_attempt_time: Option<f64>,
}
```

### Managed Values（IsLastStep, RemainingSteps）

<!-- Addresses finding: H-04 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/managed/is_last_step.py`

```rust
/// 托管值：通过 Runtime 注入的自动计算值
///
/// 节点可以检测递归限制是否即将达到，从而优雅降级
pub struct ManagedValues {
    /// 是否为最后一步（当前 step == recursion_limit）
    pub is_last_step: bool,

    /// 剩余可用步数
    pub remaining_steps: u32,
}

impl<C: Clone + Send + Sync + 'static> Runtime<C> {
    /// 获取托管值
    pub fn managed_values(&self) -> ManagedValues {
        let limit = self.execution_info.as_ref()
            .and_then(|info| /* 获取 recursion_limit */ None)
            .unwrap_or(25);
        let current_step = self.execution_info.as_ref()
            .map(|info| /* 获取当前 step */ 0)
            .unwrap_or(0);
        let remaining = limit.saturating_sub(current_step);

        ManagedValues {
            is_last_step: remaining <= 1,
            remaining_steps: remaining as u32,
        }
    }
}

// 使用示例
async fn agent_node(state: AgentState, runtime: &Runtime<()>) -> Result<Command<AgentState>> {
    if runtime.managed().remaining_steps <= 1 {
        // 即将达到递归限制，生成总结而非继续
        return Ok(Command::update(AgentStateUpdate {
            messages: Some(vec![Message::ai("I've reached my step limit. Here's a summary...")]),
            ..Default::default()
        }));
    }
    // 正常执行...
}
```

### RunControl（协作式排空）

<!-- Addresses finding: H-12 -->

```rust
/// 运行级控制面：用于协作式优雅关闭
///
/// 生产环境中 SIGTERM 处理器调用 request_drain()，
/// 图在下一个 superstep 边界保存 checkpoint 后停止，
/// 后续可从 checkpoint 恢复执行
pub struct RunControl {
    drain_reason: Mutex<Option<String>>,
}

impl RunControl {
    /// 请求在下一个 superstep 边界排空
    /// checkpoint 会在排空前保存，确保可恢复
    pub fn request_drain(&self, reason: &str) {
        *self.drain_reason.lock().unwrap() = Some(reason.to_string());
    }

    pub fn drain_requested(&self) -> bool {
        self.drain_reason.lock().unwrap().is_some()
    }
}
```

### 保留写入键

<!-- Addresses finding: Part3#12 -->

> 源码位置: `langgraph/libs/langgraph/_internal/_constants.py` — 保留键常量

图引擎内部使用以下保留键，用户不可在 update 中使用：

| 保留键 | 用途 |
|---|---|
| `__input__` | 图输入值的写入通道 |
| `__interrupt__` | 节点产生的动态中断信号 |
| `__resume__` | HITL 恢复时传入的值 |
| `__error__` | 节点错误值（error handler 场景） |
| `__error_source_node__` | 错误来源节点名 |
| `__no_writes__` | 节点未写入任何值的标记 |
| `__pregel_tasks` | Send 对象写入通道 |
| `__return__` | 记录 task 返回值 |
| `__previous__` | 上一次执行的返回值 |

---

## 4. Command 原语

> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:749` — Command 类定义
> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:654` — Send 类定义
> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:801` — interrupt() 函数

### 4.1 设计动机

LangGraph Python 引入 `Command` 作为节点返回值的统一抽象，取代了早期分散的 `NodeOutput` 枚举。Command 将以下关注点合并为一个类型：

- **状态更新**：partial update 应用到 state
- **路由控制**：决定下一步执行哪些节点
- **父图导航**：子图节点可以向父图发送路由指令
- **Send（动态 fan-out）**：运行时决定派发多少个并行任务

### 4.2 类型定义

```rust
// juncture-core/src/command.rs

/// 节点的统一返回类型。
pub struct Command<S: State> {
    /// 状态更新（None 表示不更新）
    pub update: Option<S::Update>,
    /// 路由指令（None 表示使用外部边决定）
    pub goto: Option<Goto>,
    /// 目标图（默认 Current）
    pub graph: GraphTarget,
}

/// 路由指令
/// <!-- Addresses finding: M-3 -->
/// 注意：Goto 不再是泛型类型。Send 目标使用动态序列化状态。
/// 这简化了类型签名和 proc-macro 生成代码。
pub enum Goto {
    /// 不显式路由，使用图的外部边定义
    /// 实现添加：比 Option<Goto> 更清晰地表达"使用默认边"语义
    None,
    /// 路由到单个节点
    Next(String),
    /// 路由到多个节点（并行）
    Multiple(Vec<String>),
    /// 动态 fan-out：每个 SendTarget 在下一 superstep 独立执行
    Send(Vec<SendTarget>),

    // > **实现备注 (D-03-9)**: `Goto::Send` 已完全实现。当节点返回包含 `Send` targets 的 Command 时，
    // > Pregel 引擎为每个 send target 创建独立的 `PendingTask` 条目（trigger 为 `Push { index }`），
    // > 并附带 JSON 序列化的 state override。每个 target 在下一 superstep 中作为独立任务并行执行，
    // > 结果通过 reducer 合并回主状态。
    /// 终止当前路径
    End,
}

/// <!-- Addresses finding: M-3 -->
/// Send API 的目标（非泛型版本）
/// state 使用 serde_json::Value 以避免 Goto 的泛型参数
pub struct SendTarget {
    /// 目标节点名
    pub node: String,
    /// 该任务使用的 state（覆盖当前 state）
    pub state: serde_json::Value,
}

// > **Implementation Note (C-02-5)**: `SendTarget` additionally carries a `timeout: Option<Duration>`
// > field, allowing per-send-target timeout configuration. When set, the Pregel engine applies this
// > timeout to the spawned task executing the target node, overriding the graph-level default.
// > This enables fine-grained control over fan-out operations where some targets may be expected
// > to complete faster than others.

/// Send API 的目标
pub struct SendTarget<S: State> {
    /// 目标节点名
    pub node: String,
    /// 该任务使用的 state（覆盖当前 state）
    pub state: S,
}

/// 目标图
pub enum GraphTarget {
    /// 当前图（默认）
    Current,
    /// 父图（子图向上导航）
    Parent,
}

// > **Implementation Note (C-02-7)**: Implementation introduces `ParentCommand<S>` and `CommandGoto`
// > types for structured subgraph-to-parent communication. `ParentCommand` wraps a Command destined
// > for the parent graph, enabling subgraph nodes to route to specific parent nodes or send data
// > upward. `CommandGoto` refines the `Goto` enum with additional routing metadata for multi-level
// > graph hierarchies. These types ensure type-safe inter-graph routing that the design's simpler
// > `GraphTarget::Parent` variant does not fully capture.
```

### 4.3 构造方法

```rust
impl<S: State> Command<S> {
    /// 只更新状态，路由由外部边决定
    pub fn update(update: S::Update) -> Self {
        Self { update: Some(update), goto: None, graph: GraphTarget::Current }
    }

    /// 只路由，不更新状态
    pub fn goto(target: impl Into<String>) -> Self {
        Self { update: None, goto: Some(Goto::Next(target.into())), graph: GraphTarget::Current }
    }

    /// 更新状态 + 路由
    pub fn update_and_goto(update: S::Update, target: impl Into<String>) -> Self {
        Self {
            update: Some(update),
            goto: Some(Goto::Next(target.into())),
            graph: GraphTarget::Current,
        }
    }

    /// 动态 fan-out
    pub fn send(targets: Vec<SendTarget<S>>) -> Self {
        Self { update: None, goto: Some(Goto::Send(targets)), graph: GraphTarget::Current }
    }

    /// 更新状态 + fan-out
    pub fn update_and_send(update: S::Update, targets: Vec<SendTarget<S>>) -> Self {
        Self {
            update: Some(update),
            goto: Some(Goto::Send(targets)),
            graph: GraphTarget::Current,
        }
    }

    /// 终止当前路径
    pub fn end() -> Self {
        Self { update: None, goto: Some(Goto::End), graph: GraphTarget::Current }
    }

    /// 子图向父图发送路由指令
    pub fn goto_parent(target: impl Into<String>) -> Self {
        Self { update: None, goto: Some(Goto::Next(target.into())), graph: GraphTarget::Parent }
    }
}
```

### 4.4 Command 与外部边的交互

当节点返回的 Command 包含 `goto` 时：
- `goto` 指定的目标节点 **替代** 外部边的路由结果
- 如果 `goto` 为 `None`，则使用外部边（Fixed 或 Conditional）决定下一步
- 如果节点既有外部边又返回了 `goto`，以 `goto` 为准

### 4.5 使用示例

```rust
// 简单更新（形式 A 的 blanket impl 自动包装为 Command::update）
async fn counter_node(state: MyState) -> Result<MyStateUpdate> {
    Ok(MyStateUpdate { count: Some(state.count + 1), ..Default::default() })
}

// 条件路由（形式 C）
async fn router_node(state: AgentState) -> Result<Command<AgentState>> {
    if state.messages.last().map_or(false, |m| m.has_tool_calls()) {
        Ok(Command::goto("tools"))
    } else {
        Ok(Command::end())
    }
}

// 动态 fan-out（形式 C）
async fn distribute(state: WorkflowState) -> Result<Command<WorkflowState>> {
    let targets = state.tasks.iter()
        .map(|task| SendTarget {
            node: "worker".to_string(),
            state: WorkflowState { tasks: vec![task.clone()], results: vec![] },
        })
        .collect();
    Ok(Command::send(targets))
}

// 更新 + 路由（形式 D）
async fn review_node(
    state: AgentState,
    config: &RunnableConfig,
) -> Result<Command<AgentState>> {
    let update = AgentStateUpdate {
        messages: Some(vec![Message::ai("Review complete")]),
        ..Default::default()
    };
    Ok(Command::update_and_goto(update, "publish"))
}
```

---

## 5. 编译与拓扑验证

> 源码位置: `langgraph/libs/langgraph/langgraph/pregel/_validate.py` — 图验证逻辑
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1164` — compile() 入口

### 5.1 验证流程

`StateGraph::compile()` 按以下顺序执行验证：

```
1. 检查 entry_point 是否已设置
2. 检查所有边的 from/to 是否引用已注册的节点
3. 检查条件边 path_map 中的目标是否都是已注册节点或 END
4. 可达性分析：从 START 出发 BFS，标记所有可达节点
5. 孤立节点检测：未被任何边引用的节点
6. 不可达节点检测：从 START 无法到达的节点
7. 潜在无限循环检测：SCC 分析 + 终止条件检查
```

### 5.2 TopologyError

```rust
// juncture-core/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum TopologyError {
    #[error("节点 '{name}' 已存在")]
    DuplicateNode { name: String },

    #[error("未设置入口节点（entry_point）")]
    NoEntryPoint,

    #[error("边引用了不存在的节点 '{name}'")]
    NodeNotFound { name: String },

    #[error("条件边 '{from}' 的分支 '{branch}' 指向不存在的节点 '{target}'")]
    EdgeTargetNotFound { from: String, branch: String, target: String },

    #[error("节点 '{name}' 没有任何入边或出边（孤立节点）")]
    IsolatedNode { name: String },

    #[error("从入口节点无法到达节点 '{name}'")]
    UnreachableNode { name: String },

    #[error("检测到潜在无限循环，路径: {cycle:?}")]
    PotentialInfiniteLoop { cycle: Vec<String> },
}
```

### 5.3 循环检测策略

图中的循环不一定是错误（ReAct agent 就是 agent → tools → agent 循环）。检测策略：

1. 使用 Tarjan 算法找到所有强连通分量（SCC）
2. 对每个 SCC，检查是否存在终止条件：
   - SCC 内至少有一个节点有通向 SCC 外部（或 END）的条件边
   - 如果 SCC 内所有边都是 Fixed 且无出口 → 报告 `PotentialInfiniteLoop`
3. 有条件出口的循环视为合法（依赖 recursion_limit 兜底）

---

## 6. CompiledGraph

### 6.1 结构

```rust
// juncture-core/src/graph/compiled.rs

/// CompiledGraph 是 Clone + Send + Sync，可安全在多个 tokio task 间共享。
#[derive(Clone)]
pub struct CompiledGraph<S: State> {
    inner: Arc<CompiledGraphInner<S>>,
}

struct CompiledGraphInner<S: State> {
    /// 节点注册表（保持注册顺序）
    nodes: IndexMap<String, Arc<dyn Node<S>>>,
    /// 编译后的触发关系表
    trigger_table: TriggerTable<S>,
    /// Checkpoint 存储
    checkpointer: Arc<dyn CheckpointSaver>,
    /// 图元数据（用于导出）
    metadata: GraphMetadata,
}
```

### 6.2 执行方法

<!-- Addresses finding: H-11 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:359` — `GraphOutput`

```rust
/// invoke() 的返回类型，包含执行结果和元信息
///
/// 与直接返回 S 不同，GraphOutput 允许调用者了解执行过程中
/// 是否发生了中断，无需切换到流式模式
pub struct GraphOutput<S: State> {
    /// 最终输出状态（经过 output_schema 过滤）
    pub value: S,

    /// 执行过程中发生的中断（如果有）
    /// 非空表示图在某个节点被中断，未完全执行完毕
    pub interrupts: Vec<InterruptInfo>,

    /// 执行元数据
    pub metadata: GraphOutputMetadata,
}

/// 中断信息
pub struct InterruptInfo {
    /// 被中断的节点名
    pub node: String,
    /// 中断携带的值
    pub value: serde_json::Value,
    /// 中断 ID（用于命名中断匹配）
    pub id: Option<String>,
}

/// 输出元数据
pub struct GraphOutputMetadata {
    /// 总执行步数
    pub steps: usize,
    /// 最终 checkpoint ID
    pub checkpoint_id: Option<String>,
    /// 预算使用情况
    pub budget_usage: Option<BudgetUsage>,
}
```
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 同步执行：运行图直到完成，返回 GraphOutput。
    /// <!-- Addresses finding: M-12 -->
    /// context: 可选的每次调用上下文（覆盖编译时配置的 context_schema）。
    pub async fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
        context: Option<serde_json::Value>,
    ) -> Result<GraphOutput<S>, JunctureError>;

    /// 流式执行：返回 StreamEvent 流。
    /// <!-- Addresses finding: M-12 -->
    /// context: 可选的每次调用上下文。
    pub async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
        context: Option<serde_json::Value>,
    ) -> Result<impl Stream<Item = Result<StreamEvent<S>, JunctureError>>, JunctureError>;

    /// 从中断点恢复执行。
    pub async fn resume(
        &self,
        resume_value: serde_json::Value,
        config: &RunnableConfig,
    ) -> Result<S, JunctureError>;
}
```

### 6.3 状态检查方法

```rust
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 获取当前 thread 的最新状态快照。
    pub async fn get_state(
        &self,
        config: &RunnableConfig,
    ) -> Result<StateSnapshot<S>, JunctureError>;

    /// 获取 thread 的完整状态历史（最新在前）。
    pub async fn get_state_history(
        &self,
        config: &RunnableConfig,
    ) -> Result<Vec<StateSnapshot<S>>, JunctureError>;

    /// 手动更新状态（创建新 checkpoint，经过 reducer）。
    /// as_node: 模拟哪个节点产生的更新（影响触发关系）。
    pub async fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> Result<RunnableConfig, JunctureError>;

    /// <!-- Addresses finding: H-11 -->
    /// 批量更新状态（原子操作，创建单个 checkpoint）。
    /// 所有 updates 按顺序应用，as_node 指定每个 update 的来源节点。
    pub async fn bulk_update_state(
        &self,
        updates: Vec<StateUpdate<S>>,
    ) -> Result<RunnableConfig, JunctureError>;
}

/// 单个状态更新操作
pub struct StateUpdate<S: State> {
    /// 更新值
    pub update: S::Update,
    /// 模拟的来源节点
    pub as_node: Option<String>,
}

/// <!-- Addresses finding: M-9 -->
/// 状态历史查询过滤器
pub struct StateFilter {
    /// 只返回指定 source 的 checkpoint
    pub source: Option<CheckpointSource>,
    /// 实现添加：基于 step 的过滤（比 source 更直观）
    pub after_step: Option<usize>,
    pub before_step: Option<usize>,
    /// 最大返回数量
    pub limit: Option<usize>,
}

impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 获取 thread 的完整状态历史（支持过滤和分页）。
    /// <!-- Addresses finding: M-9 -->
    pub async fn get_state_history(
        &self,
        config: &RunnableConfig,
        filter: Option<StateFilter>,
        before: Option<RunnableConfig>,
        limit: Option<usize>,
    ) -> Result<Vec<StateSnapshot<S>>, JunctureError>;

    /// <!-- Addresses finding: M-10 -->
    /// 获取当前状态（可选择是否展开子图状态）。
    pub async fn get_state(
        &self,
        config: &RunnableConfig,
        subgraphs: bool,
    ) -> Result<StateSnapshot<S>, JunctureError>;

    /// <!-- Addresses finding: M-11 -->
    /// 获取图的可视化表示（用于调试和文档生成）。
    /// xray: 可选深度，控制子图展开层级（None = 不展开，Some(0) = 一级子图）
    pub fn get_graph(&self, xray: Option<u32>) -> DrawableGraph;

    /// <!-- Addresses finding: M-11 -->
    /// 获取图中所有子图信息。
    /// namespace: 可选命名空间过滤。recurse: 是否递归展开嵌套子图。
    pub fn get_subgraphs(&self, namespace: Option<&str>, recurse: bool) -> Vec<SubgraphInfo>;
}

/// <!-- Addresses finding: M-11 -->
/// 可绘制的图结构（用于 Mermaid/DOT 导出）
pub struct DrawableGraph {
    pub nodes: Vec<DrawableNode>,
    pub edges: Vec<DrawableEdge>,
}

pub struct DrawableNode {
    pub name: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

pub struct DrawableEdge {
    pub from: String,
    pub to: String,
    pub conditional: bool,
    pub label: Option<String>,
}

/// <!-- Addresses finding: M-11 -->
/// 子图信息
pub struct SubgraphInfo {
    pub name: String,
    pub namespace: String,
    pub input_schema: String,
    pub output_schema: String,
}
```

### 6.4 图导出

```rust
impl<S: State> CompiledGraph<S> {
    /// 导出为 Mermaid 图表语法。
    pub fn to_mermaid(&self) -> String;

    /// 导出为 Graphviz DOT 格式。
    pub fn to_dot(&self) -> String;

    /// 导出为 JSON 结构（节点、边、元数据）。
    pub fn to_json(&self) -> serde_json::Value;
}
```

### 6.5 Mermaid 导出示例

```rust
let app = graph.compile(MemorySaver::new())?;
println!("{}", app.to_mermaid());
// 输出：
// graph TD
//     __start__ --> agent
//     agent -->|"tools"| tools
//     agent -->|"__end__"| __end__
//     tools --> agent
```

---

## 7. 函数式 API (entrypoint / task)

<!-- Addresses finding: C-02 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/func/__init__.py` — `task()` 和 `entrypoint()` 装饰器

LangGraph 提供基于函数的工作流定义方式，作为 StateGraph 的替代方案。用户无需构建图结构，而是使用普通函数定义工作流。

### 7.1 概念

```
@entrypoint  = 图的入口点（等价于 StateGraph 的 START → ... → END）
@task        = 可独立调度的函数单元（等价于图中的节点）
```

与 StateGraph 的区别：
- 无显式边定义，控制流由函数调用顺序自然决定
- task 支持 retry、cache、timeout 等策略
- 更接近普通函数编程，降低学习曲线

### 7.2 Juncture 适配设计

```rust
// juncture-core/src/func/mod.rs

/// Task 函数属性
pub struct TaskConfig {
    /// 重试策略（见 03-pregel-engine.md RetryPolicy）
    pub retry_policy: Option<RetryPolicy>,
    /// 缓存策略
    pub cache_policy: Option<CachePolicy>,
    /// 超时策略
    pub timeout: Option<TimeoutPolicy>,
    /// 任务名称（默认使用函数名）
    pub name: Option<String>,
}

/// entrypoint 函数属性
pub struct EntrypointConfig {
    /// Checkpoint 存储器
    pub checkpointer: Option<Arc<dyn CheckpointSaver>>,
    /// Store（跨线程存储）
    pub store: Option<Arc<dyn Store>>,
}
```

### 7.3 使用示例

```rust
use juncture::func::{entrypoint, task, Runtime};

/// 定义可重试的子任务
#[task(retry = RetryPolicy::max_attempts(3))]
async fn fetch_url(url: String) -> Result<String> {
    // HTTP 请求，可能失败需要重试
    reqwest::get(&url).await?.text().await
}

/// 定义带缓存的子任务
#[task(cache = CachePolicy::ttl(Duration::from_secs(300)))]
async fn analyze(text: String) -> Result<Analysis> {
    // 昂贵的 LLM 调用，结果可缓存
    llm.analyze(&text).await
}

/// 入口点：定义完整工作流
#[entrypoint(checkpointer = MemorySaver::new())]
async fn my_workflow(input: WorkflowInput, runtime: &Runtime<()>) -> Result<WorkflowOutput> {
    // 顺序执行
    let content = fetch_url(input.url.clone()).await?;
    let analysis = analyze(content).await?;

    Ok(WorkflowOutput { analysis })
}

// 编译并执行
let workflow = my_workflow.compile();
let result = workflow.invoke(WorkflowInput { url: "https://...".into() }, &config).await?;
```

### 7.4 内部实现

`entrypoint` 和 `task` 宏展开为等价的 StateGraph：

```rust
// #[entrypoint] 展开为：
let graph = StateGraph::<WorkflowState>::new()
    .add_node("__entrypoint__", entrypoint_fn)
    .set_entry_point("__entrypoint__")
    .set_finish_point("__entrypoint__")
    .compile(checkpointer);

// #[task] 在 entrypoint 内部展开为带 retry/cache/timeout 配置的节点调用
```

### 7.5 previous 状态访问

<!-- Addresses finding: M-07 -->

entrypoint 函数通过 Runtime 可以访问上一次执行的返回值，用于累积模式：

```rust
#[entrypoint(checkpointer = MemorySaver::new())]
async fn accumulating_workflow(
    input: Input,
    runtime: &Runtime<()>,
) -> Result<Output> {
    // 获取上一次的返回值
    let previous: Option<Output> = runtime.previous.clone()?;

    let mut result = process(input);
    if let Some(prev) = previous {
        result = merge(prev, result);
    }
    Ok(result)
}
```

### 7.6 entrypoint.final：区分返回值与保存值

<!-- Addresses finding: H-10 -->

> 参考: `langgraph/libs/langgraph/langgraph/func/__init__.py` — Final 类型

entrypoint 函数的返回值有两种用途：返回给调用者和保存到 checkpoint。默认情况下两者相同，但 `Final<V, S>` 允许区分：

```rust
/// Final 包装：区分返回给调用者的值和保存到 checkpoint 的值
///
/// - V: 返回给调用者的值（可以是 Output Schema 类型）
/// - S: 保存到 checkpoint 的值（必须是 State 类型或其 Update）
pub struct Final<V, S> {
    /// 返回给调用者的值
    pub value: V,
    /// 保存到 checkpoint 的值
    pub save: S,
}

/// 使用示例
#[entrypoint(checkpointer = MemorySaver::new())]
async fn my_workflow(
    input: Input,
    runtime: &Runtime<()>,
) -> Result<Final<Output, WorkflowState>> {
    let result = process(input).await?;

    Ok(Final {
        value: Output { summary: result.summary() },
        save: WorkflowState { data: result.full_data, ..Default::default() },
    })
}
```

当 entrypoint 返回 `Final<V, S>` 时：
- `value` 通过 output_schema 过滤后返回给 `invoke()` / `stream()` 的调用者
- `save` 写入 checkpoint 作为下一次执行的起点

---

## 8. RunnableConfig

```rust
// juncture-core/src/config.rs

#[derive(Clone, Debug, Default)]
pub struct RunnableConfig {
    /// 对话/任务标识。同一 thread_id 共享 checkpoint 历史。
    pub thread_id: Option<String>,
    /// 指定从哪个历史 checkpoint 恢复（time-travel）。
    pub checkpoint_id: Option<String>,
    /// 最大 superstep 数（默认 25）。
    pub recursion_limit: usize,
    /// 取消令牌。
    pub cancellation_token: Option<CancellationToken>,
    /// 预算配置。
    pub budget: Option<BudgetConfig>,
    /// <!-- Addresses finding: L-4 -->
    /// 持久化模式（可选，覆盖 checkpointer 默认行为）。
    pub durability: Option<Durability>,
    /// <!-- Addresses finding: L-5 -->
    /// 节点完成回调（每个节点执行完成后调用，用于进度报告）。
    pub node_finished_callback: Option<Arc<dyn Fn(&str, Duration) + Send + Sync>>,
    /// 用户自定义元数据（传递到 tracing span）。
    pub metadata: HashMap<String, serde_json::Value>,
    /// 标签（用于 streaming 过滤）。
    pub tags: Vec<String>,
    /// HITL resume 时携带的人类输入（内部使用）。
    pub(crate) resume_value: Option<serde_json::Value>,
    /// <!-- Addresses finding: M-16 -->
    /// 运行名称（用于日志和可观测性标识）。
    pub run_name: Option<String>,
    /// <!-- Addresses finding: H-4 -->
    /// 缓存配置（用于节点级缓存）。
    pub cache: Option<CacheConfig>,
    /// <!-- Addresses finding: M-2 -->
    /// Checkpoint 命名空间（子图隔离）。
    pub checkpoint_ns: Option<String>,
    /// 实现添加：HITL 中断控制（指定节点执行前/后触发中断）
    pub interrupt_before: Option<Vec<String>>,
    pub interrupt_after: Option<Vec<String>>,
}

/// <!-- Addresses finding: H-4 -->
/// 缓存配置（RunnableConfig 中的 cache 字段）
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// 缓存策略（默认、TTL、自定义键等）
    pub policy: CachePolicy,
    /// 缓存键（可选，覆盖默认键生成）
    pub key: Option<String>,
}

impl RunnableConfig {
    pub fn new() -> Self { Self { recursion_limit: 25, ..Default::default() } }
    pub fn with_thread_id(mut self, id: impl Into<String>) -> Self { ... }
    pub fn with_checkpoint_id(mut self, id: impl Into<String>) -> Self { ... }
    pub fn with_recursion_limit(mut self, limit: usize) -> Self { ... }
    pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self { ... }
    pub fn with_budget(mut self, budget: BudgetConfig) -> Self { ... }
}

/// <!-- Addresses finding: H-3 -->
/// 缓存策略配置
///
/// 定义节点结果的缓存行为，支持自定义键生成。
#[derive(Clone, Debug)]
pub struct CachePolicy {
    /// 键生成函数（基于 state 和 config 生成缓存键）。
    /// None 表示使用默认键生成策略（state 的 hash）。
    pub key_func: Option<Arc<dyn Fn(&serde_json::Value, &RunnableConfig) -> String + Send + Sync>>,
    
    /// TTL（过期时间）。
    pub ttl: Option<Duration>,
    
    /// 最大缓存条目数（LRU 淘汰）。
    pub max_entries: Option<usize>,
}

impl CachePolicy {
    /// 默认缓存策略（基于 state hash 的键，无 TTL）。
    pub fn default_policy() -> Self {
        Self {
            key_func: None,
            ttl: None,
            max_entries: None,
        }
    }
    
    /// TTL 缓存策略。
    pub fn ttl(duration: Duration) -> Self {
        Self {
            key_func: None,
            ttl: Some(duration),
            max_entries: None,
        }
    }
    
    /// 自定义键生成策略。
    pub fn custom_key<F>(key_func: F) -> Self 
    where 
        F: Fn(&serde_json::Value, &RunnableConfig) -> String + Send + Sync + 'static
    {
        Self {
            key_func: Some(Arc::new(key_func)),
            ttl: None,
            max_entries: None,
        }
    }
}
```

---

## 8. 设计决策记录

### 为什么用 Command 替代 NodeOutput 枚举

初始设计使用 `NodeOutput` 枚举（Update / Interrupt / Send / UpdateAndSend）。问题：

1. 枚举变体组合爆炸：Update+Send、Update+Goto、Goto+Send...
2. 无法表达"更新状态 + 路由到父图"这类组合
3. 与 LangGraph Python 的 Command 语义不对齐

Command 是一个 struct，各字段独立组合，天然支持任意组合。

### 为什么路由同时支持外部边和 Command.goto

- 外部边：声明式，可在编译期验证，可导出为图表
- Command.goto：命令式，运行时动态决定，适合复杂条件

两者共存的规则：如果节点返回了 `goto`，忽略外部边；否则使用外部边。这与 LangGraph Python 的行为一致。

### 为什么 Node trait 的 call 接收 owned S 而不是 &S

- 节点可能需要对 state 做大量计算（排序、过滤、转换），owned 避免不必要的 clone
- 执行引擎已经为每个节点 clone 了 state，传 owned 不增加额外开销
- 与 cognis 的 `&S` 方案相比，owned 更符合 Rust 的所有权语义，避免生命周期复杂性

---

## 源码参考索引

| LangGraph 源码路径 | 说明 |
|---|---|
| `langgraph/libs/langgraph/langgraph/graph/state.py:130` | StateGraph 类定义 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:586` | add_node() 方法（主重载） |
| `langgraph/libs/langgraph/langgraph/graph/state.py:915` | add_edge() 方法 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:969` | add_conditional_edges() 方法 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:1164` | compile() 方法 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:1431` | attach_node() — 编译阶段节点注册 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:1537` | attach_edge() — 编译阶段边注册 |
| `langgraph/libs/langgraph/langgraph/graph/_node.py` | Node 包装逻辑 |
| `langgraph/libs/langgraph/langgraph/graph/_branch.py` | Branch 条件路由逻辑 |
| `langgraph/libs/langgraph/langgraph/pregel/_read.py` | PregelNode — 编译后节点表示 |
| `langgraph/libs/langgraph/langgraph/pregel/_validate.py` | 图验证逻辑 |
| `langgraph/libs/langgraph/langgraph/types.py:749` | Command 类定义 |
| `langgraph/libs/langgraph/langgraph/types.py:654` | Send 类定义 |
| `langgraph/libs/langgraph/langgraph/types.py:801` | interrupt() 函数 |
| `langgraph-doc/graph-api.md` | Graph API 官方文档 |

---

## 附录 B: RemoteGraph（跨进程图调用）

<!-- Addresses finding: H-06 -->

> 参考: `langgraph/pregel/remote.py` — RemoteGraph 类

RemoteGraph 允许连接到部署在远程服务器上的图，像调用本地图一样调用它。
支持所有 stream mode 和 state 操作。这是多服务架构中图组合的基础。

### B.1 使用场景

- 微服务架构中，不同团队维护不同的图服务
- 将 CPU 密集型图部署到专用节点
- 跨语言调用（Python LangGraph Server <-> Rust Juncture Client）
- 灰度发布：新版本图先部署为远程服务，通过路由切换流量

### B.2 RemoteGraph 设计

```rust
/// 远程图客户端
/// 通过 HTTP/gRPC 连接到远程图服务，实现与 CompiledGraph 相同的接口
pub struct RemoteGraph<S: State> {
    /// 远程服务端点
    endpoint: String,
    /// 图 ID（多图部署时区分不同图）
    graph_id: Option<String>,
    /// HTTP 客户端
    client: reqwest::Client,
    /// 反序列化标记
    _marker: PhantomData<S>,
}
```

### B.3 PregelProtocol 实现

RemoteGraph 实现 `PregelProtocol` trait（定义见 `03-pregel-engine.md`），提供与本地图一致的 API：

```rust
#[async_trait]
impl<S: State + Serialize + DeserializeOwned> PregelProtocol<S> for RemoteGraph<S> {
    async fn invoke(&self, input: S, config: &RunnableConfig) -> Result<S, JunctureError> {
        let response = self.client
            .post(&format!("{}/invoke", self.endpoint))
            .json(&RemoteRequest {
                input: &input,
                config: config,
                stream_mode: None,
            })
            .send()
            .await
            .map_err(|e| JunctureError::Remote(e.to_string()))?;

        let result: RemoteResponse<S> = response.json().await
            .map_err(|e| JunctureError::Serialize(e))?;
        Ok(result.output)
    }

    async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>, JunctureError> {
        // SSE (Server-Sent Events) 流式连接
        let response = self.client
            .post(&format!("{}/stream", self.endpoint))
            .json(&RemoteRequest {
                input: &input,
                config: config,
                stream_mode: Some(&mode),
            })
            .send()
            .await
            .map_err(|e| JunctureError::Remote(e.to_string()))?;

        // 将 SSE 事件流转换为 StreamEvent<S>
        let stream = parse_sse_stream(response).map(|item| {
            item.map_err(|e| JunctureError::Remote(e.to_string()))
        });

        Ok(Box::pin(stream))
    }

    async fn get_state(&self, config: &RunnableConfig) -> Result<Option<StateSnapshot<S>>, JunctureError> {
        let response = self.client
            .post(&format!("{}/state", self.endpoint))
            .json(&RemoteRequest::<S> { input: &(), config, stream_mode: None })
            .send()
            .await
            .map_err(|e| JunctureError::Remote(e.to_string()))?;

        Ok(response.json().await.ok())
    }

    async fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> Result<RunnableConfig, JunctureError> {
        let response = self.client
            .post(&format!("{}/state/update", self.endpoint))
            .json(&UpdateStateRequest { update: &update, as_node, config })
            .send()
            .await
            .map_err(|e| JunctureError::Remote(e.to_string()))?;

        Ok(response.json().await.map_err(|e| JunctureError::Serialize(e))?)
    }
}
```

### B.4 Wire Protocol

远程通信使用 JSON over HTTP，与 LangGraph Server API 兼容：

```
POST /invoke     → { input, config }     → { output }
POST /stream     → { input, config, stream_mode } → SSE event stream
POST /state      → { config }            → { state_snapshot }
POST /state/update → { config, update, as_node } → { config }
POST /history    → { config, limit }     → { snapshots[] }
```

SSE 事件格式（与 LangGraph Server 一致）：

```
event: values
data: {"state": {...}, "step": 1}

event: updates
data: {"node": "agent", "update": {...}, "step": 1}

event: end
data: {"output": {...}}
```

### B.5 配置

```rust
pub struct RemoteGraphConfig {
    /// 服务端点 URL
    pub endpoint: String,
    /// 图 ID（可选，多图部署时使用）
    pub graph_id: Option<String>,
    /// 认证 token
    pub auth_token: Option<String>,
    /// 请求超时
    pub timeout: Duration,
    /// TLS 配置
    pub tls_config: Option<ClientConfig>,
}

impl RemoteGraphConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            graph_id: None,
            auth_token: None,
            timeout: Duration::from_secs(300),
            tls_config: None,
        }
    }
}
```

### B.6 在图中使用 RemoteGraph

```rust
// 将远程图作为本地节点使用
let remote_agent = RemoteGraph::<MyState>::new(RemoteGraphConfig {
    endpoint: "https://agent-service.internal:8080".into(),
    auth_token: Some("secret".into()),
    ..Default::default()
});

let mut graph = StateGraph::<MyState>::new();
graph.add_node("remote_agent", remote_agent);
graph.add_edge(START, "remote_agent");
```

### B.7 JunctureError 扩展

```rust
// 在 JunctureError 中添加远程调用错误
#[error("远程图调用失败: {0}")]
Remote(String),
```
