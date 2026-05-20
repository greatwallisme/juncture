# Human-in-the-Loop (HITL)

## 概述

Human-in-the-Loop 是 LangGraph 的核心能力之一，允许图执行在特定节点暂停，等待人类输入后继续。Juncture 实现等价语义，但利用 Rust 的类型系统和 tokio task-local 机制提供更安全的实现。

---

## 1. LangGraph 参考架构

### 核心机制

LangGraph 的 HITL 基于 checkpoint + 重新执行模型：

1. 节点内调用 `interrupt(payload)` 函数
2. 引擎捕获 `GraphInterrupt` 异常，将 payload 写入 `INTERRUPT` channel
3. 持久化当前 checkpoint（包含中断元数据）
4. 向调用方返回中断事件
5. 调用方通过 `Command(resume=value)` 恢复执行
6. 引擎从 checkpoint 恢复状态，将 resume value 写入 `RESUME` channel
7. **重新执行被中断的节点**——`interrupt()` 这次从 `RESUME` channel 读取值并返回

### 关键行为

- **节点重新执行**：resume 时整个节点从头执行，不是从中断点继续
- **多重中断**：同一节点内多个 `interrupt()` 调用按顺序索引匹配 resume values
- **子图传播**：子图内的 interrupt 冒泡到顶层图
- **interrupt_before / interrupt_after**：编译时配置的节点集合，在节点执行前/后自动中断

---

## 2. Juncture 中断机制设计

### 2.1 interrupt! 宏

```rust
/// 在节点内声明中断点
///
/// 第一次执行：发送中断信号，节点返回 Err(Interrupted)
/// resume 后执行：返回人类提供的 resume value
#[macro_export]
macro_rules! interrupt {
    ($payload:expr) => {{
        $crate::hitl::__interrupt_impl(
            $crate::hitl::__current_interrupt_index(),
            ::serde_json::to_value(&$payload).expect("interrupt payload must be serializable"),
        )
        .await
    }};
}
```

### 2.2 内部实现

<!-- Addresses finding: Part2#2 -->

#### InterruptContext（Arc-based 实现）

使用 `Arc` 替代 `task_local` + `RefCell`，避免跨 `await` 的 `RefCell` 借用问题
和未设置 `INTERRUPT_CTX` 时的 panic 风险：

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// 中断上下文（Arc-based，可安全跨 await 传递）
pub struct InterruptContext {
    /// resume 时携带的输入值（按中断 ID 索引）
    resume_values: Arc<[Option<serde_json::Value>]>,
    /// 当前中断索引计数器
    current_index: Arc<AtomicUsize>,
    /// 中断信号发送端
    interrupt_tx: mpsc::UnboundedSender<InterruptSignal>,
}

impl InterruptContext {
    pub fn new(
        resume_values: Vec<Option<serde_json::Value>>,
        interrupt_tx: mpsc::UnboundedSender<InterruptSignal>,
    ) -> Self {
        Self {
            resume_values: resume_values.into_boxed_slice().into(),
            current_index: Arc::new(AtomicUsize::new(0)),
            interrupt_tx,
        }
    }

    /// 获取下一个中断索引
    fn next_index(&self) -> usize {
        self.current_index.fetch_add(1, Ordering::Relaxed)
    }

    /// 尝试获取 resume 值（按索引）
    fn get_resume_value(&self, index: usize) -> Option<serde_json::Value> {
        self.resume_values.get(index).and_then(|v| v.clone())
    }
}

pub struct InterruptSignal {
    /// 中断索引（兼容旧模式）
    pub index: usize,
    /// <!-- Addresses finding: H-15 -->
    /// 中断 ID（命名中断，可选）
    /// 使用 xxhash 生成确定性 ID
    pub id: Option<String>,
    /// 中断 payload
    pub payload: serde_json::Value,
}
```

#### __interrupt_impl 函数

<!-- Addresses finding: H-15 -->
<!-- Addresses finding: Part3#14 -->

支持命名中断（ID-based）和索引中断（兼容模式）：

```rust
/// <!-- Addresses finding: H-5 -->
/// 生成确定性中断 ID
/// 参考: `langgraph/types.py:522-578` — xxhash-based deterministic IDs
/// 使用 xxhash_rust::xxh3::xxh3_128 替代 XxHash64，提供更强的哈希碰撞抵抗
/// 输出 32 字符十六进制字符串（128-bit）
fn generate_interrupt_id(node_name: &str, index: usize) -> String {
    use std::hash::{Hash, Hasher};
    use xxhash_rust::xxh3;
    let mut hasher = xxh3::Xxh3::new();
    node_name.hash(&mut hasher);
    index.hash(&mut hasher);
    let hash = hasher.finish128();
    format!("{:016x}{:016x}", hash.0, hash.1)
}

/// 带命名 ID 的中断实现
pub async fn __interrupt_impl(
    ctx: &InterruptContext,
    payload: serde_json::Value,
    id: Option<&str>,
) -> Result<serde_json::Value, JunctureError> {
    let index = ctx.next_index();

    // 生成确定性 ID（如果用户未指定）
    let interrupt_id = id.map(|s| s.to_string())
        .unwrap_or_else(|| generate_interrupt_id("current_node", index));

    // resume 路径：查找对应 ID 的 resume value
    if let Some(value) = ctx.get_resume_value(index) {
        return Ok(value);
    }

    // 首次执行路径：发送中断信号
    ctx.interrupt_tx
        .send(InterruptSignal {
            index,
            id: Some(interrupt_id),
            payload,
        })
        .map_err(|_| JunctureError::Internal("interrupt channel closed".into()))?;

    Err(JunctureError::Interrupted { index })
}
```
```

### 2.3 执行引擎集成

#### 首次执行流程

```
节点执行
  │
  ├─ interrupt!(payload)
  │     ├─ INTERRUPT_CTX 无 resume_values[index]
  │     ├─ 发送 InterruptSignal { index: 0, payload }
  │     └─ 返回 Err(JunctureError::Interrupted { index: 0 })
  │
  ▼
Pregel 引擎捕获 Interrupted
  │
  ├─ 收集所有 InterruptSignal（可能有多个，如果节点在 interrupt 前 spawn 了其他中断）
  ├─ 构建 InterruptMetadata { node, signals, step }
  ├─ 持久化 checkpoint（source: Interrupt）
  │     checkpoint.pending_interrupts = signals
  │     checkpoint.next = [interrupted_node]  // 标记需要重新执行的节点
  └─ 向 stream 发送 StreamEvent::Interrupt { node, payloads, resumable: true }
```

> **Implementation Note (C-06-3)**: 当前 `StreamEvent::Interrupt` 的 `ns`（namespace）字段始终发送空 `Vec`。
> 子图中断事件因此无法正确携带命名空间信息，影响子图隔离场景下的中断事件路由。
> 未来需要在发送时传入实际的子图命名空间栈。

#### Resume 流程

```
graph.resume(values, config)
  │
  ├─ 从 checkpoint 加载状态
  │     state = deserialize(checkpoint.state)
  │     interrupted_node = checkpoint.next[0]
  │
  ├─ 构建 InterruptContext {
  │     resume_values: values,  // Vec<Value>，按索引对应
  │     current_index: 0,
  │     interrupt_tx: new_channel(),
  │   }
  │
  ├─ 设置 task-local INTERRUPT_CTX
  │
  └─ 重新执行 interrupted_node
        │
        ├─ interrupt!(payload)
        │     ├─ INTERRUPT_CTX 有 resume_values[0]
        │     └─ 返回 Ok(resume_values[0].clone())
        │
        ├─ 节点继续执行...
        │
        └─ 返回 NodeOutput::Update(update)
```

---

## 3. 多重中断

### null-resume 语义

<!-- Addresses finding: H-6 -->

> 参考: `langgraph/_internal/_scratchpad.py` — Scratchpad.get_null_resume()

当使用命名中断时，scratchpad 提供 `get_null_resume()` 机制，允许在不需要显式 resume 值的情况下解决中断：

```rust
/// Scratchpad 的 null-resume 语义
///
/// 当调用 resume 时未为某个中断 ID 提供值，
/// scratchpad 可以通过 get_null_resume() 解析中断。
/// 这对于只需要"确认继续"而不需要实际数据的中断点很有用。
impl Scratchpad {
    /// 检查中断是否可以通过 null-resume 解决
    /// null-resume: 中断已标记为 processed，但 resume value 为 None
    pub fn get_null_resume(&self, interrupt_id: &str) -> bool {
        self.is_interrupt_processed(interrupt_id)
    }
}
```

**多中断匹配算法**：

```
resume 时匹配算法:
  │
  ├─ 输入: resume_map (HashMap<interrupt_id, value>)
  │
  ├─ 1. 对 scratchpad 中每个未处理的中断:
  │     ├─ 如果 resume_map 包含该中断的 ID → 使用 map 中的值
  │     ├─ 如果 resume_map 不包含但中断已标记 processed → null-resume
  │     └─ 否则 → 中断仍然挂起，节点再次中断
  │
  ├─ 2. 兼容模式: 如果 resume 值是 Vec<Value>（非 HashMap）:
  │     按 index 顺序匹配，跳过已 processed 的中断
  │
  └─ 3. 全局匹配: 如果 resume 值是单一 Value (非 Vec, 非 HashMap):
       作为所有挂起中断的通用 resume 值
```

### 设计

同一节内可以有多个 `interrupt!()` 调用。每次调用通过 `__current_interrupt_index()` 获取递增索引。Resume 时提供的 values 是一个 Vec，按索引位置匹配。

### 行为规则

- 第一次执行：遇到第一个无法满足的 `interrupt!()` 即中止（不会继续执行后续代码）
- Resume 时提供 N 个 values：节点重新执行，前 N 个 `interrupt!()` 直接返回对应 value
- 如果节点有 M 个中断点（M > N），第 N+1 个 `interrupt!()` 再次中断

### 示例

```rust
async fn multi_step_review(state: AgentState) -> Result<AgentStateUpdate> {
    // 第一个中断点（index 0）：请求内容审核
    let content_decision: serde_json::Value = interrupt!(json!({
        "type": "content_review",
        "content": state.draft,
        "question": "内容是否合规？"
    }))?;

    if !content_decision["approved"].as_bool().unwrap_or(false) {
        return Ok(AgentStateUpdate {
            status: Some("content_rejected".into()),
            ..Default::default()
        });
    }

    // 第二个中断点（index 1）：请求最终批准
    let final_decision: serde_json::Value = interrupt!(json!({
        "type": "final_approval",
        "content": state.draft,
        "question": "是否批准发布？"
    }))?;

    Ok(AgentStateUpdate {
        status: Some(if final_decision["approved"].as_bool().unwrap_or(false) {
            "published"
        } else {
            "final_rejected"
        }.into()),
        ..Default::default()
    })
}

// 调用方：
// 第一次 invoke → 中断于 index 0
// resume([json!({"approved": true})]) → 通过 index 0，中断于 index 1
// resume([json!({"approved": true}), json!({"approved": true})]) → 全部通过
```

### 重要约束

Resume 时必须提供从 index 0 开始的**完整** values 列表。不能只提供 index 1 的值而跳过 index 0——因为节点从头重新执行，必须按顺序通过所有中断点。

---

## 4. interrupt_before / interrupt_after

### should_interrupt 版本门控

<!-- Addresses finding: C-2 -->

> 参考: `langgraph/pregel/_algo.py:155` — `should_interrupt()` 函数

在检查节点名称是否在 `interrupt_before` / `interrupt_after` 集合中之前，必须先进行版本门控检查：

```
should_interrupt 算法:

1. 版本门控检查：
   - 比较 channel_versions 与 versions_seen["__interrupt__"]
   - 如果没有任何 channel 自上次中断以来被更新
     (即所有 channel 版本号与上次中断时相同)
   - 则跳过中断（版本未变化，无需再次中断）

2. 节点名称检查（仅在版本门控通过后）：
   - 对 pending_tasks 中的每个节点，检查是否在 interrupt_before 集合中
   - 如果匹配 → 触发中断

版本门控的意义：
- 防止重复中断：如果 checkpoint 恢复后没有任何 channel 变化，
  不应该再次触发 interrupt_before
- 确保 interrupt_before 只在有实际状态变更时触发
```

```rust
fn should_interrupt(
    pending_tasks: &[PendingTask],
    interrupt_before: &HashSet<String>,
    channel_versions: &HashMap<String, u64>,
    versions_seen_for_interrupt: &HashMap<String, u64>,
) -> bool {
    // 步骤 1: 版本门控——是否有 channel 自上次中断后被更新
    let any_updates_since_prev_interrupt = channel_versions.iter()
        .any(|(chan, ver)| {
            ver > versions_seen_for_interrupt.get(chan).unwrap_or(&0)
        });

    if !any_updates_since_prev_interrupt {
        return false; // 无更新，不中断
    }

    // 步骤 2: 检查节点名称
    pending_tasks.iter()
        .any(|task| interrupt_before.contains(&task.node_name))
}
```

### 配置

```rust
let app = graph.compile(CompileConfig {
    checkpointer: Some(Box::new(MemorySaver::new())),
    interrupt_before: vec!["human_review".into(), "dangerous_action".into()],
    interrupt_after: vec!["llm_call".into()],
    ..Default::default()
})?;
```

### 语义

| 配置 | 时机 | 用途 |
|------|------|------|
| `interrupt_before` | 节点执行**前** | 审批工作流：人类决定是否允许节点执行 |
| `interrupt_after` | 节点执行**后** | 审查工作流：人类审查节点输出后决定是否继续 |

### 实现

在 Pregel 主循环中：

```rust
// superstep 开始前
for node_id in &pending_nodes {
    if self.interrupt_before.contains(node_id) {
        // 持久化 checkpoint，标记 next = pending_nodes
        // 发送 StreamEvent::Interrupt
        // interrupt_before 的 payload 是空的（或包含即将执行的节点信息）
        return Ok(ExecutionResult::Interrupted { ... });
    }
}

// 节点执行完成后，merge 之前
for (node_id, output) in &step_outputs {
    if self.interrupt_after.contains(node_id) {
        // 先 apply 该节点的 writes
        // 持久化 checkpoint
        // 发送 StreamEvent::Interrupt（payload 包含节点输出）
        return Ok(ExecutionResult::Interrupted { ... });
    }
}
```

### interrupt_before 的 resume 行为

- Resume 时不提供 value（或提供 None）：继续执行被暂停的节点
- Resume 时提供 value：value 作为节点的输入覆盖（可选特性）
- Resume 时通过 `update_state` 修改 state 后再 resume：节点看到修改后的 state

### interrupt_after 的 resume 行为

- 节点已执行完毕，输出已 merge 到 state
- Resume 继续执行下一个 superstep
- 调用方可在 resume 前通过 `update_state` 修改 state（例如编辑 LLM 输出）

---

## 5. Command 与 Resume

### Command 类型

```rust
pub struct Command<S: State> {
    /// 状态更新（可选）
    pub update: Option<S::Update>,
    /// 路由目标（可选）——覆盖正常的边路由
    pub goto: Option<CommandGoto>,
    /// <!-- Addresses finding: H-16 -->
    /// <!-- Addresses finding: Part3#15 -->
    /// Resume 值：支持单一值或 ID 映射
    /// 单一值：resume 所有中断（兼容模式）
    /// HashMap：按中断 ID 精确 resume
    /// 参考: `langgraph/types.py:749-798`
    pub resume: Option<ResumeValue>,
}

/// Resume 值类型
/// <!-- Addresses finding: M-8 -->
/// 支持三种 resume 路由方式：单一值、按 ID 映射、按命名空间映射
#[derive(Clone, Debug)]
pub enum ResumeValue {
    /// 单一值，resume 所有待处理的中断
    Single(serde_json::Value),
    /// 按中断 ID 映射精确 resume
    /// key = interrupt_id, value = resume 值
    ById(HashMap<String, serde_json::Value>),
    /// <!-- Addresses finding: M-8 -->
    /// 按命名空间路由 resume（用于子图中断）
    /// key = namespace (如 "node_name:uuid"), value = resume 值
    /// 保留 Vec<Value> 作为便捷包装器（按索引匹配）
    ByNamespace(HashMap<String, serde_json::Value>),
}

/// <!-- Addresses finding: M-8 -->
/// 便捷包装器：Vec<Value> 仍可用于按索引匹配
impl From<Vec<serde_json::Value>> for ResumeValue {
    fn from(values: Vec<serde_json::Value>) -> Self {
        // 将 Vec 转换为 ByNamespace 或 Single
        if values.is_empty() {
            ResumeValue::Single(serde_json::Value::Null)
        } else if values.len() == 1 {
            ResumeValue::Single(values.into_iter().next().unwrap())
        } else {
            // 多值时使用索引作为 key
            let map: HashMap<String, serde_json::Value> = values
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect();
            ResumeValue::ByNamespace(map)
        }
    }
}

pub enum CommandGoto {
    /// 路由到单个节点
    One(String),
    /// 路由到多个节点（并行执行）
    Many(Vec<String>),
    /// 路由到父图的节点
    Parent(String),
    /// Send API：动态 fan-out
    Send(Vec<SendTarget>),
}
```

### graph.resume() 便捷方法

```rust
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 恢复被中断的图执行
    ///
    /// values: 按中断索引顺序的 resume values
    /// config: 必须包含 thread_id（用于定位 checkpoint）
    pub async fn resume(
        &self,
        values: Vec<serde_json::Value>,
        config: &RunnableConfig,
    ) -> Result<S, JunctureError> {
        // 1. 加载最新 checkpoint
        let checkpoint = self.checkpointer
            .get_checkpoint(config).await?
            .ok_or(JunctureError::NoCheckpointFound)?;

        // 2. 验证 checkpoint 是中断状态
        if checkpoint.metadata.source != CheckpointSource::Interrupt {
            return Err(JunctureError::NotInterrupted);
        }

        // 3. 构建带 resume context 的 config
        let resume_config = config.clone().with_resume_values(values);

        // 4. 从 checkpoint 恢复并继续执行
        self.invoke_from_checkpoint(checkpoint, &resume_config).await
    }

    /// Stream 模式的 resume
    pub async fn resume_stream(
        &self,
        values: Vec<serde_json::Value>,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<impl Stream<Item = Result<StreamEvent<S>, JunctureError>>, JunctureError> {
        // 同上，但返回 stream
    }
}
```

### 节点返回 Command

节点可以返回 `Command` 来同时更新状态和控制路由：

```rust
async fn router_node(state: AgentState) -> Result<NodeOutput<AgentState>> {
    let decision: serde_json::Value = interrupt!(json!({
        "options": ["approve", "reject", "escalate"],
        "context": state.summary,
    }))?;

    let action = decision["action"].as_str().unwrap_or("reject");

    Ok(NodeOutput::Command(Command {
        update: Some(AgentStateUpdate {
            decision: Some(action.to_string()),
            ..Default::default()
        }),
        goto: Some(match action {
            "approve" => CommandGoto::One("publish".into()),
            "reject" => CommandGoto::One("archive".into()),
            "escalate" => CommandGoto::Parent("manager_review".into()),
            _ => CommandGoto::One(END.into()),
        }),
        resume: None,
    }))
}
```

> **Implementation Note (C-06-1)**: 实际实现使用 `Goto` 枚举（而非设计中的 `CommandGoto`），变体命名为 `Next`、`Multiple`、`End`（对应设计中的 `One`、`Many`、`Parent`）。功能等价，命名遵循 Rust 惯用法。
> `Command::goto()` 方法用于构造 goto 路由。

---

## 6. 设计约束与最佳实践

### HIDDEN_TAG：过滤内部节点

<!-- Addresses finding: L-7 -->

> 参考: LangGraph 的 `TAG_HIDDEN` 常量

Juncture 提供 `HIDDEN_TAG` 机制，用于标记不应出现在中断检查和流输出中的内部节点：

```rust
/// 隐藏标签：标记内部节点
///
/// 携带此标签的节点在以下场景中被过滤：
/// - interrupt_before/interrupt_after 检查时跳过
/// - StreamMode::Updates 输出中不包含
/// - get_graph() 导出时可选择隐藏
///
/// 典型用途：内部路由节点、状态转换节点、错误处理器节点
pub const HIDDEN_TAG: &str = "__hidden__";

// 使用方式
graph.add_node_simple("__route_internal", route_node);
// 该节点自动在 add_node 时标记 HIDDEN_TAG（名称以 __ 开头和结尾）

// 或手动标记
let config = RunnableConfig {
    tags: vec![HIDDEN_TAG.to_string()],
    ..Default::default()
};
```

> **Implementation Note (C-06-2)**: `HIDDEN_TAG` 常量已定义于 `interrupt/mod.rs:81`，但过滤逻辑尚未实现。
> 当前 interrupt_before/interrupt_after 检查和 StreamMode 输出中未使用该标签进行过滤。
> 该功能计划在未来版本中实现。

### 幂等性要求

由于 resume 时节点从头重新执行，**interrupt!() 之前的所有代码都会被再次执行**。因此：

```rust
// 错误示范：interrupt 前有副作用
async fn bad_node(state: AgentState) -> Result<AgentStateUpdate> {
    send_notification(&state.user_id, "开始处理").await?;  // 会被执行两次！
    let approval = interrupt!(json!({"question": "确认？"}))?;
    // ...
}

// 正确做法：interrupt 放在最前面
async fn good_node(state: AgentState) -> Result<AgentStateUpdate> {
    let approval = interrupt!(json!({
        "user_id": state.user_id,
        "question": "确认？"
    }))?;
    send_notification(&state.user_id, "已确认").await?;  // 只在 resume 后执行
    // ...
}
```

### 设计规则

1. **interrupt!() 应尽量靠近节点开头**——减少重复执行的副作用
2. **interrupt!() 之前的代码必须幂等**——或者无副作用
3. **不要在循环中使用 interrupt!()**——索引语义会变得不可预测
4. **interrupt!() 的 payload 应包含足够信息**——让人类无需额外查询即可做决定
5. **resume values 必须可序列化**——通过 serde_json::Value 传递

### 错误处理

```rust
// 如果 resume value 格式不符合预期，节点应返回明确错误
async fn typed_interrupt(state: AgentState) -> Result<AgentStateUpdate> {
    let raw = interrupt!(json!({"question": "输入数量", "type": "number"}))?;

    let count = raw.as_u64().ok_or_else(|| {
        JunctureError::InvalidResumeValue {
            expected: "u64".into(),
            got: raw.to_string(),
        }
    })?;

    Ok(AgentStateUpdate {
        count: Some(count as u32),
        ..Default::default()
    })
}
```

---

## 7. 完整使用示例

### 审批工作流

```rust
use juncture::prelude::*;

#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct ApprovalState {
    #[reducer(append)]
    messages: Vec<Message>,
    draft: String,
    status: String,
    reviewer_notes: Option<String>,
}

async fn generate_draft(state: ApprovalState) -> Result<ApprovalStateUpdate> {
    // LLM 生成草稿
    let draft = llm.invoke(&state.messages).await?;
    Ok(ApprovalStateUpdate {
        draft: Some(draft.content_text().to_string()),
        status: Some("pending_review".into()),
        ..Default::default()
    })
}

async fn human_review(state: ApprovalState) -> Result<ApprovalStateUpdate> {
    // 中断等待人类审核
    let decision = interrupt!(json!({
        "draft": state.draft,
        "instruction": "请审核此草稿。返回 {approved: bool, notes: string}"
    }))?;

    let approved = decision["approved"].as_bool().unwrap_or(false);
    let notes = decision["notes"].as_str().unwrap_or("").to_string();

    Ok(ApprovalStateUpdate {
        status: Some(if approved { "approved" } else { "needs_revision" }.into()),
        reviewer_notes: Some(Some(notes)),
        ..Default::default()
    })
}

async fn publish(state: ApprovalState) -> Result<ApprovalStateUpdate> {
    // 发布逻辑
    Ok(ApprovalStateUpdate {
        status: Some("published".into()),
        ..Default::default()
    })
}

fn build_approval_graph() -> Result<CompiledGraph<ApprovalState>, TopologyError> {
    let mut graph = StateGraph::<ApprovalState>::new();

    graph.add_node("generate", generate_draft);
    graph.add_node("review", human_review);
    graph.add_node("publish", publish);
    graph.add_node("revise", generate_draft);  // 重用生成逻辑

    graph.add_edge(START, "generate");
    graph.add_edge("generate", "review");
    graph.add_conditional_edges("review", |s: &ApprovalState| {
        match s.status.as_str() {
            "approved" => "publish",
            "needs_revision" => "revise",
            _ => END,
        }
    });
    graph.add_edge("publish", END);
    graph.add_edge("revise", "review");

    graph.compile(CompileConfig {
        checkpointer: Some(Box::new(MemorySaver::new())),
        ..Default::default()
    })
}

// 使用
#[tokio::main]
async fn main() -> Result<()> {
    let app = build_approval_graph()?;
    let config = RunnableConfig::with_thread_id("approval-1");

    // 第一次执行：生成草稿后在 review 节点中断
    let mut stream = app.stream(
        ApprovalState {
            messages: vec![Message::human("写一篇关于 Rust 的博客")],
            ..Default::default()
        },
        &config,
        StreamMode::Updates,
    ).await?;

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::Interrupt { node, payloads, .. } => {
                println!("节点 {} 请求审核: {}", node, payloads[0]);
                break;
            }
            StreamEvent::Updates { node, update, .. } => {
                println!("节点 {} 完成", node);
            }
            _ => {}
        }
    }

    // 人类审核后 resume
    let final_state = app.resume(
        vec![json!({"approved": true, "notes": "内容很好"})],
        &config,
    ).await?;

    assert_eq!(final_state.status, "published");
    Ok(())
}
```

### interrupt_before 模式

```rust
// 在危险操作前自动中断，无需节点内写 interrupt!()
let app = graph.compile(CompileConfig {
    checkpointer: Some(Box::new(MemorySaver::new())),
    interrupt_before: vec!["delete_data".into(), "send_email".into()],
    ..Default::default()
})?;

// 执行到 delete_data 节点前自动中断
// 调用方可以：
//   1. 检查当前 state，确认是否继续
//   2. 通过 update_state 修改 state 后再 resume
//   3. 直接 resume 继续执行

let snapshot = app.get_state(&config).await?;
println!("即将执行: {:?}", snapshot.next);  // ["delete_data"]
println!("当前状态: {:?}", snapshot.state);

// 确认后继续
app.resume(vec![], &config).await?;
```

---

## 8. 实现清单

| 组件 | 位置 | 职责 |
|------|------|------|
| `interrupt!` 宏 | `juncture-core/src/hitl/macros.rs` | 用户接口 |
| `InterruptContext` | `juncture-core/src/hitl/context.rs` | task-local 中断状态 |
| `InterruptSignal` | `juncture-core/src/hitl/types.rs` | 中断信号数据 |
| `Command<S>` | `juncture-core/src/hitl/command.rs` | 统一的状态+路由+resume 类型 |
| Pregel 集成 | `juncture-core/src/pregel/executor.rs` | 捕获 Interrupted、持久化、resume |
| `interrupt_before/after` | `juncture-core/src/graph/compiled.rs` | 编译配置 + 执行前后检查 |
| `resume()` / `resume_stream()` | `juncture-core/src/graph/compiled.rs` | 公开 API |

---

## 源码参考索引

| 概念 | LangGraph 源码位置 | 说明 |
|------|-------------------|------|
| `interrupt()` 函数 | `langgraph/types.py:801` | 用户调用的中断入口 |
| `Interrupt` 类 | `langgraph/types.py:525` | 中断信息数据结构 |
| `Command` 类 | `langgraph/types.py:749` | 统一的 update+goto+resume 返回类型 |
| `Send` 类 | `langgraph/types.py:654` | 动态 fan-out 目标 |
| `GraphInterrupt` 异常 | `langgraph/errors.py:101` | 中断时抛出的异常（冒泡到顶层） |
| `GraphBubbleUp` 基类 | `langgraph/errors.py:85` | 中断/Send 等需要冒泡的异常基类 |
| `should_interrupt()` | `langgraph/pregel/_algo.py:155` | 判断是否应该触发 interrupt_before/after |
| `INTERRUPT` channel | `langgraph/_internal/_constants.py` | 存储中断 payload 的内部 channel |
| `RESUME` channel | `langgraph/_internal/_constants.py` | 存储 resume value 的内部 channel |
| `interrupt_before` 检查 | `langgraph/pregel/_loop.py:651-655` | tick() 中执行前中断检查 |
| `interrupt_after` 检查 | `langgraph/pregel/_loop.py:699-703` | after_tick() 中执行后中断检查 |
| resume 流程 | `langgraph/pregel/_algo.py:1292-1302` | 从 pending_writes 中读取 RESUME 值 |
| 中断传播（子图） | `langgraph/pregel/_loop.py` | GraphInterrupt 冒泡到父图 |
| HITL 文档 | `langgraph-doc/persistence.md` | 官方持久化与中断文档 |

---

## 9. 审查补充机制

<!-- Addresses finding: H-14 -->
<!-- Addresses finding: Part3#16 -->
<!-- Addresses finding: Part3#17 -->
<!-- Addresses finding: M-06 -->
<!-- Addresses finding: M-07 -->

### 9.1 ParentCommand（子图到父图的命令冒泡）

> 参考: `langgraph/errors.py:128` — ParentCommand

```rust
/// 子图节点向父图发送 Command 的异常机制
///
/// 当子图节点需要直接控制父图的路由时使用。
/// 例如：子图审批流程完成后，直接指示父图跳转到特定节点。
#[derive(Debug)]
pub struct ParentCommand<S: State>(pub Command<S>);

// 子图节点中使用
async fn approval_node(state: ApproveState, runtime: &Runtime<()>) -> Result<Command<ApproveState>, JunctureError> {
    if state.approved {
        // 向父图发送命令
        return Err(ParentCommand(Command::goto("publish")));
    }
    Ok(Command::none())
}
```

PregelLoop 在捕获到 ParentCommand 时：
1. 将 Command 从子图上下文提取出来
2. 通过 `GraphTarget::Parent` 路由到父图的调度系统
3. 父图的 `after_tick()` 处理该 Command 的 goto/update

### 9.2 Scratchpad（每任务的临时状态追踪）

> 参考: `langgraph/_internal/_scratchpad.py`

```rust
/// 每任务的临时可变状态
/// 用于在节点重执行时追踪哪些中断已经处理过
pub struct Scratchpad {
    /// 已处理的中断 ID 集合
    processed_interrupts: HashSet<String>,
    /// 任务级别的临时数据
    data: HashMap<String, serde_json::Value>,
}

impl Scratchpad {
    /// 检查中断是否已处理（防止重执行时重复处理）
    pub fn is_interrupt_processed(&self, id: &str) -> bool {
        self.processed_interrupts.contains(id)
    }

    /// 标记中断为已处理
    pub fn mark_interrupt_processed(&mut self, id: &str) {
        self.processed_interrupts.insert(id.to_string());
    }
}
```

### 9.3 Send 超时（每任务的超时覆盖）

> 参考: `langgraph/types.py:654-743`

```rust
pub struct SendTarget<S: State> {
    pub node: String,
    pub state: S,
    /// <!-- Addresses finding: Part3#17 -->
    /// 每任务的超时覆盖（覆盖节点的默认 TimeoutPolicy）
    pub timeout: Option<Duration>,
}
```

### 9.4 Heartbeat 机制

> 参考: `langgraph/runtime.py:209`

```rust
/// 心跳信号：长时间运行的节点可定期发送心跳
/// 用于配合 TimeoutPolicy 的 idle_timeout 检测
pub struct Heartbeat {
    tx: mpsc::UnboundedSender<()>,
}

impl Heartbeat {
    /// 发送心跳（表示节点仍在活跃执行中）
    pub fn ping(&self) -> Result<(), mpsc::error::SendError> {
        self.tx.send(())
    }
}

// 在 Runtime 中提供
impl<C: Clone + Send + Sync + 'static> Runtime<C> {
    pub fn heartbeat(&self) -> &Heartbeat {
        &self.heartbeat
    }
}
```

### 9.5 Previous State（函数式 API 的前一次状态访问）

> 参考: `langgraph/runtime.py:219`

```rust
/// 在函数式 API (entrypoint) 中，访问上一次执行的状态
/// 用于实现增量处理模式
pub struct Runtime<C: Clone + Send + Sync + 'static> {
    // ... 现有字段

    /// <!-- Addresses finding: M-07 -->
    /// 上一次执行的输出状态（仅在 entrypoint 重执行时有值）
    pub previous: Option<serde_json::Value>,
}
```
