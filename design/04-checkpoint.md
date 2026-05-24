# 04 - Checkpoint 持久化系统

## 概述

Checkpoint 系统是 Juncture 实现持久化执行、time-travel、human-in-the-loop 和崩溃恢复的基础。每个 superstep 结束时，执行引擎将完整的图状态持久化为一个 checkpoint，使得执行可以在任意时刻暂停、恢复、回溯或分叉。

---

## 1. LangGraph 参考架构

### 1.1 Checkpoint 存储内容

LangGraph 的 Checkpoint 存储以下核心数据：

| 字段 | 类型 | 说明 |
|------|------|------|
| `channel_values` | `dict[str, Any]` | 每个 channel 的序列化状态 |
| `channel_versions` | `dict[str, int]` | 每个 channel 的版本号（单调递增） |
| `versions_seen` | `dict[str, dict[str, int]]` | 每个节点已消费的 channel 版本 |
| `updated_channels` | `list[str]` | 本次 checkpoint 中被更新的 channel 列表 |

`versions_seen` 是调度的核心：当某个 channel 的版本号高于节点上次消费的版本时，该节点被触发执行。

### 1.2 put_writes 与 put 分离

LangGraph 将写入持久化分为两步：

1. **`put_writes(config, writes, task_id)`** — 每个节点执行完成后立即调用，将该节点的输出（channel 写入）持久化。这是增量的、per-task 的。
2. **`put(config, checkpoint, metadata)`** — 整个 superstep 结束后调用，保存完整的 checkpoint 快照。

分离的意义：
- 崩溃恢复：如果 superstep 执行到一半崩溃，已完成节点的 writes 已持久化，恢复时只需重新执行失败的节点
- DeltaChannel 优化：append-heavy 的 channel 只存储增量 writes，不需要每次存储完整值

### 1.3 CheckpointMetadata

```python
class CheckpointMetadata:
    source: "input" | "loop" | "update" | "fork"
    step: int
    parents: dict[str, str]  # namespace -> parent_checkpoint_id
    run_id: str
```

- `input`：图开始执行时的初始 checkpoint
- `loop`：每个 superstep 结束时
- `update`：外部调用 `update_state()` 时
- `fork`：从历史 checkpoint 分叉时

### 1.4 DeltaChannel 优化

对于 append-only 的 channel（如 messages），每次存储完整列表代价高昂。DeltaChannel 策略：
- 每次 `put_writes` 只存储本次追加的增量
- 周期性存储一次完整快照（delta snapshot）
- 恢复时：找到最近的快照，向前重放所有增量 writes

**DeltaSnapshot 设计**：

> 参考: `langgraph/pregel/_checkpoint.py:65-130`

DeltaSnapshot 使用祖先遍历（ancestor walk）重建完整状态：

```
恢复流程:
  │
  ├─ 1. 找到最近的完整 snapshot (full checkpoint)
  │
  ├─ 2. 向前遍历所有 delta writes
  │     每个 delta 包含: { channel_name, operation (append/set), values }
  │
  ├─ 3. 重放 delta writes 到 snapshot
  │     append channel: snapshot[channel].extend(delta.values)
  │     replace channel: snapshot[channel] = delta.values
  │
  └─ 4. 生成完整 Checkpoint 对象
```

#### 详细恢复算法

```rust
/// 从增量 checkpoint 列表中恢复完整的 checkpoint 状态
///
/// 此函数实现祖先遍历（ancestor walk）算法，通过找到最近的完整快照
/// 并向前重放所有增量写入来重建目标 checkpoint 的完整状态。
///
/// # 算法步骤
///
/// 1. **验证输入**：在 checkpoint 列表中定位目标 checkpoint
/// 2. **寻找基础快照**：向后遍历找到最近的一个完整 checkpoint（channel_values 非空）
/// 3. **收集增量写入**：收集基础快照之后的所有 pending_writes
/// 4. **重放增量**：将增量写入按顺序应用到基础快照
/// 5. **更新元数据**：更新 channel_versions、new_versions、清零 delta 计数器
///
/// # 参数
///
/// * `checkpoints` - 按时间排序的 checkpoint 列表（最新在前）
/// * `target_checkpoint_id` - 要恢复的目标 checkpoint ID
///
/// # 返回
///
/// * `Ok(Some(checkpoint))` - 成功重建的完整 checkpoint
/// * `Ok(None)` - 目标 checkpoint 不在列表中
/// * `Err(...)` - 恢复失败（例如找不到完整快照）
pub fn recover_from_deltas(
    checkpoints: &[CheckpointTuple],
    target_checkpoint_id: &str,
) -> Result<Option<Checkpoint>, CheckpointError> {
    // Step 1: 验证输入 - 在列表中找到目标 checkpoint
    let target_index = checkpoints
        .iter()
        .position(|t| t.checkpoint.id == target_checkpoint_id);

    let Some(target_idx) = target_index else {
        return Ok(None);
    };

    // 仅考虑到目标为止的 checkpoints
    let relevant_checkpoints = &checkpoints[..=target_idx];

    // Step 2: 寻找最近的完整快照
    // 完整快照是指包含完整 channel_values 的 checkpoint
    // 我们从目标向后迭代以找到最近的完整 checkpoint
    let base_snapshot = relevant_checkpoints
        .iter()
        .rev()
        .find(|t| {
            !t.checkpoint.channel_values.is_null()
                && t.checkpoint
                    .channel_values
                    .as_object()
                    .is_some_and(|obj| !obj.is_empty())
        })
        .ok_or_else(|| {
            CheckpointError::Deserialize("No full snapshot found in checkpoint chain".to_string())
        })?;

    // 克隆基础 checkpoint 作为起点
    let mut reconstructed = base_snapshot.checkpoint.clone();

    // 收集基础快照之后的所有 pending writes
    let mut all_deltas: Vec<(&String, PendingWrite)> = Vec::new();

    // Step 3: 向前遍历收集所有增量写入
    for tuple in relevant_checkpoints {
        // 跳过基础快照之前或与其同级的 checkpoints
        if tuple.checkpoint.id <= base_snapshot.checkpoint.id {
            continue;
        }

        // 收集此 checkpoint 的 pending writes
        for write in &tuple.pending_writes {
            all_deltas.push((&tuple.checkpoint.id, write.clone()));
        }
    }

    // 按 checkpoint ID 排序增量以确保正确顺序
    all_deltas.sort_by(|a, b| a.0.cmp(b.0));

    // Step 4: 将增量写入重放到快照
    let channel_values = reconstructed
        .channel_values
        .as_object_mut()
        .ok_or_else(|| {
            CheckpointError::Deserialize(
                "Base checkpoint channel_values is not an object".to_string(),
            )
        })?;

    // 跟踪哪些 channel 被修改了
    let mut modified_channels = HashMap::<String, u64>::new();

    for (_checkpoint_id, write) in all_deltas {
        let channel = &write.channel;

        // Delta channel 使用 Append 语义
        // 在完整实现中，操作类型由 channel 的 reducer 类型配置决定
        if let serde_json::Value::Array(values) = &write.value {
            // 将数组值追加到现有 channel 数据
            let entry = channel_values
                .entry(channel.clone())
                .or_insert(serde_json::Value::Array(vec![]));

            if let Some(arr) = entry.as_array_mut() {
                arr.extend(values.clone().into_iter());
            }
        } else {
            // 非数组值使用 Replace 语义
            channel_values.insert(channel.clone(), write.value.clone());
        }

        // 更新版本计数器（两个分支通用）
        *modified_channels.entry(channel.clone()).or_insert(0) += 1;
    }

    // Step 5: 更新 checkpoint 元数据
    // 为修改的 channel 更新 channel_versions
    for (channel, delta_count) in &modified_channels {
        let current_version = reconstructed
            .channel_versions
            .get(channel)
            .copied()
            .unwrap_or(0);
        reconstructed
            .channel_versions
            .insert(channel.clone(), current_version + delta_count);
    }

    // 更新 new_versions 以反映恢复期间修改的 channel
    reconstructed.new_versions = modified_channels;

    // 清除 delta 计数器，因为我们现在有了完整快照
    reconstructed.counters_since_delta_snapshot.clear();

    Ok(Some(reconstructed))
}
```

DeltaSnapshot 的 blob 格式：

```rust
/// 增量快照
pub struct DeltaSnapshot {
    /// 基础 checkpoint ID（完整快照）
    pub base_checkpoint_id: String,
    /// 增量写入列表（按 superstep 顺序）
    pub deltas: Vec<ChannelDelta>,
}

pub struct ChannelDelta {
    /// Channel 名称
    pub channel: String,
    /// 操作类型
    pub op: DeltaOp,
    /// 增量值
    pub values: Vec<serde_json::Value>,
}

pub enum DeltaOp {
    /// 追加到现有值
    Append,
    /// 替换整个值
    Replace,
}
```

### 1.5 Checkpoint ID

使用 UUID v6（时间有序），保证：
- 全局唯一
- 按创建时间单调递增
- 可用于排序和范围查询

---

## 2. Juncture CheckpointSaver Trait

```rust
use async_trait::async_trait;

#[async_trait]
pub trait CheckpointSaver: Send + Sync + 'static {
    /// 获取指定 thread/checkpoint 的最新 checkpoint
    /// config 中 thread_id 必须存在；checkpoint_id 可选（不指定则返回最新）
    async fn get_tuple(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<CheckpointTuple>, CheckpointError>;

    /// 列出指定 thread 的 checkpoint 历史（最新在前）
    async fn list(
        &self,
        config: &RunnableConfig,
        filter: Option<CheckpointFilter>,
    ) -> Result<Vec<CheckpointTuple>, CheckpointError>;

    /// 保存完整 checkpoint（superstep 结束时调用）
    /// 返回包含新 checkpoint_id 的 config（用于后续引用）
    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
    ) -> Result<RunnableConfig, CheckpointError>;

    /// 增量保存单个节点的写入（节点执行完成后立即调用）
    /// task_id 使用层级路径格式："{superstep_idx}/{node_name}/{attempt}"
    /// 层级路径支持按 superstep、节点、重试次数排序和过滤
    /// 参考: `langgraph/checkpoint/base/__init__.py:300`
    async fn put_writes(
        &self,
        config: &RunnableConfig,
        writes: Vec<PendingWrite>,
        task_id: &str,
    ) -> Result<(), CheckpointError>;
}
```

### 设计决策

- **`get_tuple` 而非 `get`**：返回 `CheckpointTuple` 包含 checkpoint + metadata + pending_writes，一次查询获取恢复所需的全部信息
- **`put` 返回 `RunnableConfig`**：新 config 包含刚创建的 checkpoint_id，调用方可直接用于后续操作
- **`put_writes` 独立方法**：支持增量持久化，不依赖完整 checkpoint 的存在
- **Error 类型使用 `CheckpointError`**：不使用 `anyhow`，保持库级别的强类型错误

---

## 3. 核心数据结构

### 3.1 Checkpoint

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    /// UUID v6，时间有序，全局唯一
    pub id: String,

    /// 序列化的完整 State（JSON 或 MessagePack）
    pub channel_values: serde_json::Value,

    /// 每个字段的版本号，用于调度决策
    pub channel_versions: HashMap<String, u64>,

    /// 每个节点已消费的字段版本
    pub versions_seen: HashMap<String, HashMap<String, u64>>,

    /// 下一 superstep 要执行的节点列表
    pub pending_tasks: Vec<PendingTask>,

    /// 未消费的 Send 目标
    pub pending_sends: Vec<SerializedSend>,

    /// State schema 版本号（用于迁移）
    pub schema_version: u32,

    /// 创建时间
    pub created_at: String, // ISO 8601

    // ─── 以下为审查补充字段 ───

    /// Checkpoint 格式版本号
    /// 用于前向兼容：当 Checkpoint 结构变更时，通过 v 字段识别格式
    /// v=1: 初始格式, v=2: 增加 new_versions, ...
    pub v: u32,

    /// 本 superstep 中哪些 channel 被更新了
    /// key 为 channel 名称，value 为更新后的版本号
    /// 用于增量 checkpoint 和 DeltaChannel 优化
    /// 参考: `langgraph/checkpoint/base/__init__.py:277`
    pub new_versions: HashMap<String, u64>,

    /// DeltaChannel 优化元数据
    /// 自上次完整 snapshot 以来的变更计数器
    /// 变更为结构化 DeltaCounters，替代原始 u64 计数器
    /// 参考: `langgraph/checkpoint/base/__init__.py:63`
    pub counters_since_delta_snapshot: HashMap<String, DeltaCounters>,
}
```

### 3.2 DeltaCounters


> Send 对象通过 `__pregel_tasks` Topic Channel 流动

Send 对象在 LangGraph 中通过特殊的内部 channel `__pregel_tasks` 流动：

```
节点返回 Command::send(targets)
  │
  ├─ 每个目标被写入 __pregel_tasks channel（Topic 类型）
  │
  ├─ checkpoint 保存时：
  │   __pregel_tasks 的值成为 checkpoint.pending_sends
  │
  └─ 恢复时（从 checkpoint 加载）：
      pending_sends 中的对象被重新注入为 PUSH tasks
      每个 Send 生成独立的 PendingTask { trigger: Push { index } }
```

```rust
/// Send 对象在 checkpoint 中的序列化形式
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerializedSend {
    /// 目标节点名
    pub node: String,
    /// 序列化的 state 覆盖
    pub state: serde_json::Value,
}
```

### 3.3 DeltaCounters (结构)


<!-- Note: See section 3.2 above for DeltaCounters struct definition -->

```rust
/// 每个 channel 的 delta 计数器
///
/// 替代原始的 HashMap<String, u64>，提供更细粒度的追踪：
/// - updates: 自上次完整 snapshot 以来的写入次数
/// - supersteps: 自上次完整 snapshot 以来的 superstep 数量
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeltaCounters {
    /// 自上次 snapshot 以来的写入次数
    pub updates: u64,
    /// 自上次 snapshot 以来的 superstep 数量
    pub supersteps: u64,
}
```

### 3.3 CheckpointMetadata

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// checkpoint 来源
    pub source: CheckpointSource,

    /// superstep 序号
    pub step: i64,

    /// 本次 superstep 中每个节点的写入摘要
    /// 注意：此字段是 Juncture 扩展，不在 LangGraph 标准规范中。
    /// LangGraph 的 CheckpointMetadata 仅包含 source, step, parents, run_id。
    /// Juncture 增加 writes 以支持调试和错误恢复场景。
    pub writes: HashMap<String, serde_json::Value>,

    /// 父 checkpoint 关系（namespace -> checkpoint_id）
    pub parents: HashMap<String, String>,

    /// 本次执行的 run_id
    pub run_id: String,
}

// > 实际实现使用了双错误类型系统以支持更精细的错误处理。
// > 除了上述 `CheckpointError`（用于存储/序列化错误）外，实现引入了 `CheckpointPutError`
// > 专门用于 `put()` 操作失败。这种分离允许调用者在无需检查错误消息字符串的情况下，
// > 区分"checkpoint 数据无效"错误和"存储后端拒绝写入"错误，从而支持更有针对性的重试
// > 和恢复策略。核心错误类型在 `juncture-core` 中定义，而特定于存储的错误类型在
// > `juncture-checkpoint` crate 中定义。

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckpointSource {
    /// 图开始执行时的初始状态
    Input,
    /// 每个 superstep 结束时
    Loop,
    /// 外部调用 update_state() 时
    Update,
    /// 从历史 checkpoint 分叉时
    Fork,
    /// Human-in-the-loop 中断（通过 `Command::interrupt` 触发）
    /// 字段值为触发中断的节点名称
    /// 用于 HITL 工作流，允许 `get_state_history` 过滤器区分 HITL 暂停点和正常执行 checkpoint
    Interrupt { node: String },
}
```

### 3.4 CheckpointTuple

```rust
#[derive(Clone, Debug)]
pub struct CheckpointTuple {
    /// 包含 thread_id + checkpoint_id + checkpoint_ns 的 config
    pub config: RunnableConfig,

    /// checkpoint 本体
    pub checkpoint: Checkpoint,

    /// 元数据
    pub metadata: CheckpointMetadata,

    /// 该 checkpoint 之后、下一个 checkpoint 之前的增量写入
    /// 用于崩溃恢复：这些写入对应的节点已完成，无需重新执行
    pub pending_writes: Vec<PendingWrite>,

    /// 父 checkpoint 的 config（用于 time-travel 导航）
    pub parent_config: Option<RunnableConfig>,
}
```

### 3.5 StateSnapshot

```rust
#[derive(Clone, Debug)]
pub struct StateSnapshot<S: State> {
    /// 反序列化后的完整 State
    pub values: S,

    /// 下一步要执行的节点
    pub next: Vec<String>,

    /// 包含 checkpoint_id 的 config，可直接用于 time-travel 恢复
    pub config: RunnableConfig,

    /// 元数据
    pub metadata: CheckpointMetadata,

    /// 创建时间
    pub created_at: String,

    /// 父 checkpoint 的 config
    pub parent_config: Option<RunnableConfig>,

    /// 当前 superstep 的任务信息（id, node_name, error, interrupts）
    pub tasks: Vec<PregelTaskInfo>,
}
```

### 3.6 PendingWrite

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingWrite {
    /// 产生此写入的任务 ID
    pub task_id: String,

    /// 写入的目标字段/channel 名
    pub channel: String,

    /// 序列化的写入值
    pub value: serde_json::Value,
}
```

### 3.7 PendingTask

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTask {
    /// 任务 ID（UUID）
    pub id: String,

    /// 目标节点名
    pub node: String,

    /// 触发此任务的 channel 列表
    pub triggers: Vec<String>,

    /// 可选的状态覆盖（Send API 场景）
    pub state_override: Option<serde_json::Value>,
}

// > checkpoint 模块中用于 checkpoint 数据的类型命名为
// > `CheckpointPendingTask` 而非 `PendingTask`，以避免与 `pregel::types::PendingTask`
// > 的命名冲突。两者字段语义相同但属于不同模块的类型。
```

### 3.8 CheckpointFilter

```rust
#[derive(Clone, Debug, Default)]
pub struct CheckpointFilter {
    /// 只返回指定 source 的 checkpoint
    pub source: Option<CheckpointSource>,

    /// 只返回指定 step 范围内的 checkpoint
    pub step_gte: Option<i64>,
    pub step_lte: Option<i64>,

    /// 只返回指定时间范围内的 checkpoint
    pub before: Option<String>, // checkpoint_id，返回此 ID 之前的
    pub after: Option<String>,

    /// 最大返回数量
    pub limit: Option<usize>,
}
```

---

## 4. 实现

### 4.1 MemorySaver

用于开发和测试，数据存储在内存中，进程退出后丢失。

```rust
pub struct MemorySaver {
    /// thread_id -> checkpoint_ns -> Vec<(CheckpointTuple)>
    storage: Arc<RwLock<HashMap<String, HashMap<String, Vec<CheckpointTuple>>>>>,
    /// (thread_id, checkpoint_id, checkpoint_ns) -> Vec<PendingWrite>
    writes: Arc<RwLock<HashMap<(String, String, String), Vec<PendingWrite>>>>,
}
```

特点：
- 无外部依赖
- `Clone + Send + Sync`（`Arc<RwLock<...>>`）
- 适合单元测试和快速原型
- 不支持跨进程共享

### 4.2 SqliteSaver

本地持久化，适合单机部署和开发环境。

```rust
pub struct SqliteSaver {
    pool: sqlx::SqlitePool,
}
```

**数据库 Schema：**

```sql
CREATE TABLE IF NOT EXISTS checkpoints (
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    parent_checkpoint_id TEXT,
    channel_values BLOB NOT NULL,
    channel_versions BLOB NOT NULL,
    versions_seen BLOB NOT NULL,
    pending_tasks BLOB,
    pending_sends BLOB,
    schema_version INTEGER NOT NULL DEFAULT 1,
    metadata BLOB NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
);

CREATE TABLE IF NOT EXISTS checkpoint_writes (
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    value BLOB NOT NULL,
    idx INTEGER NOT NULL,
    PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
);

-- 按时间倒序查询的索引
CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_time
    ON checkpoints(thread_id, checkpoint_ns, created_at DESC);
```

配置：
- WAL 模式（并发读写）
- `PRAGMA journal_mode=WAL`
- `PRAGMA synchronous=NORMAL`（平衡性能与安全）
- 连接池大小默认 5

### 4.3 PostgresSaver

生产环境使用，支持高并发和分布式部署。

```rust
pub struct PostgresSaver {
    pool: sqlx::PgPool,
}
```

**数据库 Schema：**

```sql
CREATE TABLE IF NOT EXISTS checkpoints (
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    parent_checkpoint_id TEXT,
    channel_values BYTEA NOT NULL,
    channel_versions JSONB NOT NULL,
    versions_seen JSONB NOT NULL,
    pending_tasks JSONB,
    pending_sends JSONB,
    pending_interrupts JSONB,
    schema_version INTEGER NOT NULL DEFAULT 1,
    metadata JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
);

CREATE TABLE IF NOT EXISTS checkpoint_writes (
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    value BYTEA NOT NULL,
    idx INTEGER NOT NULL,
    PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_time
    ON checkpoints(thread_id, checkpoint_ns, created_at DESC);
```

配置：
- 连接池（sqlx::PgPool），默认 max_connections = 10
- 使用 `JSONB` 存储结构化元数据字段（channel_versions、versions_seen、pending_tasks、pending_sends、pending_interrupts、metadata）以支持 SQL 级别查询
- 使用 `BYTEA` 存储二进制序列化状态数据（channel_values）
- `JSONB` 字段使用 serde_json 直接序列化/反序列化，提供 SQL 查询能力和索引支持
- pending_interrupts 列存储 human-in-the-loop 工作流中的中断信号
- 支持 `ON CONFLICT ... DO UPDATE` 实现 upsert 语义

### 4.4 通用实现注意事项

所有实现必须保证：
- **原子性**：`put()` 是原子操作，不存在半写状态
- **线程安全**：`Clone + Send + Sync`，可在多个 tokio task 间共享
- **幂等性**：相同 checkpoint_id 的重复 `put()` 不产生副作用
- **有序性**：`list()` 返回结果按 created_at 降序（最新在前）

---

## 5. 序列化策略

### 5.1 默认：MessagePack

**MessagePack 是默认序列化格式**，JSON 作为备用选项：

- **性能优先**：MessagePack 是二进制格式，体积比 JSON 小 30-40%，序列化/反序列化速度快 2-3 倍
- **适合生产**：高频 checkpoint 场景下的性能显著提升
- **向后兼容**：自动检测 checkpoint 格式，支持读取旧 JSON 格式的 checkpoint
- **调试友好**：开发环境可选择 JSON 格式便于检查

```rust
/// 序列化格式枚举
#[derive(Clone, Debug, Default)]
pub enum SerializationFormat {
    /// MessagePack（默认）：高性能二进制格式
    #[default]
    MessagePack,
    /// JSON：人类可读格式（调试兼容）
    Json,
}

/// 自动检测 checkpoint 序列化格式（独立函数）
///
/// 通过检查字节序列开头的魔数来区分 MessagePack 和 JSON 格式。
/// MessagePack 常见标记：fixmap (0x80-0x8f)、fixarray (0x90-0x9f)、map16 (0xde)、map32 (0xdf)
/// JSON 格式以 '{' (0x7b)、'[' (0x5b) 或空白字符开头
pub fn detect_format(data: &[u8]) -> SerializationFormat {
    if data.is_empty() {
        return SerializationFormat::Json;
    }

    let first_byte = data[0];

    // JSON 格式检测
    if first_byte == b'{' || first_byte == b'[' || first_byte.is_ascii_whitespace() {
        return SerializationFormat::Json;
    }

    // MessagePack 格式检测（启发式）
    if (0x80..=0x9f).contains(&first_byte)
        || first_byte == 0xde
        || first_byte == 0xdf
        || first_byte == 0xdc
        || first_byte == 0xdd
    {
        return SerializationFormat::MessagePack;
    }

    // 未知格式默认为 JSON
    SerializationFormat::Json
}

/// 使用自动格式检测反序列化（独立函数）
///
/// 检测数据是 MessagePack 还是 JSON，然后使用相应的序列化器进行反序列化。
/// 如果检测有歧义，则回退到 JSON 反序列化。
///
/// 此函数提供向后兼容性，允许读取使用不同序列化器编写的旧 checkpoint（例如，
/// 现在默认使用 MessagePack 的系统读取旧的 JSON 数据）。
pub fn deserialize_auto<T: DeserializeOwned>(data: &[u8]) -> Result<T, CheckpointError> {
    let format = detect_format(data);
    match format {
        SerializationFormat::MessagePack => {
            // 先尝试 msgpack，失败则回退到 JSON
            MsgpackSerializer::new()
                .deserialize::<T>(data)
                .or_else(|_| JsonSerializer::new().deserialize::<T>(data))
        }
        SerializationFormat::Json => JsonSerializer::new().deserialize::<T>(data),
    }
}
```

### 5.2 JSON 备用（兼容性）

- 人类可读，便于调试和检查
- 跨语言兼容（未来可能需要与其他系统交互）
- 用于迁移和遗留数据支持
- 开发环境可选格式

### 5.3 Serializer trait

```rust
pub trait CheckpointSerializer: Send + Sync + 'static {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError>;
    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError>;
    fn format(&self) -> SerializationFormat;

    /// 无类型序列化路径，用于已是 `serde_json::Value` 的数据，避免不必要的泛型开销
    fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError>;
    /// 无类型反序列化路径
    fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError>;
}

// > **实现备注 (D-04-2)**: 实际实现中 `CheckpointSerializer` trait 还包含两个额外方法：
// > `fn serialize_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, CheckpointError>` 和
// > `fn deserialize_value(&self, data: &[u8]) -> Result<serde_json::Value, CheckpointError>`。
// > 这些方法提供无类型（untyped）的序列化路径，用于 checkpoint 内部存储 channel_values 等
// > 已是 `serde_json::Value` 形式的数据，避免不必要的泛型序列化开销。

/// MessagePack 序列化器（默认）
pub struct MsgpackSerializer;

impl CheckpointSerializer for MsgpackSerializer {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
        rmp_serde::to_vec(value).map_err(CheckpointError::from)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        rmp_serde::from_slice(data).map_err(CheckpointError::from)
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::MessagePack
    }
}

/// JSON 序列化器（备用）
pub struct JsonSerializer;

impl CheckpointSerializer for JsonSerializer {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
        serde_json::to_vec(value).map_err(CheckpointError::from)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        serde_json::from_slice(data).map_err(CheckpointError::from)
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::Json
    }
}
```

### 5.4 Schema 版本迁移

Checkpoint 中存储 `schema_version`。加载时：
1. 读取 checkpoint 的 `schema_version`
2. 与当前 State 的 `schema_version()` 比较
3. 若不同，调用 `State::migrate(from_version, value)` 链式迁移
4. 反序列化为当前版本的 State

迁移函数操作的是 `serde_json::Value`，不依赖旧版本的 struct 定义。

### 5.5 加密序列化器

> 参考: `langgraph/checkpoint/sereal.py` — 加密序列化器（Python 实现）

对于包含敏感信息的 checkpoint，提供 AES-256-GCM 加密序列化器：

```rust

对于包含敏感信息的 checkpoint，提供加密序列化器：

```rust
/// AES-256-GCM 加密序列化器
/// 在标准序列化后增加加密层
pub struct EncryptedSerializer<S: CheckpointSerializer> {
    /// 内部序列化器（泛型参数，允许编译器单态化优化）
    inner: S,
    /// AES-256-GCM 密码器（构造时初始化一次，后续复用）
    cipher: Aes256Gcm,
}

impl<S: CheckpointSerializer> EncryptedSerializer<S> {
    /// 从原始 32 字节密钥创建加密序列化器
    ///
    /// 密码器在构造时初始化一次，后续所有加密/解密操作直接复用，
    /// 避免每次操作重复初始化的性能开销。
    pub fn new(inner: S, key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(GenericArray::from_slice(key));
        Self { inner, cipher }
    }

    /// 从密码短语创建加密序列化器（使用 PBKDF2 密钥派生）
    ///
    /// 使用 PBKDF2-HMAC-SHA256 从密码短语派生 32 字节密钥。
    /// 迭代次数：100,000 次（符合 OWASP 推荐）。
    ///
    /// # Errors
    ///
    /// 返回 [`CheckpointError::Serialize`] 如果密钥派生失败。
    pub fn from_passphrase(
        inner: S,
        passphrase: &str,
        salt: &[u8; 32],
    ) -> Result<Self, CheckpointError> {
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, 100_000, &mut key);
        let cipher = Aes256Gcm::new(GenericArray::from_slice(&key));
        Ok(Self { inner, cipher })
    }
}

impl<S: CheckpointSerializer> CheckpointSerializer for EncryptedSerializer<S> {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
        let plaintext = self.inner.serialize(value)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self.cipher.encrypt(&nonce, plaintext.as_ref())
            .map_err(|e| CheckpointError::serialize_msg(e.to_string()))?;
        // 格式: nonce (12 bytes) + ciphertext
        let mut output = nonce.to_vec();
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        let (nonce, ciphertext) = data.split_at(12);
        let plaintext = self.cipher.decrypt(nonce.into(), ciphertext)
            .map_err(|e| CheckpointError::deserialize_msg(e.to_string()))?;
        self.inner.deserialize(&plaintext)
    }
}
```

密钥管理策略：
- 开发环境：从环境变量 `JUNCTURE_ENCRYPTION_KEY` 读取
- 生产环境：从 KMS（AWS KMS / HashiCorp Vault）获取

### 5.6 JsonPlusSerializer

> 参考: `langgraph/checkpoint/json.py` — JsonPlusSerializer

JsonPlusSerializer 是增强的 JSON 序列化器，提供以下扩展功能：

```rust
/// JSON+ 序列化器：增强的 JSON 格式
///
/// 支持特殊类型的序列化：
/// - datetime → ISO 8601 字符串
/// - UUID → 标准字符串格式
/// - bytes → base64 编码字符串
/// - Enum → 字符串表示（而非整数索引）
pub struct JsonPlusSerializer;

// > **实现备注 (D-04-4)**: 实际实现中 `JsonPlusSerializer` 仅提供 pretty-printing 功能
// > （`serde_json::to_vec_pretty`），并未实现上述增强类型扩展（datetime/UUID/bytes）。
// > 这些类型扩展在 Rust 中通过 serde 的原生类型系统已获得充分支持（如 `chrono::DateTime` 的
// > serde 实现、`uuid::Uuid` 的字符串序列化等），无需在序列化器层面额外处理。
// > 因此 JsonPlusSerializer 等价于带 pretty-print 的 JsonSerializer。

impl CheckpointSerializer for JsonPlusSerializer {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
        // 使用 serde_json 的自定义序列化器
        serde_json::to_vec_pretty(value)
            .map_err(CheckpointError::from)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        serde_json::from_slice(data)
            .map_err(CheckpointError::from)
    }

    fn format(&self) -> SerializationFormat {
        SerializationFormat::Json
    }
}

/// 使用示例
let serializer = JsonPlusSerializer;
let encrypted = EncryptedSerializer::new(
    Box::new(serializer),
    &encryption_key,
);
```

### 5.7 缓存后端 (BaseCache)

```rust
/// Checkpoint 缓存 trait
/// 用于缓存最近的 checkpoint，避免频繁访问持久化后端
///
/// key 参数扩展为 (namespace, key) 元组，支持子图隔离缓存。
#[async_trait]
pub trait BaseCache: Send + Sync + 'static {
    /// 获取缓存值（namespace + key 复合键）
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>, CheckpointError>;
    /// 设置缓存值（namespace + key 复合键，可选 TTL）
    async fn set(&self, namespace: &str, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CheckpointError>;
    /// 删除缓存值
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CheckpointError>;
    /// 清除指定命名空间的所有缓存
    async fn clear(&self, namespace: Option<&str>) -> Result<(), CheckpointError>;
}

/// 内存缓存实现（LRU）
pub struct MemoryCache {
    entries: Arc<RwLock<LruCache<String, CacheEntry>>>,
    default_ttl: Option<Duration>,
}

struct CacheEntry {
    data: Vec<u8>,
    expires_at: Option<Instant>,
}
```

CheckpointSaver 实现可在 `get_tuple()` 时先查缓存，未命中再查持久化层。

### 5.7 Checkpoint TTL（自动过期）

```rust
/// Checkpoint 过期配置
pub struct TtlConfig {
    /// 默认 TTL（从创建时间开始计算）
    pub default_ttl: Option<Duration>,
    /// 清理间隔（定期扫描并删除过期 checkpoint）
    pub sweep_interval: Duration,
    /// 最多保留的 checkpoint 数量（超出时删除最旧的）
    pub max_checkpoints: Option<usize>,
}

impl TtlConfig {
    /// 检查给定创建时间的 checkpoint 是否已过期
    pub fn is_expired(&self, created_at: &str) -> bool {
        let Some(ttl) = self.default_ttl else {
            return false;
        };
        // 解析 created_at (ISO 8601) 并与当前时间比较
        // 返回 true 如果 (now - created_at) > ttl
    }
}
```

#### 惰性清理策略

内存实现使用惰性清理策略，在每次访问时自动触发：

```rust
async fn lazy_cleanup(
    &self,
    thread_id: &str,
    checkpoint_ns: &str,
) -> Result<(), CheckpointError> {
    // 1. 移除过期的 checkpoints
    checkpoints.retain(|tuple| !ttl_config.is_expired(&tuple.checkpoint.created_at));

    // 2. 强制执行 max_checkpoints 限制（删除最旧的）
    if let Some(max) = ttl_config.max_checkpoints {
        if checkpoints.len() > max {
            checkpoints.truncate(max);
        }
    }

    // 3. 清理已删除 checkpoint 的 pending_writes
    writes.retain(|(thread, ns, id), _| {
        thread == thread_id && ns == checkpoint_ns && checkpoint_ids.contains(id)
    });
}
```

**触发时机**：
- `list()` 操作前自动调用
- `get_tuple()` 操作前自动调用
- 确保返回的结果不包含过期或超过数量限制的 checkpoint

**优势**：
- 无需后台任务和定时器
- 减少锁竞争和内存占用
- 按需清理，避免不必要的扫描

#### 实现策略

- **惰性清理**：在 `list()` / `get_tuple()` 时检查并跳过过期项（MemorySaver 默认）
- **主动清理**：后台 tokio task 定期执行 `DELETE FROM checkpoints WHERE created_at < now() - interval`（PostgresSaver/SqliteSaver 可选）
- **数量限制**：当同一 thread 的 checkpoint 数量超过 `max_checkpoints` 时，删除最旧的

---

## 6. Time-travel

### 6.1 get_state_history

```rust
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 返回指定 thread 的所有 checkpoint 历史（最新在前）
    pub async fn get_state_history(
        &self,
        config: &RunnableConfig,
    ) -> Result<Vec<StateSnapshot<S>>, JunctureError>;
}
```

返回的每个 `StateSnapshot` 包含完整的 config（含 checkpoint_id），可直接用于恢复。

### 6.2 从历史 checkpoint 恢复

```rust
// 获取历史
let history = app.get_state_history(&config).await?;
let target = &history[3]; // 选择第 4 个 checkpoint

// 从该 checkpoint 恢复执行
let result = app.invoke(input, &target.config).await?;
```

恢复时，Pregel 引擎：
1. 从 checkpointer 加载指定 checkpoint
2. 反序列化 state（必要时执行 schema 迁移）
3. 读取 `pending_tasks` 确定下一步要执行的节点
4. 从该点继续执行

### 6.3 update_state（分叉）

```rust
impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    /// 在指定 checkpoint 上应用外部更新，创建新的 checkpoint 分支
    /// as_node: 模拟哪个节点产生了这个更新（影响 channel_versions 和调度）
    pub async fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> Result<RunnableConfig, JunctureError>;
}
```

`update_state` 的语义：
1. 加载指定 checkpoint
2. 将 update 通过 reducer 合并到 state
3. 更新 `channel_versions`（被修改的字段版本号递增）
4. 如果指定了 `as_node`，更新该节点的 `versions_seen`
5. 创建新 checkpoint（source = `Update` 或 `Fork`）
6. 返回新 checkpoint 的 config

### 6.4 Parent-child 关系

每个 checkpoint 的 metadata 中记录 `parents`：
- key: checkpoint_ns（命名空间）
- value: parent_checkpoint_id

这形成一棵 checkpoint 树。`update_state` 和 `fork` 操作创建新的分支。`list()` 默认返回线性历史（沿 parent 链回溯），不包含其他分支。

---

## 7. Thread 管理

### 7.1 thread_id

`thread_id` 是 checkpoint 系统的一级标识符，代表一个独立的对话/会话/任务流。

- 同一 `thread_id` 的所有 checkpoint 共享状态历史
- 不同 `thread_id` 完全隔离
- 由用户在 `RunnableConfig` 中指定

### 7.2 checkpoint_ns（命名空间）

用于子图隔离。每个子图在独立的命名空间中存储 checkpoint，与父图不混用。

#### 结构化类型系统

```rust
/// 单个命名空间段，包含节点名称和调用 UUID
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NamespaceSegment {
    /// 此段的节点名称
    pub node_name: String,
    /// 唯一调用标识符（UUID v4）
    pub invocation_id: String,
}

impl NamespaceSegment {
    /// 创建新的命名空间段
    pub const fn new(node_name: String, invocation_id: String) -> Self {
        Self {
            node_name,
            invocation_id,
        }
    }

    /// 获取段的字符串表示，格式为 `node_name:invocation_id`
    pub fn as_str(&self) -> String {
        format!("{}:{}", self.node_name, self.invocation_id)
    }
}

/// Checkpoint 命名空间，用于子图执行中的隔离
///
/// 提供层次化命名空间隔离，防止执行嵌套子图时 checkpoint 冲突。
///
/// 线格式使用前导 `|` 分隔每个段，每个段格式为 `node_name:invocation_id`，
/// 例如 `"|review:uuid1|detail:uuid2"`。根命名空间为 `""`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CheckpointNamespace {
    /// 形成层次路径的命名空间段
    pub segments: Vec<NamespaceSegment>,
}

impl CheckpointNamespace {
    /// 创建根命名空间（空路径）
    pub const fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// 从段创建命名空间
    pub const fn new(segments: Vec<NamespaceSegment>) -> Self {
        Self { segments }
    }

    /// 通过追加新段创建子命名空间
    ///
    /// # 参数
    ///
    /// * `node_name` - 此嵌套层级的节点名称
    /// * `invocation_id` - 唯一调用标识符（通常为 UUID v4）
    pub fn child(&self, node_name: &str, invocation_id: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(NamespaceSegment {
            node_name: node_name.to_string(),
            invocation_id: invocation_id.to_string(),
        });
        Self { segments }
    }

    /// 通过移除最后一段获取父命名空间
    ///
    /// 如果已经是根命名空间则返回 `None`。
    pub fn parent(&self) -> Option<Self> {
        if self.segments.is_empty() {
            None
        } else {
            let segments = self.segments[..self.segments.len() - 1].to_vec();
            Some(Self { segments })
        }
    }

    /// 检查是否为根命名空间
    pub const fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// 使用设计规范线格式转换为字符串表示
    ///
    /// 每个段以 `|` 为前缀，格式为 `|node_name:invocation_id`。
    /// 根产生 `""`。
    pub fn as_str(&self) -> String {
        self.segments.iter().fold(String::new(), |mut acc, s| {
            acc.push('|');
            acc.push_str(&s.node_name);
            acc.push(':');
            acc.push_str(&s.invocation_id);
            acc
        })
    }

    /// 从设计规范线格式 `|name:id|name:id` 解析
    ///
    /// 空字符串产生根。每个段在第一个 `:` 处分割以提取
    /// `node_name` 和 `invocation_id`。
    pub fn parse(s: &str) -> Self {
        if s.is_empty() {
            return Self::root();
        }
        let trimmed = s.trim_start_matches('|');
        let segments = trimmed
            .split('|')
            .filter_map(|seg| {
                let (node_name, invocation_id) = seg.split_once(':')?;
                Some(NamespaceSegment {
                    node_name: node_name.to_string(),
                    invocation_id: invocation_id.to_string(),
                })
            })
            .collect();
        Self { segments }
    }
}
```

#### 命名空间格式约定

线格式（字符串表示）：
- `""` — 根图
- `"|node_name:uuid"` — 一级子图（uuid 标识具体的子图调用实例）
- `"|outer:uuid1|inner:uuid2"` — 嵌套子图

**重要**: 使用 `|`（管道符）作为命名空间层级分隔符，而非 `:`（冒号）。
这是为了避免与 UUID v6 字符串表示中已包含的冒号产生歧义。
例如：`"node_name|uuid"` 而非 `"node_name:uuid"`。

结构化类型系统使命名空间操作类型安全，避免手动字符串解析错误：
- `ns.child("agent", uuid)` - 创建子命名空间
- `ns.parent()` - 返回父命名空间
- `ns.is_root()` - 检查是否为根图命名空间
- `CheckpointNamespace::parse(s)` - 从字符串解析命名空间
- `ns.as_str()` - 转换为字符串表示

### 7.3 Config 中的 checkpoint 相关字段

```rust
pub struct RunnableConfig {
    /// 会话标识
    pub thread_id: Option<String>,

    /// 指定恢复的 checkpoint（不指定则使用最新）
    pub checkpoint_id: Option<String>,

    /// 子图命名空间
    pub checkpoint_ns: String, // 默认 ""

    // ... 其他字段
}
```

---

## 8. 崩溃恢复

### 8.1 恢复流程

假设一个 superstep 有 3 个并行节点 [A, B, C]，执行到 B 完成后崩溃：

1. **崩溃前状态**：
   - 上一个完整 checkpoint 已保存（step N）
   - A 的 writes 已通过 `put_writes` 持久化
   - B 的 writes 已通过 `put_writes` 持久化
   - C 未完成

2. **恢复时**：
   - 加载 step N 的 checkpoint
   - 加载该 checkpoint 之后的 pending_writes
   - 发现 A 和 B 的 writes 已存在 → 这两个节点无需重新执行
   - 只重新执行 C
   - C 完成后，合并所有 writes，保存 step N+1 的 checkpoint

### 8.2 幂等性要求

为支持崩溃恢复，节点应尽量设计为幂等的：
- 相同输入产生相同输出
- 副作用（如 API 调用）应有去重机制

框架层面的保证：
- `put_writes` 使用 `(thread_id, checkpoint_id, task_id, idx)` 作为主键
- 重复写入同一 task_id 的 writes 是幂等的（upsert 语义）

### 8.3 DeltaChannel 恢复

对于使用 delta 优化的 append-only 字段：
1. 找到最近的完整快照（delta snapshot）
2. 收集该快照之后的所有 writes（按 step + idx 排序）
3. 依次 append，重建完整值

---

## 9. 错误类型

### 9.1 核心 CheckpointError（juncture-core）

核心错误类型定义在 `juncture-core::checkpoint`，用于 trait 方法和公共 API：

```rust
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("Serialization failed: {0}")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Deserialization failed: {0}")]
    Deserialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Checkpoint not found: thread={thread_id}, id={checkpoint_id}")]
    NotFound {
        thread_id: String,
        checkpoint_id: String,
    },

    #[error("Storage error: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Checkpoint error: {0}")]
    Other(String),
}
```

> **注意**: 使用 `Box<dyn Error + Send + Sync>` 而非 `String` 来保留完整的错误链，
> 通过 `#[source]` 属性支持 `std::error::Error::source()` 追踪。`Clone` 不可用，
> 因为 boxed trait objects 不支持 `Clone`。

### 9.2 Crate 特定 CheckpointError（juncture-checkpoint）

checkpoint crate 提供更详细的错误类型，包含特定于存储后端的错误：

```rust
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("Serialization failed: {0}")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Deserialization failed: {0}")]
    Deserialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Schema migration failed: from version {from} to {to}: {reason}")]
    SchemaMigration {
        from: u32,
        to: u32,
        reason: String,
    },

    #[error("Storage error: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Database error: {0}")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Serialization error: {0}")]
    Serialization(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Checkpoint not found: thread={thread_id}, id={checkpoint_id}")]
    NotFound {
        thread_id: String,
        checkpoint_id: String,
    },

    #[error("Connection pool exhausted")]
    PoolExhausted,
}

// Helper methods for string-based error creation
impl CheckpointError {
    pub fn serialize_msg(msg: String) -> Self { Self::Serialize(Box::new(StringError(msg))) }
    pub fn deserialize_msg(msg: String) -> Self { Self::Deserialize(Box::new(StringError(msg))) }
    pub fn storage_msg(msg: String) -> Self { Self::Storage(Box::new(StringError(msg))) }
    pub fn database_msg(msg: String) -> Self { Self::Database(Box::new(StringError(msg))) }
}
```

### 9.3 错误转换 Trait

实现使用 `ToCoreCheckpointError` trait 将 crate 特定错误映射到核心错误：

```rust
trait ToCoreCheckpointError<T> {
    fn map_checkpoint(self) -> Result<T, CoreCheckpointError>;
}

impl<T> ToCoreCheckpointError<T> for Result<T, CheckpointError> {
    fn map_checkpoint(self) -> Result<T, CoreCheckpointError> {
        self.map_err(|e| match e {
            CheckpointError::Serialize(msg) | CheckpointError::Serialization(msg) => {
                CoreCheckpointError::Serialize(msg)
            }
            CheckpointError::Deserialize(msg) => CoreCheckpointError::Deserialize(msg),
            CheckpointError::NotFound {
                thread_id,
                checkpoint_id,
            } => CoreCheckpointError::NotFound {
                thread_id,
                checkpoint_id,
            },
            CheckpointError::Storage(msg) | CheckpointError::Database(msg) => {
                CoreCheckpointError::Storage(msg)
            }
            CheckpointError::SchemaMigration { .. } | CheckpointError::PoolExhausted => {
                CoreCheckpointError::Other(e.to_string())
            }
        })
    }
}
```

这种双层错误系统设计使得：
- 公共 API 使用简洁的核心错误类型
- 存储实现可以提供更详细的错误信息
- 调用方可以选择处理详细错误或仅处理核心错误

---

## 10. Crate 组织

```
crates/
├── juncture-checkpoint/          # trait + MemorySaver + 核心类型
│   └── src/
│       ├── lib.rs
│       ├── saver.rs             # CheckpointSaver trait
│       ├── types.rs             # Checkpoint, CheckpointMetadata, etc.
│       ├── memory.rs            # MemorySaver
│       ├── serde.rs             # CheckpointSerializer trait
│       └── error.rs             # CheckpointError
│
├── juncture-checkpoint-sqlite/   # SqliteSaver（feature = "sqlite"）
│   └── src/
│       ├── lib.rs
│       └── migrations/          # SQL 迁移脚本
│
└── juncture-checkpoint-postgres/ # PostgresSaver（feature = "postgres"）
    └── src/
        ├── lib.rs
        └── migrations/
```

`juncture-checkpoint` 是纯 trait + 内存实现，无外部存储依赖。SQLite 和 Postgres 实现作为独立 crate，通过 feature flag 在门面 crate 中可选引入。

---

