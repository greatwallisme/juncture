# 01 - State 与 Channel 系统设计

## 1. LangGraph 的 Channel 架构（参考）

> 源码位置: `langgraph/libs/langgraph/langgraph/channels/base.py:19` — BaseChannel 定义

### 1.1 核心模型

LangGraph 内部将用户定义的 State（TypedDict）分解为独立的 Channel 对象。每个字段对应一个 Channel，每个 Channel 独立管理自己的值、版本号和更新语义。

```python
class BaseChannel(Generic[Value, Update, Checkpoint], ABC):
    def update(self, values: Sequence[Update]) -> bool   # 应用一个 superstep 内的所有写入
    def get(self) -> Value                                # 读取当前值
    def checkpoint(self) -> Checkpoint                    # 序列化用于持久化
    def from_checkpoint(self, checkpoint) -> Self         # 从持久化恢复
    def consume(self) -> bool                             # 订阅者消费后调用
```

### 1.2 Channel 类型

> 源码位置:
> - `langgraph/libs/langgraph/langgraph/channels/last_value.py:20` — LastValue
> - `langgraph/libs/langgraph/langgraph/channels/binop.py:51` — BinaryOperatorAggregate
> - `langgraph/libs/langgraph/langgraph/channels/topic.py:23` — Topic
> - `langgraph/libs/langgraph/langgraph/channels/ephemeral_value.py:15` — EphemeralValue
> - `langgraph/libs/langgraph/langgraph/channels/delta.py:25` — DeltaChannel
> - `langgraph/libs/langgraph/langgraph/channels/named_barrier_value.py:13` — NamedBarrierValue

| Channel 类型 | 语义 | 多写行为 |
|---|---|---|
| `LastValue` | 存储最新值 | 同一 superstep 内多个写入 → 报错（只允许一个写入者） |
| `BinaryOperatorAggregate` | 通过二元运算符累积 | 所有写入按顺序应用运算符 |
| `Topic` | 发布/订阅累积器 | 收集所有写入值到列表 |
| `EphemeralValue` | 临时触发值 | 每个 superstep 后自动清除 |
| `DeltaChannel` | 增量存储优化 | 只存储 delta，定期快照 |
| `NamedBarrierValue` | 等待所有命名源写入 | 所有源都写入后才触发 |
| `UntrackedValue` | 不被 checkpoint 的持久值 | 跨 superstep 存在但从不序列化到 checkpoint |
| `LastValueAfterFinish` | 延迟触发 Replace | 只在 `finish()` 后才使值可用 |
| `NamedBarrierValueAfterFinish` | 延迟触发 Barrier | 只在 `finish()` 后才使值可用 |

#### UntrackedValue 详解

<!-- Addresses finding: H-3 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/channels/last_value.py` — UntrackedValue 继承自 LastValue

```rust
/// UntrackedValue：跨 superstep 持久但不被 checkpoint 序列化的 Channel
///
/// 对应 LangGraph 的 `#[reducer(untracked)]` 属性。
/// - 在内存中跨 superstep 保留值
/// - `checkpoint()` 返回 None（不序列化到 checkpoint）
/// - `from_checkpoint()` 恢复时使用 Default 值
/// - 用途：缓存、临时计数器、无需恢复的运行时状态
pub struct UntrackedChannel<T> {
    value: T,
}

impl<T: Default + Clone> Channel<T> for UntrackedChannel<T> {
    fn update(&mut self, values: Vec<T>) -> bool {
        if let Some(v) = values.into_iter().last() {
            self.value = v;
            true
        } else {
            false
        }
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn consume(&mut self) -> bool {
        false // no-op
    }

    /// 不序列化到 checkpoint
    fn checkpoint(&self) -> Option<T> {
        None
    }

    /// 从 checkpoint 恢复时使用 Default
    fn from_checkpoint(_data: Option<T>) -> Self {
        Self { value: T::default() }
    }
}
```

**使用方式**：

```rust
#[derive(State, Clone)]
pub struct AgentState {
    pub messages: Vec<Message>,
    // 运行时缓存，不需要持久化
    #[reducer(untracked)]
    pub embedding_cache: HashMap<String, Vec<f32>>,
}
```

#### AfterFinish 变体详解

<!-- Addresses finding: M-14 -->

> 参考: `langgraph/libs/langgraph/langgraph/channels/last_value.py` — LastValueAfterFinish
> 参考: `langgraph/libs/langgraph/langgraph/channels/named_barrier_value.py` — NamedBarrierValueAfterFinish

`LastValueAfterFinish` 和 `NamedBarrierValueAfterFinish` 是延迟触发模式的 Channel 变体。它们的核心区别在于：值只在 `finish()` 被调用后才对订阅者可见。

**使用场景**：
- 在图执行的所有节点完成后，需要执行一个汇总/清理步骤
- 延迟触发模式确保只有当执行引擎发出"完成"信号时，依赖这些 Channel 的节点才被激活

```rust
/// LastValueAfterFinish：只在 finish() 后触发的 Channel
///
/// 对应 `#[reducer(replace_after_finish)]`
/// - update() 正常接受写入
/// - is_available() 只有在 finish() 被调用后才返回 true
/// - finish() 由 Pregel 引擎在检测到无更多节点需要执行时调用
pub struct LastValueAfterFinishChannel<T> {
    value: Option<T>,
    finished: bool,
}

> **Implementation Note**: Actual implementation uses three-field structure with `finished_value` field.
> Stores both current value and finished value separately, enabling richer state tracking during transitions.

impl<T: Default + Clone> Channel<T> for LastValueAfterFinishChannel<T> {
    fn update(&mut self, values: Vec<T>) -> bool {
        if let Some(v) = values.into_iter().last() {
            self.value = Some(v);
            true
        } else {
            false
        }
    }

    fn get(&self) -> &T {
        self.value.as_ref().unwrap() // 只在 available 后调用
    }

    /// <!-- Addresses finding: M-14 -->
    /// 只有在 finish() 后才标记为可用
    fn is_available(&self) -> bool {
        self.finished && self.value.is_some()
    }

    fn finish(&mut self) {
        self.finished = true;
    }
}
```

```rust
#[derive(State, Clone)]
pub struct WorkflowState {
    pub tasks: Vec<Task>,
    // 汇总结果只在所有工作完成后触发
    #[reducer(replace_after_finish)]
    pub final_summary: Option<String>,
}
```

### 1.3 版本追踪

> 源码位置: `langgraph/libs/langgraph/langgraph/pregel/_loop.py:831` — channel_versions 使用
> 源码位置: `langgraph/libs/langgraph/langgraph/pregel/_algo.py:392` — prepare_next_tasks 调度逻辑

```python
# Checkpoint 中的版本信息
channel_versions: dict[str, int]           # 每个 channel 的当前版本号
versions_seen: dict[str, dict[str, int]]   # 每个节点已消费的各 channel 版本
```

**调度逻辑**：节点 N 订阅 channel C。当 `channel_versions[C] > versions_seen[N][C]` 时，节点 N 被激活。

### 1.4 边的内部实现

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1537` — attach_edge() 方法
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:1801` — _get_channels() 字段→Channel 映射

`add_edge(A, B)` 在内部创建：
- 一个隐式 EphemeralValue channel `branch:to:B`
- A 的 writers 列表中添加对该 channel 的写入
- B 的 triggers 列表中添加对该 channel 的订阅

条件边类似，但写入目标由路由函数动态决定。

---

## 2. Juncture 的 Rust 适配

### 2.1 设计原则

1. **语义等价**：保留 Channel 的核心语义（独立版本、独立 merge、reactive 调度）
2. **静态类型化**：用 Rust 类型系统在编译期保证 Channel 操作的正确性
3. **低运行时开销**：proc-macro 生成的代码等价于手写，无动态分发（详见下方性能考量）
4. **简化实现**：不需要动态 Channel Map，因为 Rust 的 struct 字段在编译期已知

<!-- Addresses finding: Part2#7 -->

> **性能考量**：虽然 Channel dispatch 无虚表开销，但整体执行仍存在以下成本：
> - **State clone**：每个节点 spawn 时需要 `state.clone()`，对于大状态（长对话历史）开销显著（见 03-pregel-engine.md 的 CowState 优化）
> - **HashMap lookups**：VersionsSeen 中的 `HashMap<NodeId, Vec<u64>>` 查找为 O(1) 均摊，但哈希计算和内存间接访问有常数因子
> - **Vec extend**：Append reducer 的 `current.extend(v)` 在容量不足时触发堆分配
> - **serde_json 序列化**：checkpoint 持久化时的 JSON 序列化/反序列化为主要热路径瓶颈
>
> 综合评估：对于典型 Agent 工作负载（状态 < 100KB，节点数 < 50），这些开销在可接受范围内。对于大规模场景，需关注 CowState 优化和 MessagePack 序列化。

### 2.2 State Trait

```rust
// juncture-core/src/state/trait.rs

// <!-- Addresses finding: L-1 -->
// Debug bound added: enables troubleshooting, structured logging, error messages
// that include state snapshots, and Debug StreamMode output.
pub trait State: Clone + Send + Sync + std::fmt::Debug + 'static {
    /// proc-macro 生成的 partial update 类型
    type Update: Default + Send + Sync + 'static;

    /// 字段版本追踪类型
    type FieldVersions: Default + Clone + Send + Sync + 'static;

    /// 将 update 合并进 self，返回被修改的字段集合
    fn apply(&mut self, update: Self::Update) -> FieldsChanged;

    /// 清除 ephemeral 字段（superstep 结束后调用）
    fn reset_ephemeral(&mut self);

    /// 获取当前字段版本号
    fn field_versions(&self) -> &Self::FieldVersions;

    /// 递增被修改字段的版本号
    fn bump_versions(&mut self, changed: &FieldsChanged);

    /// schema 版本号
    fn schema_version() -> u32 { 1 }

    /// 历史 checkpoint 迁移
    fn migrate(from_version: u32, value: serde_json::Value) -> serde_json::Value {
        value
    }

    // > **Implementation Note (C-01-8)**: Implementation adds `finish_field(field_index: usize)` method
    // > to support `LastValueAfterFinish` channels. When the Pregel engine detects no more nodes to
    // > execute in a superstep, it calls `finish_field()` for each field using the `replace_after_finish`
    // > reducer, making their values visible to subscribers. This is the Rust equivalent of LangGraph's
    // > per-channel `finish()` call on `LastValueAfterFinish` channels.
}

/// <!-- Addresses finding: R-A4-1 -->
/// CowState: Copy-on-Write State wrapper（默认状态包装器）
///
/// 对于大型 State（如包含长对话历史的 messages 字段），
/// 每次节点 spawn 都 clone 完整 State 会产生显著内存开销。
/// CowState 使用 Arc 实现写时复制，只有修改字段的节点才会真正复制。
///
/// 这是 Juncture 的 DEFAULT State wrapper，不是可选优化。
pub struct CowState<S: State> {
    /// 共享的不可变状态
    shared: Arc<S>,
    /// 本地修改（延迟应用）
    pending: Option<S::Update>,
}

impl<S: State> CowState<S> {
    /// 从共享 State 创建 CowState
    pub fn new(state: Arc<S>) -> Self {
        Self {
            shared: state,
            pending: None,
        }
    }

    /// 获取当前状态（只读）
    ///
    /// 如果有待处理的更新，先应用更新到本地副本。
    /// 这是写时复制的关键点：只有调用 get_mut() 才会真正复制。
    pub fn get(&self) -> &S {
        if let Some(ref pending) = self.pending {
            // 这里需要通过 unsafe 或内部 mutability 来实现
            // 实际实现使用 Arc::make_mut 进行写时复制
            // 仅在首次修改时克隆，后续修改重用本地副本
            // 参见 trait_.rs 中 CowState 的完整实现
        } else {
            &self.shared
        }
    }

    /// 获取可变状态（写时复制）
    ///
    /// 首次调用时会克隆 shared Arc，后续调用重用本地副本。
    ///
    /// > **Implementation Note (C-01-3)**: The actual implementation replaces the `todo!()` placeholders
    /// > with production-ready `Arc::make_mut()` for proper clone-on-write semantics. Only the first
    /// > mutation triggers a clone; subsequent mutations reuse the local copy. See `trait_.rs:100-173`.
    pub fn get_mut(&mut self) -> &mut S {
        if self.pending.is_none() {
            // 首次修改：克隆共享状态
            let local = (*self.shared).clone();
            self.pending = Some(S::Update::default());
        }
        todo!("return mutable reference to local copy")
    }

    /// 应用更新（延迟执行）
    ///
    /// > **Implementation Note (C-01-3)**: `update()` merges changes into the pending update struct
    /// > and applies them via `S::apply()` when `commit()` is called, using `Arc::make_mut()` to
    /// > ensure copy-on-write semantics without unnecessary clones.
    pub fn update(&mut self, changes: S::Update) {
        if let Some(ref mut pending) = self.pending {
            // 合并到待处理的更新
            todo!("merge changes into pending update");
        } else {
            self.pending = Some(changes);
        }
    }

    /// 提交更新，返回新的共享状态
    ///
    /// 当节点执行完毕时，调用此方法将更新应用回共享状态。
    pub fn commit(self) -> Arc<S> {
        if let Some(pending) = self.pending {
            let mut state = (*self.shared).clone();
            state.apply(pending);
            Arc::new(state)
        } else {
            self.shared
        }
    }
}

/// Clone 实现：只是复制 Arc 引用，零成本
impl<S: State> Clone for CowState<S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            pending: None,  // clone 后不继承待处理的更新
        }
    }
}

/// 标记哪些字段被修改（位集合）
///
/// <!-- Addresses finding: Part2#4 -->
///
/// 默认实现使用 `u64` 位掩码，最多支持 64 个字段。
/// 对于大多数 Agent 场景（通常 < 30 个字段），这已足够。
///
/// <!-- Addresses finding: R-A1-1 -->
/// 使用 const generics 在编译时验证字段数量不超过 u64 容量（64）。
/// proc-macro 生成的代码会检查字段数量，如果超过 64 个字段，
/// 必须显式启用 `wide-state` feature，否则编译失败。
///
/// 如果状态需要超过 64 个字段，启用 `wide-state` feature 后，
/// proc-macro 将自动生成基于 `FixedBitSet` 的实现：
///
/// ```rust,ignore
/// // feature = "wide-state" 时自动使用
/// #[derive(Clone, Debug, Default)]
/// pub struct FieldsChanged(FixedBitSet);
///
/// impl FieldsChanged {
///     pub fn with_capacity(bits: usize) -> Self {
///         Self(FixedBitSet::with_capacity(bits))
///     }
///     pub fn is_empty(&self) -> bool { self.0.is_empty() }
///     pub fn has_field(&self, index: usize) -> bool { self.0.contains(index) }
///     pub fn set_field(&mut self, index: usize) { self.0.insert(index); }
///     pub fn merge(&mut self, other: &FieldsChanged) { self.0.union_with(&other.0); }
/// }
/// ```
#[derive(Clone, Debug, Default)]
pub struct FieldsChanged(pub u64);

impl FieldsChanged {
    pub fn is_empty(&self) -> bool { self.0 == 0 }
    pub fn has_field(&self, index: usize) -> bool { self.0 & (1 << index) != 0 }
    pub fn set_field(&mut self, index: usize) { self.0 |= 1 << index; }
    pub fn merge(&mut self, other: &FieldsChanged) { self.0 |= other.0; }
}

> **Implementation Note (C-01-5)**: `is_empty()` and `has_field()` are implemented as `const fn`,
> enabling compile-time evaluation and zero-cost field change tracking in hot paths without runtime overhead.
> This optimization matters in the Pregel scheduler where field checks occur on every superstep iteration.

// proc-macro 生成的字段数量验证（编译时检查）
//
// #[derive(State)]
// pub struct MyState {
//     pub field1: String,
//     // ... 70 个字段 ...
//     pub field70: i32,
// }
//
// 展开后生成：
//
// const _: () = {
//     // 编译时检查字段数量
//     let _ = [(); 70 - 64];  // 如果字段数 >= 64，编译失败
//     // 提示用户启用 wide-state feature
//     // "error: State has 70 fields, exceeds u64 capacity of 64.
//     //         Enable 'wide-state' feature to use FixedBitSet-based FieldsChanged."
// };
```

### 2.3 Reducer Trait

```rust
// juncture-core/src/state/channel.rs

/// Reducer 定义了单个字段的合并语义，等价于 LangGraph 的 Channel.update()
pub trait Reducer<T> {
    /// 单值快速路径：合并单个写入值
    /// 避免为单个值创建 Vec 的堆分配开销
    /// <!-- Addresses finding: Part2#5 -->
    fn reduce_one(current: &mut T, value: T) {
        // 默认实现委托给 reduce_many，具体 Reducer 可覆写以避免 Vec 分配
        Self::reduce(current, vec![value]);
    }

    /// 将新值合并到当前值
    /// values 是同一 superstep 内所有节点对该字段的写入（按节点注册顺序）
    fn reduce(current: &mut T, values: Vec<T>);
}

/// 内置 Reducer 实现

/// Replace：等价于 LastValue channel
/// 同一 superstep 内只允许一个写入者（多写入者 panic，与 LangGraph 行为一致）
pub struct ReplaceReducer;

impl<T> Reducer<T> for ReplaceReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        assert!(values.len() <= 1, "Replace reducer: multiple writes in same superstep");
        if let Some(v) = values.into_iter().next() {
            *current = v;
        }
    }
}

/// Append：等价于 BinaryOperatorAggregate(operator.add)
pub struct AppendReducer;

impl<T> Reducer<Vec<T>> for AppendReducer {
    /// 单值快速路径：直接 extend，无 Vec<Vec<T>> 分配
    fn reduce_one(current: &mut Vec<T>, value: Vec<T>) {
        current.extend(value);
    }

    fn reduce(current: &mut Vec<T>, values: Vec<Vec<T>>) {
        for v in values {
            current.extend(v);
        }
    }
}

/// <!-- Addresses finding: M-1 -->
/// AnyValue：假设所有值相等的 Channel
///
/// 类似 LastValue，但语义上假设所有写入者提供的值相等。
/// 如果值不相等，返回最后一个（与 LastWriteWins 相同行为）。
///
/// 用途：标记多个节点必须就某个值达成一致的场景，
/// 例如多个节点计算同一个聚合指标（结果应该相同）。
pub struct AnyValueReducer;

impl<T: PartialEq> Reducer<T> for AnyValueReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        if let Some(last) = values.into_iter().last() {
            // 语义检查：所有值应该相等
            if let Some(first) = values.first() {
                debug_assert!(values.iter().all(|v| v == first),
                    "AnyValue reducer: all values should be equal");
            }
            *current = last;
        }
    }
}

/// LastWriteWins：与 Replace 类似，但允许多写入者（最后一个获胜）
pub struct LastWriteWinsReducer;

impl<T> Reducer<T> for LastWriteWinsReducer {
    fn reduce(current: &mut T, values: Vec<T>) {
        if let Some(v) = values.into_iter().last() {
            *current = v;
        }
    }
}
```

### 2.4 proc-macro `#[derive(State)]`

#### 用户代码

```rust
#[derive(State, Clone, Debug, Serialize, Deserialize)]
#[state_version(2)]
#[migrate_from(1, migrate_v1_to_v2)]
pub struct AgentState {
    /// 消息列表，追加语义
    #[reducer(append)]
    pub messages: Vec<Message>,

    /// 当前步骤，替换语义（默认）
    pub step: u32,

    /// 中间结果，每个 superstep 后清除
    #[reducer(ephemeral)]
    pub scratch: Option<String>,

    /// 自定义合并逻辑
    #[reducer(custom = merge_scores)]
    pub scores: HashMap<String, f32>,

    /// 允许多写入者，最后一个获胜
    #[reducer(last_write_wins)]
    pub status: String,
}
```

#### proc-macro 展开结果

```rust
// ═══════════════════════════════════════════════════════════
// 1. 生成 Update 类型
// ═══════════════════════════════════════════════════════════

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct AgentStateUpdate {
    pub messages: Option<Vec<Message>>,
    pub step: Option<u32>,
    pub scratch: Option<Option<String>>,
    pub scores: Option<HashMap<String, f32>>,
    pub status: Option<String>,
}

// ═══════════════════════════════════════════════════════════
// 2. 生成 FieldVersions 类型
// ═══════════════════════════════════════════════════════════

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct AgentStateFieldVersions {
    pub messages: u64,
    pub step: u64,
    pub scratch: u64,
    pub scores: u64,
    pub status: u64,
}

// ═══════════════════════════════════════════════════════════
// 3. 字段索引常量（用于 FieldsChanged 位集合）
// ═══════════════════════════════════════════════════════════

impl AgentState {
    pub const FIELD_MESSAGES: usize = 0;
    pub const FIELD_STEP: usize = 1;
    pub const FIELD_SCRATCH: usize = 2;
    pub const FIELD_SCORES: usize = 3;
    pub const FIELD_STATUS: usize = 4;
}

// ═══════════════════════════════════════════════════════════
// 4. State trait 实现
// ═══════════════════════════════════════════════════════════

impl ::juncture_core::State for AgentState {
    type Update = AgentStateUpdate;
    type FieldVersions = AgentStateFieldVersions;

    fn apply(&mut self, update: AgentStateUpdate) -> FieldsChanged {
        let mut changed = FieldsChanged::default();

        // messages: append reducer
        if let Some(msgs) = update.messages {
            self.messages.extend(msgs);
            changed.set_field(Self::FIELD_MESSAGES);
        }

        // step: replace reducer (default)
        if let Some(s) = update.step {
            self.step = s;
            changed.set_field(Self::FIELD_STEP);
        }

        // scratch: ephemeral (replace semantics, cleared after consumption)
        if let Some(s) = update.scratch {
            self.scratch = s;
            changed.set_field(Self::FIELD_SCRATCH);
        }

        // scores: custom reducer
        if let Some(sc) = update.scores {
            merge_scores(&mut self.scores, sc);
            changed.set_field(Self::FIELD_SCORES);
        }

        // status: last_write_wins reducer
        if let Some(st) = update.status {
            self.status = st;
            changed.set_field(Self::FIELD_STATUS);
        }

        changed
    }

    fn reset_ephemeral(&mut self) {
        self.scratch = None;
    }

    fn field_versions(&self) -> &AgentStateFieldVersions {
        // 存储在内部（实际实现中 field_versions 可能存储在 PregelLoop 中
        // 而非 State 本身，此处为概念展示）
        todo!("field_versions stored in PregelLoop context")
    }

    fn bump_versions(&mut self, changed: &FieldsChanged) {
        // 由 PregelLoop 在外部管理版本号递增
        todo!("managed externally by PregelLoop")
    }

    fn schema_version() -> u32 { 2 }

    fn migrate(from_version: u32, value: serde_json::Value) -> serde_json::Value {
        match from_version {
            1 => migrate_v1_to_v2(value),
            _ => value,
        }
    }
}
```

### 2.5 Channel 生命周期：consume() 步骤

<!-- Addresses finding: C-3 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/channels/base.py:19` — `consume()` 方法定义

在 LangGraph 中，每个 Channel 都有 `consume()` 方法。在 `apply_writes` 完成后（所有写入已合并到 channels），所有被当前 superstep 的 tasks 触发的 channels 会调用 `consume()`。

**语义**：
- **EphemeralValue**：`consume()` 清除值（恢复为 None/默认），使其只存在于一个 superstep
- **其他 Channel 类型**（LastValue、BinaryOperatorAggregate、Topic 等）：`consume()` 是 no-op，但仍然更新版本号以支持版本追踪

**Rust 适配**：

```rust
/// Channel trait 中的 consume 方法
pub trait Channel<T> {
    /// 应用写入，返回值是否发生变化
    fn update(&mut self, values: Vec<T>) -> bool;

    /// 读取当前值
    fn get(&self) -> &T;

    /// <!-- Addresses finding: C-3 -->
    /// 在 apply_writes 后调用，用于清理或版本更新
    /// EphemeralChannel 实现会清除值；其他实现为 no-op
    fn consume(&mut self) -> bool {
        // 默认 no-op
        false
    }

    // > **实现备注 (D-01-3)**: 实际的 Channel trait 还包含两个额外方法用于自包含的 checkpoint 序列化：
    // > `fn checkpoint() -> Option<serde_json::Value>` 和
    // > `fn from_checkpoint(value: serde_json::Value) -> Result<Self, String>`。
    // > 这将 checkpoint 持久化逻辑混入 Channel trait，但使每个 channel 可以独立完成序列化/反序列化，
    // > 无需外部序列化器感知内部类型。
    //
    // > **Implementation Note (C-01-1)**: Implementation extends beyond design with `checkpoint()` and
    // > `from_checkpoint()` methods directly on the Channel trait. These methods enable self-contained
    // > serialization: each channel independently handles its own checkpoint serialization and
    // > deserialization, rather than requiring an external serializer that knows about all channel types.
    // > This design trades trait pollution for simplification of the checkpoint pipeline.
}

/// EphemeralChannel 的 consume 实现
impl<T: Default> Channel<T> for EphemeralChannel<T> {
    fn consume(&mut self) -> bool {
        if self.value.is_some() {
            self.value = None;
            true
        } else {
            false
        }
    }
}
```

**执行引擎调用时机**（见 03-pregel-engine.md）：
1. `apply_writes()` — 合并所有 task 的 writes 到 state
2. `consume()` — 对所有被触发的 channels 调用 consume
3. `reset_ephemeral()` — 清除 ephemeral 字段（与 consume 协同工作）

### 2.6 版本追踪与调度（实际存储位置）

字段版本号不存储在 State 本身，而是由 PregelLoop 管理：

```rust
// juncture-core/src/pregel/scheduler.rs

/// 追踪每个字段的版本号（等价于 LangGraph 的 channel_versions）
pub struct FieldVersionTracker {
    /// field_index → current_version
    versions: Vec<u64>,
}

/// 追踪每个节点已消费的字段版本（等价于 LangGraph 的 versions_seen）
///
/// <!-- Addresses finding: R-A4-2 -->
/// 使用 IndexMap 而非 HashMap 保证确定性迭代顺序。
/// 对于节点数很多的大型图，考虑使用 `Vec<Vec<u64>>` 优化：
/// - 外层 Vec 索引直接对应 node_id（假设 node_id 是紧凑的 usize）
/// - 内层 Vec 存储每个字段的已消费版本
/// - 避免哈希计算和内存间接访问，提升缓存局部性
pub struct VersionsSeen {
    /// node_id → (field_index → last_seen_version)
    /// 使用 IndexMap 保证确定性迭代顺序（与节点注册顺序一致）
    seen: IndexMap<NodeId, Vec<u64>>,
}

impl VersionsSeen {
    /// 判断节点是否应该被激活
    /// 条件：节点订阅的任何字段的版本号高于该节点上次消费的版本
    pub fn should_activate(
        &self,
        node_id: &NodeId,
        triggers: &[usize],  // 节点订阅的字段索引列表
        current_versions: &FieldVersionTracker,
    ) -> bool {
        let seen = &self.seen[node_id];
        triggers.iter().any(|&field_idx| {
            current_versions.versions[field_idx] > seen[field_idx]
        })
    }

    /// 节点执行完毕后，更新其 versions_seen
    pub fn mark_consumed(&mut self, node_id: &NodeId, current_versions: &FieldVersionTracker) {
        let seen = self.seen.get_mut(node_id).unwrap();
        seen.copy_from_slice(&current_versions.versions);
    }
}
```

### 2.7 简化：边驱动调度 vs 纯 reactive 调度

LangGraph 的纯 reactive 调度（基于 channel 版本变化触发节点）在 Python 动态类型中是自然的，但在 Rust 中引入不必要的复杂性。Juncture 采用**混合调度**：

1. **主路径**：边驱动。superstep 结束后，根据已完成节点的出边确定下一批节点。这覆盖 90% 的场景。
2. **辅助路径**：版本感知。对于 NamedBarrier（wait-all）等高级模式，使用 field_versions 判断是否所有前置条件满足。
3. **Send 路径**：动态创建。Send 目标直接加入下一 superstep 的待执行列表。

这比纯 reactive 模型更容易理解和调试，同时保留了所有必要的语义。

### 2.8 输入/输出 Schema 分离

<!-- Addresses finding: C-03 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/state.py:130` — `StateGraph(state_schema, input_schema, output_schema)`

LangGraph 允许为 StateGraph 指定三种不同的 Schema：
- **state_schema**：内部状态（所有字段）
- **input_schema**：图入口接受的输入（公开字段子集）
- **output_schema**：图出口返回的输出（公开字段子集）

私有字段（如中间结果、scratch space）对外部调用者不可见。

**Rust 适配**：

```rust
/// StateGraph 支持三个类型参数，默认均为 S
///
/// - S: 完整内部状态类型
/// - I: 输入 schema（S 的子集），默认为 S
/// - O: 输出 schema（S 的子集），默认为 S
pub struct StateGraph<S: State, I: IntoState<S> = S, O: FromState<S> = S> {
    // ...
}

/// 定义如何从 Input schema 构造完整 State
pub trait IntoState<S: State>: Clone + Send + Sync + 'static {
    fn into_state(self) -> S;
}

/// 定义如何从完整 State 提取 Output schema
pub trait FromState<S: State>: Clone + Send + Sync + 'static {
    fn from_state(state: &S) -> Self;
}
```

**使用示例**：

```rust
// 完整内部状态
#[derive(State, Clone)]
pub struct AgentState {
    pub messages: Vec<Message>,     // 公开：输入/输出
    pub context: String,            // 公开：输入/输出
    pub scratch: Option<String>,    // 私有：仅内部使用
    pub retry_count: u32,           // 私有：仅内部使用
}

// 输入 Schema（调用者只看到 messages 和 context）
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentInput {
    pub messages: Vec<Message>,
    pub context: String,
}

impl IntoState<AgentState> for AgentInput {
    fn into_state(self) -> AgentState {
        AgentState {
            messages: self.messages,
            context: self.context,
            scratch: None,
            retry_count: 0,
        }
    }
}

// 输出 Schema（返回时隐藏 scratch 和 retry_count）
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentOutput {
    pub messages: Vec<Message>,
    pub context: String,
}

impl FromState<AgentState> for AgentOutput {
    fn from_state(state: &AgentState) -> Self {
        AgentOutput {
            messages: state.messages.clone(),
            context: state.context.clone(),
        }
    }
}

// 编译时指定 schema
let graph: CompiledGraph<AgentState, AgentInput, AgentOutput> =
    StateGraph::<AgentState, AgentInput, AgentOutput>::new()
        .add_node("think", think_node)
        .add_node("respond", respond_node)
        .add_edge(START, "think")
        .add_edge("think", "respond")
        .add_edge("respond", END)
        .compile(None);

// 调用者使用 Input schema
let output: AgentOutput = graph.invoke(AgentInput { ... }, &config)?;
```

**proc-macro 支持**（可选）：

```rust
#[derive(State, Clone)]
#[state_input(AgentInput)]   // 自动生成 IntoState impl
#[state_output(AgentOutput)] // 自动生成 FromState impl
pub struct AgentState {
    pub messages: Vec<Message>,
    pub context: String,
    #[state_private]
    pub scratch: Option<String>,
    #[state_private]
    pub retry_count: u32,
}
// proc-macro 自动生成 AgentInput（仅包含非 private 字段）和 AgentOutput
```

---

## 3. Channel 语义在 Rust 中的保留

### 3.1 LastValue → Replace Reducer

```rust
// 用户声明（默认，无需注解）
pub step: u32,

// 语义：
// - 同一 superstep 内只允许一个节点写入该字段
// - 多个节点同时写入 → panic（与 LangGraph 行为一致）
// - 这是编译期无法检查的约束，运行时强制执行
```

**多写入检测**：在 `apply_writes` 阶段，如果同一 superstep 内多个节点的 Update 中该字段都是 `Some`，则 panic 并报告冲突的节点名。

### 3.2 BinaryOperatorAggregate → Append / Custom Reducer

```rust
// append：等价于 Annotated[list, operator.add]
#[reducer(append)]
pub messages: Vec<Message>,

// custom：等价于 Annotated[T, custom_reducer]
#[reducer(custom = merge_scores)]
pub scores: HashMap<String, f32>,

// 语义：
// - 同一 superstep 内多个节点可以同时写入
// - 所有写入按节点注册顺序依次应用 reducer
// - 结果确定性（相同输入 → 相同输出）
```

### 3.3 EphemeralValue → Ephemeral Reducer

```rust
#[reducer(ephemeral)]
pub scratch: Option<String>,

// 语义：
// - 值只存在于当前 superstep
// - superstep 结束后自动调用 reset_ephemeral() 清零
// - 不持久化到 checkpoint（或持久化为 None）
// - 用途：节点间的临时通信、触发信号
```

### 3.4 多写入冲突处理总结

| Reducer | 多写入行为 | 错误处理 |
|---|---|---|
| `replace`（默认） | 禁止多写入 | 运行时 panic + 报告冲突节点 |
| `last_write_wins` | 允许，最后注册的节点获胜 | 无错误 |
| `append` | 允许，所有值追加 | 无错误 |
| `any`（<!-- Addresses finding: M-1 -->） | 允许，假设所有值相等 | debug_assert 检查相等性 |
| `custom` | 允许，由用户函数决定 | 用户函数可返回 Error |
| `ephemeral` | 允许，最后写入获胜 | 无错误 |

<!-- Addresses finding: L-1 -->

### 3.4.1 REMOVE_ALL_MESSAGES Sentinel

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/message.py:161` — `RemoveAll` 类

在 LangGraph 中，`REMOVE_ALL_MESSAGES` 是一个特殊 sentinel，用于清空整个 messages 列表。

```rust
/// 特殊 sentinel：清空所有消息
///
/// 对应 LangGraph 的 `REMOVE_ALL_MESSAGES` 常量。
/// 使用方式：`Command::update(AgentStateUpdate { messages: Some(REMOVE_ALL_MESSAGES), .. })`
/// 实现：使用工厂方法而非 const（避免 String::new() 在 const context 中的限制）
/// Message::remove_all() -> Message  // 清空所有消息
/// Message::remove(id: &str) -> Message  // 删除指定消息
///
/// > **Implementation Note (C-01-1)**: The implementation uses factory methods instead of const values.
/// > `Message::remove_all()` and `Message::remove(id: &str)` are static methods returning constructed
/// > `Message` instances, while `REMOVE_ALL_MESSAGES` is a `&str` constant (`"__remove_all__"`). This avoids
/// > `String` field initialization issues in const context and provides better API ergonomics.
pub const REMOVE_ALL_MESSAGES: Message = Message {
    id: "__remove_all__".to_string(),
    role: Role::System,
    content: Content::Text(String::new()),
    tool_calls: vec![],
    tool_call_id: None,
    name: None,
    usage: None,
};

/// messages reducer 对 REMOVE_ALL_MESSAGES 的特殊处理
pub fn messages_reducer(current: &mut Vec<Message>, incoming: Vec<Message>) {
    for msg in incoming {
        if msg.id == "__remove_all__" {
            // 清空所有消息
            current.clear();
        } else if msg.id.starts_with("__remove__:") {
            let target_id = &msg.id["__remove__:".len()..];
            current.retain(|m| m.id != target_id);
        } else if let Some(existing) = current.iter_mut().find(|m| m.id == msg.id) {
            *existing = msg;
        } else {
            current.push(msg);
        }
    }
}
```

### 3.5 apply_writes 的完整流程

```rust
// juncture-core/src/pregel/loop_.rs

fn apply_writes<S: State>(
    state: &mut S,
    writes: &[(NodeId, S::Update)],  // 按节点注册顺序排列
    field_versions: &mut FieldVersionTracker,
) -> Result<FieldsChanged, ExecutionError> {
    // 对于 replace reducer 字段：检查多写入冲突
    // 对于 append/custom 字段：收集所有写入，按顺序应用
    
    let mut total_changed = FieldsChanged::default();
    
    for (_node_id, update) in writes {
        let changed = state.apply(update.clone());
        total_changed.merge(&changed);
    }
    
    // 递增被修改字段的版本号
    for field_idx in 0..64 {
        if total_changed.has_field(field_idx) {
            field_versions.bump(field_idx);
        }
    }
    
    Ok(total_changed)
}
```

### 3.6 Overwrite 原语（绕过 Reducer）

<!-- Addresses finding: H-01 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/types.py:928` — `Overwrite` 类

在 LangGraph 中，`Overwrite(value)` 包装器允许节点绕过字段的 reducer，直接写入值。这对于需要重置累积字段（如清空消息列表）的场景至关重要。

```rust
/// 绕过 Reducer，直接写入值
///
/// 当字段使用 append/custom 等累积 reducer 时，
/// 正常写入只能追加或合并。Overwrite 允许直接替换整个字段值。
///
/// 同一 superstep 内同一字段只能有一个 Overwrite 写入，
/// 否则触发 InvalidUpdateError。
pub struct Overwrite<T>(pub T);

// 在 State::apply 实现中，proc-macro 生成的代码检测 Overwrite：
//
// if let Some(msgs) = update.messages {
//     match msgs {
//         Overwrite(new_msgs) => {
//             self.messages = new_msgs;  // 直接替换，不调用 reducer
//         }
//         normal_msgs => {
//             AppendReducer::reduce_one(&mut self.messages, normal_msgs);
//         }
//     }
// }
```

**使用示例**：

```rust
fn clear_history(state: AgentState, _config: &RunnableConfig) -> Command<AgentState> {
    // 正常情况：messages 使用 append reducer，新消息被追加
    // 但有时需要完全清空历史记录
    Command::update(AgentStateUpdate {
        messages: Some(Overwrite(vec![])),  // 直接清空，而非追加空列表
        ..Default::default()
    })
}
```

#### 序列化格式

<!-- Addresses finding: C-6 -->

在 checkpoint JSON 中，Overwrite 值必须使用 LangGraph 兼容的 wire format 进行序列化。Rust 的 `Overwrite<T>` 类型提供编译时类型安全，但序列化时使用 `__overwrite__` 标记键：

```rust
/// Overwrite<T> 的 serde 实现
///
/// 序列化示例：
///   Overwrite(vec![]) 序列化为 {"__overwrite__": []}
///   正常值 vec![msg1] 序列化为 [msg1]
///
/// 这确保了与 LangGraph Python 的 checkpoint 格式兼容。
/// Python 端使用 `{"__overwrite__": value}` 标记来区分 Overwrite 和普通值。
impl<T: Serialize> Serialize for Overwrite<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("__overwrite__", &self.0)?;
        map.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Overwrite<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 反序列化时检测 __overwrite__ 键
        #[derive(Deserialize)]
        struct OverwriteWrapper<T> {
            __overwrite__: T,
        }
        let wrapper = OverwriteWrapper::<T>::deserialize(deserializer)?;
        Ok(Overwrite(wrapper.__overwrite__))
    }
}
```

#### 替代设计：字段级属性替代包装类型

<!-- Addresses finding: H-1 -->

**当前方案**：使用 `Overwrite<T>` 包装类型
```rust
Command::update(AgentStateUpdate {
    messages: Some(Overwrite(vec![])),  // 需要 Overwrite 包装
    ..Default::default()
})
```

**替代方案**：使用 `#[reducer(overwrite)]` 字段级属性，控制语义而不改变 Update struct 的类型：

```rust
#[derive(State, Clone)]
pub struct AgentState {
    #[reducer(append)]
    pub messages: Vec<Message>,
}

// 生成的 Update 中 messages 仍然是 Option<Vec<Message>>
// 但 proc-macro 生成额外的 overwrite 检测逻辑：
//
// impl AgentState {
//     pub fn overwrite_messages(value: Vec<Message>) -> AgentStateUpdate {
//         AgentStateUpdate {
//             messages: Some(value),
//             _messages_overwrite: true,  // 隐藏的内部标记
//             ..Default::default()
//         }
//     }
// }
```

**两种方案对比**：

| 维度 | `Overwrite<T>` 包装 | `#[reducer(overwrite)]` 属性 |
|------|---------------------|----------------------------|
| 类型安全 | 编译期区分 Overwrite 与普通值 | 运行时标记，类型签名相同 |
| API 复杂度 | 需要 match 处理两种类型 | 更简洁，Update 类型统一 |
| 与 serde 兼容 | 需要自定义序列化 | 可直接使用 `__overwrite__` 标记 |
| 实现复杂度 | proc-macro 需生成更多类型 | proc-macro 生成额外标记字段 |
| LangGraph 兼容 | 语义清晰映射 | 更接近 Python 行为 |

**推荐**：保持当前 `Overwrite<T>` 方案，因为它提供更强的编译时保证，同时 serde 实现确保 wire format 兼容。

### 3.7 InvalidUpdateError

<!-- Addresses finding: M-03 -->

```rust
/// 当写入操作违反 Channel 约束时抛出
///
/// 触发场景：
/// - Replace reducer 字段在同一个 superstep 内被多个节点写入
/// - 同一字段收到多个 Overwrite 写入
/// - 其他非法状态更新操作
#[derive(Debug, thiserror::Error)]
pub enum InvalidUpdateError {
    #[error("字段 `{field}` 的 reducer 不允许多写入，冲突节点: {conflicting_nodes:?}")]
    MultipleWriters {
        field: String,
        conflicting_nodes: Vec<String>,
    },

    #[error("字段 `{field}` 在同一 superstep 内收到多个 Overwrite")]
    MultipleOverwrite { field: String },

    #[error("字段 `{field}` 收到非法更新值")]
    InvalidValue { field: String, reason: String },
}
```

---

## 4. MessagesState 内置实现

> 源码位置: `langgraph/libs/langgraph/langgraph/graph/message.py:61` — add_messages reducer
> 源码位置: `langgraph/libs/langgraph/langgraph/graph/message.py:117` — MessagesState 定义

```rust
// juncture-core/src/state/messages.rs

use crate::{State, Message};

#[derive(State, Clone, Debug, Serialize, Deserialize)]
pub struct MessagesState {
    #[reducer(messages)]  // 特殊 reducer：支持 add_messages 语义
    pub messages: Vec<Message>,
}

// messages reducer 的特殊行为（与 LangGraph 的 add_messages 一致）：
// 1. 如果新消息的 id 与已有消息匹配 → 更新（替换）该消息
// 2. 如果新消息是 RemoveMessage → 删除对应 id 的消息
// 3. 否则 → 追加

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Role {
    System,
    Human,
    Ai,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ContentPart {
    Text(String),
    Image { url: String },
    // 可扩展

> **Implementation Note**: `ContentPart::Thinking` variant supports Anthropic extended thinking.
> Enables modeling of internal reasoning process without affecting tool call execution.
}

/// LLM Token 使用统计（实现添加，用于追踪 LLM 调用的 token 消耗）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

/// 特殊消息：删除指定 id 的消息
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoveMessage {
    pub id: String,
}

/// messages reducer 实现
pub fn messages_reducer(current: &mut Vec<Message>, incoming: Vec<Message>) {
    for msg in incoming {
        if msg.id.starts_with("__remove__:") {
            let target_id = &msg.id["__remove__:".len()..];
            current.retain(|m| m.id != target_id);
        } else if let Some(existing) = current.iter_mut().find(|m| m.id == msg.id) {
            *existing = msg;
        } else {
            current.push(msg);
        }
    }
}

impl Message {
    pub fn human(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Human,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    pub fn ai(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::System,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Tool,
            content: Content::Text(content.into()),
            tool_calls: vec![],
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            usage: None,
        }
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    pub fn remove(id: impl Into<String>) -> Self {
        Self {
            id: format!("__remove__:{}", id.into()),
            role: Role::System,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }
    }
}
```

> **Implementation Note**: `Message::content_text()` helper provides convenience for text extraction.
> Returns text content from both simple `Content::Text` and multimodal `Content::MultiPart` messages.

---

## 5. Schema 版本管理与迁移

### 5.1 问题场景

Agent 系统长期运行，State 结构随业务迭代变化。数据库中存在大量历史 checkpoint，其 schema 版本可能落后于当前代码。

### 5.2 设计

```rust
#[derive(State, Clone, Serialize, Deserialize)]
#[state_version(3)]
#[migrate_from(1, migrate_v1_to_v2)]
#[migrate_from(2, migrate_v2_to_v3)]
pub struct AgentState {
    #[reducer(append)]
    pub messages: Vec<Message>,
    pub summary: Option<String>,      // v2 新增
    pub metadata: AgentMetadata,      // v3 新增
}

fn migrate_v1_to_v2(mut v: serde_json::Value) -> serde_json::Value {
    v["summary"] = serde_json::Value::Null;
    v
}

fn migrate_v2_to_v3(mut v: serde_json::Value) -> serde_json::Value {
    v["metadata"] = serde_json::json!({"created_at": null, "tags": []});
    v
}
```

### 5.3 迁移流程

```
load_checkpoint(raw_json, schema_version=1)
  │
  ├─ schema_version == current (3)? → 直接反序列化
  │
  └─ schema_version < current?
       → migrate_v1_to_v2(json)
       → migrate_v2_to_v3(json)
       → 反序列化为当前版本的 State
```

### 5.4 Checkpoint 中的版本信息

```rust
pub struct Checkpoint {
    pub id: String,
    pub state: serde_json::Value,
    pub schema_version: u32,          // 写入时的 State schema 版本
    pub field_versions: serde_json::Value,  // 序列化的 FieldVersions
    pub versions_seen: serde_json::Value,   // 序列化的 VersionsSeen
    pub next: Vec<String>,
    pub pending_sends: Vec<serde_json::Value>,
    pub created_at: String,
}
```

### 5.5 向后兼容规则

| 变更类型 | 是否需要迁移函数 | 行为 |
|---|---|---|
| 新增 Option 字段 | 否（serde 默认 None） | 自动兼容 |
| 新增非 Option 字段 | 是 | 迁移函数提供默认值 |
| 删除字段 | 否（serde 忽略未知字段） | 自动兼容（需 `#[serde(deny_unknown_fields)]` 关闭） |
| 重命名字段 | 是 | 迁移函数重命名 key |
| 修改字段类型 | 是 | 迁移函数转换值 |

---

## 6. 与 LangGraph 的关键差异总结

| 维度 | LangGraph | Juncture | 理由 |
|---|---|---|---|
| Channel 存储 | `dict[str, BaseChannel]` 动态映射 | struct 字段 + proc-macro | 编译期安全，零开销 |
| 版本追踪 | Channel 对象内部 | 外部 FieldVersionTracker | 关注点分离 |
| 多写入检测 | Channel.update() 内部 | apply_writes 阶段 | 集中处理，错误信息更好 |
| Ephemeral 清除 | Channel.consume() | State::reset_ephemeral() | 显式调用，可测试 |
| UntrackedValue | 不序列化的 LastValue | `#[reducer(untracked)]` | 跨 superstep 但不 checkpoint |
| AfterFinish 变体 | LastValueAfterFinish 等 | `#[reducer(replace_after_finish)]` | 延迟触发，finish() 后可用 |
| 调度模型 | 纯 reactive（channel 版本驱动） | 混合（边驱动 + 版本辅助） | 更简单，覆盖所有场景 |
| 类型安全 | 运行时（TypedDict 提示） | 编译期（泛型 + proc-macro） | Rust 核心优势 |

---

## 7. DeltaChannel 实现

<!-- Addresses finding: M-17 -->

> 源码位置: `langgraph/libs/langgraph/langgraph/channels/delta.py:25` — `DeltaChannel`

### 7.1 设计动机

对于使用累积 reducer（如 append）的高频更新字段，每次 checkpoint 都存储完整状态值会产生大量冗余数据。DeltaChannel 只存储增量写入，定期快照，显著减少存储开销。

### 7.2 核心原理

```rust
/// 增量 Channel：不存储完整值，而是存储 sentinel，恢复时通过祖先写入重建
///
/// checkpoint 时：
/// - 非快照步骤：返回 MISSING sentinel（不写入 channel_values）
/// - 快照步骤：返回 _DeltaSnapshot(current_value)
///
/// 恢复时（从 checkpoint 加载）：
/// - 如果 blob 是 MISSING：初始化为默认值，需要回放祖先写入
/// - 如果 blob 是 _DeltaSnapshot(value)：直接恢复值
/// - 如果 blob 是普通值（旧格式迁移）：直接使用
pub struct DeltaChannel<T> {
    value: T,
    reducer: fn(&mut T, Vec<T>),
    snapshot_frequency: u32,     // 每 N 次更新写入一次快照
    update_count_since_snapshot: u32,
}

/// DeltaSnapshot 存储类型
pub enum DeltaBlob<T> {
    /// 需要回放祖先写入来重建
    Missing,
    /// 完整快照值
    Snapshot(T),
}

// > **实现备注 (D-01-6)**: 实际实现中 `DeltaBlob` 的 `Snapshot` 变体使用 `serde_json::Value`
// > 而非泛型 `T`：`Snapshot(serde_json::Value)`。这消除了编译时类型保证，但简化了 checkpoint
// > 序列化——所有快照统一为 JSON 值，避免了为每个泛型 T 实现 serde trait 的约束传播。
```

### 7.3 快照策略

快照由两个计数器驱动：

1. **per-channel update count**：每次 `update()` 后递增，达到 `snapshot_frequency` 时写入快照
2. **全局 superstep count**：达到系统上限（默认 5000 步）时强制快照，即使该 channel 未被更新

```rust
impl<T: Default + Clone> DeltaChannel<T> {
    fn checkpoint(&mut self, supersteps_since_snapshot: u32) -> DeltaBlob<T> {
        self.update_count_since_snapshot += 1;

        let should_snapshot = self.update_count_since_snapshot >= self.snapshot_frequency
            || supersteps_since_snapshot >= DELTA_MAX_SUPERSTEPS_SINCE_SNAPSHOT;

        if should_snapshot {
            self.update_count_since_snapshot = 0;
            DeltaBlob::Snapshot(self.value.clone())
        } else {
            DeltaBlob::Missing
        }
    }
}
```

### 7.4 祖先回放重建（Ancestor Walk）

从 checkpoint 恢复时，如果 blob 是 `Missing`，需要从 checkpoint 历史中回放写入：

```rust
impl<T: Default + Clone> DeltaChannel<T> {
    /// 回放祖先写入（最旧到最新）
    ///
    /// 如果写入序列中包含 Overwrite，最后一个 Overwrite 作为新的基准值，
    /// 只有其后的写入被传递给 reducer
    fn replay_writes(&mut self, writes: &[PendingWrite<T>]) {
        let values: Vec<&T> = writes.iter().map(|w| &w.value).collect();
        if values.is_empty() {
            return;
        }

        // 找到最后一个 Overwrite 作为基准
        let mut base = self.value.clone();
        let mut start = 0;
        for (i, v) in values.iter().enumerate() {
            if let Some(overwrite_val) = v.as_overwrite() {
                base = overwrite_val.clone();
                start = i + 1;
            }
        }

        let remaining = &values[start..];
        if !remaining.is_empty() {
            (self.reducer)(&mut base, remaining.iter().cloned().collect());
        }
        self.value = base;
    }
}
```

### 7.5 Reducer 约束

DeltaChannel 的 reducer 必须满足：
- **确定性**：相同输入总是产生相同输出
- **批量不变性（associative across folds）**：分批应用等价于一次应用全部
  ```
  reducer(reducer(state, xs), ys) == reducer(state, xs ++ ys)
  ```
  这允许 LangGraph 以比原始写入更大的批次回放，不影响重建结果。

---

## 源码参考索引

| LangGraph 源码路径 | 说明 |
|---|---|
| `langgraph/libs/langgraph/langgraph/channels/base.py:19` | BaseChannel 抽象基类定义 |
| `langgraph/libs/langgraph/langgraph/channels/last_value.py:20` | LastValue channel（默认 replace 语义） |
| `langgraph/libs/langgraph/langgraph/channels/binop.py:51` | BinaryOperatorAggregate（reducer 语义） |
| `langgraph/libs/langgraph/langgraph/channels/topic.py:23` | Topic channel（pub/sub 累积） |
| `langgraph/libs/langgraph/langgraph/channels/ephemeral_value.py:15` | EphemeralValue（superstep 后清除） |
| `langgraph/libs/langgraph/langgraph/channels/delta.py:25` | DeltaChannel（增量存储优化） |
| `langgraph/libs/langgraph/langgraph/channels/named_barrier_value.py:13` | NamedBarrierValue（wait-all 语义） |
| `langgraph/libs/langgraph/langgraph/graph/state.py:1801` | _get_channels() — 字段到 Channel 的映射逻辑 |
| `langgraph/libs/langgraph/langgraph/graph/state.py:1537` | attach_edge() — 边如何创建触发 channel |
| `langgraph/libs/langgraph/langgraph/graph/message.py:61` | add_messages reducer 实现 |
| `langgraph/libs/langgraph/langgraph/graph/message.py:117` | MessagesState 定义 |
| `langgraph/libs/langgraph/langgraph/pregel/_algo.py:232` | apply_writes() — channel 写入应用 |
| `langgraph/libs/langgraph/langgraph/pregel/_algo.py:392` | prepare_next_tasks() — 基于 versions_seen 调度 |
| `langgraph/libs/langgraph/langgraph/pregel/_loop.py:583` | tick() — 主循环状态机 |
| `langgraph/libs/langgraph/langgraph/pregel/_loop.py:831` | channel_versions 使用位置 |
| `langgraph-doc/persistence.md` | Checkpoint 与 channel_versions 文档 |

---

## 8. Implementation Enhancements (Category C)

The implementation exceeds the design in the following areas:

- **[C-01-001]** Overwrite&lt;T&gt; serialization correctly uses `{"__overwrite__": value}` format per design. The serde implementation matches LangGraph Python's wire format exactly, ensuring checkpoint compatibility beyond the design's specification.

- **[C-01-002]** REMOVE_ALL_MESSAGES provides factory methods (`Message::remove_all()`, `Message::remove()`) instead of the design's const-based approach. More ergonomic and avoids `String::new()` limitations in const context.

- **[C-01-003]** `Message::content_text()` helper extracts text from both `Content::Text` and `Content::MultiPart` variants. A convenience method not described in the design.

- **[C-01-004]** `Message::ai_with_tool_calls()` constructor creates AI messages with tool calls in a single call. Eliminates boilerplate that the design's basic constructors require.

- **[C-01-005]** DeltaBlob uses `serde_json::Value` for its `Snapshot` variant instead of generic `T`. Simplifies checkpoint serialization by avoiding generic serde trait constraints.

- **[C-01-006]** `FieldVersions` derives `Debug`. Enables structured logging and troubleshooting output not required by the design.

- **[C-01-007]** `FieldsChanged` methods (`is_empty()`, `has_field()`) are `const fn`. Enables compile-time evaluation and zero-cost field tracking beyond the design's runtime-only specification.

- **[C-01-008]** Proc-macro supports 8 reducer types (`replace`, `append`, `ephemeral`, `custom`, `last_write_wins`, `any`, `messages`, `untracked`) rather than the 5 listed in design section 2.4. Covers additional LangGraph semantics not captured in the original design.
