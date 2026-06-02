# 示例指南

[English Version / 英文版](../en/examples-guide.md)

本指南详细讲解所有 17 个 Juncture 示例，解释每个示例演示的内容以及如何运行。

## 示例进阶路径

示例按复杂度递进组织：

| 阶段 | 示例 | 核心概念 | 需要 API 密钥 |
|------|------|----------|--------------|
| 核心模式 | 01-03 | 状态、Reducer、路由 | 否 |
| LLM 基础 | 04-05 | 聊天、工具调用（模拟） | 否 |
| 高级功能 | 06-09 | 流式、HITL、检查点、错误 | 否 |
| 真实 LLM | 10-15 | 聊天、流式、工具、智能体 | 是 |
| 生产级 | 深度研究、遥测 | 多智能体、OTel | 是 |

---

## 阶段 1：核心模式

### 示例 01：基本状态机

**文件：** `examples/src/01_state_machine.rs`

最简单的 Juncture 图：包含两个节点的线性流程。

```bash
cargo run -p juncture-simple-example --bin 01_state_machine
```

**演示内容：**
- `#[derive(State)]` 生成状态/更新对
- `StateGraph::new()` 创建图构建器
- `add_node_simple()` 与 `NodeFnUpdate` 闭包
- `add_edge()` 定义线性流程
- `set_entry_point()` 和 `set_finish_point()`
- `compile()` 和 `invoke()`
- 读取 `output.value` 和 `output.metadata.steps`

**流程：** `START -> greet -> finish -> END`

**关键代码模式：**
```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}
```

---

### 示例 02：不同 Reducer 的计数器

**文件：** `examples/src/02_counter_reducers.rs`

演示不同 Reducer 类型如何处理状态合并。

```bash
cargo run -p juncture-simple-example --bin 02_counter_reducers
```

**演示内容：**
- 默认 `replace` Reducer（标量值最后写入者胜出）
- `#[reducer(append)]` 用于向量累积
- `#[reducer(last_write_wins)]` 用于显式语义
- `HashMap` 的自定义合并函数
- 循环图执行（increment -> set_status -> increment -> collect）

**流程：** `START -> increment -> set_status -> increment -> collect -> END`（含循环）

**关键代码模式：**
```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CounterState {
    value: u32,                                    // replace（默认）
    #[reducer(append)]
    items: Vec<String>,                            // append
    #[reducer(last_write_wins)]
    status: String,                                // last_write_wins
}
```

---

### 示例 03：条件路由

**文件：** `examples/src/03_conditional_routing.rs`

展示如何根据状态值进行路由。

```bash
cargo run -p juncture-simple-example --bin 03_conditional_routing
```

**演示内容：**
- 检查状态并返回目标节点名称的路由函数
- `PathMap` 将路由输出映射到节点名称
- `add_conditional_edges()` 用于动态路由
- 使用多个初始状态测试（score=95, 75, 50）

**流程：** `START -> grade -> {excellent|good|retry} -> END`

**关键代码模式：**
```rust
const fn grade_router(state: &ScoreState) -> &str {
    if state.score >= 90 { "excellent" }
    else if state.score >= 70 { "good" }
    else { "retry" }
}

graph.add_conditional_edges(
    "grade",
    Arc::new(grade_router) as Arc<dyn Router<ScoreState>>,
    PathMap::from(&[("excellent", "excellent"), ("good", "good"), ("retry", "retry")]),
);
```

---

## 阶段 2：LLM 基础

### 示例 04：使用 MessagesState 的基本聊天

**文件：** `examples/src/04_chat_basic.rs`

使用 `MessagesState` 的简单聊天机器人，无需真实 LLM。

```bash
cargo run -p juncture-simple-example --bin 04_chat_basic
```

**演示内容：**
- `MessagesState` 用于对话历史
- `Message` 构造器（`Message::human()`、`Message::ai()`）
- `Role` 枚举（`Human`、`Ai`、`System`、`Tool`）
- `Content` 枚举（`Text`、`MultiPart`）
- 处理消息的单节点图

**关键代码模式：**
```rust
let initial_state = MessagesState {
    messages: vec![Message::human("Hi there!".to_string())],
};
```

---

### 示例 05：工具调用（手动）

**文件：** `examples/src/05_tool_calling.rs`

演示无需真实 LLM 的工具定义和执行。

```bash
cargo run -p juncture-simple-example --bin 05_tool_calling
```

**演示内容：**
- `Tool` trait 实现（`name`、`description`、`schema`、`invoke`）
- `ToolError` 变体（`InvalidInput`、`ExecutionFailed`、`Timeout`、`ToolNotFound`、`ValidationError`）
- 直接工具调用
- 构建带工具感知智能体节点的图

**关键代码模式：**
```rust
#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Adds two numbers" }
    fn schema(&self) -> serde_json::Value { json!({...}) }
    async fn invoke(&self, input: Value) -> Result<String, ToolError> { ... }
}
```

---

## 阶段 3：高级功能

### 示例 06：流式执行

**文件：** `examples/src/06_streaming.rs`

展示如何流式获取图执行事件。

```bash
cargo run -p juncture-simple-example --bin 06_streaming
```

**演示内容：**
- 使用 `stream()` 代替 `invoke()`
- `StreamMode::Values` 在每个超步后流式传输状态
- `StreamEvent` 变体（`Values`、`End`）
- 使用 `futures::StreamExt` 消费流
- 三个顺序节点的实时进度

**关键代码模式：**
```rust
let handle = compiled.stream(initial_state, &config, StreamMode::Values).await?;
let mut stream = handle.stream;
while let Some(result) = stream.next().await {
    match result? {
        StreamEvent::Values { state, step } => { /* 处理 */ }
        StreamEvent::End { output } => { /* 完成 */ }
        _ => {}
    }
}
```

---

### 示例 07：人在回路（HITL）

**文件：** `examples/src/07_human_in_the_loop.rs`

演示中断执行以供人工审批。

```bash
cargo run -p juncture-simple-example --bin 07_human_in_the_loop
```

**演示内容：**
- `CompileConfig` 配置 `interrupt_before` 和 `interrupt_after`
- `compile_with_config()` 用于 HITL 工作流
- 检查 `output.interrupts` 检测中断
- 审批工作流模式（propose -> review -> execute）

**流程：** `START -> propose -> [中断] -> review -> execute -> END`

**关键代码模式：**
```rust
let config = CompileConfig {
    interrupt_before: vec!["review".to_string()],
    interrupt_after: vec![],
};
let compiled = graph.compile_with_config(config)?;

let output = compiled.invoke(initial_state, &config)?;
if !output.interrupts.is_empty() {
    // 执行暂停 -- 等待人工审批
}
```

---

### 示例 08：检查点与恢复

**文件：** `examples/src/08_checkpoint_resume.rs`

展示跨执行的状态持久化。

```bash
cargo run -p juncture-simple-example --bin 08_checkpoint_resume
```

**演示内容：**
- `MemorySaver` 用于内存检查点存储
- `compile_with_checkpointer()` 用于持久化
- `RunnableConfig::with_run_id()` 用于线程标识
- 从保存的状态恢复执行
- 基于检查点连续性的循环图

**关键代码模式：**
```rust
let checkpointer = MemorySaver::new();
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

let config = RunnableConfig::default().with_run_id("my-session");
let output = compiled.invoke(state, &config)?;
// 稍后，使用相同的 run_id 恢复
```

---

### 示例 09：错误恢复

**文件：** `examples/src/09_error_recovery.rs`

演示图中的错误处理模式。

```bash
cargo run -p juncture-simple-example --bin 09_error_recovery
```

**演示内容：**
- 从节点返回 `Err(JunctureError::execution(...))`
- 使用 `?` 操作符进行错误传播
- 重试逻辑模式（process -> recovery -> process -> fallback）
- 带回退节点的优雅降级

**流程：** `START -> process -> recovery -> process -> fallback -> END`（含重试循环）

---

## 阶段 4：真实 LLM 应用

这些示例需要真实的 LLM API 密钥。首先配置 `.env` 文件：

```bash
cp examples/.env.example examples/.env
# 设置 OPENAI_API_KEY，可选设置 OPENAI_BASE_URL 和 OPENAI_MODEL
```

### 示例 10：使用真实 LLM 的基本聊天

**文件：** `examples/src/10_basic_chat.rs`

与真实 LLM 的单轮和多轮对话。

```bash
cargo run -p juncture-simple-example --bin 10_basic_chat
```

**演示内容：**
- `ChatOpenAI` 客户端构建
- `ChatModel::invoke()` 用于单轮对话
- 通过累积 `Message` 历史实现多轮对话
- 系统提示词

---

### 示例 11：流式聊天

**文件：** `examples/src/11_streaming_chat.rs`

从真实 LLM 逐 token 流式传输。

```bash
cargo run -p juncture-simple-example --bin 11_streaming_chat
```

**演示内容：**
- `ChatModel::stream()` 用于逐块流式传输
- 实时处理 `StreamChunk` 值
- 累积完整响应

---

### 示例 12：使用真实 LLM 的工具调用

**文件：** `examples/src/12_tool_calling.rs`

LLM 驱动的工具选择和执行。

```bash
cargo run -p juncture-simple-example --bin 12_tool_calling
```

**演示内容：**
- `bind_tools()` 将工具绑定到 LLM
- LLM 根据用户输入决定何时调用工具
- `ToolCall` 结构体（`name`、`arguments`、`id`）
- 将工具结果发送回 LLM
- 多步工具执行流程

---

### 示例 13：ReAct 智能体循环

**文件：** `examples/src/13_react_agent.rs`

包含天气和数学工具的手动智能体循环。

```bash
cargo run -p juncture-simple-example --bin 13_react_agent
```

**演示内容：**
- 手动智能体循环（LLM -> 工具 -> LLM -> ... 直到没有更多工具调用）
- 多个工具（`WeatherTool`、`MathTool`）
- `ToolDefinition` 用于将工具绑定到 LLM
- `Role::Tool` 消息用于工具结果
- 最大迭代安全限制

---

### 示例 14：多轮对话

**文件：** `examples/src/14_multi_turn.rs`

烹饪助手的对话历史累积。

```bash
cargo run -p juncture-simple-example --bin 14_multi_turn
```

**演示内容：**
- 跨多次 LLM 调用累积 `Vec<Message>`
- 系统提示词用于角色设定
- 多轮上下文构建

---

### 示例 15：结构化输出提取

**文件：** `examples/src/15_structured_output.rs`

从 LLM 响应中提取结构化 JSON。

```bash
cargo run -p juncture-simple-example --bin 15_structured_output
```

**演示内容：**
- 定义目标 schema 作为工具
- `ToolChoice::Required` 强制使用工具
- `CallOptions` 控制 LLM 行为
- 使用 `serde` 将工具调用参数解析为 Rust 结构体
- 实体提取模式（姓名、职业、事实、情感）

---

## 阶段 5：生产级示例

### 深度研究

**包：** `examples/deep-research`

基于 LLM 驱动编排的多智能体研究助手。

```bash
cargo run -p deep-research -- "量子计算的现状是什么？"
cargo run -p deep-research -- --model gpt-4o-mini "主题"
cargo run -p deep-research -- --verbose "主题"
```

**演示内容：**
- `create_agent_with_middleware()` 创建带中间件链的智能体
- `SubagentTool` 委托任务给子智能体
- `InMemoryAgentRegistry` 管理子智能体图
- `ThinkTool` 用于智能体自我反思
- `LoopDetectionMiddleware` 防止无限循环
- `ToolErrorHandlingMiddleware` 优雅的错误恢复
- `FactStore` 用于跨会话持久化记忆
- `clap` CLI 参数解析

**架构：**
```
编排器（ReAct 智能体）
  -> SubagentTool -> 研究员子智能体（WebSearch + ThinkTool）
  -> ThinkTool（每次委托后反思）
  -> WebSearch（Tavily API）
  -> Calculator
  -> ReadFile
```

---

### 遥测演示

**文件：** `examples/src/telemetry_demo.rs`

带真实 LLM 和工具的端到端 OpenTelemetry 流水线。

```bash
# 启动遥测基础设施
docker compose -f docker/telemetry/docker-compose.yml up -d

# 运行演示
cargo run -p juncture-simple-example --bin telemetry_demo

# 验证
# Jaeger UI: http://localhost:16686
# Prometheus: http://localhost:9090
```

**演示内容：**
- `juncture_tracing::init()` 用于 OTel 流水线设置
- `RegistryMetricsCollector` 用于 Prometheus 指标
- `GraphCallbackHandler` 用于生命周期回调
- `CallbackHandlerAdapter` 桥接回调到 OTel span
- 带完整遥测的真实 LLM + 工具执行
- 错误路径图用于错误指标验证

**遥测覆盖范围：**

| 维度 | 指标/Span |
|------|-----------|
| LLM 调用 | `juncture.llm.call` span |
| LLM token | `juncture.tokens.input/output` |
| 工具调用 | `juncture.tool.call` span |
| 图生命周期 | `juncture.graph.invocations`、`duration_ms` |
| 节点执行 | `juncture.node.duration_ms` |
| 错误路径 | `juncture.graph.errors` |
| 回调 | `on_node_start/end`、`on_graph_end` |

---

### 示例 16：Juncture 遥测（Langfuse 兼容仪表盘）

**文件：** `examples/src/16_juncture_telemetry.rs`

真实 LLM 代理与工具调用，使用 `init()` 一行式初始化 + Langfuse 云端自动导出。

```bash
# 需要 .env 中配置 OPENAI_API_KEY
cargo run -p juncture-simple-example --bin 16_juncture_telemetry

# 打开仪表盘
open http://127.0.0.1:8123

# Langfuse 云端导出（添加到 .env）：
# LANGFUSE_PUBLIC_KEY=pk-lf-...
# LANGFUSE_SECRET_KEY=sk-lf-...
# LANGFUSE_BASE_URL=https://cloud.langfuse.com

# 公网访问
BIND_PUBLIC=1 cargo run -p juncture-simple-example --bin 16_juncture_telemetry
```

**演示内容：**
- `init()` builder 一行式遥测初始化
- `with_langfuse_from_env()` 自动读取 `LANGFUSE_*` 环境变量
- `with_dashboard(8123)` 启动内嵌 Web 服务器
- `TelemetryHandle` RAII 自动 flush
- 真实 agent 循环：`bind_tools` + 工具执行
- 多 agent 追踪：嵌套观测树（`parent_observation_id`）
- 每个观测的 token 用量和成本追踪
- 跨 trace 的 session 追踪

**Agent 流程：**
```
trace (react_agent)
  ├── span: iteration_1
  │   ├── generation: llm_call (决定使用工具)
  │   ├── tool_call: get_weather({"city":"Tokyo"})
  │   └── tool_call: calculator({"expression":"42 * 17"})
  └── span: iteration_2
      └── generation: llm_call (综合最终答案)
```

**仪表盘功能：**
- 概览：统计卡片、trace 时间图、模型成本条、延迟百分位
- Traces：名称/用户/日期筛选，token 流量表示（`input -> output (total)`）
- Trace 详情：双面板树+详情，类型筛选（All/Gen/Tool/Span），观测搜索
- Sessions：带聚合统计的富卡片

---

## 下一步

- [高级功能](advanced-features.md) -- 深入了解流式、HITL、检查点、工具和遥测
- [核心概念](core-concepts.md) -- 理解框架基础
