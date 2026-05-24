# 05 - Streaming 系统

## 概述

Streaming 是 Juncture 向调用方实时传递执行进度的机制。从 token 级别的 LLM 输出到 superstep 级别的状态变更，streaming 系统提供多粒度、可组合的事件流，使调用方能够构建实时 UI、监控面板和调试工具。

> **Implementation Note (C-05-3)**: Public streaming types (`StreamMode`, `StreamEvent`, `StreamConfig`, etc.) have minimal doc examples. Consider adding usage examples for each variant to improve developer experience.

---

## 1. LangGraph 参考：7 种 Stream Mode

> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:120` — StreamMode 类型定义
> 源码位置: `langgraph/libs/langgraph/langgraph/pregel/_loop.py:1348` — _emit() 事件发射
> 源码位置: `langgraph/libs/langgraph/langgraph/stream/__init__.py` — Stream 基础设施
> 源码位置: `langgraph/libs/langgraph/langgraph/stream/run_stream.py` — 运行时 stream 管理

| Mode | 事件内容 | 典型用途 |
|------|----------|----------|
| `values` | 每个 superstep 后的完整 state | 状态面板、调试 |
| `updates` | 每个节点产生的变更（仅修改的字段） | 增量 UI 更新 |
| `messages` | LLM token 级别流式输出 (chunk, metadata) | 聊天 UI 逐字显示 |
| `custom` | 节点通过 stream_writer 发送的任意数据 | 进度条、中间结果 |
| `checkpoints` | checkpoint 保存事件 | 持久化监控 |
| `tasks` | 任务开始/结束事件 | 执行追踪 |
| `debug` | 以上所有信息的合集 | 开发调试 |

LangGraph v2 统一格式：每个 chunk 是 `StreamPart { type, ns, data }`，其中 `ns` 标识来源子图。

---

## 2. Juncture StreamMode 与 StreamEvent

### 2.1 StreamMode

> 注意：LangGraph 定义了 7 种 stream mode 字面量（values, updates, checkpoints, tasks, debug, messages, custom）。
> Juncture 扩展为 9 种：
> - `Tools`：工具调用生命周期事件（ToolStarted, ToolOutputDelta, ToolFinished, ToolError）
> - `Multi`：组合多种模式的 Vec<StreamMode>，允许同时订阅多种事件类型
>
> 这两种是 Juncture 独有的扩展，不在 LangGraph 标准中。

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum StreamMode {
    /// 每个 superstep 后输出完整 state
    Values,

    /// 每个节点输出它产生的 update（仅变更字段）
    Updates,

    /// LLM token 级别的流式输出
    Messages,

    /// 节点通过 StreamWriter 发送的自定义数据
    Custom,

    /// 所有内部事件（包含 tasks + checkpoints + 其他调试信息）
    Debug,

    /// 工具执行生命周期事件
    /// 参考: `langgraph/pregel/_tools.py`
    Tools,

    /// Checkpoint 保存事件（每次 checkpoint 保存时触发）
    /// 参考: `langgraph/types.py:120-134`
    Checkpoints,

    /// 任务级别事件（TaskStart / TaskEnd 的详细版本）
    Tasks,

    /// 组合多种模式，同时接收多种事件
    Multi(Vec<StreamMode>),
}

impl Default for StreamMode {
    fn default() -> Self {
        StreamMode::Values
    }
}
```

### 2.2 StreamEvent

```rust
#[derive(Clone, Debug)]
pub enum StreamEvent<S: State> {
    /// 完整 state 快照（StreamMode::Values）
    Values {
        state: S,
        step: usize,
    },

    /// FilteredValues 变体支持 output_keys 过滤，
    /// 避免 Values 模式克隆整个 state。仅包含调用方指定的字段，显著优化大 state 场景的性能。
    FilteredValues {
        data: serde_json::Value,
        step: usize,
    },

    /// 单个节点的增量更新（StreamMode::Updates）
    Updates {
        node: String,
        update: S::Update,
        step: usize,
    },

    /// FilteredUpdates 变体支持 output_keys 过滤，
    /// 避免 Updates 模式克隆整个 update。仅包含调用方指定的字段，显著优化大 update 场景的性能。
    FilteredUpdates {
        node: String,
        data: serde_json::Value,
        step: usize,
    },

    /// LLM token 级别的流式 chunk（StreamMode::Messages）
    Messages {
        chunk: MessageChunk,
        metadata: MessageStreamMetadata,
    },

    /// 节点发送的自定义数据（StreamMode::Custom）
    Custom {
        node: String,
        data: serde_json::Value,
        ns: Vec<String>, // 子图命名空间路径
    },

    /// 任务开始执行
    TaskStart {
        node: String,
        task_id: String,
        step: usize,
    },

    /// 任务执行完成
    TaskEnd {
        node: String,
        task_id: String,
        step: usize,
        duration_ms: u64,
    },

    /// HITL 中断事件
    Interrupt {
        node: String,
        payload: serde_json::Value,
        resumable: bool,
        ns: Vec<String>,
    },

    /// 预算超限事件
    BudgetExceeded {
        reason: BudgetExceededReason,
        usage: BudgetUsage,
    },

    /// 图执行完成
    End {
        output: S,
    },

    /// 图执行被取消（如调用方 drop 了 stream）。
    ///
    /// 当图在到达自然 `End` 之前被中断时触发。
    /// 消费者通过此变体区分正常完成（`End`）、取消（`Cancelled`）
    /// 和错误（通过 `Result` 传播）。
    Cancelled { step: usize },

    /// 调试事件（StreamMode::Debug）
    Debug(DebugEvent),

    // ─── 审查补充事件类型 ───

    /// 工具执行生命周期事件（StreamMode::Tools）
    /// 参考: `langgraph/pregel/_tools.py`
    Tools(ToolsEvent),

    /// Checkpoint 保存事件（StreamMode::Checkpoints）
    CheckpointSaved {
        checkpoint_id: String,
        metadata: CheckpointMetadata,
        step: usize,
    },

    /// 详细任务事件（StreamMode::Tasks）
    TaskDetail {
        task_id: String,
        node: String,
        step: usize,
        attempt: usize,
        event: TaskEventType,
    },
}

/// 工具生命周期事件
#[derive(Clone, Debug)]
pub enum ToolsEvent {
    /// 工具开始执行
    ToolStarted {
        tool_name: String,
        tool_call_id: String,
        node: String,
        input: serde_json::Value,
        /// 工具开始执行的时间戳
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// 工具输出增量（流式工具结果）
    ToolOutputDelta {
        tool_call_id: String,
        delta: String,
    },
    /// 工具执行完成
    ToolFinished {
        tool_call_id: String,
        output: serde_json::Value,
        duration_ms: u64,
        /// 是否执行成功（false 时表示非错误的正常失败，如空结果）
        success: bool,
    },
    /// 工具执行失败
    ToolError {
        tool_call_id: String,
        error: String,
    },
}

/// 任务详细事件类型
#[derive(Clone, Debug)]
pub enum TaskEventType {
    Started,
    Completed { duration_ms: u64 },
    Failed { error: String },
    Retrying { attempt: usize },
}
```

### 2.3 辅助类型

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageChunk {
    /// 文本内容增量
    pub content: String,

    /// tool_call 增量片段
    pub tool_call_chunks: Vec<ToolCallChunk>,

    /// 本次 chunk 的 token 使用增量
    pub usage_delta: Option<TokenUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallChunk {
    pub id: Option<String>,
    pub name: Option<String>,
    pub args_delta: String, // JSON 片段
    pub index: usize,
}

> **Implementation Note (C-05-2)**: `MessageChunk` and `ToolCallChunk` fields are `pub` (data class pattern)
> rather than private with accessor methods. This is acceptable for stream-event DTOs where the caller
> consumes all fields and no invariant enforcement is needed.

#[derive(Clone, Debug)]
pub struct MessageStreamMetadata {
    /// 产生此 chunk 的节点名
    pub node: String,

    /// LLM 模型名
    pub model: String,

    /// 用户标签（用于过滤）
    pub tags: Vec<String>,

    /// 子图命名空间路径
    pub ns: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum DebugEvent {
    /// superstep 开始
    SuperstepStart { step: usize, nodes: Vec<String> },

    /// superstep 结束
    SuperstepEnd { step: usize, duration_ms: u64 },

    /// checkpoint 保存完成
    CheckpointSaved { checkpoint_id: String, step: usize },

    /// channel 版本变更
    ChannelUpdate { channel: String, version: u64 },

    /// 路由决策
    RouteDecision { from: String, to: Vec<String>, step: usize },

    /// 预算使用情况
    BudgetStatus { usage: BudgetUsage },
}
```

### 2.4 统一流事件格式 (StreamPart)

> 参考: `langgraph/types.py:331-355` -- StreamPart 统一类型

为了确保所有流事件具有一致的命名空间信息，定义统一的事件包装类型：

```rust
/// 统一流事件格式，所有事件都携带命名空间信息
#[derive(Clone, Debug)]
pub struct StreamPart<S: State> {
    /// 事件命名空间路径（用于子图事件区分）
    /// 例如: [] 根图, ["subgraph:agent"] 子图内
    pub ns: Vec<String>,

    /// 事件类型标签
    pub event: &'static str,

    /// 事件数据
    pub data: StreamEvent<S>,

    /// 事件元数据
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}
```

### 2.5 StreamChannel 与 Transformer

> 参考: `langgraph/stream/stream_channel.py`

StreamChannel 允许节点在执行过程中持续向特定 channel 输出流数据，
而 Transformer 则在数据到达消费者之前进行转换：

```rust
/// 流 Channel：节点侧的持续输出通道
pub struct StreamChannel {
    /// Channel 名称
    pub name: String,
    /// 数据发送端
    tx: mpsc::Sender<serde_json::Value>,
}

impl StreamChannel {
    pub fn send(&self, data: serde_json::Value) -> Result<(), mpsc::error::SendError> {
        self.tx.send(data)
    }
}

/// 流 Transformer：在数据到达消费者前进行转换
/// > **Implementation Note (C-05-1)**: StreamTransformer is not re-exported as public API.
/// > Currently internal-only; make public when custom stream pipelines are needed.
pub trait StreamTransformer: Send + Sync + 'static {
    fn transform(&self, data: serde_json::Value) -> serde_json::Value;
}

/// 预定义 Transformer
pub struct JsonParseTransformer;  // 将字符串解析为 JSON
pub struct FilterFieldsTransformer { pub fields: Vec<String> }  // 只保留指定字段
pub struct BatchTransformer { pub size: usize }  // 将多个事件合并为一个批次
```

---

## 3. 实现：tokio channel 架构

### 3.1 核心架构

```
┌─────────────────────────────────────────────────────────┐
│                    Pregel 执行引擎                        │
│                                                         │
│  superstep loop                                         │
│    ├─ emit TaskStart                                    │
│    ├─ node.call(state, config, stream_writer)           │
│    │    ├─ LLM streaming → emit Messages               │
│    │    └─ stream_writer.send() → emit Custom          │
│    ├─ emit TaskEnd                                      │
│    ├─ merge updates → emit Updates                      │
│    ├─ checkpoint → emit Debug(CheckpointSaved)          │
│    └─ emit Values (完整 state)                          │
│                                                         │
│  ┌──────────────┐                                       │
│  │ EventEmitter │ ← 统一事件发射器                       │
│  └──────┬───────┘                                       │
│         │ mpsc::Sender<StreamEvent>                      │
└─────────┼───────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────┐
│  StreamReceiver<S>  │ ← 实现 Stream<Item = StreamEvent<S>>
│                     │
│  内部过滤逻辑：      │
│  根据 StreamMode    │
│  只传递匹配的事件    │
└─────────────────────┘
```

### 3.2 EventEmitter

```rust
/// 执行引擎内部使用的事件发射器
/// 所有事件通过此对象发送，由 StreamReceiver 根据 mode 过滤
pub(crate) struct EventEmitter<S: State> {
    tx: mpsc::Sender<StreamEvent<S>>,
    mode: StreamMode,
}

impl<S: State> EventEmitter<S> {
    pub fn new(tx: mpsc::Sender<StreamEvent<S>>, mode: StreamMode) -> Self {
        Self { tx, mode }
    }

    /// 发送事件（如果当前 mode 不需要此类事件，直接丢弃）
    pub async fn emit(&self, event: StreamEvent<S>) {
        // 忽略发送失败（接收端已关闭 = 调用方不再关心）
        let _ = self.tx.send(event).await;
    }

    /// 创建 StreamWriter handle（传递给节点用于 custom streaming）
    pub fn stream_writer(&self, node: String) -> StreamWriter<S> {
        StreamWriter::new(self.tx.clone(), node, self.mode.clone())
    }

    fn should_emit(&self, event: &StreamEvent<S>) -> bool {
        match &self.mode {
            StreamMode::Values => matches!(event,
                StreamEvent::Values { .. }
                | StreamEvent::FilteredValues { .. }
                | StreamEvent::End { .. }
            ),
            StreamMode::Updates => matches!(event,
                StreamEvent::Updates { .. }
                | StreamEvent::FilteredUpdates { .. }
                | StreamEvent::End { .. }
            ),
            StreamMode::Messages => matches!(event, StreamEvent::Messages { .. } | StreamEvent::End { .. }),
            StreamMode::Custom => matches!(event, StreamEvent::Custom { .. } | StreamEvent::End { .. }),
            StreamMode::Debug => true, // Debug 模式接收所有事件
            StreamMode::Multi(modes) => modes.iter().any(|m| {
                EventEmitter::<S>::mode_matches(m, event)
            }),
        }
    }

    /// has_nostream_tag() 方法检查 CallOptions.tags
    /// 是否包含 "nostream" 标记，用于跳过特定 LLM 调用的流式输出。此方法与 should_emit()
    /// 集成，支持细粒度的流式事件过滤。
    pub fn has_nostream_tag(&self, options: Option<&CallOptions>) -> bool {
        options
            .is_some_and(|opts| opts.tags.iter().any(|tag| tag == "nostream"))
    }
}
```

### 3.3 StreamWriter（节点侧 API）

```rust
/// 传递给节点的 stream writer handle
/// 节点可通过此对象发送自定义流式数据
#[derive(Clone)]
pub struct StreamWriter<S: State> {
    tx: Option<mpsc::Sender<StreamEvent<S>>>,
    node: String,
    mode: StreamMode,
    ns: Vec<String>,
}

impl<S: State> StreamWriter<S> {
    /// 创建连接到真实 channel 的 writer
    pub const fn new(
        tx: mpsc::Sender<StreamEvent<S>>,
        node: String,
        mode: StreamMode,
    ) -> Self {
        Self {
            tx: Some(tx),
            node,
            mode,
            ns: Vec::new(),
        }
    }

    /// 创建断开连接的 writer（no-op send）。
    ///
    /// 当当前执行未配置 streaming 时使用。
    pub const fn disconnected(node: String, mode: StreamMode) -> Self {
        Self {
            tx: None,
            node,
            mode,
            ns: Vec::new(),
        }
    }

    /// 发送自定义数据到流。
    ///
    /// 如果 writer 断开连接或事件不匹配配置的 [`StreamMode`]，
    /// 静默丢弃事件。
    pub async fn send(&self, data: serde_json::Value) {
        let Some(ref tx) = self.tx else {
            return;
        };

        let event = StreamEvent::Custom {
            node: self.node.clone(),
            data,
            ns: self.ns.clone(),
        };

        let emitter = EventEmitter::new(tx.clone(), self.mode.clone());
        if emitter.should_emit(&event) {
            let _ = tx.send(event).await;
        }
    }

    /// 创建子命名空间的 writer（用于子图）
    pub fn with_ns(&self, ns_segment: String) -> Self {
        let mut new_ns = self.ns.clone();
        new_ns.push(ns_segment);
        Self {
            tx: self.tx.clone(),
            node: self.node.clone(),
            mode: self.mode.clone(),
            ns: new_ns,
        }
    }
}
```

### 3.4 Channel 容量与背压

```rust
/// stream() 方法创建 channel 并返回 receiver
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    pub async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<impl Stream<Item = Result<StreamEvent<S>, JunctureError>>, JunctureError> {
        // output_keys: 可选的字段名列表，限制 Values/Updates 事件只包含指定字段
        // 如果为 None，包含所有字段
        // channel 容量：
        // - Messages 模式需要较大 buffer（LLM token 产生速度快）
        // - 其他模式 buffer 较小即可
        let capacity = match &mode {
            StreamMode::Messages | StreamMode::Debug => 256,
            StreamMode::Multi(modes) if modes.contains(&StreamMode::Messages) => 256,
            _ => 32,
        };

        let (tx, rx) = mpsc::channel(capacity);
        let emitter = EventEmitter::new(tx, mode);

        // 在后台 task 中执行图
        let graph = self.clone();
        let config = config.clone();
        tokio::spawn(async move {
            let result = graph.execute_with_emitter(input, &config, emitter).await;
            // 执行完成或出错时，channel 自动关闭（tx drop）
            if let Err(e) = result {
                // 错误通过 channel 传递给调用方
                // （如果 tx 已关闭则忽略）
            }
        });

        Ok(ReceiverStream::new(rx))
    }
}
```

背压策略：
- `mpsc::channel` 有固定容量，满时 `send().await` 会阻塞发送方
- 这自然地对执行引擎施加背压：如果消费方处理慢，引擎会等待
- Messages 模式使用较大 buffer 避免 LLM streaming 被频繁阻塞
- 如果消费方 drop receiver，后续 send 静默失败，引擎继续执行但不再发送事件

---

## 4. Message Streaming（token 级别）

### 4.1 与 LLM Provider 集成

LLM provider 的 `stream()` 方法返回 `BoxStream<MessageChunk>`。在节点内部调用 LLM 时，框架自动将 chunks 转发到 stream：

```rust
// 框架内部：ToolNode 或 agent 节点调用 LLM 时
async fn call_llm_streaming(
    model: &dyn ChatModel,
    messages: &[Message],
    options: Option<&CallOptions>,
    emitter: &EventEmitter<S>,
    node_name: &str,
) -> Result<Message, LlmError> {
    let mut stream = model.stream(messages, options).await?;
    let mut full_content = String::new();
    let mut tool_calls = Vec::new();
    let mut total_usage = TokenUsage::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;

        // 累积完整消息
        full_content.push_str(&chunk.content);
        // ... 累积 tool_calls ...

        // 同时转发到 stream
        emitter.emit(StreamEvent::Messages {
            chunk: chunk.clone(),
            metadata: MessageStreamMetadata {
                node: node_name.to_string(),
                model: model.model_name().to_string(),
                tags: vec![],
                ns: vec![],
            },
        }).await;
    }

    Ok(Message::ai(full_content).with_tool_calls(tool_calls).with_usage(total_usage))
}
```

### 4.2 过滤

调用方可以按节点名或标签过滤 Messages 事件：

```rust
let stream = app.stream(input, &config, StreamMode::Messages).await?;

// 只关心 "agent" 节点的 LLM 输出
let filtered = stream.filter(|event| {
    matches!(event, Ok(StreamEvent::Messages { metadata, .. }) if metadata.node == "agent")
});
```

### 4.3 nostream 标记

节点可以标记某些 LLM 调用不产生 stream 事件（如内部推理、工具选择）：

```rust
// 通过 CallOptions 控制
let options = CallOptions {
    tags: vec!["nostream".to_string()],
    ..Default::default()
};
let result = model.invoke(messages, Some(&options)).await?;
// 此调用不会产生 Messages 事件
```

**Implementation Note (C-05-003)**: nostream 标记过滤已全面实现：
- `EventEmitter::has_nostream_tag()` 方法检查 CallOptions.tags 是否包含 "nostream"
- 与 should_emit() 集成，在事件发射前进行标记检查
- 支持每条消息级别的标记过滤，允许细粒度控制流式输出

---

## 5. 子图 Streaming

### 5.1 命名空间传播

子图执行时，EventEmitter 携带命名空间信息。子图产生的所有事件都带有 `ns` 字段标识来源：

```rust
// 父图执行子图时
async fn execute_subgraph(
    subgraph: &CompiledGraph<Sub>,
    input: Sub,
    config: &RunnableConfig,
    parent_emitter: &EventEmitter<S>,
    subgraph_name: &str,
) -> Result<Sub::Update, JunctureError> {
    // 创建子图的 emitter，继承父图的 ns 并追加当前子图标识
    let sub_emitter = parent_emitter.with_subgraph_ns(subgraph_name);
    subgraph.execute_with_emitter(input, config, sub_emitter).await
}
```

### 5.2 事件格式

子图事件的 `ns` 字段是路径数组：

```rust
// 根图 "agent" 节点的事件
StreamEvent::Messages { ns: vec![], .. }

// 一级子图 "review" 中 "critic" 节点的事件
StreamEvent::Messages { ns: vec!["review".to_string()], .. }

// 嵌套子图的事件
StreamEvent::Messages { ns: vec!["review".to_string(), "detail_check".to_string()], .. }
```

### 5.3 subgraphs 参数

默认情况下，子图事件不传播到父图的 stream。通过配置启用：

```rust
pub struct StreamConfig {
    pub mode: StreamMode,
    /// 是否包含子图事件
    pub include_subgraphs: bool,
    /// 只包含指定命名空间的子图事件（空 = 全部）
    pub subgraph_filter: Vec<String>,
}

// > **实现备注 (D-05-6)**: 实际实现中 `subgraph_filter` 字段类型为 `Option<Vec<String>>` 而非 `Vec<String>`。
// > `None` 表示包含所有子图事件（等价于设计中的"空 = 全部"），`Some(vec![])` 表示不包含任何子图事件，
// > `Some(names)` 表示只包含指定命名空间。使用 `Option` 包装更符合 Rust 惯用法，
// > 避免了"空 Vec 有两种含义"的歧义。
```

---

## 6. Stream 生命周期

### 6.1 创建

```rust
// stream() 调用时创建 channel + 后台执行 task
// 每个 stream 调用会生成唯一的 run_id（UUID）
// run_id 用于：1) 日志关联  2) 流重连  3) 取消特定 run
let stream = app.stream(input, &config, StreamMode::Values).await?;

// Stream 重连：如果连接中断，客户端可使用 run_id 恢复
// 从最近的 checkpoint + run_id 恢复，跳过已发送的事件
let config_with_run = config.clone().with_run_id("previous-run-uuid");
let resumed_stream = app.stream(input, &config_with_run, StreamMode::Values).await?;
```

**run_id 匹配逻辑**：

```rust
/// Stream 重连时的事件过滤
///
/// 通过 run_id + checkpoint_id 确定恢复点。
/// 只发送恢复点之后的事件，跳过已确认的事件。
pub struct StreamResumption {
    /// 原始 run 的 ID
    pub run_id: String,
    /// 最后确认的 checkpoint_id
    pub last_checkpoint_id: Option<String>,
    /// 最后确认的 step
    pub last_step: Option<usize>,
}

impl StreamResumption {
    /// 判断事件是否应被跳过（已发送过的）
    pub fn should_skip(&self, event: &StreamEvent<()>) -> bool {
        match event {
            StreamEvent::Values { step, .. } | StreamEvent::Updates { step, .. } => {
                if let Some(last) = self.last_step {
                    *step <= last
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}
```

### 6.2 事件发射时机（Pregel loop 中）

```
superstep 开始:
  → Debug(SuperstepStart { step, nodes })

每个节点开始执行:
  → TaskStart { node, task_id, step }

节点内部 LLM streaming:
  → Messages { chunk, metadata }  (逐 token)

节点内部 custom write:
  → Custom { node, data, ns }

每个节点执行完成:
  → TaskEnd { node, task_id, step, duration_ms }
  → Updates { node, update, step }

所有节点 merge 完成:
  → Values { state, step }

checkpoint 保存完成:
  → Debug(CheckpointSaved { checkpoint_id, step })

HITL 中断:
  → Interrupt { node, payload, resumable, ns }

预算超限:
  → BudgetExceeded { reason, usage }

superstep 结束:
  → Debug(SuperstepEnd { step, duration_ms })

图执行完成:
  → End { output }
  → channel 关闭（tx drop）
```

### 6.3 完成与错误

- **正常完成**：发送 `End { output }` 后 drop tx，receiver 收到 `None`
- **错误终止**：发送错误事件后 drop tx
- **取消**：CancellationToken 触发后，执行 task 退出，tx drop，receiver 收到 `None`
- **消费方放弃**：receiver drop 后，后续 send 静默失败，执行继续但不再发送事件

### 6.4 invoke 与 stream 的关系

`invoke` 内部不创建 stream channel，直接执行并返回最终 state。如果需要流式输出，必须使用 `stream`：

```rust
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 阻塞执行，返回最终 state
    pub async fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> Result<S, JunctureError> {
        // 无 emitter，不产生 stream 事件
        self.execute_internal(input, config, None).await
    }

    /// 流式执行，返回事件流
    pub async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<impl Stream<Item = Result<StreamEvent<S>, JunctureError>>, JunctureError> {
        // 创建 emitter + channel，后台执行
        // ...
    }
}
```

---

## 7. 性能考量

### 7.1 事件过滤前置

EventEmitter 在发送前检查 mode，不匹配的事件直接丢弃，不经过 channel。这避免了不需要的事件占用 channel 容量。

### 7.2 Clone 开销

`StreamEvent::Values` 包含完整 state clone。对于大 state，这可能有显著开销。优化策略：
- 使用 `Arc<S>` 共享 state（如果 State 实现了 `Clone` 且内部使用 Arc）
- Values 模式下，只在 superstep 结束时 clone 一次
- 考虑提供 `StreamEvent::ValuesRef` 变体（零拷贝，但限制生命周期）

### 7.3 Messages 模式的吞吐

LLM streaming 可能产生大量小 chunk（每个 token 一个）。优化：
- Messages 模式使用较大 channel buffer（256）
- 考虑批量发送（累积 N 个 chunk 或 M 毫秒后一次性发送）
- 提供 `MessageBatchConfig` 配置批量策略

**Implementation Note (C-05-002)**: MessageBatchConfig 已完整实现，提供可配置的批量优化：
- `max_chunks: 10` — 每批最多累积 10 个 chunk 后发送
- `flush_interval_ms: 100` — 最多等待 100ms 后强制发送（即使未达 max_chunks）
- 这解决了设计中关于吞吐性能的担忧，平衡了延迟与吞吐量。

---

## 8. Crate 组织

Streaming 相关代码位于 `juncture-core` 中：

```
crates/juncture-core/src/
├── stream/
│   ├── mod.rs          # StreamMode, StreamEvent 定义
│   ├── emitter.rs      # EventEmitter（内部）
│   ├── writer.rs       # StreamWriter（节点侧 API）
│   └── config.rs       # StreamConfig
```

StreamWriter 通过 `RunnableConfig` 或节点参数传递给用户代码，不需要用户直接依赖 `juncture-core` 的内部类型。

---

