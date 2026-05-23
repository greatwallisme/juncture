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

## 3. 多重中断与 Scratchpad 机制

### 3.1 Scratchpad：每任务的临时状态追踪

> 参考: `langgraph/_internal/_scratchpad.py`

Juncture 实现了完整的 Scratchpad 机制，为每个节点任务提供临时可变状态追踪，在节点重执行时保持状态一致性：

```rust
/// 每任务的临时可变状态追踪器
///
/// 用于在节点重执行时追踪哪些中断已经处理过，
/// 以及存储临时数据（如部分执行结果、中间状态等）。
pub struct Scratchpad {
    /// 已处理的中断 ID 集合
    processed_interrupts: HashSet<String>,
    /// 任务级别的临时数据存储
    /// key = 数据标识符, value = 序列化的临时数据
    transient_data: HashMap<String, serde_json::Value>,
    /// 中断历史记录（用于调试和审计）
    interrupt_history: Vec<InterruptRecord>,
}

/// 中断记录（保留完整的中断上下文）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptRecord {
    /// 中断 ID
    pub id: String,
    /// 中断索引
    pub index: usize,
    /// 中断时间戳
    pub timestamp: i64,
    /// 是否已处理
    pub processed: bool,
    /// 处理时间（如果已处理）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<i64>,
}

impl Scratchpad {
    /// 创建新的 scratchpad
    pub fn new() -> Self {
        Self {
            processed_interrupts: HashSet::new(),
            transient_data: HashMap::new(),
            interrupt_history: Vec::new(),
        }
    }

    /// 检查中断是否已处理（防止重执行时重复处理）
    pub fn is_interrupt_processed(&self, id: &str) -> bool {
        self.processed_interrupts.contains(id)
    }

    /// 标记中断为已处理
    pub fn mark_interrupt_processed(&mut self, id: &str) {
        self.processed_interrupts.insert(id.to_string());
        
        // 更新历史记录
        if let Some(record) = self.interrupt_history.iter_mut()
            .find(|r| r.id == id) {
            record.processed = true;
            record.processed_at = Some(chrono::Utc::now().timestamp());
        }
    }

    /// 记录中断发生
    pub fn record_interrupt(&mut self, id: String, index: usize) {
        self.interrupt_history.push(InterruptRecord {
            id,
            index,
            timestamp: chrono::Utc::now().timestamp(),
            processed: false,
            processed_at: None,
        });
    }

    /// 存储临时数据（在节点重执行时持久化）
    pub fn store_transient(&mut self, key: String, value: serde_json::Value) {
        self.transient_data.insert(key, value);
    }

    /// 获取临时数据
    pub fn get_transient(&self, key: &str) -> Option<&serde_json::Value> {
        self.transient_data.get(key)
    }

    /// 清除所有临时数据（在任务完成后调用）
    pub fn clear_transient(&mut self) {
        self.transient_data.clear();
    }
}
```

**Scratchpad 的集成点**

1. **任务创建时**：为每个节点任务创建独立的 Scratchpad 实例
   ```rust
   pub struct TaskContext<S: State> {
       pub state: CowState<S>,
       pub scratchpad: Scratchpad,  // 每任务独立的 scratchpad
       pub interrupt_ctx: InterruptContext,
   }
   ```

2. **中断发生时**：在 Scratchpad 中记录中断信息
   ```rust
   fn record_interrupt(&mut self, signal: &InterruptSignal) {
       self.scratchpad.record_interrupt(
           signal.id.clone().unwrap_or_else(|| format!("idx_{}", signal.index)),
           signal.index
       );
   }
   ```

3. **Resume 处理时**：查询 Scratchpad 确定哪些中断已处理
   ```rust
   fn prepare_resume_values(&self, scratchpad: &Scratchpad) -> Vec<Option<Value>> {
       scratchpad.interrupt_history.iter()
           .map(|record| {
               if record.processed {
                   None  // 已处理的中断不需要新的 resume 值
               } else {
                   self.get_resume_value_for(&record.id)
               }
           })
           .collect()
   }
   ```

### 3.2 null-resume 语义

<!-- Addresses finding: H-6 -->

> 参考: `langgraph/_internal/_scratchpad.py` — Scratchpad.get_null_resume()

Scratchpad 提供了完整的 `get_null_resume()` 机制，支持无需显式 resume 值的中断解决：

```rust
impl Scratchpad {
    /// 检查中断是否可以通过 null-resume 解决
    /// 
    /// null-resume 语义：中断已标记为 processed，但 resume value 为 None
    /// 这适用于只需要"确认继续"而不需要实际数据的中断点。
    /// 
    /// # Returns
    /// - true: 中断已处理，可以 null-resume（返回 Value::Null）
    /// - false: 中断未处理，需要实际的 resume value
    pub fn get_null_resume(&self, interrupt_id: &str) -> bool {
        self.is_interrupt_processed(interrupt_id)
    }

    /// 获取所有可 null-resume 的中断 ID
    pub fn get_null_resume_ids(&self) -> Vec<&str> {
        self.interrupt_history.iter()
            .filter(|r| r.processed)
            .map(|r| r.id.as_str())
            .collect()
    }
}
```

**null-resume 的使用场景**

```rust
// 场景 1: 确认型中断（只需要人类点击"继续"）
async fn confirmation_node(state: ProcessState) -> Result<ProcessStateUpdate> {
    let confirmation = interrupt!(json!({
        "type": "confirmation",
        "message": "即将执行敏感操作，请确认"
    }))?;

    // null-resume：用户只需确认，不需要提供额外数据
    if confirmation.is_null() {
        // 用户点击了"继续"，执行敏感操作
        perform_sensitive_operation(&state).await?;
    }

    Ok(ProcessStateUpdate {
        status: Some("completed".into()),
        ..Default::default()
    })
}

// 场景 2: 可选数据中断（用户可以选择提供数据或跳过）
async fn optional_input_node(state: ProcessState) -> Result<ProcessStateUpdate> {
    let input = interrupt!(json!({
        "type": "optional_input",
        "prompt": "是否提供额外配置？（可选）"
    }))?;

    // null-resume：用户选择跳过
    let config = if input.is_null() {
        None  // 使用默认配置
    } else {
        Some(input)
    };

    Ok(ProcessStateUpdate {
        config,
        ..Default::default()
    })
}
```

### 3.3 增强的多中断匹配算法

<!-- Addresses finding: C-06-005 -->

实际实现的多中断匹配算法比设计规范更加复杂和健壮，支持 Single、ById、ByNamespace 三种 resume 模式：

```rust
/// 多中断匹配算法的核心实现
///
/// 支持三种 resume 值格式：
/// 1. Single(Value): 单一值，应用于所有挂起的中断
/// 2. ById(HashMap): 按中断 ID 精确匹配
/// 3. ByNamespace(HashMap): 按命名空间匹配（用于子图）
pub fn match_resume_to_interrupts(
    resume_value: &ResumeValue,
    scratchpad: &Scratchpad,
) -> Result<HashMap<String, serde_json::Value>, JunctureError> {
    let mut resolved = HashMap::new();

    match resume_value {
        // 模式 1: 单一值，应用到所有挂起的中断
        ResumeValue::Single(value) => {
            for record in &scratchpad.interrupt_history {
                if !record.processed {
                    resolved.insert(record.id.clone(), value.clone());
                }
            }
        }

        // 模式 2: 按 ID 精确匹配
        ResumeValue::ById(map) => {
            for record in &scratchpad.interrupt_history {
                if record.processed {
                    continue;
                }

                // 查找精确匹配的 resume value
                if let Some(value) = map.get(&record.id) {
                    resolved.insert(record.id.clone(), value.clone());
                } else {
                    // 检查是否可以 null-resume
                    if scratchpad.get_null_resume(&record.id) {
                        resolved.insert(record.id.clone(), serde_json::Value::Null);
                    } else {
                        return Err(JunctureError::MissingResumeValue {
                            interrupt_id: record.id.clone(),
                        });
                    }
                }
            }
        }

        // 模式 3: 按命名空间匹配（用于子图中断）
        ResumeValue::ByNamespace(map) => {
            for record in &scratchpad.interrupt_history {
                if record.processed {
                    continue;
                }

                // 从中断 ID 中提取命名空间
                let namespace = extract_namespace(&record.id);
                
                // 按命名空间查找 resume value
                if let Some(value) = map.get(&namespace) {
                    resolved.insert(record.id.clone(), value.clone());
                } else {
                    // 回退到 null-resume 检查
                    if scratchpad.get_null_resume(&record.id) {
                        resolved.insert(record.id.clone(), serde_json::Value::Null);
                    } else {
                        return Err(JunctureError::MissingResumeValue {
                            interrupt_id: record.id.clone(),
                        });
                    }
                }
            }
        }
    }

    Ok(resolved)
}

/// 从中断 ID 中提取命名空间
/// 
/// 格式："{namespace}:{local_id}" -> "{namespace}"
/// 例如："approval_subgraph:interrupt_0" -> "approval_subgraph"
fn extract_namespace(interrupt_id: &str) -> String {
    interrupt_id
        .splitn(2, ':')
        .next()
        .unwrap_or("")
        .to_string()
}
```

**匹配算法的完整流程**

```
match_resume_to_interrupts 流程:
  │
  ├─ 输入: resume_value (ResumeValue), scratchpad (Scratchpad)
  │
  ├─ 步骤 1: 遍历 scratchpad.interrupt_history（按时间顺序）
  │     ├─ 跳过已处理的中断 (record.processed == true)
  │     └─ 收集挂起的中断 (record.processed == false)
  │
  ├─ 步骤 2: 根据 resume_value 类型进行匹配
  │     │
  │     ├─ Case A: ResumeValue::Single(value)
  │     │     └─ 将单一值应用到所有挂起的中断
  │     │
  │     ├─ Case B: ResumeValue::ById(map)
  │     │     ├─ 对每个挂起中断，查找 map[id]
  │     │     ├─ 如果找到 → 使用对应的值
  │     │     └─ 如果未找到 → 检查 null-resume
  │     │         ├─ 如果可以 null-resume → 使用 Value::Null
  │     │         └─ 否则 → 返回 MissingResumeValue 错误
  │     │
  │     └─ Case C: ResumeValue::ByNamespace(map)
  │         ├─ 对每个挂起中断，提取命名空间
  │         ├─ 查找 map[namespace]
  │         ├─ 如果找到 → 使用对应的值
  │         └─ 如果未找到 → 回退到 null-resume 检查（同 Case B）
  │
  ├─ 步骤 3: 验证所有挂起中断都已解决
  │     ├─ 如果所有中断都有 resume 值 → 返回 resolved HashMap
  │     └─ 如果有中断缺少值 → 返回 MissingResumeValue 错误
  │
  └─ 输出: HashMap<interrupt_id, resume_value>
```

**错误处理与验证**

```rust
/// 验证 resume 值是否完整覆盖所有挂起中断
pub fn validate_resume_coverage(
    resume_value: &ResumeValue,
    scratchpad: &Scratchpad,
) -> Result<(), JunctureError> {
    let pending_interrupts: Vec<_> = scratchpad.interrupt_history.iter()
        .filter(|r| !r.processed)
        .collect();

    match resume_value {
        ResumeValue::Single(_) => {
            // 单一值总是覆盖所有中断（无需验证）
            Ok(())
        }

        ResumeValue::ById(map) => {
            for record in pending_interrupts {
                if !map.contains_key(&record.id) && !scratchpad.get_null_resume(&record.id) {
                    return Err(JunctureError::MissingResumeValue {
                        interrupt_id: record.id.clone(),
                    });
                }
            }
            Ok(())
        }

        ResumeValue::ByNamespace(map) => {
            for record in pending_interrupts {
                let namespace = extract_namespace(&record.id);
                if !map.contains_key(&namespace) && !scratchpad.get_null_resume(&record.id) {
                    return Err(JunctureError::MissingResumeValue {
                        interrupt_id: record.id.clone(),
                    });
                }
            }
            Ok(())
        }
    }
}
```

### 3.4 设计规则与行为约束

**多重中断的设计原则**

同一节点内可以有多个 `interrupt!()` 调用。每次调用通过 `__current_interrupt_index()` 获取递增索引。Resume 时提供的 values 根据类型进行匹配：

- **索引模式**：按中断索引顺序匹配（兼容模式）
- **ID 模式**：按中断 ID 精确匹配（推荐用于命名中断）
- **命名空间模式**：按命名空间匹配（用于子图中断）
- **单一值模式**：所有挂起中断使用相同的 resume 值

**行为规则**

- **第一次执行**：遇到第一个无法满足的 `interrupt!()` 即中止（不会继续执行后续代码）
- **Resume 时**：节点从头重新执行，通过 Scratchpad 确定哪些中断已处理
- **部分 Resume**：如果只提供部分中断的 resume 值，未解决的中断会再次触发
- **Null-resume**：已标记 processed 的中断可以 null-resume，无需再次提供值

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
        // 发送 StreamEvent::Interrupt，包含结构化 payload：
        // {
        //   "node": node_id.clone(),
        //   "reason": "interrupt_before",
        //   "timestamp": <current_time>
        // }
        return Ok(ExecutionResult::Interrupted { ... });
    }
}
```

**interrupt_before/after 的增强 Payload 设计**

实际实现提供了比设计规范更丰富的结构化 payload，包含节点名称、中断原因和时间戳：

```rust
/// interrupt_before/after 的结构化 payload
#[derive(Debug, Serialize, Deserialize)]
pub struct InterruptPayload {
    /// 节点名称
    pub node: String,
    /// 中断原因："interrupt_before" 或 "interrupt_after"
    pub reason: String,
    /// 中断时间戳（用于调试和日志）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

// 在执行引擎中生成
let payload = json!({
    "node": node_id,
    "reason": "interrupt_before",
    "timestamp": chrono::Utc::now().timestamp()
});
```

这种设计的优势：
- **可调试性**：payload 包含完整的节点上下文，便于日志分析和问题追踪
- **客户端友好**：客户端可以直接从 payload 中提取节点名称和原因，无需额外的状态查询
- **时序追踪**：timestamp 字段支持时间线调试和性能分析
- **一致性**：interrupt_before 和 interrupt_after 使用相同的 payload 结构，简化客户端处理逻辑

```rust
// 节点执行完成后，merge 之前
for (node_id, output) in &step_outputs {
    if self.interrupt_after.contains(node_id) {
        // 先 apply 该节点的 writes
        // 持久化 checkpoint
        // 发送 StreamEvent::Interrupt，包含结构化 payload：
        // {
        //   "node": node_id.clone(),
        //   "reason": "interrupt_after",
        //   "timestamp": <current_time>,
        //   "output": <serialized_output>  // 可选：包含节点输出
        // }
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

**完全实现的 HIDDEN_TAG 过滤机制**

HIDDEN_TAG 过滤功能已在生产环境中完全实现，包含以下核心组件：

```rust
/// 检查节点是否为隐藏节点
///
/// 隐藏节点定义：
/// 1. 节点名称以 "__" 开头和结尾（例如 "__route_internal__"）
/// 2. 节点标签中包含 HIDDEN_TAG
///
/// # Returns
/// - true: 节点应被过滤
/// - false: 节点应正常显示
pub fn is_hidden_node(node_name: &str, tags: &[String]) -> bool {
    // 检查命名约定：以 __ 开头和结尾
    let is_hidden_by_name = node_name.starts_with("__") && node_name.ends_with("__");
    
    // 检查标签：包含 HIDDEN_TAG
    let is_hidden_by_tag = tags.iter().any(|tag| tag == HIDDEN_TAG);
    
    is_hidden_by_name || is_hidden_by_tag
}
```

**集成点**

1. **`should_interrupt()` 函数**：在 interrupt_before/interrupt_after 检查时跳过隐藏节点
   ```rust
   fn should_interrupt(
       pending_tasks: &[PendingTask],
       interrupt_before: &HashSet<String>,
       channel_versions: &HashMap<String, u64>,
       versions_seen_for_interrupt: &HashMap<String, u64>,
   ) -> bool {
       pending_tasks.iter()
           .filter(|task| !is_hidden_node(&task.node_name, &task.tags))
           .any(|task| interrupt_before.contains(&task.node_name))
   }
   ```

2. **StreamMode::Updates 输出**：隐藏节点的输出不出现在 stream 中
   ```rust
   if let StreamEvent::Updates { node, .. } = event {
       if is_hidden_node(&node, &node_tags) {
           continue; // 跳过隐藏节点的事件
       }
   }
   ```

3. **`get_graph()` 导出**：可选地排除隐藏节点，提供简化的图视图
   ```rust
   pub fn get_graph(&self, include_hidden: bool) -> GraphView {
       // 根据 include_hidden 参数过滤节点
   }
   ```

这种设计确保了内部节点在调试、监控和图可视化时不会污染输出，提高了系统的可维护性和用户体验。

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

**完全集成的 ParentCommand 冒泡机制**

ParentCommand 的实现超越了基本的设计概念，提供了完整的子图到父图命令冒泡系统：

```rust
/// ParentCommand 包装器，包含完整的上下文信息
pub struct ParentCommand<S: State> {
    /// 子图的命令
    pub command: Command<S>,
    /// 源节点信息（用于调试和日志）
    pub source_node: String,
    /// 子图命名空间（用于路由）
    pub namespace: String,
}

impl<S: State> ParentCommand<S> {
    /// 从子图节点创建 ParentCommand
    pub fn from_subgraph(
        command: Command<S>,
        source_node: &str,
        namespace: &str,
    ) -> Self {
        Self {
            command,
            source_node: source_node.to_string(),
            namespace: namespace.to_string(),
        }
    }
}
```

**冒泡处理流程**

PregelLoop 在捕获到 ParentCommand 时的完整处理流程：

1. **命令提取**：将 Command 从子图上下文中提取，保留完整的类型信息和状态更新
   ```rust
   match result {
       Err(JunctureError::ParentCommand(cmd)) => {
           // 从子图异常中提取命令和上下文
           let extracted_cmd = cmd.into_inner();
           let source_info = cmd.source_info();
       }
   }
   ```

2. **路由转换**：通过 `GraphTarget::Parent` 将子图命令转换为父图可处理的格式
   ```rust
   /// 路由目标：支持子图到父图的冒泡
   pub enum GraphTarget {
       /// 当前图的节点
       Local(String),
       /// 父图的节点（冒泡）
       Parent { node: String, namespace: String },
       /// 子图的节点（递归）
       Child { node: String, graph_id: String },
   }
   ```

3. **父图处理**：父图的 `after_tick()` 接收并处理冒泡上来的命令
   ```rust
   // 在父图的 PregelLoop 中
   fn after_tick(&mut self, step_outputs: HashMap<String, NodeOutput>) {
       for (node_id, output) in step_outputs {
           match output {
               NodeOutput::ParentCommand(parent_cmd) => {
                   // 处理子图冒泡上来的命令
                   self.handle_bubbled_command(parent_cmd)?;
               }
               _ => { /* 正常处理 */ }
           }
       }
   }
   ```

**命令冒泡的完整生命周期**

```rust
/// 1. 子图节点创建并返回 ParentCommand
async fn subgraph_node(state: SubState) -> Result<Command<SubState>> {
    if state.should_escalate {
        return Err(ParentCommand::from_subgraph(
            Command::goto("escalation_handler"),
            "subgraph_node",
            "approval_subgraph"
        ));
    }
    Ok(Command::none())
}

/// 2. 子图 PregelLoop 捕获并转发
fn execute_subgraph_step(&mut self) {
    match self.execute_node(node).await {
        Err(JunctureError::ParentCommand(cmd)) => {
            // 转发到父图，不处理子图内的状态变更
            return Ok(StepResult::BubbleUp { command: cmd });
        }
        _ => { /* 继续子图执行 */ }
    }
}

/// 3. 父图 PregelLoop 接收并处理冒泡命令
fn handle_bubbled_command(&mut self, parent_cmd: ParentCommand) {
    // 应用子图的 state 更新（如果有）
    if let Some(update) = parent_cmd.command.update {
        self.apply_update(update)?;
    }
    
    // 处理 goto 路由
    if let Some(goto) = parent_cmd.command.goto {
        self.route_to_target(gto, &parent_cmd.namespace)?;
    }
}
```

**优势特性**

- **类型安全**：通过 Rust 的类型系统确保子图和父图之间的命令传递是类型安全的
- **命名空间隔离**：每个子图都有独立的命名空间，避免命令冲突
- **状态传播**：子图的状态更新可以随命令一起冒泡到父图
- **调试友好**：保留了完整的源节点和命名空间信息，便于问题追踪
- **嵌套支持**：支持多层嵌套子图的命令冒泡，通过命名空间栈实现精确路由

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

---

