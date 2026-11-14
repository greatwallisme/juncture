# 12 - MCP 客户端集成与缓存前缀稳定性

## 概述

本文档定义两项能力的实现方案。它们服务的业务形态是：**一个多租户业务系统把 Juncture 当作底层 agent 服务**，固定流程为「业务背景 + 用户身份 → 评估信息完整性 → 必要时向用户追问 → 调 MCP 取数 → 产出报告 → 按修改意见迭代」。

这两项能力共同的约束是**成本与延迟**：固定流程意味着同一租户在多次迭代中重复发送几乎相同的前缀（system 提示、工具表、业务背景），而 MCP 工具表又要动态并入。前缀只要有一个字节漂移，整段缓存作废。

- **第一部分：MCP 客户端**。Juncture 目前完全没有 MCP 支持（`rg -i mcp crates/` 零命中），而该业务场景「不需要 bash 工具、需要外部数据源」，MCP 是主要取数通道，比自建 HTTP 工具更值得接入。
- **第二部分：缓存前缀稳定性**。Juncture 目前没有任何显式缓存控制：`cache_control`、`prompt_cache_key`、`prompt_cache_options` 全部零命中，`ContentPart::Text` 也没有承载断点的字段。也就是说，命中与不命中完全取决于实现是否碰巧稳定。

设计目标：

- **能力边界清晰**：MCP 做成 `Tool` 的生产者，不侵入 Graph / Pregel / State 任何一层。
- **前缀稳定性是硬约束**：把「稳定前缀」本身做成可断言的属性（字节级），而不是靠约定。
- **不做过度实现**：MCP 2026-07-28 已是无状态协议，会话管理、反向请求分发这些历史上的重活全部消失。

---

## 0. 事实基线（取证于 2026-11-13）

本节所有断言都对应到具体文件与行号，改写前请勿凭记忆改动。

### 0.1 Juncture 现状

| 事实 | 位置 |
| --- | --- |
| 完全没有 MCP | `rg -i mcp crates/` 零命中 |
| `ChatModel` 四方法：`invoke` / `stream` / `bind_tools` / `with_structured_output` | `crates/juncture-core/src/llm.rs:191` |
| `CallOptions` 无任何缓存相关字段（temperature / max_tokens / stop / top_p / model_override / tool_choice / response_format / tags） | `crates/juncture-core/src/llm.rs:91` |
| `ToolDefinition` 只有 `name` / `description` / `parameters` | `crates/juncture-core/src/llm.rs:155`、`crates/juncture/src/tools/trait_.rs:15` |
| `ContentPart` 只有 `Text{text}` / `Image` / `Thinking`，**没有** `cache_control` 承载点 | `crates/juncture-core/src/state/messages.rs:48` |
| Anthropic 请求体 `system: Option<String>`（纯字符串，不是 block 数组） | `crates/juncture/src/llm/anthropic.rs:680-684` |
| Anthropic `AnthropicTool` 只有 `name` / `description` / `input_schema` | `crates/juncture/src/llm/anthropic.rs:746-750` |
| `bind_tools` 是「克隆 + 覆写 `tools`」，`Vec` 保序 | `crates/juncture/src/llm/anthropic.rs:667`、`crates/juncture/src/llm/openai.rs:644` |
| react 组装工具表：`tools.iter().map(definition)` 保留调用方顺序 | `crates/juncture/src/prebuilt/react.rs:190-192` |
| `ToolNode` 内部用 `HashMap<String, ToolEntry<S>>` 存工具 | `crates/juncture/src/tools/node.rs:276` |
| 该 `HashMap` 只用于查找；对外只暴露 `tool_names()`（**顺序不定**） | `crates/juncture/src/tools/node.rs:796` |
| system 消息由 `build_messages` 每次新建并前插，`Message::system` 每次生成新 `uuid` | `crates/juncture/src/prebuilt/react.rs:462`、`crates/juncture-core/src/state/messages.rs:237` |
| `Message.id`（uuid）**不进入** wire：OpenAI/Anthropic 转换都不带 `id` | `crates/juncture/src/llm/openai.rs:677-684`、`crates/juncture/src/llm/anthropic.rs:701-704` |
| `TokenUsage` 只有 `input_tokens` / `output_tokens` / `total_tokens`，无 cached 字段 | `crates/juncture-core/src/state/messages.rs:93` |
| 无 `prompt_cache_key` / `prompt_cache_options` / `stream_options` | `rg` 零命中 |
| `Tool` trait：`name` / `description` / `schema` / `invoke(Value) -> Result<String, ToolError>` | `crates/juncture/src/tools/trait_.rs:111` |
| MRTR 可映射的既有原语：`interrupt!` 宏 + `Command.resume: Option<ResumeValue>` | `crates/juncture-core/src/lib.rs:68`、`crates/juncture-core/src/command.rs:23` |
| `juncture` 的 tokio 只开 `rt` / `macros` / `time`，**没有 `process`** | `crates/juncture/Cargo.toml:23` |

### 0.2 一个已证实的真实缺陷（缓存可观测性的前提）

`AnthropicResponse.usage: Option<TokenUsage>`（`crates/juncture/src/llm/anthropic.rs:775`），而 `TokenUsage::total_tokens` 无 `#[serde(default)]`（`crates/juncture-core/src/state/messages.rs:93`）。

Anthropic Messages API 的 usage 形状是：

```json
{"input_tokens": 2095, "output_tokens": 503,
 "cache_creation_input_tokens": 1886, "cache_read_input_tokens": 0}
```

**没有 `total_tokens`**。用与 `TokenUsage` 同构的探针实测：

```
NO_TOTAL ERR: missing field `total_tokens` at line 1 column 50
CACHE ERR:    missing field `total_tokens` at line 1 column 109
```

后果是 `serde_json::from_str::<AnthropicResponse>` 在 `crates/juncture/src/llm/anthropic.rs:367` 直接失败，`invoke` 返回 `LlmError::InvalidResponse("Failed to parse response: ...")`。也就是说只要 Anthropic 回带 usage（正常响应都会带），整条 invoke 路径失败。这条缺陷与本文档第一部分无关，但第二部分的所有缓存观测都以 usage 可用为前提，所以它必须先修。

修复方向（不在本文档施工范围，登记为前置）：给 `TokenUsage` 加 `#[serde(default)]`，并新增 `cache_creation_input_tokens` / `cache_read_input_tokens`（Anthropic）与 `cached_tokens` / `cache_write_tokens`（OpenAI `prompt_tokens_details`）字段。

### 0.3 外部规范基线

**MCP 2026-07-28**（当前 latest，取代 2025-11-25）：

- 移除协议级会话与 `Mcp-Session-Id`；移除 `initialize` / `notifications/initialized` 握手。**MCP 变为无状态**：每个请求在 `_meta` 里携带 `protocolVersion` / `clientCapabilities` / `clientInfo`。
- 新增 `server/discover`（MUST 实现），返回支持的版本、能力与身份。
- 移除 HTTP GET 端点与 `resources/subscribe`，改为 `subscriptions/listen` 单条长连通知流。
- 移除 `ping` / `logging/setLevel` / `notifications/roots/list_changed`；日志级别改为逐请求 `_meta` 指定。
- **MRTR（多轮往返）取代服务端主动请求**：服务端返回 `resultType: "input_required"` + `inputRequests`，客户端带 `inputResponses` + `requestState` 重试原请求（重试必须换新 JSON-RPC id）。`elicitation/create`、`sampling/createMessage`、`roots/list` 都走这条路径。
- 所有结果必带 `resultType`（`complete` / `input_required`）。
- 移除 SSE 断点续传（`Last-Event-ID` / event id）。
- `tools/list` **SHOULD** 返回确定性顺序，理由原文即「enable client-side caching and improve LLM prompt cache hit rates」。
- 列表类结果（`tools/list`、`prompts/list`、`resources/list`、`resources/read`、`resources/templates/list`、`server/discover`）必带 `ttlMs` + `cacheScope`（`public` / `private`）。
- Streamable HTTP 必带 `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` 头，头与体不一致返回 `-32020 HeaderMismatch`。
- 工具名建议 1–128 字符，仅 `A-Za-z0-9_-.`；聚合多服务器时客户端**应**做前缀消歧（`server.name` 不保证唯一）。
- Roots / Sampling / Logging 已被标为 Deprecated，新实现不应采纳。

**Anthropic 缓存前缀**（官方 prompt caching 文档）：

- 缓存覆盖顺序固定：**tools → system → messages**，断点包住其之前全部内容。
- 每请求最多 **4 个** `cache_control` 断点；顶层的自动缓存也占掉 1 个槽位。
- TTL：默认 5 分钟，可选 `{"type":"ephemeral","ttl":"1h"}`（写入价 2x）。
- 最小可缓存长度按模型不同：512 / 1,024 / 2,048 / 4,096 tokens 不等。不足阈值**静默不缓存且不报错**。
- 回看窗口 20 个 block（连续 `tool_use` block 算 1 个位置）。
- `tool_choice` 变化、或 prompt 中任意位置新增/删除图片，都会让缓存整体失效。
- 命中判据看响应 usage：`cache_creation_input_tokens` 与 `cache_read_input_tokens` 同时为 0 即未缓存。
- 官方明示：JSON 里 tool_use 的 key 顺序不稳定（Swift/Go 会随机化）会破坏缓存。

**OpenAI 缓存前缀**（Azure/OpenAI 官方文档，两者请求结构一致）：

- 自动前缀缓存：最小 1,024 tokens，**前 1,024 tokens 必须逐字节相同**。
- GPT-5.6 起：`prompt_cache_key` 变为官方推荐（区分租户/前缀分流路由），`prompt_cache_options.mode` ∈ `implicit` / `explicit`，`prompt_cache_options.ttl` ∈ `30m` / `12h`。
- GPT-5.6 起缓存写入开始计费（1.25x 输入价）；GPT-5.6 之前写入免费。
- 显式断点 `prompt_cache_breakpoint: {"mode":"explicit"}`；每请求最多 4 个新写入，读侧最多回看 50 个断点。
- 命中观测：`usage.prompt_tokens_details.cached_tokens`；GPT-5.6 起另有 `cache_write_tokens`。
- 缓存覆盖内容：messages 数组 + **工具定义** + 结构化输出 schema（schema 会作为前缀拼到 system 前）。

两家共同的一条硬事实：**缓存是前缀哈希，任何位置的一个字节变化都让该断点之后全部失效**。这就是本文档第二部分的全部设计出发点。

---

## 1. MCP 客户端集成

### 1.1 定位：MCP 是 `Tool` 的生产者，不是新的一层

Juncture 已有完整的工具闭环：`Tool` trait → `ToolDefinition` → `bind_tools` → `ToolNode` 执行 → 结果写回 `Message`。MCP 服务器提供的工具，语义上就是「远程的 `Tool`」。

因此**唯一正确的接入点**是让 MCP 客户端产出 `Vec<Box<dyn Tool>>`，交给既有的 `create_agent_with_config(model, tools, config)`。这样：

- Graph / Pregel / State / Checkpoint / Streaming **一行不改**；
- 工具调用的错误处理、拦截器、校验、流式事件全部复用既有实现；
- MCP 断连、重连、超时都收敛在 `Tool::invoke` 内部，对图执行不可见。

**明确不做的**：不引入 `McpNode`、不在 Pregel 里增加 MCP 生命周期、不让 `bind_tools` 感知 MCP。任何把 MCP 抬到图层的做法都会让两条工具执行路径分叉，后续所有工具特性（interceptor / validation / streaming）都要写两遍。

### 1.2 crate 落位

新增 `crates/juncture-mcp`，与 `juncture-checkpoint` 同级（都是「可选外部系统适配层」）。

```
crates/juncture-mcp/src/
├── lib.rs          # 公开 API：McpClient, McpToolset, McpTransport
├── protocol.rs     # JSON-RPC 2.0 帧 + MCP 结果类型（resultType/inputRequests/...）
├── transport/
│   ├── mod.rs      # McpTransport trait
│   ├── stdio.rs    # 子进程 stdio（feature stdio，需 tokio/process）
│   └── http.rs     # Streamable HTTP（feature http，需 reqwest）
├── cache.rs        # tools/list 缓存（ttlMs + cacheScope）
├── naming.rs       # 多服务器工具名消歧
└── tool_adapter.rs # McpTool -> impl Tool
```

依赖方向：`juncture-mcp` → `juncture-core`（仅 `ToolDefinition` / 错误类型）+ `juncture`（`Tool` trait）。**不反向依赖**。

### 1.3 核心类型

```rust
/// 一个 MCP 服务器连接。无状态协议下它只是「端点 + 能力快照」。
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    server_info: ServerInfo,        // 来自 server/discover
    capabilities: ServerCapabilities,
    next_id: AtomicU64,             // JSON-RPC id，必须单调
}

pub trait McpTransport: Send + Sync {
    async fn request(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;
    /// subscriptions/listen 返回的长连通知流；未订阅则为 None
    fn notifications(&self) -> Option<BoxStream<'static, JsonRpcNotification>>;
}

/// 从多个服务器聚合出的工具集，顺序稳定。
pub struct McpToolset {
    tools: Vec<McpToolEntry>,       // Vec，不是 HashMap
}

pub struct McpToolEntry {
    pub qualified_name: String,     // 消歧后的名字，进 wire 的就是它
    pub server: String,
    pub tool: McpTool,              // name/description/inputSchema/outputSchema
}
```

`McpToolset::into_tools()` 产出 `Vec<Box<dyn Tool>>`，直接喂给既有 prebuilt 工厂。

### 1.4 启动握手（无状态协议的正确形态）

2026-07-28 没有 `initialize`。启动序列是：

1. 打 `server/discover`（MUST 由服务端实现），拿到 `supportedVersions`、`capabilities`、`serverInfo`。
2. 选定协议版本（取交集里最高者），此后每个请求都在 `_meta` 里带：
   - `io.modelcontextprotocol/protocolVersion`
   - `io.modelcontextprotocol/clientInfo`
   - `io.modelcontextprotocol/clientCapabilities`
3. 版本不匹配时服务端返回 `UnsupportedProtocolVersionError`，客户端按 `supportedVersions` 降级重试。

**向后兼容探测**（仅 Streamable HTTP 需要）：直接发现代请求；若回 `400`，先解析 body——如果是现代 JSON-RPC 错误（版本不支持 / 缺能力 / 头不匹配）就按现代协议处理，**不要**回退；只有当 body 为空或不是已知现代错误时，才回退到 `initialize` 老协议。

强制要求：`stdio` 场景下 `server.discover` 同样是首选探测手段（规范原文即把它列为 STDIO 上的兼容性探针），但老服务器可能不实现，此时按 `initialize` 回退并**记住该服务器是 legacy 模式**，后续请求不再尝试现代 `_meta`。

### 1.5 MRTR 是这套设计里唯一需要「多轮」的地方

服务端要追加输入时返回：

```json
{"resultType":"input_required",
 "inputRequests":{ "github_login": {"method":"elicitation/create","params":{...}} },
 "requestState":"eyJ..."}
```

客户端必须：**带 `inputResponses` + 原样回传 `requestState` 重试原请求，且换新的 JSON-RPC id**。

这里刻意**不**引入新的图原语。两条路径：

- **默认路径（无人工）**：`Tool::invoke` 内部循环处理。若 `inputRequests` 里的请求客户端能自行满足（例如服务端只是要一个先前工具返回过的 handle），就地填 `inputResponses` 重试。这覆盖绝大多数真实场景（MCP 规范自己说「handle 就是普通工具参数」）。
- **需要真人时**：`invoke` 返回 `ToolError::NeedsInput { requests, request_state }`，由上层把它变成一次 `interrupt!`。**复用既有的 `interrupt!` + `Command.resume`**，不新增恢复机制。

关键约束：MRTR 的重试结果**不可缓存**（规范明文），且 `requestState` 必须逐字节原样回传，任何重编码都可能破坏。

#### 1.5.1 为什么 `interrupt!` 不能在 `Tool::invoke` 内部调用（三条硬约束）

这一点必须写死，否则实现时一定会踩。四处取证：

**约束一：task-local 不跨 `tokio::spawn`。** `INTERRUPT_CONTEXT` 是 `tokio::task_local!`（`crates/juncture-core/src/interrupt/mod.rs:22`），由 `runner.rs:485/511` 在**节点执行**处用 `.scope()` 挂上，实例本身则在 `execute_superstep` 里创建（`runner.rs:323`）。而 `ToolNode` 用 `JoinSet::spawn` 并发执行每个工具调用（`crates/juncture/src/tools/node.rs:521`）。spawn 出的 task **继承不到**父 task 的 task-local，所以工具体内 `interrupt!` 里的 `INTERRUPT_CONTEXT.try_with(...)` 必然走到 `unwrap_or_else` 分支，返回 `"interrupt context not set in task-local"`。

实测验证（最小复现，`tokio::task_local!` + `JoinSet`）：

```
same-task try_with ok      = true
joinSet-spawn try_with ok  = false      ← 工具体内 interrupt! 走这条
rescope-inside-spawn ok    = true       （显式 .scope() 才能恢复）
```

即 `interrupt!` 在工具体内不是「难用」，而是**确定性地返回错误**。

**约束二：索引语义与并发冲突（机制已取证）。** `__interrupt_impl`（`crates/juncture-core/src/interrupt/mod.rs:454`）用 `ctx.next_index()` 自增取号，这个号就是 resume 值数组的下标。而 resume 时的回填依赖 `pending_interrupts` 这个 `Vec` 的**排列顺序**：`match_resume_to_interrupts`（`runner.rs:841`）按下标逐个 `map`，而 `pending_interrupts` 是 `loop_.rs:1575-1581` 从 mpsc channel **按到达顺序** `try_recv` 出来的。

即：`resume_values[i]` 对应「第 i 个到达的中断」。若多个工具并行 spawn 各自中断，到达顺序由调度器决定，`Vec` 的排列在两次运行间可以不同，于是同一个 resume 值可能落到不同的工具调用上。

`design/06-hitl.md:1119` 规则 3 已写明「不要在循环中使用 `interrupt!`——索引语义会变得不可预测」，并行工具是同一条规则的更严重版本：连「第几个」都不稳定。

**约束三：重执行语义会在工具路径上放大。** `design/06-hitl.md:1094` 明确：resume 时**整个节点从头重执行**，`interrupt!` 之前的所有代码都会再跑一次，因此要求「之前的代码必须幂等」。这条在普通节点上尚可约束，但放在工具路径上不成立——`ToolNode` 的节点 future（`react.rs:661-691`）整段重执行意味着**外部工具会被重复调用**（副作用、重复扣费、重复写入）。这不是靠文档提醒能兜住的。

#### 1.5.2 定案：MRTR 的人工分支放在节点层，不在工具层

结论：**`McpTool::invoke` 永不调用 `interrupt!`**。分工是：

```
McpTool::invoke                 → 只做「能自动满足的 MRTR」+ 返回 NeedsInput
ToolNode::execute               → NeedsInput 变成一条 tool message（不中断）
需要真人时，由使用方在【节点】里调度
```

给使用方的形态有两种，都不改核心：

1. **预取模式（推荐，无重执行问题）**：节点先 `interrupt!` 拿到用户输入，再调用工具时把值作为普通参数传入。因为 `interrupt!` 在节点开头、工具调用在它之后，重执行时只会重跑「读 resume 值」这一步，不会重复调工具。
2. **两段式模式**：先跑一次工具，结果写进 state 的 `pending_input` 字段；下一轮由条件边进入一个**专门的「追问节点」**，该节点开头 `interrupt!`，resume 后带着答案回到工具节点重试。工具本身仍保持无中断语义。

两种形态都满足 `06-hitl.md` 的规则 1/2/3，且不需要把 `InterruptContext` 塞进 `ToolRuntime`——**这正是不该做的事**：即使技术上可做（`ToolNode` 在收集结果的循环里、即节点上下文中重新触发），它也会破坏约束二、三，并让工具获得「图中断」这种与工具职责无关的权力。

#### 1.5.3 `ToolError::NeedsInput` 的形状

```rust
ToolError::NeedsInput {
    /// 原样保留，逐字节回传，不得解析后重编码
    request_state: Option<String>,
    /// 需要人类回答的项；key 是服务端给的 request key
    requests: BTreeMap<String, InputRequest>,
}
```

`BTreeMap` 而非 `HashMap`：这个结构会进入 tool message，进而进入对话历史（即缓存前缀的 S3 段），顺序必须确定。这与 §2.4 是同一条理由。

`ToolNode` 把它转成 tool message 时，文本要写成「工具需要补充信息：<字段列表>」而不是错误堆栈——模型据此前向追问或改用其它工具，是本路径的主要出口。

### 1.6 传输实现

**stdio**：`tokio::process` 起子进程，按行读写 JSON-RPC。

- 现状约束：`crates/juncture/Cargo.toml:23` 的 tokio 只有 `rt`/`macros`/`time`，**没有 `process`**。所以 `process` 特性必须在新 crate 里声明，不能去改门面 crate 的 tokio 特性（会波及 wasm 目标）。
- `notifications/cancelled` 只在 stdio 上有意义；进程退出时按进程组 kill（三平台门控：unix 用 `killpg`，windows 用 job object），避免留下孤儿。
- 子进程 stderr 必须单独泵走并落到 `tracing`，否则管道写满会阻塞子进程——这是 stdio 传输最常见的死锁。

**Streamable HTTP**：每个 JSON-RPC 消息一个 POST。

- 必带头：`Accept: application/json, text/event-stream`、`MCP-Protocol-Version`、`Mcp-Method`，`tools/call` 等另带 `Mcp-Name`。
- 响应二选一：`application/json` 单对象，或 `text/event-stream`。客户端必须都支持；SSE 里先到的 `notifications/*` 属于本请求，最后一帧是响应。
- **取消 = 关闭 SSE 流**。没有 `notifications/cancelled`，也不需要。
- `x-mcp-header` 必须支持：读 `inputSchema` 里标了 `x-mcp-header` 的参数，把值镜像成 `Mcp-Param-{Name}` 头；非 ASCII / 控制字符 / 首尾空白要用 `=?base64?{b64}?=` 包裹。不合法的 `x-mcp-header` 工具必须**从 tools/list 结果里剔除并告警**，不能整体失败。
- `Origin` 校验、`X-Accel-Buffering: no`（服务端侧）、SSE 注释行当 keep-alive（客户端必须忽略以 `:` 开头的行）。

### 1.7 工具表获取与缓存

`tools/list` 返回 `ttlMs` + `cacheScope`，且支持游标分页。

```
获取流程：
  if cache 中有未过期条目（now < t_received + ttlMs）:
      return cached
  翻页拉全量（每页独立 ttl，翻页途中 cursor 失效则丢弃全部重来）
  按 ttlMs 落缓存
```

`cacheScope` 的处理直接决定多租户正确性，**不能省**：

- `public` → 进程级共享缓存，key = `(server_id, method, cursor)`。
- `private` → 必须按**授权上下文**分桶，key = `(server_id, auth_fingerprint, method, cursor)`。规范原文：不同 access token 必须用不同缓存。把 `private` 结果放进共享缓存就是跨租户数据泄漏。

收到 `notifications/tools/list_changed`（经 `subscriptions/listen`）时立即失效该服务器条目，不等 TTL。

### 1.8 工具名消歧（多服务器聚合的必要条件）

规范明说：工具名唯一性只在一个服务器内成立，聚合多服务器**会**撞名，且 `serverInfo.name` 不可靠。

方案：`qualified_name = format!("{server_alias}__{tool_name}")`。

- `server_alias` 来自配置（用户可读、稳定、ASCII 小写），默认取 `serverInfo.name` 的规范化形式，冲突时追加序号。
- 进 wire 的名字就是这个 `qualified_name`；`description` 前面追加一行 `[from server X]`，让模型知道来源。
- **`__` 分隔符的选择理由**：MCP 工具名字符集是 `A-Za-z0-9_-.`，`__` 合法；且比 `.` 更不会被模型当成层级误读。

### 1.9 把 MCP 工具变成稳定前缀（与第二部分的接缝）

这是两部分唯一的耦合点，也是整个方案里最容易被写错的地方。

反例（常见写法，**禁止**）：

```rust
// 错：HashMap 迭代顺序不稳定
let defs: Vec<_> = toolset.tools.values().map(|t| t.definition()).collect();
model.bind_tools(defs);
```

正确做法：`McpToolset` 内部用 `Vec` 保存，并且**排序键固定**：

```
排序键 = (server_alias 升序, tool_name 升序)
```

理由：`server_alias` 由用户配置决定，比服务端返回的顺序更稳定；而服务端的顺序即使「确定性」，也可能在一次服务器升级后整体变化。

补充两条：

- `ToolNode` 的 `HashMap`（`crates/juncture/src/tools/node.rs:276`）只用于查找，不参与组装，因此**不需要改**。但 `tool_names()`（同文件 :796）顺序不定，**不得**用于生成工具表。
- 工具表一旦进入缓存前缀，任何工具增删都必须视为「前缀变了」。这在多租户下意味着：A 租户接入新 MCP 服务器，不影响 B 租户的前缀（因为前缀按租户构造），但同一租户内工具集变化会重建一次缓存。

### 1.10 错误模型

```
McpError
├── Transport(io/http)          连接层
├── Protocol(JsonRpcError)      -32601 Method not found 等
├── UnsupportedVersion { supported: Vec<String> }
├── HeaderMismatch              -32020
├── NeedsInput { requests, request_state }   MRTR，需上层决定
└── ToolExecution { message }   工具侧 isError:true，应作为 tool message 交给模型
```

映射到 `ToolError` 时区分两类（MCP 规范明确要求）：

- **协议错误**（未知工具、请求畸形）→ 模型很难自救，作为错误返回。
- **工具执行错误**（`isError: true`）→ **必须**作为 tool message 交给模型，这是模型自我修正的依据。

### 1.11 明确不做

- 不实现 Roots / Sampling / Logging（已 Deprecated）。
- 不实现 `resources/*` / `prompts/*`（该业务场景只需要 tools；资源读取可由工具代替）。留接口位置但不写实现。
- 不做会话粘性、不做 SSE 断点续传（协议已移除）。
- 不做 MCP 服务端（Juncture 只做客户端）。

---

## 2. 缓存前缀稳定性

### 2.1 问题陈述：Juncture 现在无法保证前缀稳定

缓存命中要求「断点之前逐字节相同」。Juncture 当前的实现有三处会破坏这个前提：

**问题一：没有断点能力。** `CallOptions`（`crates/juncture-core/src/llm.rs:91`）只有采样参数；`ContentPart::Text { text }`（`crates/juncture-core/src/state/messages.rs:48`）没有承载 `cache_control` 的字段；Anthropic 的 `system` 是 `Option<String>`（`crates/juncture/src/llm/anthropic.rs:684`），而断点必须打在 block 上。结论：**现在一个断点都发不出去**，完全依赖服务端自动缓存，命中率不可控。

**问题二：工具表顺序不受控。** react 路径（`crates/juncture/src/prebuilt/react.rs:190`）依赖调用方传入的顺序。一旦工具来自 MCP 或任何集合，顺序就可能不稳定；而工具表在 Anthropic 上是前缀的**第一段**（tools → system → messages），它变了整段作废。

**问题三：system 每次重建。** `build_messages`（`crates/juncture/src/prebuilt/react.rs:462`）每次调用都新建 `Message::system(text)`，而 `Message::system` 每次都生成新 `uuid`（`crates/juncture-core/src/state/messages.rs:237`）。

关于问题三需要说清楚一个**容易误判的点**：`Message.id` 目前**不进入 wire**——`convert_message`（`crates/juncture/src/llm/openai.rs:822`）和 Anthropic 的转换只取 `role` / `content` / `tool_calls` / `tool_call_id`，`id` 被丢弃。所以 uuid 目前**不**破坏缓存。但这属于「靠实现细节侥幸」，一旦有人为了排查问题把 `id` 透传，缓存会静默失效。**因此要把它变成有测试钉住的属性**，而不是依赖没人改动。

### 2.2 设计原则

按优先级排序，前一条不满足就不做后一条：

1. **先证明稳定，再加断点**。没有字节级稳定性的断点只会精准地增强一个错误前缀。
2. **断点数量最少**。Anthropic 4 个、OpenAI 每请求 4 个新写入。断点越多，越容易有一个落在易变内容后面，反而降低收益。
3. **不引入新的必填配置**。全部有合理默认值；不配置也能正确工作，只是命中率低一些。
4. **失败要可观测**。命中率必须是可读的数字，否则所有优化都是猜测。

### 2.3 前缀分段（Prefix Segments）

把一次请求的 wire 内容显式切段，每段有明确的生命周期：

| 段 | 内容 | 生命周期 | 默认断点 |
| --- | --- | --- | --- |
| S1 `tools` | 工具定义（含 MCP 工具） | 进程/租户级，仅工具集变化时变 | **是** |
| S2 `system` | 系统提示（含业务背景、输出格式约束） | 租户级 | **是** |
| S3 `history` | 已完成的对话轮次（只追加） | 会话级 | **是** |
| S4 `turn` | 本轮新增（用户输入、工具结果） | 每次请求 | 否 |

这张表的直接推论：**断点必须放在 S1/S2/S3 的末尾，绝不能放在 S4**。S4 每次都变，把断点打在那里等于每次写一次缓存再立刻失效——Anthropic 的「常见错误」原文就是这条。

### 2.4 序列化稳定性：这是最容易写错的一层

前缀哈希是对**序列化后的 JSON 字节**做的，不是对 Rust 结构体做的。因此以下每一处都会独立地破坏缓存：

**A. 字段顺序。** 实测本仓库**未启用** `serde_json/preserve_order`（`rg preserve_order` 零命中），因此 `serde_json::Value` 走 `BTreeMap`，key 天然按字典序，这一点目前是安全的。但这是个**隐性依赖**：任何一处为了性能或可读性打开 `preserve_order`，所有 `Value` 的 key 顺序会立刻变成插入顺序，缓存静默失效且无任何报错。

→ 两道防线：(1) 在 workspace `Cargo.toml` 上固定该特性并加注释说明原因；(2) `tools/list`（含 MCP 的 `inputSchema`）与结构化输出 schema 在**进入 `ToolDefinition` 之前**做一次显式递归排序，不依赖 `Value` 的默认行为。规范化函数只允许有一个实现。

**B. 浮点数。** JSON 里 `1.0` 与 `1` 是不同字节。schema 默认值是常见来源。

→ 规范化的同时把整数值 float 收敛为整数形式。

**C. 工具 key 顺序。** 官方明确点名 Swift/Go 会在 JSON 转换时随机化 key 顺序。Rust 侧我们控制住 `serde_json` 即可，但**工具参数（`tool_calls[].arguments`）来自模型输出**，其 key 顺序不可控。

→ 这条**无法**靠本仓库解决，只能观测：`tool_calls[].arguments` 参与 S3（历史），所以一次工具调用的 key 顺序抖动会让 S3 断点失效。缓解手段是把工具结果作为独立 message 追加（tool 消息只在 S4 侧变动），而不是把参数回写成新内容。

**D. 模型名与版本。** 模型自动升级会让全量缓存失效。

→ 配置层要求 pin 具体版本；文档明确写出这一点（Anthropic 官方同款建议）。

**E. 图片。** Anthropic：prompt 中任意位置新增/删除图片都会让缓存整体失效。`tool_choice` 变化同理。

→ 图片只能出现在 S4；`tool_choice` 保持在全流程恒定。

### 2.5 API 变更（最小面）

**`ContentPart` 增加断点承载点。** 这是 Anthropic 与 OpenAI 都能落到同一个抽象上的唯一位置（两家都是「在某 content block 上标一个 marker」）：

```rust
pub enum ContentPart {
    Text {
        text: String,
        cache_control: Option<CacheControl>,   // 新增；None 时序列化不出现
    },
    Image(ImageData),
    Thinking { text: String, signature: Option<String> },
}

pub struct CacheControl {
    pub ttl: CacheTtl,          // FiveMinutes | OneHour
}
```

刻意**不**在 `CallOptions` 上放断点：断点属于「内容」，不属于「调用参数」。放在 `ContentPart` 上的另一个好处是：OpenAI 的 `prompt_cache_breakpoint` 也标在 content block 上，两家同构。

**`TokenUsage` 增加缓存字段**（同时修 0.2 节那个缺陷）：

```rust
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,                          // Anthropic 不返回，必须 default
    #[serde(default)]
    pub cache_creation_input_tokens: u64,           // Anthropic
    #[serde(default)]
    pub cache_read_input_tokens: u64,               // Anthropic
    #[serde(default)]
    pub cache_write_tokens: u64,                    // OpenAI GPT-5.6+
    #[serde(default)]
    pub cached_tokens: u64,                         // OpenAI prompt_tokens_details
}
```

Anthropic 的 `system: Option<String>` 必须升级为 block 数组才能挂断点，这是本次唯一的破坏性 wire 变更：

```rust
#[serde(skip_serializing_if = "Option::is_none")]
system: Option<Vec<AnthropicSystemBlock>>,

struct AnthropicSystemBlock {
    #[serde(rename = "type")]
    type_: &'static str,                    // "text"
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<AnthropicCacheControl>,
}
```

**向后兼容检查点**：`system` 从 string 变成 array 必须核对——Anthropic Messages API 两种形式都接受，但**老测试的 wire 断言会红**。这属于预期红，要同步更新。

**`bind_tools` 前增加排序保证。** 不需要改 trait，只需在 `create_agent_with_config`（`crates/juncture/src/prebuilt/react.rs:191`）与 `ToolNode` 的组装路径上，对 `ToolDefinition` 做一次固定键排序（`name` 升序）。注意：**不要把这条逻辑塞进 `ChatOpenAI::bind_tools`**——trait 的实现方不该替调用方决定顺序，否则 middleware 包装后的模型行为会不一致。

### 2.6 OpenAI 的额外开关

OpenAI 的自动缓存不要求断点，但要求：

- 前缀 ≥ 1,024 tokens，且前 1,024 逐字节相同。
- 高流量下建议给 `prompt_cache_key`（同一前缀复用同一个 key），否则路由可能落到没有该前缀的机器上（官方给的量级是约 15 req/min 以上）。
- GPT-5.6 起写入计费，所以「每次请求都写一遍」是**花钱**的。显式模式下若无断点则完全不缓存也不产生写入费——这给了一个可用的降级开关。

→ `CallOptions` 增加：

```rust
pub prompt_cache_key: Option<String>,
pub prompt_cache_mode: Option<PromptCacheMode>,  // Implicit | Explicit | Disabled
pub prompt_cache_ttl: Option<PromptCacheTtl>,    // Min30 | Hour12
```

`Disabled` 对应 OpenAI 的 `prompt_cache_options.mode = explicit` + 不给断点；在 Anthropic 上对应「不给任何 `cache_control`」。

### 2.7 断点分配策略

默认（自动）分配，基于 2.3 的分段：

```
S1 末尾（最后一个 tool 定义之后）  breakpoint #1, ttl=5m
S2 末尾（system block 之后）       breakpoint #2, ttl=1h   ← 业务背景最稳定，最值得长 TTL
S3 末尾（最后一条已完成轮次）      breakpoint #3, ttl=5m
```

三条明确规则：

1. **S1/S2 用 1h，S3 用 5m**。理由：S1/S2 是租户级内容，跨会话复用价值最高、变更最少；S3 是会话级，5 分钟滚动命中已经足够。1h 写入贵（2x），所以要把它押在最不可能变的地方。
2. **某段低于模型最小可缓存长度时，不给它打断点**。阈值按模型查表（512 / 1,024 / 2,048 / 4,096）。这不是优化，是正确性：低于阈值时断点无效且**不报错**，留着一个空断点等于浪费 4 个槽位之一。
3. **绝不使用第 4 个断点**。留一个空位是刻意的：Anthropic 的自动缓存、以及未来可能需要的「临时定位某个异常前缀」都占用它。用满 4 个会导致无处调试。

### 2.8 可观测性（命中率的唯一可信来源）

缓存是否命中**只能从响应 usage 读**，不能推断。所以先修 0.2 的缺陷，然后：

```
cache_hit_rate = cache_read_input_tokens / (cache_read_input_tokens + input_tokens)
```

（Anthropic 的 `input_tokens` 是「未命中部分」。OpenAI 的 `cached_tokens` 已含在 `prompt_tokens` 内，公式为 `cached_tokens / prompt_tokens`，两者要在适配层各自归一，不能共用一个公式。）

落地三件事：

1. 把上述字段记到既有 span 属性（`juncture-tracing` 的 `attrs`）与 budget tracker。
2. 稳态判据：**同一租户连续两次请求，第二次的 `cache_read_input_tokens` 应等于第一次的 `cache_creation_input_tokens` 加第一次的 `input_tokens`（在未超 TTL、前缀未变时）**。这是可断言的等式，比「命中率 > 90%」这种阈值断言更强。
3. 前缀漂移检测：把每次请求的 S1+S2 字节做一次哈希并记入 span。哈希变化 ⇒ 前缀变了，可以直接定位是哪一次改动导致的失效，而不是靠猜。

### 2.9 测试策略（这一层的测试就是设计本身）

缓存前缀无法用「结果对不对」来测——答案对了不代表命中。因此测试必须断言**字节**与**计数**：

| 类型 | 断言 |
| --- | --- |
| 稳定性 | 同一输入构造两次请求，序列化后 `assert_eq!(bytes_a, bytes_b)`（固定夹具，覆盖含 MCP 工具的场景） |
| 顺序无关 | 以两种不同工具插入顺序构造，序列化结果必须相同 |
| 断点位置 | 解析产出的 Anthropic 请求体，断言 `cache_control` 只出现在 S1/S2/S3 末尾，且数量 ≤ 3 |
| 低于阈值 | 构造短 system，断言该段**没有** `cache_control` |
| 不污染 volatile | 断言 `cache_control` 不出现在最后一条 message 上 |
| usage 解析 | 用真实 Anthropic usage JSON（含 `cache_creation_input_tokens`、无 `total_tokens`）反序列化必须成功 |
| 命中等式 | mock 服务端两次返回，断言第二次 `cache_read == 第一次 cache_creation + input` |

其中「顺序无关」与「不污染 volatile」这两条是**变异敏感**的：把排序去掉、或把断点打到末尾，它们必然变红。其余几条也都要能在对应实现被破坏时变红。

### 2.10 与 MCP 的接缝（复述 1.9，因为这是唯一耦合点）

- MCP 工具必须按 `(server_alias, tool_name)` 排序后才能进 S1。
- `tools/list` 的 `listChanged` 通知到达 ⇒ S1 变化 ⇒ 前缀失效，需重建缓存条目。
- MCP 的 `ttlMs` 只控制「多久重新拉工具表」，与 LLM 前缀缓存是**两套独立的时间**，不能混用同一个数值。

### 2.11 明确不做

- 不做 response cache（相同请求直接复用模型输出）。这与 prompt cache 是不同层次的机制，本设计只做后者。
- 不做自动压缩/截断来「塞进缓存」。压缩本身改变前缀，属于另一条独立议题。
- 不把断点暴露成图层的概念。断点是 transport 层的事，`StateGraph` 与 `Pregel` 不应知道 `cache_control` 的存在。

---

## 3. 施工顺序

依赖关系决定了顺序不可交换：

1. **修 `TokenUsage`**（0.2 缺陷 + 缓存字段）。不做这一步，命中率无法观测，后续全部只能靠猜。**先写会红的测试**：用真实 Anthropic usage JSON 反序列化。
2. **序列化规范化 + 工具表排序**（2.4 / 2.5 的 `bind_tools` 前排序）。加「顺序无关」「两次构造字节相同」的测试。
3. **`ContentPart::cache_control` + Anthropic `system` 改 block 数组**。同步更新既有 wire 断言。
4. **断点分配策略 + 阈值查表**（2.7）。加「断点只落在 S1/S2/S3」「低于阈值不打点」的测试。
5. **OpenAI 开关**（2.6）。
6. **`juncture-mcp`**：先 `protocol.rs` + `transport/stdio.rs` + `tool_adapter.rs`，做一个只读工具端到端；再补 HTTP 传输；再补 tools/list 缓存与消歧。
7. **跨面验证**：MCP 工具 + 缓存前缀的联合场景（1.9 / 2.10），断言工具集变化只影响 S1。

第 1–5 步与第 6 步之间**没有依赖**，可以并行；但 7 必须最后。

---

## 4. 风险与未决

| 项 | 说明 |
| --- | --- |
| `system` wire 变更 | Anthropic 从 string 改 array 是行为变更，既有测试会红，属预期。需要逐条确认红的是「形状断言」而不是「语义断言」 |
| 1h TTL 成本 | 写入 2x。低频租户用 1h 可能不划算，应可配置为 5m |
| 最小长度查表 | 按模型名查表会随新模型过期。表必须集中一处并有测试锁定，不能散落在调用点 |
| `tool_calls.arguments` key 顺序 | 来自模型输出，不可控（2.4 C）。只能观测，无法根治 |
| MRTR 与 interrupt 的衔接 | **已定案**（§1.5.1–1.5.3）：`interrupt!` 不得在工具体内调用，三条硬约束均已取证。人工分支上移到节点层，工具只负责返回 `NeedsInput` |

### 4.1 需要在实现前再确认的两点

1. **`join_next` 分支的 task-local 归属。** `ToolNode` 收集结果的循环（`crates/juncture/src/tools/node.rs:593` 起）跑在节点 future 里，**继承**了 `INTERRUPT_CONTEXT.scope`（这一点由上面的实测对照项 `same-task try_with ok = true` 保证）。这意味着「在收集侧响应 `NeedsInput` 并中断」技术上确实可行。但它会落在 `interrupt!` 之后的重执行路径上（约束三），所以本设计**明确不用**它。若将来有人想走这条路，必须先解决「重执行时如何避免工具被二次调用」——例如为已执行过的 `tool_call_id` 记一个状态字段。这是独立议题，不在本文档范围。
2. **`InterruptContext` 是 superstep 级共享的，不是节点级的。** 实测 `InterruptContext::new` 在 `execute_superstep` 里创建（`crates/juncture-core/src/pregel/runner.rs:323`），即**一次 superstep 内所有并行节点共享同一个实例**；`next_index()`（`crates/juncture-core/src/interrupt/mod.rs:449` 调用）是这条共享实例上的自增计数器。

   这使约束二比「同一节点内并行工具」更广：同一 superstep 内**多个节点**并行中断时，索引归谁取决于调度顺序。这也是 `06-hitl.md:1119` 规则 3 的根因。对 MCP 的直接影响：**绝不能有第二个会在工具路径上触发中断的来源**，否则会与节点级中断争抢同一个计数器。

   顺带记录一条独立观察（不在本文档施工范围）：该共享语义意味着「同一 superstep 内两个节点都 `interrupt!`」本身就需要谨慎——`match_resume_to_interrupts`（`runner.rs:319`）是它的缓解机制，但索引仍与调度顺序相关。

---
