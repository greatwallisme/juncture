# 08 - LLM 集成与工具系统

## 概述

本模块定义 Juncture 与大语言模型的交互层，包括统一的 ChatModel trait、消息类型系统、多 Provider 实现、Tool 抽象以及预构建 Agent。设计目标：

- 统一抽象：一套 trait 覆盖所有 LLM Provider，用户代码不绑定具体供应商
- 流式优先：invoke 和 stream 是同等重要的一等公民
- 工具集成：Tool trait + ToolNode 实现 ReAct 模式的工具调用闭环
- 预算感知：每次 LLM 调用自动上报 token 消耗，与 Budget 系统联动

---

## 1. 消息类型系统

### 1.1 核心类型

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub content: Content,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    Human,
    #[serde(rename = "assistant")]
    Ai,  // Rust 惯用法命名；serde rename 确保 JSON 序列化为 "assistant"
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Content {
    Text(String),
    MultiPart(Vec<ContentPart>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentPart {
    Text { text: String },
    Image(ImageData),
    /// <!-- Addresses finding: Part3#22 -->
    /// Anthropic API 的 thinking block（扩展思考）
    /// 包含模型的内部推理过程，不影响工具调用逻辑
    Thinking { text: String, signature: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageData {
    pub media_type: String,
    pub source: ImageSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImageSource {
    Base64(String),
    Url(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}
```

### 1.2 Message 构造器

```rust
impl Message {
    pub fn system(content: impl Into<String>) -> Self;
    pub fn human(content: impl Into<String>) -> Self;
    pub fn ai(content: impl Into<String>) -> Self;
    pub fn ai_with_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self;
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self;

    pub fn content_text(&self) -> &str;
    pub fn has_tool_calls(&self) -> bool;
}
```

### 1.3 流式消息块

```rust
#[derive(Clone, Debug)]
pub struct MessageChunk {
    pub role: Option<Role>,
    pub content: String,
    pub tool_call_chunks: Vec<ToolCallChunk>,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug)]
pub struct ToolCallChunk {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}
```

`MessageChunk` 是增量数据。调用方需要累积 chunk 来重建完整 Message：
- `content` 字段直接拼接
- `tool_call_chunks` 按 `index` 分组，`arguments` 字段拼接后 JSON 解析

---

## 2. ChatModel Trait

```rust
#[async_trait]
pub trait ChatModel: Send + Sync + Clone + 'static {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError>;

    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError>;

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self;

    fn with_structured_output<T: JsonSchema + DeserializeOwned>(
        self,
    ) -> StructuredOutputModel<Self, T>
    where
        Self: Sized;

    fn model_name(&self) -> &str;
}
```

### 设计决策

**为什么 `Clone` 约束**：ChatModel 实例需要在多个节点间共享（`create_react_agent` 内部 agent 节点持有模型引用）。`Clone` 允许通过 `Arc` 内部实现廉价克隆，同时保持 trait object 的灵活性。

**为什么 `invoke` 返回完整 Message 而非 Update**：LLM 调用的结果是一条完整的 AI 消息，不是对现有消息的增量修改。节点函数负责将 Message 包装为 State Update。

**为什么 `stream` 返回 `BoxStream`**：不同 Provider 的流式实现差异巨大（SSE、WebSocket、gRPC），`BoxStream` 擦除具体类型，统一消费接口。

### CallOptions

```rust
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub top_p: Option<f32>,
    pub model_override: Option<String>,

    // ─── 审查补充字段 ───

    /// <!-- Addresses finding: Part3#10 -->
    /// 工具选择策略
    pub tool_choice: Option<ToolChoice>,

    /// <!-- Addresses finding: Part3#11 -->
    /// 响应格式（用于结构化输出）
    pub response_format: Option<ResponseFormat>,
}

/// <!-- Addresses finding: Part3#10 -->
/// 工具选择策略
#[derive(Clone, Debug)]
pub enum ToolChoice {
    /// 自动决定是否调用工具
    Auto,
    /// 不调用任何工具
    None,
    /// 必须调用工具
    Required,
    /// 必须调用指定的工具
    Specific { name: String },
}

/// <!-- Addresses finding: Part3#11 -->
/// 响应格式
#[derive(Clone, Debug)]
pub enum ResponseFormat {
    /// JSON 模式（模型输出合法 JSON）
    JsonObject,
    /// 带 JSON Schema 的结构化输出
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        strict: bool,
    },
}
```

`CallOptions` 是每次调用的覆盖参数。`None` 字段使用模型实例的默认值。`model_override` 允许在同一 Provider 实例上临时切换模型（如从 claude-sonnet 到 claude-haiku 做低成本任务）。

`tool_choice` 控制模型是否以及如何调用工具。`response_format` 用于请求结构化 JSON 输出，可与工具调用组合使用。

---

## 3. Provider 实现

### 3.1 ChatAnthropic (`feature = "anthropic"`)

```rust
#[derive(Clone)]
pub struct ChatAnthropic {
    model: String,
    api_key: String,
    base_url: String,
    default_options: CallOptions,
    tools: Vec<ToolDefinition>,
    http_client: reqwest::Client,
    max_tokens: u32,
}

impl ChatAnthropic {
    pub fn new(model: impl Into<String>) -> Self;
    pub fn from_env() -> Self;
    pub fn with_api_key(self, key: impl Into<String>) -> Self;
    pub fn with_base_url(self, url: impl Into<String>) -> Self;
    pub fn with_max_tokens(self, n: u32) -> Self;
    pub fn with_temperature(self, t: f32) -> Self;
}
```

**Anthropic Messages API 格式转换**：

| Juncture 类型 | Anthropic API 字段 |
|---|---|
| `Role::System` | 顶层 `system` 参数（非 messages 数组） |
| `Role::Human` | `{"role": "user", "content": [...]}` |
| `Role::AI` | `{"role": "assistant", "content": [...]}` |
| `Role::Tool` | `{"role": "user", "content": [{"type": "tool_result", ...}]}` |
| `ToolCall` | AI content block `{"type": "tool_use", "id": ..., "name": ..., "input": ...}` |

**流式实现**：使用 SSE（Server-Sent Events）。Anthropic 的流式事件类型：
- `message_start` → 提取 message.id
- `content_block_start` → 新 content block 开始（text 或 tool_use）
- `content_block_delta` → 增量内容
- `content_block_stop` → block 结束
- `message_delta` → stop_reason、usage
- `message_stop` → 消息完成

**工具格式**：
```json
{
  "name": "calculator",
  "description": "Perform arithmetic",
  "input_schema": { "type": "object", "properties": {...} }
}
```

**Extended Thinking 支持**：当模型支持 extended thinking 时，通过 `thinking` content block 返回推理过程。Juncture 将其作为 `ContentPart::Thinking { text }` 保留在 Message 中，不影响工具调用逻辑。

**ModelPricing**：
```rust
pub trait ModelPricing {
    fn input_price_per_mtok(&self) -> f64;
    fn output_price_per_mtok(&self) -> f64;
    fn cost_for_usage(&self, usage: &TokenUsage) -> f64 {
        (usage.input_tokens as f64 * self.input_price_per_mtok()
         + usage.output_tokens as f64 * self.output_price_per_mtok())
         / 1_000_000.0
    }
}
```

内置定价表按模型名称查找，支持用户覆盖。

### 3.2 ChatOpenAI (`feature = "openai"`)

```rust
#[derive(Clone)]
pub struct ChatOpenAI {
    model: String,
    api_key: String,
    base_url: String,
    default_options: CallOptions,
    tools: Vec<ToolDefinition>,
    http_client: reqwest::Client,
}

impl ChatOpenAI {
    pub fn new(model: impl Into<String>) -> Self;
    pub fn from_env() -> Self;
    pub fn with_api_key(self, key: impl Into<String>) -> Self;
    pub fn with_base_url(self, url: impl Into<String>) -> Self;
}
```

**兼容性**：`base_url` 可设置为任何兼容 OpenAI Chat Completions API 格式的服务：
- Groq (`https://api.groq.com/openai/v1`)
- Together AI (`https://api.together.xyz/v1`)
- 本地 vLLM (`http://localhost:8000/v1`)
- Azure OpenAI（需额外 header 配置）

**Function Calling 格式**：
```json
{
  "type": "function",
  "function": {
    "name": "calculator",
    "description": "...",
    "parameters": { "type": "object", ... }
  }
}
```

**流式实现**：SSE，`data: [DONE]` 标记结束。每个 chunk 包含 `choices[0].delta`。

### 3.3 ChatOllama (`feature = "ollama"`)

```rust
#[derive(Clone)]
pub struct ChatOllama {
    model: String,
    base_url: String,
    default_options: CallOptions,
    tools: Vec<ToolDefinition>,
    http_client: reqwest::Client,
}

impl ChatOllama {
    pub fn new(model: impl Into<String>) -> Self;
    pub fn with_base_url(self, url: impl Into<String>) -> Self;
}
```

**API 端点**：`POST {base_url}/api/chat`

**特殊处理**：
- 无 API key（本地服务）
- 流式默认开启（`"stream": true`）
- 工具支持取决于模型能力，部分模型不支持 function calling
- TokenUsage 可能不可用（取决于 Ollama 版本）

---

## 4. Tool 系统

### 4.0 ToolRuntime 注入类型

<!-- Addresses finding: L-13 -->

> 工具执行时可以访问运行时上下文，类似于 Node 的 Runtime 注入

```rust
/// 工具运行时：注入到工具执行中的上下文信息
///
/// 当工具需要访问图状态、配置或流式输出时，
/// 通过 ToolRuntime 注入，而非 Tool trait 的参数。
pub struct ToolRuntime<S: State> {
    /// 当前图状态（只读快照）
    pub state: S,
    /// 当前工具调用 ID（对应 AI 消息中的 tool_call.id）
    pub tool_call_id: String,
    /// 运行配置
    pub config: RunnableConfig,
    /// 跨线程持久化存储
    pub store: Option<Arc<dyn Store>>,
}

impl<S: State> ToolRuntime<S> {
    /// 发射工具输出增量（流式工具结果）
    /// 允许工具在执行过程中持续输出中间结果
    pub fn emit_output_delta(&self, delta: &str) {
        // 通过 config 中的 stream_writer 发送 ToolOutputDelta 事件
        if let Some(tx) = &self.config.stream_writer {
            let _ = tx.send(StreamEvent::Tools(ToolsEvent::ToolOutputDelta {
                tool_call_id: self.tool_call_id.clone(),
                delta: delta.to_string(),
            }));
        }
    }
}
```

**使用方式**：

```rust
/// 需要 ToolRuntime 的工具实现 StatefulTool
impl<S: AgentState> StatefulTool<S> for SearchTool {
    async fn invoke_with_runtime(
        &self,
        input: serde_json::Value,
        runtime: &ToolRuntime<S>,
    ) -> Result<String, ToolError> {
        // 访问当前状态
        let context = &runtime.state.context;

        // 流式输出中间结果
        runtime.emit_output_delta("Searching...");

        let result = self.search(input).await?;
        Ok(result)
    }
}
```

### 4.1 tools_condition() 独立函数

<!-- Addresses finding: L-14 -->

> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py:1800` — `tools_condition()`

```rust
/// 工具条件路由函数：判断是否需要执行工具节点
///
/// 标准模式：检查最后一条消息是否包含 tool_calls
/// 如果有 → 路由到 "tools" 节点
/// 如果没有 → 路由到 END
///
/// 这是 create_react_agent 中条件边的默认路由函数，
/// 也可以独立使用于自定义图。
pub fn tools_condition<S: State + serde::Serialize>(
    state: &S,
    messages_field: &str,  // 默认 "messages"
) -> &'static str {
    // > **Implementation Note (C-08-2)**: The actual implementation requires `S: State + serde::Serialize`
    // > (not just `S: State`) because `has_pending_tool_calls()` serializes state to JSON to extract
    // > the messages array. This is a serialization-based approach rather than direct field access,
    // > providing flexibility for any state type without requiring a `messages()` accessor method.
    // 从 state 中读取消息列表
    // 检查最后一条 AI 消息是否有 tool_calls
    // 有 → "tools"，无 → END
    if has_pending_tool_calls(state, messages_field) {
        "tools"
    } else {
        END
    }
}

/// 使用示例：在自定义图中
graph.add_conditional_edges(
    "agent",
    |state: &MyState| tools_condition(state, "messages"),
    path_map! {
        "tools" => "tools",
        END => END,
    },
);
```

### 4.2 ValidationNode 预构建

<!-- Addresses finding: L-15 -->

> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/chat_agent_executor.py`
> 注意：ValidationNode 在 LangGraph v1 中已标记为 deprecated，但仍可用于某些场景。

```rust
/// 输入验证节点：在图执行前验证输入数据
///
/// 此节点可插入到图的入口处，用于：
/// - 验证消息格式（确保 messages 字段非空、角色正确）
/// - 检查 token 限制（消息长度不超过模型上下文窗口）
/// - 清理/规范化输入数据
///
/// 注意：LangGraph v1 中已 deprecated，Juncture 提供作为便捷工具。
#[derive(Clone)]
pub struct ValidationNode {
    /// 最大允许的输入 token 数
    pub max_input_tokens: Option<u64>,
    /// 自定义验证函数
    pub validator: Option<Arc<dyn Fn(&[Message]) -> Result<(), ToolError> + Send + Sync>>,
}

impl ValidationNode {
    pub fn new() -> Self {
        Self {
            max_input_tokens: None,
            validator: None,
        }
    }

    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.max_input_tokens = Some(max);
        self
    }

    pub fn with_validator(mut self, f: impl Fn(&[Message]) -> Result<(), ToolError> + Send + Sync + 'static) -> Self {
        self.validator = Some(Arc::new(f));
        self
    }
}

impl Node<MessagesState> for ValidationNode {
    async fn call(&self, state: MessagesState, _config: &RunnableConfig) -> Result<Command<MessagesState>, JunctureError> {
        // 验证消息列表非空
        if state.messages.is_empty() {
            return Err(JunctureError::NodeFailed {
                node: "validation".into(),
                source: Box::new(ToolError::InvalidInput("Messages list is empty".into())),
            });
        }

        // 自定义验证
        if let Some(validator) = &self.validator {
            validator(&state.messages).map_err(|e| JunctureError::NodeFailed {
                node: "validation".into(),
                source: Box::new(e),
            })?;
        }

        // 验证通过，不修改状态
        Ok(Command::update(MessagesStateUpdate::default()))
    }

    fn name(&self) -> &str { "validation" }
}
```

> 源码位置: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py:622` — ToolNode 类定义
> 源码位置: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py:793` — ToolNode._func() 执行逻辑
> 源码位置: `langgraph/libs/prebuilt/langgraph/prebuilt/chat_agent_executor.py:278` — create_react_agent()

### 4.1 Tool Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.schema(),
        }
    }
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
    #[error("timeout")]
    Timeout,
}

// > **实现备注 (D-08-4)**: 实际实现中 `ToolError` 额外包含两个变体：
// > `ToolNotFound { name: String }` ——当 AI 返回的 tool_call.name 不在已注册工具列表中时返回；
// > `ValidationError { errors: Vec<String> }` ——当工具输入不符合 JSON Schema 验证时返回。
// > 这些变体使调用方能够区分"工具不存在"和"工具执行失败"等不同错误场景。
// >
// > **Implementation Note (D-08-9)**: `ValidationError` uses `String` (single error) instead of `Vec<String>` (multiple errors), which is a simplification from the design spec.
```

### 4.2 ToolNode

```rust
pub struct ToolNode {
    tools: HashMap<String, Arc<dyn Tool>>,
    handle_errors: bool,
}

impl ToolNode {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self;
    pub fn with_error_handling(self, handle: bool) -> Self;
}
```

#### ToolNode 高级特性

<!-- Addresses finding: H-09 -->
<!-- Addresses finding: Part3#18 -->
<!-- Addresses finding: Part3#19 -->

> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py`
> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_validator.py`
> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/_tool_call_transformer.py`

```rust
/// 高级 ToolNode 配置
pub struct ToolNodeConfig {
    /// 工具列表
    pub tools: Vec<Box<dyn Tool>>,
    /// 错误处理模式
    pub handle_errors: bool,

    // ─── 审查补充高级特性 ───

    /// 工具输入验证：在执行前对 input 进行 JSON Schema 验证
    /// 不符合 schema 的输入直接返回验证错误，不调用工具
    pub validate_input: bool,

    /// 工具调用转换器：在执行前转换工具调用的参数
    /// 例如：将旧版 API 参数格式转换为新版
    pub call_transformer: Option<Box<dyn ToolCallTransformer>>,

    /// <!-- Addresses finding: M-3 -->
    /// 工具调用拦截器：在工具执行前后注入自定义逻辑
    pub interceptor: Option<Arc<dyn ToolInterceptor>>,

    /// 工具条件函数：决定是否需要调用工具
    /// 用于 tools_condition 模式
    pub tools_condition: Option<Arc<dyn Fn(&Message) -> bool + Send + Sync>>,
}

/// <!-- Addresses finding: M-3 -->
/// 工具调用拦截器 trait
///
/// 允许在工具执行前后注入自定义逻辑，例如：
/// - 日志记录和追踪
/// - 参数转换和验证
/// - 缓存和去重
/// - 权限检查
pub trait ToolInterceptor: Send + Sync + 'static {
    /// 工具执行前调用
    /// 返回 Err 会取消工具执行，使用错误消息作为结果
    fn pre_execute(
        &self,
        tool_call: &ToolCall,
        state: &serde_json::Value,
    ) -> BoxFuture<'_, Result<(), ToolError>>;

    /// 工具执行后调用
    /// 可以修改工具返回结果
    fn post_execute(
        &self,
        tool_call: &ToolCall,
        result: &Result<String, ToolError>,
    ) -> BoxFuture<'_, Result<String, ToolError>>;
}

/// 默认空拦截器实现
pub struct NopToolInterceptor;

#[async_trait]
impl ToolInterceptor for NopToolInterceptor {
    async fn pre_execute(&self, _tool_call: &ToolCall, _state: &serde_json::Value) -> Result<(), ToolError> {
        Ok(())
    }

    async fn post_execute(&self, _tool_call: &ToolCall, result: &Result<String, ToolError>) -> Result<String, ToolError> {
        result.clone()
    }
}

/// 工具调用转换器 trait
pub trait ToolCallTransformer: Send + Sync + 'static {
    fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError>;
}

/// 带状态访问的工具执行
/// 工具可以通过 InjectedState 访问当前图的 State
pub trait StatefulTool<S: State>: Tool {
    /// 带 State 的执行
    fn invoke_with_state(
        &self,
        input: serde_json::Value,
        state: &S,
    ) -> BoxFuture<'_, Result<String, ToolError>>;

    /// 带 Store 的执行
    fn invoke_with_store(
        &self,
        input: serde_json::Value,
        store: &dyn Store,
    ) -> BoxFuture<'_, Result<String, ToolError>>;
}

/// 工具执行追踪（集成到 ToolNode 的执行循环中）
pub struct ToolExecutionTrace {
    pub tool_name: String,
    pub tool_call_id: String,
    pub attempt: usize,
    pub first_attempt_time: DateTime<Utc>,
    pub duration_ms: u64,
    pub success: bool,
}
```

**工具输入验证** (`validate_input = true`):

```rust
// 执行工具前验证输入是否符合 schema
fn validate_tool_input(tool: &dyn Tool, input: &serde_json::Value) -> Result<(), ToolError> {
    let schema = tool.schema();
    // 使用 jsonschema crate 进行验证
    let validator = jsonschema::validator(&schema)
        .map_err(|e| ToolError::InvalidInput(format!("Invalid schema: {}", e)))?;

    let result = validator.validate(input);
    if let Err(errors) = result {
        let msg = errors.map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
        return Err(ToolError::InvalidInput(msg));
    }
    Ok(())
}
```

**执行逻辑**：
1. 从 state 的最后一条 AI 消息中提取 `tool_calls`
2. 对每个 tool_call，查找对应的 Tool 实例
3. 所有工具调用通过 `JoinSet` 真正并发执行
4. 收集结果，生成 `Vec<Message>` (role = Tool)
5. 返回 State Update，将 tool result messages append 到 messages

**错误处理策略**：
- `handle_errors = true`（默认）：工具执行失败时，将错误信息包装为 tool result message 返回给 LLM，让 LLM 决定如何处理
- `handle_errors = false`：工具执行失败时，直接返回 `JunctureError::NodeFailed`，终止图执行

```rust
// handle_errors = true 时的错误消息
Message::tool_result(
    tool_call.id,
    format!("Error: {}", err),
)
```

### 4.3 工具查找失败

当 AI 返回的 tool_call.name 不在已注册工具列表中时：
- `handle_errors = true`：返回错误消息 `"Tool '{name}' not found"`
- `handle_errors = false`：返回 `ToolError::InvalidInput`

---

## 5. 预构建 Agent

> 源码位置: `langgraph/libs/prebuilt/langgraph/prebuilt/chat_agent_executor.py:278` — create_react_agent() 完整实现
> 源码位置: `langgraph/libs/prebuilt/langgraph/prebuilt/tool_node.py:622` — ToolNode 类

### 5.1 create_react_agent

```rust
pub fn create_react_agent<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
) -> Result<CompiledGraph<MessagesState>, TopologyError>
```

**内部图结构**：

```
START → agent → [conditional] → tools → agent
                     ↓
                    END
```

**等价的手动构建代码**：

```rust
pub fn create_react_agent<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
) -> Result<CompiledGraph<MessagesState>, TopologyError> {
    let tool_defs: Vec<ToolDefinition> = tools.iter().map(|t| t.definition()).collect();
    let model_with_tools = model.bind_tools(tool_defs);
    let tool_node = ToolNode::new(tools);

    let mut graph = StateGraph::<MessagesState>::new();

    graph.add_node("agent", AgentNode::new(model_with_tools));
    graph.add_node("tools", tool_node);

    graph.set_entry_point("agent");
    graph.add_conditional_edges(
        "agent",
        |state: &MessagesState| {
            if state.messages.last().map_or(false, |m| m.has_tool_calls()) {
                "tools"
            } else {
                END
            }
        },
        [("tools", "tools"), (END, END)],
    );
    graph.add_edge("tools", "agent");

    graph.compile_ephemeral()
}
```

> **Implementation Note (D-08-8)**: `create_react_agent` is implemented in the facade crate (`juncture`) rather than `juncture-core`, following the layered architecture principle where the facade provides convenience functions.

### 5.2 create_react_agent 高级选项

<!-- Addresses finding: H-08 -->

> 参考: `langgraph/libs/prebuilt/langgraph/prebuilt/chat_agent_executor.py:278`

```rust
/// create_react_agent 的高级配置
pub struct ReactAgentConfig<S: State, M: ChatModel> {
    /// LLM 模型
    pub model: M,
    /// 工具列表
    pub tools: Vec<Box<dyn Tool>>,
    /// 系统提示（字符串或函数）
    pub prompt: Option<PromptSource<S>>,
    /// 响应格式（结构化输出）
    pub response_format: Option<ResponseFormat>,
    /// agent 节点执行前的钩子
    pub pre_model_hook: Option<Arc<dyn Node<S>>>,
    /// agent 节点执行后的钩子
    pub post_model_hook: Option<Arc<dyn Node<S>>>,
    /// 自定义 State Schema（默认 MessagesState）
    pub state_schema: PhantomData<S>,
    /// 上下文 Schema 注入
    pub context_schema: Option<PhantomData<()>>,
    /// Store 注入
    pub store: Option<Arc<dyn Store>>,
    /// 中断配置
    pub interrupt_before: Vec<String>,
    pub interrupt_after: Vec<String>,
    /// 动态模型选择：根据 state 选择不同的模型
    pub model_selector: Option<Arc<dyn Fn(&S) -> M + Send + Sync>>,
}

/// 提示来源：静态字符串或动态函数
pub enum PromptSource<S: State> {
    Static(String),
    Dynamic(Arc<dyn Fn(&S) -> String + Send + Sync>),
}

/// 动态模型选择示例
pub fn create_react_agent_with_config<S, M>(
    config: ReactAgentConfig<S, M>,
) -> Result<CompiledGraph<S>, TopologyError>
where
    S: State,
    M: ChatModel,
{
    let mut graph = StateGraph::<S>::new();

    // 注册节点（与基础版相同，但增加 hooks）
    graph.add_node("agent", AgentNode::new(config.model.clone()));
    graph.add_node("tools", ToolNode::new(config.tools));

    // 可选的 pre/post hooks
    if let Some(pre_hook) = config.pre_model_hook {
        graph.add_node("pre_model_hook", pre_hook);
        graph.add_edge("pre_model_hook", "agent");
        graph.set_entry_point("pre_model_hook");
    } else {
        graph.set_entry_point("agent");
    }

    if let Some(post_hook) = config.post_model_hook {
        graph.add_node("post_model_hook", post_hook);
        // agent → post_model_hook → [conditional]
    }

    // ... 边和条件路由 ...

    graph.compile_ephemeral()
}
```

### 5.3 AgentNode

```rust
struct AgentNode<M: ChatModel> {
    model: M,
}

impl<M: ChatModel> Node<MessagesState> for AgentNode<M> {
    async fn process(
        &self,
        state: &MessagesState,
        ctx: &NodeContext,
    ) -> Result<Command<MessagesState>, JunctureError> {
        let response = self.model.invoke(&state.messages, None).await?;
        Ok(Command::update(MessagesStateUpdate {
            messages: Some(vec![response]),
        }))
    }
}
```

### 5.3 扩展选项

```rust
pub struct ReactAgentConfig {
    pub system_message: Option<String>,
    pub max_iterations: Option<usize>,
    pub interrupt_before_tools: bool,
}

pub fn create_react_agent_with_config<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
    config: ReactAgentConfig,
) -> Result<CompiledGraph<MessagesState>, TopologyError>
```

- `system_message`：自动在每次 LLM 调用前注入 system message
- `max_iterations`：限制 agent-tools 循环次数（通过 recursion_limit 实现）
- `interrupt_before_tools`：工具执行前触发 HITL interrupt，人工审批工具调用

> **Implementation Note (C-08-3)**: `juncture` facade crate 的 `ReactAgentConfig` 包含 `system_message` 字段（与设计一致），
> 但 `juncture-core` 的 `ReactAgentConfig<S, M>` 使用 `prompt: Option<PromptSource<S>>` 替代 `system_message`，
> 提供更灵活的 prompt 来源（静态字符串或动态函数）。两个 crate 的配置结构体字段名存在差异。

---

## 6. Structured Output

### 6.1 StructuredOutputModel

```rust
pub struct StructuredOutputModel<M: ChatModel, T: JsonSchema + DeserializeOwned> {
    inner: M,
    _phantom: PhantomData<T>,
}

#[async_trait]
impl<M: ChatModel, T: JsonSchema + DeserializeOwned + Send + 'static> ChatModel
    for StructuredOutputModel<M, T>
{
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // 1. 从 T 的 JsonSchema 生成 ToolDefinition
        // 2. 设置 tool_choice = "required" (或 "any")
        // 3. 调用内部模型
        // 4. 从 tool_call.arguments 反序列化为 T
        // 5. 将 T 序列化为 JSON 字符串作为 message content 返回
    }
}
```

**实现原理**：利用 LLM 的 function calling 能力强制输出结构化 JSON。创建一个虚拟工具，其 schema 就是目标类型 T 的 JSON Schema。设置 `tool_choice` 为强制使用该工具，LLM 的输出就是符合 schema 的 JSON。

**使用示例**：
```rust
#[derive(Deserialize, JsonSchema)]
struct Sentiment {
    score: f32,
    label: String,
    confidence: f32,
}

let model = ChatAnthropic::new("claude-sonnet-4-20250514")
    .with_structured_output::<Sentiment>();

let result = model.invoke(&messages, None).await?;
// result.content 是 Sentiment 的 JSON 表示
```

---

## 7. Budget 集成

### 7.1 自动 Token 追踪

每个 ChatModel 实现在 `invoke` / `stream` 完成后，自动将 `TokenUsage` 上报到当前执行上下文的 `BudgetTracker`。

```rust
// ChatModel 实现内部（以 ChatAnthropic 为例）
async fn invoke(&self, messages: &[Message], options: Option<&CallOptions>) -> Result<Message, LlmError> {
    let response = self.call_api(messages, options).await?;
    let message = self.parse_response(response)?;

    // 自动上报 token 使用
    if let Some(usage) = &message.usage {
        BudgetTracker::current().report_usage(usage, self.cost_for_usage(usage));
    }

    Ok(message)
}
```

### 7.2 ModelPricing Trait

```rust
pub trait ModelPricing {
    fn input_price_per_mtok(&self) -> f64;
    fn output_price_per_mtok(&self) -> f64;

    fn cost_for_usage(&self, usage: &TokenUsage) -> f64 {
        let input_cost = usage.input_tokens as f64 * self.input_price_per_mtok() / 1_000_000.0;
        let output_cost = usage.output_tokens as f64 * self.output_price_per_mtok() / 1_000_000.0;
        input_cost + output_cost
    }
}
```

内置定价表（可通过 `with_pricing` 覆盖）：

| 模型 | Input $/MTok | Output $/MTok |
|---|---|---|
| claude-sonnet-4-20250514 | 3.00 | 15.00 |
| claude-haiku-4-5-20251001 | 0.80 | 4.00 |
| gpt-4o | 2.50 | 10.00 |
| gpt-4o-mini | 0.15 | 0.60 |

### 7.3 BudgetTracker 交互

`BudgetTracker` 通过 task-local 变量传递到 LLM 调用层。Pregel 执行引擎在启动节点执行前设置 task-local context，LLM Provider 在调用完成后自动上报。用户无需手动传递或配置。

---

## 8. 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("authentication failed: {0}")]
    AuthError(String),

    #[error("rate limited, retry after {retry_after:?}")]
    RateLimited { retry_after: Option<Duration> },

    #[error("context length exceeded: {used} tokens used, {limit} limit")]
    ContextLengthExceeded { used: u64, limit: u64 },

    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("content filtered")]
    ContentFiltered,

    #[error("timeout after {0:?}")]
    Timeout(Duration),
}
```

// > **实现备注 (D-08-7)**: 实际实现中 `LlmError` 额外包含 `Other(#[source] Box<dyn std::error::Error + Send + Sync>)`
// > 捕获所有变体。这为 Provider 实现提供了兜底机制，允许返回未预见的错误类型而不需要
// > 扩展 enum 本身。该变体使用 `#[source]` 属性保留错误链追踪能力。
```

**重试策略**：`RateLimited` 错误携带 `retry_after` 信息。上层可实现自动重试（指数退避 + jitter）。Juncture 不在 ChatModel trait 层面强制重试，而是提供 `RetryingModel<M>` 包装器：

```rust
pub struct RetryingModel<M: ChatModel> {
    inner: M,
    max_retries: usize,
    initial_backoff: Duration,
}
```

---

## 9. 依赖关系

| 依赖 | 用途 | Feature Gate |
|---|---|---|
| `reqwest` | HTTP 客户端（SSE streaming） | anthropic, openai, ollama |
| `schemars` | 从 Rust 类型生成 JSON Schema | 始终启用 |
| `eventsource-stream` | SSE 解析 | anthropic, openai |
| `serde` / `serde_json` | 序列化 | 始终启用 |
| `async-trait` | 异步 trait | 始终启用 |
| `futures` | BoxStream | 始终启用 |

---

## 10. 模块文件结构

```
crates/juncture/src/
├── llm/
│   ├── mod.rs              # 导出 ChatModel, CallOptions, Message 等
│   ├── trait.rs            # ChatModel trait 定义
│   ├── message.rs          # Message, Role, Content, ToolCall, TokenUsage
│   ├── anthropic.rs        # ChatAnthropic 实现
│   ├── openai.rs           # ChatOpenAI 实现
│   ├── ollama.rs           # ChatOllama 实现
│   ├── mock.rs             # MockChatModel（测试用）
│   ├── pricing.rs          # ModelPricing trait + 内置定价表
│   ├── retry.rs            # RetryingModel 包装器
│   └── structured.rs       # StructuredOutputModel
├── tools/
│   ├── mod.rs              # 导出 Tool, ToolDefinition, ToolNode
│   ├── trait.rs            # Tool trait 定义
│   ├── node.rs             # ToolNode 实现
│   └── error.rs            # ToolError
└── prebuilt/
    ├── mod.rs              # 导出 create_react_agent
    └── react.rs            # ReAct agent 构建逻辑
```

