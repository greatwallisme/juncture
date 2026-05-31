# 核心概念

[English Version / 英文版](../en/core-concepts.md)

本文档介绍 Juncture 的基本构建模块。在使用示例之前，理解这些概念至关重要。

## 状态（State）

状态是流经 Juncture 图的核心数据结构。每个节点读取当前状态，并返回一个更新，该更新会被合并回状态。

### 使用 `#[derive(State)]` 定义状态

`#[derive(State)]` 宏生成两种类型：
1. **状态结构体**（如 `WorkflowState`）-- 持有实际数据
2. **更新结构体**（如 `WorkflowStateUpdate`）-- 持有 `Option<T>` 字段用于部分更新

```rust
use juncture_derive::State;

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}

// 自动生成：
// struct WorkflowStateUpdate {
//     step: Option<String>,
//     count: Option<u32>,
// }
```

更新结构体的每个字段都使用 `Option<T>`。当字段为 `None` 时，不会改变。当为 `Some(value)` 时，根据字段的 Reducer 进行合并。

### Reducer

Reducer 控制更新如何合并到当前状态。Juncture 支持多种内置 Reducer：

| Reducer | 行为 | 使用场景 |
|---------|------|----------|
| `replace`（默认） | 最后写入者胜出（同超步重复写入会 panic） | 标量值，单所有者字段 |
| `append` | 扩展 Vec 集合 | 随时间增长的列表 |
| `ephemeral` | 每个超步后重置为 Default | 临时计算结果 |
| `last_write_wins` | 最后写入者胜出（同超步重复写入不会 panic） | 状态字段，时间戳 |
| `untracked` | 不跨检查点持久化 | 临时状态 |
| `replace_after_finish` | 仅在 finish() 后可用 | 完成后更新 |
| `any` | 所有写入者应提供相等的值 | 共识字段 |
| `custom = path::to::func` | 用户自定义合并函数 `fn(&mut T, T)` | 复杂合并策略 |

```rust
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CounterState {
    // 默认 replace Reducer -- 最后写入者胜出
    value: u32,

    // append Reducer -- 扩展向量
    #[reducer(append)]
    items: Vec<String>,

    // last_write_wins -- 显式语义
    #[reducer(last_write_wins)]
    status: String,
}
```

### FieldsChanged 位掩码

`#[derive(State)]` 宏还生成一个 `FieldsChanged` 位掩码（u64），用于跟踪更新中修改了哪些字段。这实现了高效的变化检测，无需比较整个状态结构体。

### CowState

Juncture 使用 `CowState<S>`（基于 Arc 的写时复制）作为默认状态包装器。这避免了在节点之间传递状态时的昂贵克隆 -- 只读状态的节点共享同一个 Arc，只有在发生写入时才会克隆。

## StateGraph

`StateGraph<S>` 是构建计算图的构建器。它对状态类型 `S` 是泛型的。

### 创建图

```rust
use juncture_core::StateGraph;

let mut graph = StateGraph::<MyState>::new();
```

### 添加节点

节点是计算单元。每个节点接收当前状态的引用并返回更新：

```rust
use juncture_core::node::NodeFnUpdate;

graph.add_node_simple(
    "my_node",
    NodeFnUpdate(|state: &MyState| {
        async move {
            Ok(MyStateUpdate {
                field: Some("new value".to_string()),
                ..Default::default()
            })
        }
    }),
)?;
```

节点函数：
- 接收 `&MyState`（当前状态的共享引用）
- 返回 `Result<MyStateUpdate, JunctureError>`
- 是异步的（使用 `async move` 块）
- 只设置需要更改的字段（其他字段使用 `..Default::default()`）

### 添加边

边定义节点之间的执行顺序：

```rust
// 简单边：在 "a" 之后运行 "b"
graph.add_edge("a", "b");

// 条件边：根据状态路由
graph.add_conditional_edges(
    "router_node",
    Arc::new(my_router_fn) as Arc<dyn Router<MyState>>,
    PathMap::from(&[("path_a", "node_a"), ("path_b", "node_b")]),
);
```

### 入口点和完成点

```rust
graph.set_entry_point("first_node");
graph.set_finish_point("last_node");
```

入口点是执行开始的地方。完成点是执行结束的地方。如果未设置完成点，执行会持续到没有更多可达节点。

### 编译

执行前必须编译图：

```rust
let compiled = graph.compile()?;

// 或带检查点器：
let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

// 或带编译配置（用于 HITL）：
let compiled = graph.compile_with_config(CompileConfig {
    interrupt_before: vec!["review".to_string()],
    interrupt_after: vec![],
})?;
```

## 条件路由

条件路由允许图根据当前状态采取不同路径。这是实现分支逻辑的主要机制。

### 路由函数

路由函数检查状态并返回下一个节点的名称：

```rust
use juncture_core::edge::{PathMap, Router};

const fn grade_router(state: &ScoreState) -> &str {
    if state.score >= 90 {
        "excellent"
    } else if state.score >= 70 {
        "good"
    } else {
        "retry"
    }
}
```

### PathMap

`PathMap` 将路由返回值映射到节点名称：

```rust
graph.add_conditional_edges(
    "grade",
    Arc::new(grade_router) as Arc<dyn Router<ScoreState>>,
    PathMap::from(&[
        ("excellent", "excellent"),
        ("good", "good"),
        ("retry", "retry"),
    ]),
);
```

### Router trait

对于更复杂的路由（异步操作、错误处理），实现 `Router` trait：

```rust
struct AgentRouter;

impl Router<MessagesState> for AgentRouter {
    fn route(
        &self,
        state: &MessagesState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        let target = if has_tool_calls(state) { "tools" } else { END };
        Box::pin(async move { Ok(RouteResult::One(target.to_string())) })
    }
}
```

## Pregel 引擎

Pregel 引擎是 Juncture 的执行运行时。它以超步（superstep）处理图 -- 每个超步并行执行所有就绪节点，然后合并它们的更新。

### 执行模型

1. **超步 0**：入口节点执行
2. **合并**：所有节点输出通过 Reducer 合并到状态
3. **路由**：条件边确定下一组就绪节点
4. **超步 N**：所有就绪节点并行执行
5. **重复**直到达到完成点或没有更多就绪节点

### 并行性

Juncture 使用 `tokio::spawn` + `JoinSet` 实现真正的多核并行。当同一超步中有多个就绪节点时，它们并发执行。基于 `Semaphore` 的有界并发控制防止资源耗尽。

### 执行模式

| 模式 | 方法 | 描述 |
|------|------|------|
| 阻塞 | `invoke()` | 运行图直到完成，返回最终状态 |
| 异步 | `invoke_async()` | invoke 的异步版本 |
| 流式 | `stream()` | 返回 `StreamEvent` 值的流 |

## MessagesState

对于 LLM 应用，Juncture 提供了 `MessagesState` -- 一个用于管理对话历史的预构建状态类型：

```rust
use juncture_core::state::messages::{Message, MessagesState};
use juncture_core::state::{Content, Role};

let state = MessagesState {
    messages: vec![
        Message::system("You are a helpful assistant."),
        Message::human("Hello!"),
    ],
};
```

### 消息角色

| 角色 | 描述 |
|------|------|
| `Role::System` | 系统指令 |
| `Role::Human` | 用户消息 |
| `Role::Ai` | AI 响应（可能包含工具调用） |
| `Role::Tool` | 工具执行结果 |

### 消息构造器

```rust
Message::system("instructions".to_string())
Message::human("user input".to_string())
Message::ai("assistant response".to_string())
```

## Tool Trait

`Tool` trait 定义了 LLM 可调用的工具：

```rust
use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};

#[derive(Debug)]
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Adds two numbers" }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        })
    }
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let a = input["a"].as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'a'".into()))?;
        let b = input["b"].as_f64()
            .ok_or_else(|| ToolError::InvalidInput("Missing 'b'".into()))?;
        Ok((a + b).to_string())
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

## ChatModel Trait

`ChatModel` trait 为 LLM 提供者提供统一接口：

```rust
use juncture::llm::{ChatModel, ChatOpenAI};
use futures::StreamExt;

let llm = ChatOpenAI::new("sk-...".to_string())
    .with_model("gpt-4o".to_string());

// 单次调用
let response = llm.invoke(&messages, None).await?;

// 流式（异步，返回 Result<BoxStream>）
let mut stream = llm.stream(&messages, None).await?;
while let Some(chunk) = stream.next().await {
    let chunk = chunk?;
    print!("{}", chunk.content);
}

// 带工具
let llm_with_tools = llm.bind_tools(vec![tool_def]);
```

## GraphOutput

当图完成执行时，返回一个 `GraphOutput`：

```rust
let output = compiled.invoke(initial_state, &config)?;

output.value       // 最终状态（类型 S）
output.output      // 通过 FromState 提取的输出（类型 O，默认为 S）
output.interrupts  // InterruptInfo 列表（用于 HITL）
output.metadata    // 执行元数据
```

### GraphOutputMetadata

```rust
output.metadata.steps           // 执行的超步数
output.metadata.run_id          // 唯一运行标识符
output.metadata.checkpoint_id   // 检查点 ID（如果启用了检查点）
output.metadata.budget_usage    // 预算使用情况（如果启用了预算跟踪）
```

## 下一步

- [示例指南](examples-guide.md) -- 在 17 个示例中查看这些概念的实际应用
- [高级功能](advanced-features.md) -- 流式、人在回路、检查点、遥测
