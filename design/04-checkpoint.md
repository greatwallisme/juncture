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

<!-- Addresses finding: Part3#20 -->
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
    /// <!-- Addresses finding: Part3#5 -->
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
    /// <!-- Addresses finding: Part3#7 -->
    /// 用于前向兼容：当 Checkpoint 结构变更时，通过 v 字段识别格式
    /// v=1: 初始格式, v=2: 增加 new_versions, ...
    pub v: u32,

    /// 本 superstep 中哪些 channel 被更新了
    /// <!-- Addresses finding: Part3#6 -->
    /// key 为 channel 名称，value 为更新后的版本号
    /// 用于增量 checkpoint 和 DeltaChannel 优化
    /// 参考: `langgraph/checkpoint/base/__init__.py:277`
    pub new_versions: HashMap<String, u64>,

    /// DeltaChannel 优化元数据
    /// <!-- Addresses finding: Part3#21 -->
    /// 自上次完整 snapshot 以来的变更计数器
    /// <!-- Addresses finding: C-5 -->
    /// 变更为结构化 DeltaCounters，替代原始 u64 计数器
    /// 参考: `langgraph/checkpoint/base/__init__.py:63`
    pub counters_since_delta_snapshot: HashMap<String, DeltaCounters>,
}
```

### 3.2 DeltaCounters

<!-- Addresses finding: M-6 -->

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

<!-- Addresses finding: C-5 -->

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

    /// <!-- Addresses finding: L-3 -->
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
}

// > **Implementation Note (C-04-1)**: Implementation adds `CheckpointSource::Interrupt` variant
// > for human-in-the-loop (HITL) workflows. When a node triggers an interrupt (via `Command::interrupt`),
// > the checkpoint saved at that point is tagged with `source: Interrupt`. This allows `get_state_history`
// > filters to distinguish HITL pause points from normal execution checkpoints, enabling UIs to
// > display "awaiting human input" status and filter history by interrupt events.
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

// > **实现备注 (D-04-1)**: checkpoint 模块中用于 checkpoint 数据的类型命名为
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
- 使用 JSONB 存储结构化元数据（支持查询）
- 使用 BYTEA 存储序列化的 state（避免 JSON 编码开销）
- 支持 `ON CONFLICT ... DO UPDATE` 实现 upsert 语义

### 4.4 通用实现注意事项

所有实现必须保证：
- **原子性**：`put()` 是原子操作，不存在半写状态
- **线程安全**：`Clone + Send + Sync`，可在多个 tokio task 间共享
- **幂等性**：相同 checkpoint_id 的重复 `put()` 不产生副作用
- **有序性**：`list()` 返回结果按 created_at 降序（最新在前）

---

## 5. 序列化策略

### 5.1 默认：MessagePack（<!-- Addresses finding: R-A6-1 -->）

<!-- Addresses finding: R-A6-1 -->
**MessagePack 是默认序列化格式**，JSON 作为备用选项：

- **性能优先**：MessagePack 是二进制格式，体积比 JSON 小 30-40%，序列化/反序列化速度快 2-3 倍
- **适合生产**：高频 checkpoint 场景下的性能显著提升
- **向后兼容**：自动检测 checkpoint 格式，支持读取旧 JSON 格式的 checkpoint
- **调试友好**：开发环境可选择 JSON 格式便于检查

```rust
/// <!-- Addresses finding: R-A6-1 -->
/// 序列化格式枚举
#[derive(Clone, Debug, Default)]
pub enum SerializationFormat {
    /// MessagePack（默认）：高性能二进制格式
    #[default]
    MessagePack,
    /// JSON：人类可读格式（调试兼容）
    Json,
}

/// 自动检测 checkpoint 序列化格式
impl Checkpoint {
    pub fn detect_format(data: &[u8]) -> SerializationFormat {
        // MessagePack 格式以特定字节开头
        // JSON 格式以 { 或 [ 开头
        if data.starts_with(&[0x82, 0xa7]) || data.starts_with(&[0x83]) {
            SerializationFormat::MessagePack
        } else {
            SerializationFormat::Json
        }
    }
}

// > **Implementation Note (C-04-3)**: The serialization system is more comprehensive than designed.
// > In addition to Msgpack and JSON formats, the implementation provides auto-detection of the
// > serialization format on read (via magic byte inspection), allowing seamless migration from
// > JSON to Msgpack without data loss. The `Encrypted` variant (AES-256-GCM) is implemented as
// > a serializer wrapper that composes with any inner format, and auto-detection correctly handles
// > encrypted payloads by checking for the encryption header before delegating to the appropriate
// > deserializer. This three-layer system (Msgpack/JSON + Encryption + Auto-detection) exceeds
// > the design's two-format specification.
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

/// <!-- Addresses finding: R-A6-1 -->
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

### 5.5 加密序列化器（<!-- Addresses finding: L-5 -->）

> 参考: `langgraph/checkpoint/sereal.py` — 加密序列化器（Python 实现）

对于包含敏感信息的 checkpoint，提供 AES-256-GCM 加密序列化器：

```rust

<!-- Addresses finding: M-10 -->

对于包含敏感信息的 checkpoint，提供加密序列化器：

```rust
/// AES-256-GCM 加密序列化器
/// 在标准序列化后增加加密层
pub struct EncryptedSerializer<S: CheckpointSerializer> {
    inner: S,  // 泛型参数，允许编译器单态化优化，消除虚表分发开销
    cipher: Aes256Gcm,
}

impl<S: CheckpointSerializer> EncryptedSerializer<S> {
    pub fn new(inner: S, key: &[u8; 32]) -> Self {
        let cipher = Aes256Gcm::new(key.into());
        Self { inner, cipher }
    }
}

// > **实现备注 (D-04-3)**: 实际实现中 `EncryptedSerializer` 使用泛型参数 `Inner: CheckpointSerializer`
// > 而非 `Box<dyn CheckpointSerializer>` 进行内部序列化器组合。这允许编译器进行单态化优化，
// > 消除虚表分发开销。此外还提供 `from_passphrase(phrase: &str) -> Self` 便捷方法，
// > 通过 PBKDF2 或类似 KDF 从密码短语派生 32 字节密钥。

impl CheckpointSerializer for EncryptedSerializer {
    fn serialize(&self, value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
        let plaintext = self.inner.serialize(value)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self.cipher.encrypt(&nonce, plaintext.as_ref())
            .map_err(|e| CheckpointError::Serialize(e.to_string()))?;
        // 格式: nonce (12 bytes) + ciphertext
        let mut output = nonce.to_vec();
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    fn deserialize<T: DeserializeOwned>(&self, data: &[u8]) -> Result<T, CheckpointError> {
        let (nonce, ciphertext) = data.split_at(12);
        let plaintext = self.cipher.decrypt(nonce.into(), ciphertext)
            .map_err(|e| CheckpointError::Serialize(format!("Decryption failed: {}", e)))?;
        self.inner.deserialize(&plaintext)
    }
}
```

密钥管理策略：
- 开发环境：从环境变量 `JUNCTURE_ENCRYPTION_KEY` 读取
- 生产环境：从 KMS（AWS KMS / HashiCorp Vault）获取

### 5.6 JsonPlusSerializer（<!-- Addresses finding: M-17 -->）

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

<!-- Addresses finding: M-11 -->

```rust
/// Checkpoint 缓存 trait
/// 用于缓存最近的 checkpoint，避免频繁访问持久化后端
///
/// <!-- Addresses finding: L-16 -->
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

<!-- Addresses finding: M-12 -->

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
```

实现策略：
- **惰性清理**：在 `list()` / `get_tuple()` 时检查并跳过过期项
- **主动清理**：后台 tokio task 定期执行 `DELETE FROM checkpoints WHERE created_at < now() - interval`
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

命名空间格式：
- `""` — 根图
- `"node_name:uuid"` — 一级子图（uuid 标识具体的子图调用实例）
- `"outer:uuid|inner:uuid"` — 嵌套子图

> **Implementation Note (C-04-5)**: The actual implementation uses `|` (pipe) as the namespace
> separator between nesting levels instead of `:` (colon) shown in the design above. For example,
> a nested subgraph uses `"node_name|uuid"` format for one level and `"outer|uuid1|inner|uuid2"`
> for two levels. The pipe separator was chosen to avoid ambiguity with UUID v6 string representation
> which already contains colons. Code that parses or constructs checkpoint namespaces must use `|`.

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

```rust
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("序列化失败: {0}")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("反序列化失败: {0}")]
    Deserialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Schema 迁移失败: 从版本 {from} 到 {to}: {reason}")]
    SchemaMigration { from: u32, to: u32, reason: String },

    #[error("存储错误: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Checkpoint 不存在: thread={thread_id}, id={checkpoint_id}")]
    NotFound { thread_id: String, checkpoint_id: String },

    #[error("连接池耗尽")]
    PoolExhausted,
}

// > **Implementation Note (C-04-7)**: Implementation uses dual error types for finer-grained error
// > handling. In addition to `CheckpointError` shown above (for storage/serialization errors), the
// > implementation introduces `CheckpointPutError` specifically for `put()` operation failures.
// > This separation allows callers to distinguish between "checkpoint data is invalid" errors and
// > "storage backend rejected the write" errors without inspecting error message strings, enabling
// > more targeted retry and recovery strategies.
```

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

