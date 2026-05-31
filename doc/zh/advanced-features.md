# 高级功能

[English Version / 英文版](../en/advanced-features.md)

本文档介绍 Juncture 在基本图构建之外的高级功能。

## 流式（Streaming）

流式允许您实时观察图执行，在每个超步完成时接收事件。

### 流模式

| 模式 | 描述 |
|------|------|
| `StreamMode::Values` | 每个超步后发出完整状态（默认） |
| `StreamMode::Updates` | 只发出每个节点的更新（增量） |
| `StreamMode::Messages` | 发出 LLM token 流 |
| `StreamMode::Custom` | 发出节点自定义事件 |
| `StreamMode::Debug` | 发出详细的调试信息 |
| `StreamMode::Tools` | 发出工具执行生命周期事件 |
| `StreamMode::Checkpoints` | 发出检查点保存事件 |
| `StreamMode::Tasks` | 发出详细的任务事件 |
| `StreamMode::Multi(vec)` | 组合多种流模式 |

### StreamEvent 变体

| 变体 | 描述 |
|------|------|
| `Values { state, step }` | 超步后的完整状态快照 |
| `FilteredValues { data, step }` | 过滤后的状态（设置了 `output_keys` 时） |
| `Updates { node, update, step }` | 每节点更新 |
| `FilteredUpdates { node, data, step }` | 过滤后的每节点更新 |
| `Messages { chunk, metadata }` | LLM token 块 |
| `Custom { node, data, ns }` | 节点自定义事件 |
| `TaskStart { node, task_id, step }` | 任务开始 |
| `TaskEnd { node, task_id, step, duration_ms }` | 任务完成 |
| `Interrupt { node, payload, resumable, ns }` | HITL 中断 |
| `BudgetExceeded { reason, usage }` | 超出预算限制 |
| `End { output }` | 图执行完成 |
| `Cancelled { step }` | 执行被取消 |
| `Debug(event)` | 调试事件 |
| `Tools(event)` | 工具生命周期事件 |
| `CheckpointSaved { checkpoint_id, metadata, step }` | 检查点已保存 |
| `TaskDetail { task_id, ... }` | 详细任务事件 |

### 基本流式

```rust
use juncture_core::stream::{StreamEvent, StreamMode};
use futures::StreamExt;

let handle = compiled
    .stream(initial_state, &config, StreamMode::Values)
    .await?;

let mut stream = handle.stream;
while let Some(result) = stream.next().await {
    match result? {
        StreamEvent::Values { state, step } => {
            println!("步骤 {step}: {:?}", state);
        }
        StreamEvent::End { output } => {
            println!("最终输出: {:?}", output);
        }
        _ => {}
    }
}
```

### 使用真实 LLM 的流式

对于 LLM 应用，`ChatModel::stream()` 提供逐 token 流式传输：

```rust
use futures::StreamExt;

// stream() 是异步的，返回 Result<BoxStream>
let mut stream = llm.stream(&messages, None).await?;
let mut full_response = String::new();

while let Some(chunk_result) = stream.next().await {
    let chunk = chunk_result?;
    if !chunk.content.is_empty() {
        print!("{}", chunk.content);
        full_response.push_str(&chunk.content);
    }
}
```

---

## 人在回路（HITL）

HITL 允许您在特定点暂停图执行以供人工审查或输入。

### 中断配置

使用 `CompileConfig` 指定执行暂停的位置：

```rust
use juncture_core::graph::CompileConfig;

let config = CompileConfig {
    interrupt_before: vec!["review".to_string()],  // 在 "review" 节点前暂停
    interrupt_after: vec!["propose".to_string()],   // 在 "propose" 节点后暂停
};

let compiled = graph.compile_with_config(config)?;
```

### 检测中断

执行后检查 `output.interrupts` 以查看图是否暂停：

```rust
let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

if output.interrupts.is_empty() {
    println!("执行完成，无中断");
} else {
    println!("在以下位置中断: {:?}", output.interrupts);
    println!("当前状态: {:?}", output.value);

    // 在实际应用中：
    // 1. 向人工审查者展示状态
    // 2. 等待批准/拒绝
    // 3. 使用更新后的状态恢复执行
}
```

### HITL 工作流模式

典型的 HITL 工作流遵循以下模式：

```
propose -> [中断] -> review -> execute
```

`propose` 节点生成操作。执行暂停。人工审查操作。如果批准，执行继续到 `review` 然后 `execute`。

---

## 检查点（Checkpointing）

检查点将图状态持久化到存储，支持跨会话或进程恢复执行。

### MemorySaver（内存）

```rust
use juncture_checkpoint::MemorySaver;
use std::sync::Arc;

let checkpointer = MemorySaver::new();
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;
```

### 线程标识

使用 `run_id` 维持执行连续性：

```rust
let config = RunnableConfig::default().with_run_id("session-123");
let output = compiled.invoke(state, &config)?;

// 稍后，在不同进程或会话中：
let config = RunnableConfig::default().with_run_id("session-123");
let output = compiled.invoke(fresh_state, &config)?;
// 检查点器从最后一个检查点恢复实际状态
```

### 检查点存储选项

| 存储 | Crate | 使用场景 |
|------|-------|----------|
| `MemorySaver` | juncture-checkpoint | 开发、测试 |
| `SqliteSaver` | juncture-checkpoint | 单节点生产 |
| `PostgresSaver` | juncture-checkpoint | 分布式生产 |

---

## 错误处理

Juncture 为图执行提供结构化错误处理。

### 从节点返回错误

节点可以返回 `Err(JunctureError)` 来表示失败：

```rust
graph.add_node_simple(
    "risky_operation",
    NodeFnUpdate(|state: &MyState| async move {
        if something_wrong {
            return Err(JunctureError::execution("操作失败"));
        }
        Ok(MyStateUpdate { .. })
    }),
)?;
```

### 错误恢复模式

**带退避的重试：**
```rust
graph.add_node_simple(
    "process",
    NodeFnUpdate(|state: &MyState| {
        let retries = state.retries;
        async move {
            if retries < 3 {
                // 失败并重试
                return Err(JunctureError::execution("瞬时错误"));
            }
            Ok(MyStateUpdate { status: Some("done".into()), .. })
        }
    }),
)?;

graph.add_edge("process", "recovery");
graph.add_edge("recovery", "process");  // 重试循环
graph.add_edge("process", "fallback");  // 最终回退
```

**条件错误路由：**
```rust
const fn error_router(state: &MyState) -> &str {
    if state.status == "error" { "fallback" }
    else if state.retries < 3 { "retry" }
    else { "continue" }
}
```

---

## 工具（Tools）

工具通过提供可调用函数来扩展 LLM 能力。

### 定义工具

```rust
use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};

#[derive(Debug)]
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "返回城市的当前天气。输入: {\"city\": \"城市名\"}"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "城市名称"
                }
            },
            "required": ["city"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let city = input["city"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("缺少 'city'".into()))?;

        // 工具实现
        Ok(format!("{city}的天气: 22C, 晴"))
    }
}
```

### ToolError 变体

| 变体 | 构造函数 | 使用场景 |
|------|----------|----------|
| `InvalidInput` | `ToolError::InvalidInput(msg)` | 输入格式错误 |
| `ExecutionFailed` | `ToolError::ExecutionFailed(msg)` | 运行时失败 |
| `Timeout` | `ToolError::Timeout` | 执行超时 |
| `ToolNotFound` | `ToolError::ToolNotFound(name)` | 未知工具名称 |
| `ValidationError` | `ToolError::ValidationError(msg)` | Schema 验证失败 |

### 将工具绑定到 LLM

```rust
let tool_def = ToolDefinition {
    name: weather.name().to_string(),
    description: weather.description().to_string(),
    parameters: weather.schema(),
};

let llm_with_tools = llm.bind_tools(vec![tool_def]);
let response = llm_with_tools.invoke(&messages, None).await?;

// 检查工具调用
for tc in &response.tool_calls {
    println!("工具: {}({})", tc.name, tc.arguments);
    let result = weather.invoke(tc.arguments.clone()).await?;
    println!("结果: {result}");
}
```

### 智能体循环模式

常见的模式是 ReAct 智能体循环：

```rust
let max_iterations = 10;
for iteration in 0..max_iterations {
    let response = llm_with_tools.invoke(&messages, None).await?;
    messages.push(response.clone());

    if response.tool_calls.is_empty() {
        break;  // 没有更多工具调用
    }

    // 执行每个工具调用
    for tc in &response.tool_calls {
        let result = execute_tool(tc).await?;
        messages.push(Message {
            role: Role::Tool,
            content: Content::Text(result),
            tool_call_id: Some(tc.id.clone()),
            name: Some(tc.name.clone()),
            ..Default::default()
        });
    }
}
```

### 内置工具

| 工具 | 描述 |
|------|------|
| `ThinkTool` | 智能体自我反思 |
| `SubagentTool` | 委托任务给子智能体 |

---

## 结构化输出

使用基于工具的提取从 LLM 响应中提取结构化数据。

### 模式

1. 将目标 schema 定义为工具
2. 使用 `ToolChoice::Required` 绑定工具
3. 将工具调用参数解析为 Rust 结构体

```rust
use juncture::llm::{CallOptions, ToolChoice};

let extraction_tool = ToolDefinition {
    name: "extract_info".to_string(),
    description: "提取人物信息".to_string(),
    parameters: serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"},
            "occupation": {"type": "string"}
        },
        "required": ["name", "age", "occupation"]
    }),
};

let llm_with_tool = llm.bind_tools(vec![extraction_tool]);

let options = CallOptions {
    tool_choice: Some(ToolChoice::Required),
    ..CallOptions::default()
};

let response = llm_with_tool.invoke(&messages, Some(&options)).await?;

// 解析为结构体
if let Some(tc) = response.tool_calls.first() {
    let info: ExtractedInfo = serde_json::from_value(tc.arguments.clone())?;
    println!("姓名: {}, 年龄: {}", info.name, info.age);
}
```

---

## 中间件（Middleware）

中间件拦截智能体执行以处理横切关注点。

### LoopDetectionMiddleware

防止无限工具调用循环：

```rust
use juncture::prebuilt::{AgentMiddlewareChain, LoopDetectionMiddleware};

let middleware = AgentMiddlewareChain::new()
    .with(LoopDetectionMiddleware::new(3));  // 最多 3 次重复
```

### ToolErrorHandlingMiddleware

为工具失败提供优雅的错误恢复：

```rust
let middleware = AgentMiddlewareChain::new()
    .with(ToolErrorHandlingMiddleware::new());
```

### 与智能体一起使用中间件

```rust
use juncture::prebuilt::{AgentConfig, create_agent_with_middleware};

let config = AgentConfig {
    system_message: Some("You are a helpful assistant.".into()),
    middleware,
    ..Default::default()
};

let graph = create_agent_with_middleware(model, tools, config)?;
```

---

## 遥测（OpenTelemetry）

Juncture 提供完整的 OpenTelemetry 集成以实现可观测性。

### 设置

```rust
use juncture_tracing::init;

let metrics_registry = init()
    .with_service_name("my-service")
    .with_otlp_endpoint("http://localhost:4317")
    .with_metrics(true)
    .install()?
    .expect("metrics enabled");
```

### 指标收集器

```rust
use juncture_core::observability::MetricsCollector;
use juncture_tracing::RegistryMetricsCollector;

let collector: Arc<dyn MetricsCollector> =
    Arc::new(RegistryMetricsCollector::new(metrics_registry));

let config = RunnableConfig::default()
    .with_metrics_collector(collector);
```

### 回调处理器

```rust
use juncture_core::observability::GraphLifecycleCallback;
use juncture_tracing::callback::{CallbackHandlerAdapter, GraphCallbackHandler};

struct MyCallback;

impl GraphCallbackHandler for MyCallback {
    fn on_node_start(&self, node: &str, task_id: &str) {
        println!("节点 {node} 开始（任务 {task_id}）");
    }
    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        println!("节点 {node} 完成，耗时 {duration_ms}ms");
    }
    fn on_node_error(&self, node: &str, error: &JunctureError) {
        println!("节点 {node} 失败: {error}");
    }
    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        println!("图完成: {:?}", result.is_ok());
    }
}

let handler: Arc<dyn GraphLifecycleCallback> =
    Arc::new(CallbackHandlerAdapter::new(Arc::new(MyCallback)));

let config = RunnableConfig::default()
    .with_callback_handler(handler);
```

### 可用指标

| 指标 | 类型 | 描述 |
|------|------|------|
| `juncture_graph_invocations_total` | Counter | 总图调用次数 |
| `juncture_graph_errors_total` | Counter | 总图错误次数 |
| `juncture_node_duration_ms` | Histogram | 每节点执行持续时间 |
| `juncture_graph_duration_ms` | Histogram | 每图执行持续时间 |
| `juncture_llm_calls` | Counter | 总 LLM API 调用次数 |
| `juncture_tokens_input` | Counter | 消耗的输入 token |
| `juncture_tokens_output` | Counter | 生成的输出 token |

### 可用 Span

| Span | 描述 |
|------|------|
| `juncture.graph.invoke` | 图执行 |
| `juncture.node.execute` | 节点执行 |
| `juncture.llm.call` | LLM API 调用 |
| `juncture.tool.call` | 工具执行 |

---

## 子智能体（Sub-agents）

Juncture 支持委托任务给子智能体以构建多智能体架构。

### 注册子智能体

```rust
use juncture::prebuilt::{InMemoryAgentRegistry, AgentEntry};

let mut registry = InMemoryAgentRegistry::new();
registry.register(
    "researcher".to_string(),
    AgentEntry::from_graph(researcher_graph),
);
```

### 使用 SubagentTool

```rust
use juncture::prebuilt::SubagentTool;

let tools: Vec<Box<dyn Tool>> = vec![
    Box::new(SubagentTool::new(registry)),
    // ... 其他工具
];
```

编排器智能体可以通过调用 `task` 工具来委托任务给子智能体，并提供要完成的工作描述。

---

## Store（持久化键值存储）

Juncture 提供 `Store` trait 用于跨线程持久化存储。

### MemoryStore

```rust
use juncture_core::store::Store;

let store = MemoryStore::new();
store.put("namespace", "key", json_value, None).await?;
let value = store.get("namespace", "key").await?;
```

### FactStore（示例模式）

`FactStore` 是一个示例模式（位于 `examples/deep-research`），封装了 `Store`，提供面向研究应用的事实特定操作。您可以为自己的领域实现类似的模式：

```rust
// FactStore 不是 juncture crate 的一部分 -- 它是一个示例模式。
// 完整实现请参见 examples/deep-research/src/memory/store.rs

use juncture_core::store::Store;

// 您的自定义 store 包装器
struct MyStore {
    store: Arc<dyn Store>,
    namespace: String,
}

impl MyStore {
    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError> {
        self.store.put(&self.namespace, key, value, None).await
    }

    async fn get(&self, key: &str) -> Result<Option<Item>, StoreError> {
        self.store.get(&self.namespace, key).await
    }
}
```

---

## 下一步

- [快速入门](getting-started.md) -- 安装和第一个图
- [核心概念](core-concepts.md) -- 框架基础
- [示例指南](examples-guide.md) -- 所有 17 个示例详解
