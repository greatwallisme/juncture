# Juncture

[English](README.md) | 中文

[LangGraph](https://github.com/langchain-ai/langgraph) 的 Rust 实现，用于构建 LLM 智能体应用的状态机框架。

Juncture 保留了 LangGraph 的核心编程模型 -- `StateGraph` + Pregel 执行引擎 -- 同时借助 Rust 的类型系统实现编译期安全和真正的多核并行。API 设计尽量贴近 LangGraph Python，使熟悉原版的开发者能够直接迁移。

## 致谢

没有 [LangGraph](https://github.com/langchain-ai/langgraph) 就没有这个项目。Juncture 的编程模型、执行语义和 API 设计均源自 LangGraph 的架构。衷心感谢 LangChain 团队在图驱动智能体编排方面的开创性工作。

在开发过程中，还学习和参考了 Rust 社区中同类项目的实践经验：

| 项目 | 学到了什么 |
|------|-----------|
| [rust-langgraph](https://github.com/lookfirst/rust-langgraph) | 早期验证了 LangGraph 模型可以良好地映射到 Rust trait |
| [oxidizedgraph](https://github.com/nicholasgasior/oxidizedgraph) | 使用 tokio 进行异步图执行的模式 |
| [cognis](https://github.com/cognis/cognis) | 多 crate 工作空间组织智能体框架的方式 |

这些项目各自探索了将 LangGraph 适配到 Rust 时的不同取舍。Juncture 在它们的启发下选择了另一条路：优先保证与 LangGraph Python 的语义等价，而非追求全新的抽象。

## 为什么用 Rust？

性能优势主要来自 Rust 的运行时特性，而非巧妙的工程设计。当节点在 `tokio::spawn` 的 work-stealing 调度器上执行，而不是 Python 的单线程 asyncio 中运行时，并行性是自然而然的结果。

基准测试确实显示出显著差异。以下是摘要（完整方法论和局限性说明见 [`benchmarks/README.md`](benchmarks/README.md)）：

| 场景 | Juncture (Rust) | LangGraph (Python) | 加速比 |
|------|----------------|-------------------|--------|
| 顺序执行 3000 节点 | 16.9 ms | 7,652 ms | 452x |
| 流式 10000 节点 | 142.7 ms | 78,085 ms | 547x |
| 扇出 100 主题 | 1.35 ms | 566 ms | 420x |
| 宽状态 1200 迭代 | 95.4 ms | 3,593 ms | 38x |
| 条件路由 50 分支 | 0.7 ms | 3.9 ms | 5.6x |

**重要说明**：这些数字反映的是空操作节点上的框架开销。实际 LLM 调用占据了绝大部分执行时间，框架开销的差异在实践中可以忽略不计。Rust 在这里的价值更多体现在类型安全、内存效率和部署灵活性上，而非单纯的执行速度。

## 功能特性

### 核心 (juncture-core)

- `#[derive(State)]` -- 编译期类型化状态，支持逐字段 reducer（`replace`、`append`、`ephemeral`、`last_write_wins`、`custom`）
- `StateGraph` 构建器，提供 `add_node`、`add_edge`、`add_conditional_edges`
- Pregel 执行引擎，基于 `tokio::spawn` + `JoinSet` 实现真正的并行执行
- `CowState<S>`（基于 Arc 的写时复制）避免昂贵的状态克隆
- `Command<S>` 用于节点返回路由（goto、resume、父图导航）
- `Send` 用于向并行子图动态扇出
- `interrupt!` 宏实现人机协作（Human-in-the-Loop）工作流
- 9 种流式模式（Values、Updates、Messages、Custom、Debug、Tools、Checkpoints、Tasks、Multi）
- `Store` trait 提供跨线程持久化键值存储
- 每节点可配置 `RetryPolicy` 和 `TimeoutPolicy`
- `Durability` 模式（Sync、Async、Exit）
- `Runtime<C>` 用于上下文注入、心跳和执行信息

### 检查点 (juncture-checkpoint)

- `MemorySaver` 用于开发环境
- `SqliteSaver` 和 `PostgresSaver` 用于生产环境
- 序列化后端：JSON、MessagePack、JSON+、AES-256-GCM 加密
- 增量写入持久化（每个任务完成后执行 `put_writes`）

### LLM 集成 (juncture 门面 crate)

- `ChatModel` trait，内置提供商：OpenAI、Anthropic、Ollama
- `Tool` trait，配合 `ToolNode`、拦截器和转换器
- `create_react_agent()` 工厂函数创建 ReAct 风格智能体
- `AgentMiddleware` 链（循环检测、错误处理）
- `SubagentTool` 和 `AgentRegistry` 实现多智能体委派
- `RetryingModel` 包装器，支持可配置重试策略
- 通过 `schemars` 提取结构化输出
- `CircuitBreaker` 用于提供商健康状态追踪

### 可观测性

**juncture-tracing** -- OpenTelemetry 集成：
- 节点级 span 和 token 使用量指标
- `GraphCallbackHandler` 生命周期回调
- 跨服务 trace context 传播

**juncture-telemetry** -- Langfuse 兼容的内嵌可观测性引擎：
- 一行式初始化：`init().with_store("db").with_langfuse_from_env().with_dashboard(8123).install().await?`
- SQLite 存储 trace/observation/session 数据
- 内嵌 Web 仪表盘（trace 树、observation 详情、成本/token 图表）
- Langfuse 云端导出（自动读取 `LANGFUSE_*` 环境变量）
- Langfuse 兼容 REST API（traces、sessions、stats、ingestion）
- OTLP HTTP 接入
- 通过嵌套观测树实现多 agent 追踪
- RAII 自动 flush + 信号处理优雅关闭

| 本地仪表盘 | Langfuse 云端同步 |
|:---:|:---:|
| ![本地仪表盘](asset/local-dashboard-1.png) | ![Langfuse 云端](asset/langfuse.png) |
| ![Trace 详情](asset/local-dashboard-2.png) | |

### WASM 支持

- 浏览器（`wasm32-unknown-unknown`）通过 `wasm-bindgen`
- 边缘 CLI（`wasm32-wasip1`）通过 WASI
- 边缘 HTTP 服务通过 Fermyon Spin
- Feature 门控：`wasm` feature flag、`web-time` 替代 `Instant`、`getrandom` 启用 `wasm_js`

## 快速开始

在 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
juncture = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde = { version = "1", features = ["derive"] }
```

### 基本示例

```rust
use juncture::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct MyState {
    #[reducer(replace)]
    count: i32,
    #[reducer(append)]
    history: Vec<String>,
}

async fn increment(state: &MyState) -> Result<MyState::Update> {
    Ok(MyStateUpdate {
        count: Some(state.count + 1),
        history: Some(vec![format!("count -> {}", state.count + 1)]),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut graph = StateGraph::<MyState>::new();
    graph.add_node("increment", increment);
    graph.add_edge(START, "increment");
    graph.add_edge("increment", END);

    let compiled = graph.compile()?;
    let result = compiled.invoke(MyState { count: 0, history: vec![] }, &RunnableConfig::default()).await?;
    println!("Result: {:?}", result);
    Ok(())
}
```

### 带工具的 ReAct 智能体

```rust
use juncture::prelude::*;
use juncture::tools::Tool;
use juncture::llm::ChatOpenAI;

// 定义工具，创建智能体
let model = ChatOpenAI::new("gpt-4o")?;
let agent = create_react_agent(model, vec![my_tool])?;
let result = agent.invoke(input, &config).await?;
```

更多示例见 [`examples/`](examples/) 目录，包含 15 个渐进式示例，从基础状态机到生产级 LLM 流水线。

## 示例列表

| 编号 | 示例 | 核心概念 |
|------|------|---------|
| 01 | 状态机 | `#[derive(State)]`、线性图、`invoke()` |
| 02 | 计数器 Reducer | `#[reducer(append)]`、`#[reducer(last_write_wins)]` |
| 03 | 条件路由 | `Router` trait、`PathMap`、`add_conditional_edges` |
| 04 | 基础对话 | `MessagesState`、`Message`、`MockChatModel` |
| 05 | 工具调用 | `Tool` trait、`ToolNode`、手动构建智能体图 |
| 06 | 流式输出 | `stream()`、`StreamMode`、`StreamEvent` |
| 07 | 人机协作 | `CompileConfig` 中断、`interrupt_before` |
| 08 | 检查点恢复 | `MemorySaver`、`compile_with_checkpointer()`、thread_id |
| 09 | 错误恢复 | Result 传播、`?` 错误处理 |
| 10 | 基础对话（真实 LLM） | `ChatOpenAI`、单轮/多轮对话 |
| 11 | 流式对话 | `ChatModel::stream`、逐 token 显示 |
| 12 | 工具执行（真实 LLM） | `bind_tools`、工具执行循环 |
| 13 | ReAct 智能体 | `create_react_agent`、天气 + 数学工具 |
| 14 | 多轮对话 | 对话历史累积、系统提示词 |
| 15 | 结构化输出 | `ToolChoice::Required`、JSON 实体提取 |
| 16 | 遥测 | `init()` builder、Langfuse 仪表盘、云端导出、真实 LLM + 工具 |
| -- | 深度研究 | 多智能体研究助手（独立包） |
| -- | WASM 浏览器示例 | 通过 wasm-bindgen 在浏览器中执行图 |
| -- | WASM 边缘 CLI | WASI 独立二进制 |
| -- | WASM 边缘服务 | Fermyon Spin HTTP 边缘服务 |

```bash
# 运行任意模拟示例（无需 API key）
cargo run -p juncture-simple-example --bin 01_state_machine

# 运行真实 LLM 示例（需要 .env 中配置 OPENAI_API_KEY）
cargo run -p juncture-simple-example --bin 13_react_agent
```

## 工作空间结构

```
juncture/
  crates/
    juncture/            # 门面 crate -- LLM 提供商、工具、预构建智能体
    juncture-core/       # Channel 系统、StateGraph、Pregel 引擎、Node/Edge
    juncture-derive/     # #[derive(State)] 过程宏
    juncture-checkpoint/ # MemorySaver、SqliteSaver、PostgresSaver
    juncture-tracing/    # OpenTelemetry 集成
    juncture-telemetry/  # Langfuse 兼容内嵌可观测性引擎
    juncture-store/      # 跨线程持久化键值存储
  benchmarks/            # Juncture vs LangGraph 性能对比
  examples/              # 16 个示例 + 深度研究 + WASM 演示
  design/                # 架构设计文档（11 个模块）
```

## 构建与测试

```bash
# 构建全部
cargo build --workspace --all-features

# 运行所有测试
cargo test --workspace --all-targets --all-features

# 代码检查（零警告策略）
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 格式检查
cargo fmt --all -- --check

# 运行基准测试
cargo bench -p juncture-benchmarks
```

## 与其他 Rust 实现的对比

Juncture 在 Rust 版 LangGraph 实现中有着明确的定位：

| 方面 | Juncture | 其他 Rust 移植版 |
|------|----------|-----------------|
| **目标** | 与 LangGraph Python 语义等价 | 全新抽象或子集实现 |
| **状态系统** | `#[derive(State)]` 过程宏 + 逐字段 reducer | 手动 trait 实现或动态 map |
| **Channel 模型** | 静态，编译期验证 | 动态或简化版 |
| **执行引擎** | 完整 Pregel + 字段版本调度 | 简化顺序执行或基础并行 |
| **功能覆盖** | HITL、子图、Send、流式、检查点、Store | 部分覆盖 |
| **可观测性** | Langfuse 兼容内嵌仪表盘 + 云端导出 + OTLP | 通常不支持 |
| **WASM** | 浏览器 + WASI + Spin 边缘 | 通常不支持 |
| **成熟度** | 早期阶段，设计驱动 | 各不相同 |

取舍是明确的：Juncture 优先考虑完整性和兼容性，而非简洁性。如果你需要一个轻量级图执行器，其他选择可能更合适。如果你想将 LangGraph 应用迁移到 Rust 且尽量保持语义一致，Juncture 正是为此设计的。

## 路线图

- [ ] 生产环境加固（错误恢复、资源限制）
- [ ] 宽状态场景的性能优化
- [ ] 更多 LLM 提供商
- [ ] 图可视化工具
- [ ] LangGraph Platform API 兼容

## 贡献

欢迎贡献。请确保：

- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 零警告通过
- `cargo fmt --all -- --check` 通过
- 所有测试通过
- 提交代码中不包含 `unwrap()`、`todo!()` 或 `unimplemented!()`

## 许可证

根据 [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0)