# 03 - Pregel 执行引擎

## 概述

Pregel 执行引擎是 Juncture 的核心运行时，负责按 superstep 推进图执行。它实现了 LangGraph 的 Pregel 模型：每个 superstep 内节点并行执行，superstep 之间顺序推进，通过 checkpoint 实现持久化和恢复。

---

## 1. LangGraph Pregel 架构参考

### 1.1 源码结构

| 文件 | 职责 |
|------|------|
| `langgraph/pregel/__init__.py` | Pregel 类定义（CompiledStateGraph 继承自 Pregel） |
| `langgraph/pregel/_loop.py:583` | `PregelLoop.tick()` — 单次迭代状态机 |
| `langgraph/pregel/_loop.py:667` | `PregelLoop.after_tick()` — superstep 后处理 |
| `langgraph/pregel/_algo.py:392` | `prepare_next_tasks()` — 确定下一批执行节点 |
| `langgraph/pregel/_algo.py:260` | `apply_writes()` — 将节点输出写入 channels |
| `langgraph/pregel/_runner.py:75` | `FuturesDict` — 并发任务管理 |
| `langgraph/pregel/_runner.py:135` | `PregelRunner` — 并发执行器 |

### 1.2 执行模型

LangGraph 的 Pregel 每个 superstep 分三个阶段：

1. **Plan（tick）**：确定哪些节点应该执行
   - 检查 recursion_limit（`step > stop`）
   - 调用 `prepare_next_tasks()` 基于 channel_versions 和 versions_seen 确定就绪节点
   - 检查 `interrupt_before` 集合
   - 返回 True（有任务）或 False（完成）

2. **Execute（runner）**：并发执行所有就绪节点
   - `PregelRunner` 使用 `concurrent.futures` 线程池
   - `FuturesDict` 追踪完成状态
   - 每个 task 完成后立即调用 `put_writes()` 持久化其输出

3. **Update（after_tick）**：应用写入、保存 checkpoint
   - `apply_writes()` 将所有节点输出写入对应 channels
   - 更新 `channel_versions`
   - 发射 stream 事件
   - 调用 `_put_checkpoint({"source": "loop"})` 保存完整 checkpoint
   - 检查 `interrupt_after` 集合

### 1.3 关键算法：prepare_next_tasks

> 参考: `langgraph/pregel/_algo.py:392-450`

```python
def prepare_next_tasks(checkpoint, pending_writes, processes, channels, ...):
    """确定下一 superstep 要执行的任务集合。
    
    返回 PUSH tasks (Send) 和 PULL tasks (边触发的节点) 的并集。
    """
    tasks = []
    # 1. 消费 TASKS channel 中的 Send 目（PUSH tasks）
    tasks_channel = channels.get(TASKS)
    if tasks_channel and tasks_channel.is_available():
        for idx, send in enumerate(tasks_channel.get()):
            tasks.append(prepare_single_task((PUSH, idx), ...))
    
    # 2. 确定被触发的节点（PULL tasks）
    for name, proc in processes.items():
        # 检查节点的 trigger channels 是否有更新
        seen = checkpoint["versions_seen"].get(name, {})
        if any(
            checkpoint["channel_versions"].get(chan, null_version) > seen.get(chan, null_version)
            for chan in proc.triggers
        ):
            tasks.append(prepare_single_task((PULL, name), ...))
    
    return {task.id: task for task in tasks}
```

### 1.4 关键算法：apply_writes

> 参考: `langgraph/pregel/_algo.py:260-345`

```python
def apply_writes(checkpoint, channels, tasks, next_version, trigger_to_nodes):
    """将所有 task 的写入应用到 channels。"""
    # 1. 收集所有写入，按 channel 分组
    pending_writes_by_channel = defaultdict(list)
    for task in tasks:
        for chan, val in task.writes:
            if chan not in RESERVED:  # 跳过 INTERRUPT, RESUME, ERROR 等
                pending_writes_by_channel[chan].append(val)
    
    # 2. 逐 channel 应用写入
    updated_channels = set()
    for chan, vals in pending_writes_by_channel.items():
        if channels[chan].update(vals):  # channel 自己决定如何 merge
            checkpoint["channel_versions"][chan] = next_version
            updated_channels.add(chan)
    
    # 3. 通知未更新的 channels（新 step 开始）
    for chan in channels:
        if chan not in updated_channels:
            channels[chan].update(EMPTY_SEQ)
    
    return updated_channels
```

### 1.5 put_writes 时序

> 参考: `langgraph/pregel/_loop.py:667-697`

```
task_1 完成 → put_writes(task_1.writes)  ← 立即持久化
task_2 完成 → put_writes(task_2.writes)  ← 立即持久化
task_3 完成 → put_writes(task_3.writes)  ← 立即持久化
所有 tasks 完成 → apply_writes()         ← 内存中合并
                → _put_checkpoint()       ← 保存完整快照
```

---

## 2. Juncture PregelLoop 设计

### 2.1 核心数据结构

```rust
// juncture-core/src/pregel/loop_.rs

pub struct PregelLoop<S: State> {
    /// 当前 state
    state: S,
    
    /// 节点注册表（保持注册顺序，决定 merge 顺序）
    nodes: IndexMap<String, Arc<dyn Node<S>>>,
    
    /// 编译后的触发关系表
    trigger_table: TriggerTable<S>,
    
    /// 当前 superstep 编号（从 0 开始）
    step: usize,
    
    /// 最大 superstep 数（recursion_limit）
    stop: usize,
    
    /// 字段版本追踪器
    field_versions: FieldVersionTracker,
    
    /// 每个节点已消费的字段版本
    versions_seen: VersionsSeen,
    
    /// Checkpoint 存储
    checkpointer: Arc<dyn CheckpointSaver>,
    
    /// 运行配置
    config: RunnableConfig,
    
    /// 取消令牌
    cancellation_token: CancellationToken,
    
    /// 预算追踪器
    budget_tracker: BudgetTracker,
    
    /// Stream 事件发送端
    stream_tx: Option<mpsc::UnboundedSender<StreamEvent<S>>>,
    
    /// 当前 superstep 待执行的节点
    pending_tasks: Vec<PendingTask<S>>,
    
    /// interrupt_before 节点集合
    interrupt_before: HashSet<String>,
    
    /// interrupt_after 节点集合
    interrupt_after: HashSet<String>,
    
    /// 执行状态
    status: LoopStatus,

    /// 当前 checkpoint 的 pending writes（用于崩溃恢复）
    checkpoint_pending_writes: Vec<PendingWrite>,
}

// ─────────────────────────────────────────────────────
// PregelLoop 拆分为三个职责清晰的结构
// ─────────────────────────────────────────────────────

/// 执行上下文：持有可变状态和版本追踪
pub struct ExecutionContext<S: State> {
    /// 当前 state
    pub state: S,
    /// 字段版本追踪器
    pub field_versions: FieldVersionTracker,
    /// 每个节点已消费的字段版本
    pub versions_seen: VersionsSeen,
    /// 当前 superstep 的 checkpoint pending writes
    pub pending_writes: Vec<PendingWrite>,
    /// **Scratchpad 实现**: ExecutionContext 包含 scratchpad 字段，
    /// 用于多中断场景下的 null-resume 处理。
    /// - scratchpad: HashMap<String, serde_json::Value> 存储中断上下文
    /// - get_null_resume(key) 提供部分恢复能力，返回已存储的上下文
    /// - set_null_resume(key, value) 保存中断上下文供后续恢复
    /// 这种机制支持同一节点多次中断而不会丢失状态。
}

/// 执行配置：不可变的运行时参数
pub struct ExecutionConfig {
    /// 最大 superstep 数
    pub recursion_limit: usize,
    /// interrupt_before 节点集合
    pub interrupt_before: HashSet<String>,
    /// interrupt_after 节点集合
    pub interrupt_after: HashSet<String>,
    /// 预算配置
    pub budget: Option<BudgetConfig>,
    /// 持久化模式
    pub durability: Durability,
    /// 重试策略（节点级别覆盖）
    pub retry_policies: HashMap<String, RetryPolicy>,
    /// 超时策略（节点级别覆盖）
    pub timeout_policies: HashMap<String, TimeoutPolicy>,
}

/// PregelLoop：仅负责编排逻辑（tick / after_tick 调度）
pub struct PregelLoop<S: State> {
    /// 可变执行上下文
    pub context: ExecutionContext<S>,
    /// 不可变配置
    pub config: ExecutionConfig,
    /// 节点注册表
    pub nodes: IndexMap<String, Arc<dyn Node<S>>>,
    /// 触发关系表
    pub trigger_table: TriggerTable<S>,
    /// Checkpoint 存储后端
    pub checkpointer: Arc<dyn CheckpointSaver>,
    /// 运行配置
    pub runnable_config: RunnableConfig,
    /// 取消令牌
    pub cancellation_token: CancellationToken,
    /// 预算追踪器
    pub budget_tracker: BudgetTracker,
    /// Stream 事件发送端
    pub stream_tx: Option<mpsc::UnboundedSender<StreamEvent<S>>>,
    /// 当前 superstep 编号
    pub step: usize,
    /// 循环状态
    pub status: LoopStatus,
}

// > **实现备注 (D-03-1)**: 实际实现中 PregelLoop 不使用独立的 `ExecutionContext<S>` 和 `ExecutionConfig` 结构体。
// > 而是将所有字段平铺在 PregelLoop 中，通过 `as_context()` 和 `as_config()` 访问器方法
// > 提供等同于上述两结构体的视图。这种扁平化设计减少了间接访问开销，
// > 同时通过访问器方法保持逻辑分组。

/// 待执行任务
pub struct PendingTask<S: State> {
    /// 任务 ID（UUID）
    pub id: String,
    /// 目标节点名
    pub node_name: String,
    /// 任务类型
    pub trigger: TaskTrigger,
    /// 状态覆盖（Send API 使用）
    pub state_override: Option<S>,
}

/// 任务触发类型
pub enum TaskTrigger {
    /// 边触发（PULL）
    Pull,
    /// Send API 触发（PUSH）
    Push { index: usize },
}

/// 循环状态
pub enum LoopStatus {
    /// 正在执行
    Running,
    /// 正常完成
    Done,
    /// 超出步数限制
    OutOfSteps,
    /// interrupt_before 触发（携带具体中断信号）
    InterruptBefore(Vec<InterruptSignal>),
    /// interrupt_after 触发（携带具体中断信号）
    InterruptAfter(Vec<InterruptSignal>),
    /// 预算超限
    BudgetExceeded,
    /// 被取消
    Cancelled,
    /// 优雅停止（RunControl 请求）
    Drained,
}

// > **实现备注 (D-03-3)**: 实际实现中 `InterruptBefore` 和 `InterruptAfter` 变体携带 `Vec<InterruptSignal>` 数据
// > 而非作为单元变体（unit variants）。这提供了更丰富的中断信息，
// > 允许调用方了解具体哪些中断信号被触发，而不仅仅是知道发生了中断。

/// Superstep 执行结果
pub struct SuperstepResult<S: State> {
    /// 每个节点的输出（按完成顺序）
    pub task_outputs: Vec<TaskOutput<S>>,
    /// 子图冒泡事件（中断、排空、父命令），需要由父 PregelLoop 处理
    pub bubble_ups: Vec<BubbleUp<S>>,
}

// > **实现备注 (C-03-004)**: 实际实现中 `SuperstepResult` 额外包含 `bubble_ups: Vec<BubbleUp<S>>` 字段。
// > 该字段包含从嵌套子图执行中冒泡的事件（中断、排空、父命令），父 PregelLoop 需要处理这些事件。
// > 这支持子图到父图的控制流传播，符合 LangGraph 的 GraphBubbleUp 语义。

/// 单个任务的输出
pub struct TaskOutput<S: State> {
    pub task_id: String,
    pub node_name: String,
    pub command: Command<S>,
    pub duration: Duration,
    pub trigger: TaskTrigger,  // PULL/PUSH 来源，用于 merge 阶段确定性排序
    pub triggered_fields: Vec<usize>,  // 触发该任务的字段索引，用于细粒度 consumption
    pub error: Option<JunctureError>,  // 执行错误（如果有），用于错误恢复
}

// > **实现备注 (D-03-4)**: 实际实现中 `TaskOutput` 额外包含 `trigger: TaskTrigger` 字段。
// > 该字段记录任务是 PULL（边触发）还是 PUSH（Send API 触发，含 send index），
// > 用于 merge 阶段的确定性排序：PULL tasks 按节点名字母序，PUSH tasks 按 send index 排序。
//
// > **实现备注 (C-03-003)**: 实际实现中 `TaskOutput` 额外包含 `triggered_fields: Vec<usize>` 和
// > `error: Option<JunctureError>` 字段：
// > - `triggered_fields` 记录哪些字段的更新导致该任务被调度，用于细粒度 consumption
// > - `error` 在节点有注册的 error_handler 时包含错误信息，用于错误恢复
```


### 2.2 FieldVersionTracker 与 VersionsSeen

```rust
// juncture-core/src/pregel/scheduler.rs

/// 追踪每个字段的版本号
/// 等价于 LangGraph 的 checkpoint["channel_versions"]
///
///
/// **版本递增算法**：与 LangGraph 一致，使用全局最大版本号递增策略。
/// 每次 bump 时，所有在同一 superstep 中更新的字段获得**相同的版本号**，
/// 即全局最大版本号 + 1。这与独立递增不同，确保了 checkpoint 格式的兼容性。
///
/// 参考: `langgraph/pregel/_algo.py:232-345` — `GetNextVersion` 函数
pub struct FieldVersionTracker {
    /// field_index → current_version
    /// 注意：同一 superstep 内更新的所有字段共享相同的版本号
    versions: Vec<u64>,
    /// 全局最大版本号（用于递增）
    global_max: u64,
}

impl FieldVersionTracker {
    pub fn new(num_fields: usize) -> Self {
        Self { versions: vec![0; num_fields], global_max: 0 }
    }

    /// 在 superstep 结束时，将所有 changed 字段统一设置为 global_max + 1
    /// 这与 LangGraph 的 GetNextVersion 算法一致
    pub fn bump_all(&mut self, changed: &[usize]) {
        self.global_max += 1;
        for &idx in changed {
            self.versions[idx] = self.global_max;
        }
    }

    /// 单字段递增（保持向后兼容，某些场景仍需要）
    pub fn bump(&mut self, field_idx: usize) {
        self.global_max += 1;
        self.versions[field_idx] = self.global_max;
    }

    pub fn get(&self, field_idx: usize) -> u64 {
        self.versions[field_idx]
    }

    pub fn as_slice(&self) -> &[u64] {
        &self.versions
    }
}

/// 追踪每个节点已消费的字段版本
/// 等价于 LangGraph 的 checkpoint["versions_seen"]
pub struct VersionsSeen {
    /// node_name → field_versions snapshot
    seen: HashMap<String, Vec<u64>>,
}

impl VersionsSeen {
    pub fn new(node_names: &[String], num_fields: usize) -> Self {
        let seen = node_names.iter()
            .map(|name| (name.clone(), vec![0; num_fields]))
            .collect();
        Self { seen }
    }

    /// 判断节点是否应该被激活（有未消费的字段更新）
    pub fn should_activate(
        &self,
        node_name: &str,
        trigger_fields: &[usize],
        current: &FieldVersionTracker,
    ) -> bool {
        let seen = &self.seen[node_name];
        trigger_fields.iter().any(|&idx| current.get(idx) > seen[idx])
    }

    /// 标记节点已消费当前版本
    pub fn mark_consumed(&mut self, node_name: &str, current: &FieldVersionTracker) {
        let seen = self.seen.get_mut(node_name).unwrap();
        seen.copy_from_slice(current.as_slice());
    }
}
```

---

## 3. 完整执行流程

### 3.1 invoke() 入口

```rust
// juncture-core/src/graph/compiled.rs

impl<S: State + Serialize + DeserializeOwned> CompiledGraph<S> {
    pub async fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> Result<S, JunctureError> {
        let mut loop_ = self.create_pregel_loop(input, config).await?;
        
        while loop_.tick()? {
            loop_.execute_superstep().await?;
            loop_.after_tick().await?;
        }
        
        Ok(loop_.into_state())
    }
}
```

### 3.2 完整流程图

```
invoke(input, config)
│
├─ 1. 加载或创建 Checkpoint
│   ├─ config.thread_id 存在?
│   │   ├─ YES → checkpointer.get_tuple(config)
│   │   │   ├─ 有历史 checkpoint → 恢复 state + field_versions + versions_seen
│   │   │   │                      + pending_tasks（从 checkpoint.next 计算）
│   │   │   └─ 无历史 → 使用 input 作为初始 state
│   │   └─ NO → 使用 input 作为初始 state（无持久化）
│   └─ 保存初始 checkpoint（source: "input"）
│
├─ 2. 计算初始 pending_tasks
│   └─ 从 entry_point 出发，确定第一批节点
│
└─ 3. Pregel Loop
    │
    ┌─── tick() ──────────────────────────────────────────────────┐
    │                                                              │
    │  a. 检查 recursion_limit                                     │
    │     step > stop → status = OutOfSteps, return false          │
    │                                                              │
    │  b. 检查 cancellation_token                                  │
    │     token.is_cancelled() → status = Cancelled, return Err    │
    │                                                              │
    │  c. prepare_next_tasks()                                     │
    │     基于 trigger_table + field_versions + versions_seen       │
    │     + Send targets → 确定 pending_tasks                      │
    │                                                              │
    │  d. pending_tasks 为空?                                      │
    │     YES → status = Done, return false                        │
    │                                                              │
    │  e. 检查 interrupt_before                                    │
    │     中断检查算法（should_interrupt）:                           │
    │     1. 首先检查是否有 channel 自上次中断以来被更新:              │
    │        比较 channel_versions 与 versions_seen[INTERRUPT]      │
    │        如果 any_updates_since_prev_interrupt == false          │
    │        → 跳过中断（版本未变化，无需再次中断）                   │
    │     2. 然后检查 pending_tasks 中是否有节点在                    │
    │        interrupt_before 集合中                                 │
    │     pending_tasks 中有节点在 interrupt_before 集合中?          │
    │     YES → 保存 checkpoint, status = InterruptBefore,         │
    │           return Err(Interrupted)                             │
    │                                                              │
    │     **多中断匹配算法**: 实现支持 3 种中断匹配策略              │
    │     - Single: 单次匹配（默认），触发后即停止                  │
    │     - ById: 按 interrupt signal ID 匹配，精确匹配特定信号    │
    │     - ByNamespace: 按命名空间前缀匹配，支持批量处理           │
    │     通过 scratchpad 实现 null-resume 处理，避免重复触发        │
    │     相同中断。interrupt_versions_seen HashMap 存储中断时       │
    │     的 channel 版本快照，用于中断去重检测。                    │
    │                                                              │
    │  f. return true（有任务需要执行）                              │
    └──────────────────────────────────────────────────────────────┘
    │
    ┌─── execute_superstep() ─────────────────────────────────────┐
    │                                                              │
    │  a. 为每个 pending_task 克隆 state（或使用 state_override）   │
    │                                                              │
    │  b. tokio::spawn 每个 task 到 JoinSet                        │
    │     每个 task 内部：                                          │
    │     tokio::select! {                                         │
    │         biased;                                              │
    │         _ = token.cancelled() => Err(Cancelled),             │
    │         result = node.call(state, &config) => result         │
    │     }                                                        │
    │                                                              │
    │  c. 逐个收集完成的 task                                      │
    │     每个 task 完成后：                                        │
    │     - 记录 TaskOutput                                        │
    │     - 调用 checkpointer.put_writes() 持久化该 task 的输出    │
    │     - 发射 StreamEvent::TaskEnd                              │
    │     **CallbackHandler 集成**: 在任务执行各阶段调用            │
    │     - on_node_start(task_id, node_name): task 开始前调用     │
    │     - on_node_end(task_id, node_name, duration): task 成功后 │
    │     - on_node_error(task_id, node_name, error): 失败时调用   │
    │     这允许外部监听器追踪节点执行生命周期。                    │
    │                                                              │
    │                                                              │
    │  d. 任何 task 失败 → 取消剩余 tasks，返回错误                 │
    │                                                              │
    │  e. 返回 SuperstepResult                                     │
    └──────────────────────────────────────────────────────────────┘
    │
    ┌─── after_tick() ────────────────────────────────────────────┐
    │                                                              │
    │  a. apply_writes: 按节点注册顺序合并所有 task 的 update       │
    │     - 对每个 task 的 Command.update:                          │
    │       changed |= state.apply(update)                         │
    │     - 检测 Replace reducer 多写入冲突                         │
    │     - 更新 field_versions（被修改的字段版本号 +1）            │
    │                                                              │
    │  b. 清除 ephemeral 字段                                      │
    │     state.reset_ephemeral()                                  │
    │                                                              │
    │  c. 发射 stream 事件                                         │
    │     - StreamMode::Values → StreamEvent::Values { state }     │
    │     - StreamMode::Updates → StreamEvent::Updates { updates }  │
    │     - Command.stream_data → StreamEvent::Custom (逐项发射)  │
    │     **StreamData Custom Events**: Command 包含               │
    │     stream_data: Vec<serde_json::Value>，每个 Value 作为独立   │
    │     StreamEvent::Custom 发射。支持节点发出自定义流式数据     │
    │                                                              │
    │  d. 清空 checkpoint_pending_writes                            │
    │                                                              │
    │  e. 保存 checkpoint                                          │
    │     checkpointer.put(checkpoint, metadata{source: "loop"})   │
    │     **Delta Counter 优化**: checkpoint 性能通过 delta 计数器优化  │
    │     HashMap<String, DeltaCounters> 追踪自上次完整快照        │
    │     以来的更新和 superstep 数，仅在必要时执行完整快照。       │
    │                                                              │
    │     DeltaCounters 基础设施 (C-03-007):                      │
    │     - PregelLoop.delta_counters: HashMap<String, DeltaCounters> │
    │     - DeltaCounters { writes_since_last_snapshot, supersteps_since_last_snapshot } │
    │     - update(): 每次写操作后增加计数器                       │
    │     - reset(): 完整快照后重置计数器                          │
    │     - should_take_full_snapshot(): 检查是否需要完整快照      │
    │                                                              │
    │     注意: 当前实现总是保存完整快照。Delta 计数器基础设施已就位，│
    │     但增量快照格式和恢复逻辑需要扩展以支持部分快照。完整快照更简单，│
    │     并保证恢复正确性。增量快照可作为未来优化添加。            │
    │                                                              │
    │  f. 检查 interrupt_after                                     │
    │     当前执行的节点中有在 interrupt_after 集合中的?             │
    │     YES → status = InterruptAfter, return Err(Interrupted)   │
    │                                                              │
    │  g. 检查 budget                                              │
    │     budget_tracker.check() → BudgetExceeded?                 │
    │     YES → 根据 on_exceeded 策略处理                           │
    │                                                              │
    │  h. 计算下一 superstep 的 pending_tasks                      │
    │     - 处理 Command.goto（覆盖外部边）                         │
    │     - 处理外部边（Fixed + Conditional）                       │
    │     - 处理 Send targets                                      │
    │     - 去重                                                   │
    │                                                              │
    │  i. step += 1                                                │
    └──────────────────────────────────────────────────────────────┘
    │
    └─ 循环回到 tick()
```

---

## 4. Superstep 并发执行

### 4.1 实现

**State Cloning 策略**：

当前设计中每个 task spawn 都获得完整的 state clone，对于大状态（长对话历史）开销较高。
生产环境建议使用 `CowState<S>`（Copy-on-Write）模式：


```rust
/// 写时复制状态包装，避免大状态的完整 clone
///
/// 节点接收 CowState<S> 而非 S，只读节点无需 clone。
/// 只有实际修改 state 的节点才会触发 clone。
pub struct CowState<S: State> {
    /// 共享的不可变基础状态（Arc 引用计数）
    inner: Arc<S>,
    /// 是否已发生修改（dirty flag）
    dirty: bool,
}

impl<S: State> CowState<S> {
    /// 从共享引用创建只读视图（零拷贝）
    pub fn shared(state: &Arc<S>) -> Self {
        Self {
            inner: Arc::clone(state),
            dirty: false,
        }
    }

    /// 获取可变引用（首次调用时触发 clone）
    pub fn get_mut(&mut self) -> &mut S
    where
        S: Clone,
    {
        if !self.dirty {
            self.inner = Arc::new((*self.inner).clone());
            self.dirty = true;
        }
        Arc::get_mut(&mut self.inner).unwrap()
    }

    /// 获取不可变引用（零拷贝）
    pub fn get(&self) -> &S {
        &self.inner
    }
}
```

**使用方式**：节点接收 `CowState<S>`，只读节点通过 `get()` 访问（无 clone），写节点通过 `get_mut()` 访问（首次 clone）。

**Debug trait bound**：

`State` trait 应包含 `Debug` bound 以支持可观测性和故障排查：

```rust
pub trait State: Clone + Send + Sync + std::fmt::Debug + 'static { ... }
```

这使得 `PregelLoop<S>` 中的状态可在日志和错误消息中格式化输出。

```rust
// juncture-core/src/pregel/runner.rs

pub async fn execute_superstep<S: State>(
    pending_tasks: &[PendingTask<S>],
    state: &S,
    nodes: &IndexMap<String, Arc<dyn Node<S>>>,
    config: &RunnableConfig,
    cancellation_token: &CancellationToken,
    checkpointer: &Arc<dyn CheckpointSaver>,
    stream_tx: &Option<mpsc::UnboundedSender<StreamEvent<S>>>,
    /// 有界并发控制：使用 Semaphore 限制并行任务数量。
    /// 防止大量 Send 目标或大型图中节点过多导致系统资源耗尽。
    /// Semaphore::new(permits) 创建许可池，task spawn 前获取许可，
    /// task 结束后自动释放许可（Drop trait）。
    max_parallel_tasks: Option<usize>,
) -> Result<SuperstepResult<S>, JunctureError> {
    // 创建 Semaphore（如果设置了 max_parallel_tasks）
    let semaphore = max_parallel_tasks.map(|limit| Arc::new(Semaphore::new(limit)));
    
    let mut join_set = JoinSet::new();
    let mut task_outputs = Vec::with_capacity(pending_tasks.len());
    
    for task in pending_tasks {
        let node = nodes[&task.node_name].clone();
        let task_state = match &task.state_override {
            Some(override_state) => override_state.clone(),
            None => state.clone(),  // 或使用 CowState::shared(&Arc::new(state.clone()))
        };
        let task_config = config.clone();
        let token = cancellation_token.clone();
        let checkpointer = checkpointer.clone();
        let stream_tx = stream_tx.clone();
        let task_id = task.id.clone();
        let node_name = task.node_name.clone();
        
        // 获取 semaphore 许可（如果配置了）
        let permit = match &semaphore {
            Some(sem) => Some(sem.acquire().await.unwrap()),
            None => None,
        };
        
        join_set.spawn(async move {
            // permit 在作用域结束时自动释放
            let _permit = permit;
            
            let start = Instant::now();
            let result = tokio::select! {
                biased;
                _ = token.cancelled() => Err(JunctureError::Cancelled),
                result = node.call(task_state, &task_config) => result,
            };
            let duration = start.elapsed();
            
            let command = result?;
            
            // 立即持久化单个 task 的输出
            checkpointer.put_writes(&task_id, &node_name, &command.update).await?;
            
            // 发射流事件
            if let Some(tx) = stream_tx {
                let _ = tx.send(StreamEvent::TaskEnd {
                    task_id: task_id.clone(),
                    node_name: node_name.clone(),
                    duration,
                });
            }
            
            Ok(TaskOutput {
                task_id,
                node_name,
                command,
                duration,
            })
        });
    }
) -> Result<SuperstepResult<S>, JunctureError> {
    let mut join_set = JoinSet::new();
    let mut task_map: HashMap<tokio::task::Id, String> = HashMap::new();

    for pending in pending_tasks {
        let node = nodes.get(&pending.node_name)
            .ok_or_else(|| JunctureError::NodeNotFound(pending.node_name.clone()))?
            .clone();
        
        // 每个 task 获得独立的 state 克隆（或 Send 的 state_override）
        let task_state = pending.state_override
            .clone()
            .unwrap_or_else(|| state.clone());
        
        let task_config = config.clone();
        let token = cancellation_token.clone();
        let task_id = pending.id.clone();
        let node_name = pending.node_name.clone();

        let handle = join_set.spawn(async move {
            let start = Instant::now();
            let result = tokio::select! {
                biased;  // 优先检查取消信号
                _ = token.cancelled() => Err(JunctureError::Cancelled),
                r = node.call(task_state, &task_config) => r,
            };
            TaskOutput {
                task_id,
                node_name,
                command: result?,
                duration: start.elapsed(),
            }
        });
    }

    // 收集结果
    let mut outputs = Vec::with_capacity(pending_tasks.len());
    
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(Ok(output)) => {
                // 立即持久化该 task 的写入（崩溃恢复用）
                if let Some(ref update) = output.command.update {
                    let writes = vec![PendingWrite {
                        task_id: output.task_id.clone(),
                        data: serde_json::to_value(update)
                            .map_err(|e| JunctureError::Serialize(e))?,
                    }];
                    checkpointer.put_writes(config, writes, &output.task_id).await?;
                }
                
                // 发射 TaskEnd 事件
                if let Some(tx) = stream_tx {
                    let _ = tx.send(StreamEvent::TaskEnd {
                        node: output.node_name.clone(),
                        step: config.metadata.get("step")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                    });
                }
                
                outputs.push(output);
            }
            Ok(Err(e)) => {
                // 节点执行失败：取消所有剩余 tasks
                cancellation_token.cancel();
                // 等待剩余 tasks 完成（被取消）
                join_set.shutdown().await;
                return Err(e);
            }
            Err(join_err) => {
                // tokio task panic
                cancellation_token.cancel();
                join_set.shutdown().await;
                return Err(JunctureError::TaskPanicked(join_err.to_string()));
            }
        }
    }

    Ok(SuperstepResult { task_outputs: outputs })
}
```

### 4.2 并发保证

- **无共享可变状态**：每个 task 持有 state 的独立克隆
- **真多核并行**：`tokio::spawn` 将 task 分发到 work-stealing 线程池
- **结构化取消**：`CancellationToken` + `select! biased` 保证取消信号优先响应
- **确定性 merge**：虽然执行顺序不确定，但 merge 按注册顺序执行


---

## 5. Merge 阶段（apply_writes）

### 5.1 实现


**IMPORTANT: versions_seen 时序**：`versions_seen` 必须在 `apply_writes` 开始时、任何 channel 更新或版本递增之前更新。这记录了 tasks 执行时的 channel 版本快照。

**Merge 排序**：LangGraph 使用 path-based sorting 而非节点注册顺序。PULL tasks 按节点名字母序排列，PUSH tasks 按 send index 排序：

```rust
// juncture-core/src/pregel/loop_.rs

fn apply_writes<S: State>(
    state: &mut S,
    superstep_result: &SuperstepResult<S>,
    nodes: &IndexMap<String, Arc<dyn Node<S>>>,
    field_versions: &mut FieldVersionTracker,
) -> Result<FieldsChanged, JunctureError> {
    let mut total_changed = FieldsChanged::default();

    // 步骤 0: 在任何更新前，先记录当前版本到 versions_seen
    // versions_seen[node] = current channel_versions (snapshot before mutation)

    // 按 path-based sorting 排列 tasks（替代 IndexMap 注册顺序）
    // PULL tasks: 按节点名字母序排列
    // PUSH tasks: 按 send index 排序
    //
    // Path-based sorting 实现细节：
    // - PULL tasks 按 node_name.cmp() 排序（字母序）
    // - PUSH tasks 按 send index 排序（原始顺序）
    // - PULL 优先于 PUSH（match 分支排序：Pull < Push）
    //
    // 这种排序与 LangGraph 的 prepare_single_task 行为一致，确保
    // merge 阶段的确定性，无论并发执行顺序如何。
    let mut sorted_tasks: Vec<&TaskOutput<S>> = superstep_result.task_outputs.iter().collect();
    sorted_tasks.sort_by(|a, b| {
        match (&a.trigger, &b.trigger) {
            (TaskTrigger::Pull, TaskTrigger::Pull) => a.node_name.cmp(&b.node_name),
            (TaskTrigger::Push { index: idx_a }, TaskTrigger::Push { index: idx_b }) => idx_a.cmp(idx_b),
            (TaskTrigger::Pull(_), TaskTrigger::Push(_)) => std::cmp::Ordering::Less,
            (TaskTrigger::Push(_), TaskTrigger::Pull(_)) => std::cmp::Ordering::Greater,
        }
    });

    // 逐个应用 update
    for output in &sorted_tasks {
        if let Some(ref update) = output.command.update {
            let changed = state.apply(update.clone());
            total_changed.merge(&changed);
        }
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

> **IndexMap trade-off note (M-1)**：早期设计依赖 IndexMap 的插入顺序来保证 merge 确定性。实际实现中，path-based sorting（PULL 按名字、PUSH 按索引）提供更强的确定性保证，与 LangGraph 的 `prepare_single_task` 行为一致。IndexMap 仍用于节点注册表以保持 O(1) 查找 + 有序迭代。

> **Implementation Note (D-03-9)**: The actual `apply_writes()` does not take a `nodes` parameter. Instead, it sorts task outputs by trigger type (PULL by node name, PUSH by send index) to provide deterministic merge order that matches LangGraph semantics.

### 5.2 多写入冲突检测

对于 `Replace` reducer 的字段，同一 superstep 内只允许一个节点写入。检测逻辑：

```rust
fn check_replace_conflicts<S: State>(
    superstep_result: &SuperstepResult<S>,
    replace_fields: &[usize],  // 使用 Replace reducer 的字段索引
) -> Result<(), JunctureError> {
    for &field_idx in replace_fields {
        let writers: Vec<&str> = superstep_result.task_outputs.iter()
            .filter(|o| {
                o.command.update.as_ref()
                    .map_or(false, |u| update_has_field(u, field_idx))
            })
            .map(|o| o.node_name.as_str())
            .collect();
        
        if writers.len() > 1 {
            return Err(JunctureError::MultipleWriters {
                field_index: field_idx,
                writers: writers.iter().map(|s| s.to_string()).collect(),
            });
        }
    }
    Ok(())
}
```

### 5.3 Ephemeral 字段重置

> 等价于 LangGraph 的 `EphemeralValue` channel 在 `consume()` 后清除

```rust
// apply_writes 完后立即调用
state.reset_ephemeral();
// proc-macro 生成的实现将所有 #[reducer(ephemeral)] 字段设为 None/Default
```

### 5.4 consume() 步骤


在 `apply_writes` 合并所有写入后、`reset_ephemeral()` 之前，对所有被当前 superstep 中 tasks 触发的 channels 调用 `consume()`：

```rust
/// apply_writes 后的 consume 步骤
///
/// 对所有在 task triggers 中引用的 channels 调用 consume()。
/// - EphemeralValue channels：清除值（值只在当前 superstep 有效）
/// - 其他 channels：no-op，但仍然更新版本号
///
/// 参考: `langgraph/channels/base.py:19` — BaseChannel.consume()
fn consume_triggered_channels<S: State>(
    state: &mut S,
    triggered_channels: &[usize],  // 被 tasks 触发的字段索引
) {
    for &field_idx in triggered_channels {
        state.consume_field(field_idx);
    }
}
```

> **Implementation Note (D-03-10)**: The current implementation resets all ephemeral fields each superstep via `reset_ephemeral()` instead of selectively consuming only triggered channels. This is functionally correct but less optimized than the fine-grained `consume_triggered_channels` approach described above.

### 5.5 finish() 通知

当 superstep 产生的更新 channels 不再触发任何后续节点时（执行完成），Pregel 引擎对所有 channels 调用 `finish()`：

```rust
/// 图执行完成时的 finish 通知
///
/// 对所有 channels 调用 finish()，允许它们最终化状态。
/// 这对于 AfterFinish 变体（LastValueAfterFinish、NamedBarrierValueAfterFinish）
/// 至关重要——它们只在 finish() 后才使值对订阅者可见。
///
/// 参考: `langgraph/pregel/_algo.py` — finish 信号处理
fn finish_all_channels<S: State>(
    state: &mut S,
    field_count: usize,
) {
    for field_idx in 0..field_count {
        state.finish_field(field_idx);
    }
}
```

`finish()` 的调用条件：当 `compute_next_tasks()` 返回空的任务列表时（所有活跃路径都到达 END，或无更多节点被 channel 更新触发）。

> **实现备注 (C-03-005)**: 实际实现中 `PregelLoop` 包含 `channels_finished: bool` 字段防止重复调用。
> 该标志确保 `finish_all_channels()` 在每次执行中只调用一次，即使有多个终止路径（中断、取消、预算、递归限制等）。
> 多次调用会导致冗余操作，并可能引起 `LastValueAfterFinishChannel` 语义问题。

### 5.6 确定性保证

| 因素 | 保证方式 |
|------|----------|
| Merge 顺序 | Path-based sorting（PULL 按名字、PUSH 按索引） |
| 多写入 Append | 按 path sorting 顺序 extend |
| 多写入 Replace | 禁止（运行时检测） |
| 多写入 Custom | 按 path sorting 顺序调用用户函数 |

---

## 6. 节点调度算法

### 6.1 主路径：边驱动调度

**优化：trigger_to_nodes 映射**

为了高效确定哪些节点需要被 channel 更新触发，维护一个反向映射表：

```rust
/// Channel 名称 → 订阅该 channel 的节点集合
///
/// 当 updated_channels 集合已知时，只需检查被触发的节点，
/// 而非遍历所有节点。将 O(nodes) 调度降低为 O(triggered_nodes)。
pub struct TriggerToNodes {
    /// channel_name → 订阅该 channel 的节点名集合
    mapping: HashMap<String, HashSet<String>>,
}

impl TriggerToNodes {
    /// 从编译后的 TriggerTable 构建
    pub fn from_trigger_table<S: State>(table: &TriggerTable<S>) -> Self {
        let mut mapping: HashMap<String, HashSet<String>> = HashMap::new();
        for (node_name, sources) in &table.incoming {
            for source in sources {
                match source {
                    TriggerSource::Edge { from } | TriggerSource::Send { from } => {
                        mapping
                            .entry(from.clone())
                            .or_default()
                            .insert(node_name.clone());
                    }
                }
            }
        }
        Self { mapping }
    }

    /// 给定已更新的 channel 集合，返回需要检查的节点
    pub fn triggered_nodes(&self, updated_channels: &[String]) -> HashSet<String> {
        updated_channels.iter()
            .filter_map(|ch| self.mapping.get(ch))
            .flatten()
            .cloned()
            .collect()
    }
}
```

> **实现备注 (C-03-006)**: `TriggerToNodes` 已完全集成到调度路径。
> `compute_next_tasks()` 使用 `triggered_nodes()` 方法基于更新的 channels
> 高效确定需要调度的节点，将 O(nodes) 复杂度降低为 O(triggered_nodes)。
> 反向映射在编译时从 `TriggerTable` 构建，调度时只需查询。

```rust
// juncture-core/src/pregel/scheduler.rs

pub fn compute_next_tasks<S: State>(
    completed_tasks: &[TaskOutput<S>],
    trigger_table: &TriggerTable<S>,
    state: &S,
) -> Result<Vec<PendingTask<S>>, JunctureError> {
    let mut next_tasks: Vec<PendingTask<S>> = Vec::new();
    let mut seen_nodes: HashSet<String> = HashSet::new();
    
    for output in completed_tasks {
        // 1. Command.goto 优先级最高
        if let Some(ref goto) = output.command.goto {
            match goto {
                Goto::Next(target) => {
                    if target != END && seen_nodes.insert(target.clone()) {
                        next_tasks.push(PendingTask::pull(target.clone()));
                    }
                }
                Goto::Multiple(targets) => {
                    for target in targets {
                        if target != END && seen_nodes.insert(target.clone()) {
                            next_tasks.push(PendingTask::pull(target.clone()));
                        }
                    }
                }
                Goto::Send(send_targets) => {
                    for (idx, target) in send_targets.iter().enumerate() {
                        next_tasks.push(PendingTask::push(
                            target.node.clone(),
                            idx,
                            Some(target.state.clone()),
                        ));
                    }
                }
                Goto::End => {
                    // 该路径终止，不添加后续节点
                }
            }
            continue;  // goto 覆盖外部边
        }
        
        // 2. 无 goto → 使用外部边
        if let Some(edges) = trigger_table.outgoing.get(&output.node_name) {
            for edge in edges {
                match edge {
                    CompiledEdge::Fixed { target } => {
                        if target != END && seen_nodes.insert(target.clone()) {
                            next_tasks.push(PendingTask::pull(target.clone()));
                        }
                    }
                    CompiledEdge::Conditional { router, path_map } => {
                        let route_result = router.route(state).await?;
                        match route_result {
                            RouteResult::One(target) => {
                                if target != END && seen_nodes.insert(target.clone()) {
                                    next_tasks.push(PendingTask::pull(target.clone()));
                                }
                            }
                            RouteResult::Multiple(targets) => {
                                for target in targets {
                                    if target != END && seen_nodes.insert(target.clone()) {
                                        next_tasks.push(PendingTask::pull(target.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(next_tasks)
}
```

### 6.2 调度优先级

```
1. Command.goto（节点返回的路由指令）
   ↓ 如果 goto 为 None
2. 外部边（Fixed / Conditional）
   ↓ 合并
3. Send targets（动态 fan-out）
   ↓ 去重（同一节点不重复执行，Send 除外）
4. 最终 pending_tasks
```

### 6.3 去重规则

- PULL tasks：同一节点名只出现一次（`seen_nodes` HashSet）
- PUSH tasks（Send）：不去重，每个 Send 是独立任务，即使目标相同

---

## 7. 取消传播

### 7.1 CancellationToken 传播链

```
用户调用 token.cancel()
    │
    ├─ PregelLoop.tick() 检查 → 返回 Err(Cancelled)
    │
    ├─ execute_superstep() 内部：
    │   └─ 每个 task 的 tokio::select! {
    │       biased;
    │       _ = token.cancelled() => Err(Cancelled)  ← 优先响应
    │       result = node.call(...) => result
    │   }
    │
    ├─ 子图执行：
    │   └─ 子图的 PregelLoop 继承父图的 CancellationToken
    │       └─ 子图内的所有 task 同样通过 select! 响应
    │
    └─ JoinSet.shutdown()：等待所有 task 完成清理
```

### 7.2 实现要点

```rust
// 创建子 token（子图使用）
let child_token = cancellation_token.child_token();

// 超时取消
let timeout_token = cancellation_token.clone();
tokio::spawn(async move {
    tokio::time::sleep(config.budget.max_duration.unwrap()).await;
    timeout_token.cancel();
});

// 资源清理保证
// tokio::select! 在取消分支触发时，另一个分支的 Future 被 drop
// Rust 的 Drop 保证资源释放（文件句柄、网络连接等）
```

### 7.3 与 LangGraph 的差异

| 维度 | LangGraph | Juncture |
|------|-----------|----------|
| 取消机制 | asyncio.Task.cancel() + CancelledError | CancellationToken + select! |
| 传播可靠性 | CancelledError 可被捕获忽略 | token 状态不可逆，select! 保证响应 |
| 子任务取消 | 需要手动传播 | child_token 自动传播 |
| 资源清理 | 依赖 finally/context manager | Rust Drop 保证 |

---

## 8. 预算控制

### 8.1 数据结构

```rust
// juncture-core/src/pregel/budget.rs

#[derive(Clone, Debug, Default)]
pub struct BudgetConfig {
    /// 最大 token 消耗量
    pub max_tokens: Option<u64>,
    /// 最大费用（美元）
    pub max_cost_usd: Option<f64>,
    /// 最大执行时间
    pub max_duration: Option<Duration>,
    /// 最大 superstep 数（与 recursion_limit 独立）
    pub max_steps: Option<usize>,
    /// 超限时的处理策略
    pub on_exceeded: BudgetExceededAction,
}

#[derive(Clone, Debug, Default)]
pub enum BudgetExceededAction {
    /// 直接终止，返回当前 state
    #[default]
    Terminate,
    /// 发 HITL interrupt，等待人工确认
    Interrupt,
    /// 自定义处理
    Custom(Arc<dyn Fn(BudgetUsage) -> BudgetExceededAction + Send + Sync>),
}

/// 预算使用量追踪器
pub struct BudgetTracker {
    pub tokens_used: AtomicU64,
    pub cost_usd_micros: AtomicU64,  // micros-USD 精度，避免 atomic_float 依赖
    /// 费用以微秒级 USD 精度存储（1 USD = 1,000,000 micros-USD）
    /// report_usage() 将 API 调用费用转换为 micros-USD 并原子累加
    pub start_time: Instant,
    pub steps_completed: AtomicUsize,
    config: BudgetConfig,
}

// > **实现备注 (D-03-5)**: 实际实现中 `BudgetTracker` 使用 `AtomicU64` 配合微秒级 USD 缩放
// > （`cost_micros: AtomicU64`）替代 `AtomicF64`。这避免了引入 `atomic_float` 外部依赖。
// > 费用值在内部以 micros-USD 精度存储和计算，仅在对外报告时转换为 `f64`。

#[derive(Clone, Debug)]
pub struct BudgetUsage {
    pub tokens_used: u64,
    pub cost_usd: f64,
    pub duration: Duration,
    pub steps_completed: usize,
}

impl BudgetTracker {
    /// LLM 层调用此方法上报 token 消耗
    pub fn report_usage(&self, usage: &TokenUsage, model_pricing: &dyn ModelPricing) {
        self.tokens_used.fetch_add(
            usage.total_tokens as u64,
            Ordering::Relaxed,
        );
        let cost = usage.input_tokens as f64 * model_pricing.input_price_per_mtok() / 1_000_000.0
            + usage.output_tokens as f64 * model_pricing.output_price_per_mtok() / 1_000_000.0;
        self.cost_usd.fetch_add(cost, Ordering::Relaxed);
    }

    /// 每个 superstep 结束后检查
    pub fn check(&self) -> Option<BudgetExceededReason> {
        let usage = self.current_usage();
        
        if let Some(max) = self.config.max_tokens {
            if usage.tokens_used > max {
                return Some(BudgetExceededReason::Tokens { used: usage.tokens_used, limit: max });
            }
        }
        if let Some(max) = self.config.max_cost_usd {
            if usage.cost_usd > max {
                return Some(BudgetExceededReason::Cost { used: usage.cost_usd, limit: max });
            }
        }
        if let Some(max) = self.config.max_duration {
            if usage.duration > max {
                return Some(BudgetExceededReason::Duration { elapsed: usage.duration, limit: max });
            }
        }
        if let Some(max) = self.config.max_steps {
            if usage.steps_completed > max {
                return Some(BudgetExceededReason::Steps { completed: usage.steps_completed, limit: max });
            }
        }
        None
    }
}
```

### 8.2 集成点

```
LLM Provider (ChatAnthropic/ChatOpenAI)
    │
    ├─ 每次 API 调用完成后
    │   └─ budget_tracker.report_usage(&response.usage, &self.pricing)
    │
PregelLoop.after_tick()
    │
    ├─ budget_tracker.check()
    │   ├─ None → 继续
    │   └─ Some(reason) → 根据 on_exceeded 处理
    │       ├─ Terminate → status = BudgetExceeded, break
    │       ├─ Interrupt → 保存 checkpoint, 返回 Interrupted
    │       └─ Custom(f) → 调用用户函数决定
```

---

## 9. 递归限制与终止条件

### 9.1 终止条件（按优先级）

| 条件 | 检查时机 | 结果 |
|------|----------|------|
| `cancellation_token.is_cancelled()` | tick() 开头 | `Err(Cancelled)` |
| `step > stop` (recursion_limit) | tick() 开头 | `status = OutOfSteps` |
| `pending_tasks` 为空 | tick() 中 | `status = Done` |
| 所有路径到达 END | compute_next_tasks 返回空 | `status = Done` |
| `interrupt_before` 触发 | tick() 中 | `Err(Interrupted)` |
| `interrupt_after` 触发 | after_tick() 中 | `Err(Interrupted)` |
| Budget 超限 | after_tick() 中 | 根据策略处理 |
| 节点执行失败 | execute_superstep() 中 | `Err(NodeFailed)` |

### 9.2 recursion_limit 默认值

```rust
impl Default for RunnableConfig {
    fn default() -> Self {
        Self {
            recursion_limit: 25,  // 与 LangGraph 一致
            ..
        }
    }
}
```

---

## 10. 错误处理

### 10.1 JunctureError 层次

```rust
#[derive(Debug, thiserror::Error)]
pub enum JunctureError {
    // ── 拓扑错误（编译阶段）──
    #[error(transparent)]
    Topology(#[from] TopologyError),

    // ── 执行错误 ──
    #[error("节点 '{node}' 执行失败: {source}")]
    NodeFailed {
        node: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("超出递归限制 ({limit} supersteps)")]
    RecursionLimitExceeded { limit: usize },

    #[error("执行被取消")]
    Cancelled,

    #[error("图执行被中断")]
    Interrupted,

    #[error("预算超限: {reason}")]
    BudgetExceeded { reason: BudgetExceededReason },

    #[error("Replace reducer 冲突: 字段 {field_index} 被多个节点同时写入: {writers:?}")]
    MultipleWriters { field_index: usize, writers: Vec<String> },

    #[error("tokio task panic: {0}")]
    TaskPanicked(String),

    // ── Checkpoint 错误 ──
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),

    // ── 序列化错误 ──
    #[error("序列化失败: {0}")]
    Serialize(#[from] serde_json::Error),

    // ── LLM 错误 ──
    #[error(transparent)]
    Llm(#[from] LlmError),

    // ── 节点未找到 ──
    #[error("节点 '{0}' 不存在")]
    NodeNotFound(String),

    // ── 节点超时 ──
    #[error(transparent)]
    NodeTimeout(#[from] NodeTimeoutError),

    // ── 图被优雅停止 ──
    #[error("图执行被优雅停止: {reason}")]
    GraphDrained { reason: String },

    #[error("读取空的 Channel: {channel}")]
    EmptyChannel { channel: String },

    #[error("输入状态为空")]
    EmptyInput,

    #[error("任务 '{task_id}' 未找到")]
    TaskNotFound { task_id: String },
}
```

### 10.1b GraphBubbleUp 异常类型


> 参考: `langgraph/errors.py:85` — `GraphBubbleUp` 基类

Pregel 循环中有一类特殊的"冒泡"信号，它们不是错误而是控制流信号，需要从子图向父图传播。Pregel 循环捕获这些信号并干净退出，不保存错误状态：

```rust
/// 需要从执行循环中冒泡的控制信号
///
/// 与 JunctureError 不同，BubbleUp 不是错误——它是正常的控制流。
/// Pregel 循环捕获 BubbleUp 变体并干净退出（保存 checkpoint 但不保存错误状态）。
///
/// 参考: `langgraph/errors.py:85-128` — GraphBubbleUp 类层次
#[derive(Debug)]
pub enum BubbleUp<S: State> {
    /// 节点触发的 interrupt（GraphInterrupt）
    /// Pregel 循环保存 checkpoint 并退出，等待 resume
    Interrupt(GraphInterrupt),

    /// 图被优雅排空（GraphDrained）
    /// 所有活跃路径完成后保存 checkpoint 并退出
    Drained(GraphDrained),

    /// 子图向父图发送 Command（ParentCommand）
    /// 子图 Pregel 循环退出，将 Command 冒泡到父图
    ParentCommand(Command<S>),
}

/// 中断信息
#[derive(Debug, Clone)]
pub struct GraphInterrupt {
    /// 中断信号列表
    pub interrupts: Vec<InterruptSignal>,
    /// 中断发生的 step
    pub step: usize,
}

/// 排空信息
#[derive(Debug, Clone)]
pub struct GraphDrained {
    /// 排空原因
    pub reason: String,
}

**BubbleUp 处理实现**: Pregel 循环完整处理所有 BubbleUp 变体。
> - Interrupt: 保存 checkpoint，设置 LoopStatus::InterruptBefore
> - Drained: 保存 checkpoint，设置 LoopStatus::Drained
> - ParentCommand: 冒泡 Command 到父图，子图退出
> 所有种类的 BubbleUp 都不保存错误状态到 checkpoint（正常控制流）。

```rust
// PregelLoop::tick() 或 execute_superstep() 中
match result {
    Err(JunctureError::BubbleUp(bubble)) => {
        match bubble {
            BubbleUp::Interrupt(gi) => {
                self.save_checkpoint(CheckpointSource::Interrupt).await?;
                self.status = LoopStatus::InterruptBefore;
                return Ok(false);
            }
            BubbleUp::Drained(gd) => {
                self.save_checkpoint(CheckpointSource::Loop).await?;
                self.status = LoopStatus::Drained;
                return Ok(false);
            }
            BubbleUp::ParentCommand(cmd) => {
                // 子图冒泡的 Command，交由父图处理
                return Err(JunctureError::ParentCommand(cmd));
            }
        }
    }
    // ... 其他错误处理
}
```

### 10.2 部分失败处理

当并行执行的多个节点中有一个失败时：

```
Task A: 成功 (writes 已通过 put_writes 持久化)
Task B: 失败
Task C: 正在执行

处理流程：
1. Task B 失败 → 触发 cancellation_token.cancel()
2. Task C 收到取消信号 → 通过 select! 退出
3. JoinSet.shutdown() 等待所有 task 完成
4. 返回 Err(NodeFailed { node: "B", source: ... })

崩溃恢复：
- Task A 的 writes 已持久化（put_writes）
- 下次 resume 时，Task A 不需要重新执行
- 只需重新执行 Task B 和 Task C
```

### 10.3 错误传播策略

| 场景 | 行为 |
|------|------|
| 单节点失败 | 取消同 superstep 其他节点，返回错误 |
| 多节点同时失败 | 返回第一个完成的错误 |
| 子图内节点失败 | 错误冒泡到父图 |
| Checkpoint 写入失败 | 返回 CheckpointError（不影响 state） |
| LLM 调用失败 | 由节点内部处理或冒泡为 NodeFailed |

### 10.4 ErrorCode 枚举

> 参考: `langgraph/errors.py` — 各种错误类型

Juncture 定义标准的错误代码枚举，用于错误传播和错误处理器匹配：

```rust
/// 标准错误代码
///
/// 用于 RetryPolicy 的 retry_on 条件和 error_handler 的错误分类。
/// 参考 LangGraph errors.py 中的各种异常类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// 超出递归限制（recursion_limit）
    GraphRecursionLimit,

    /// 不合法的并发更新（同一 Replace 字段被多个节点写入）
    InvalidConcurrentUpdate,

    /// 节点返回值类型不合法（如 Command 格式错误）
    InvalidNodeReturnValue,

    /// 不允许多个子图（同一节点注册多个子图）
    MultipleSubgraphs,

    /// Chat history 格式不合法（消息顺序、类型错误等）
    InvalidChatHistory,
}

impl JunctureError {
    /// 从错误中提取标准错误代码
    pub fn error_code(&self) -> Option<ErrorCode> {
        match self {
            JunctureError::RecursionLimitExceeded { .. } => Some(ErrorCode::GraphRecursionLimit),
            JunctureError::MultipleWriters { .. } => Some(ErrorCode::InvalidConcurrentUpdate),
            _ => None,
        }
    }

    /// Microsoft 风格错误类型检查辅助方法
    /// 参考: https://docs.microsoft.com/en-us/dotnet/standard/exceptions/best-practices-for-exceptions

    /// 检查是否为递归限制错误
    pub fn is_graph_recursion_limit(&self) -> bool {
        matches!(self, JunctureError::RecursionLimitExceeded { .. })
    }

    /// 检查是否为并发更新冲突错误
    pub fn is_invalid_concurrent_update(&self) -> bool {
        matches!(self, JunctureError::MultipleWriters { .. })
    }

    /// 检查是否为节点执行失败错误
    pub fn is_node_failed(&self) -> bool {
        matches!(self, JunctureError::NodeFailed { .. })
    }

    /// 检查是否为取消错误
    pub fn is_cancelled(&self) -> bool {
        matches!(self, JunctureError::Cancelled)
    }

    /// 检查是否为中断错误
    pub fn is_interrupted(&self) -> bool {
        matches!(self, JunctureError::Interrupted)
    }

    /// 检查是否为预算超限错误
    pub fn is_budget_exceeded(&self) -> bool {
        matches!(self, JunctureError::BudgetExceeded { .. })
    }

    /// 检查是否为序列化错误
    pub fn is_serialize(&self) -> bool {
        matches!(self, JunctureError::Serialize(_))
    }

    /// 检查是否为 Checkpoint 错误
    pub fn is_checkpoint(&self) -> bool {
        matches!(self, JunctureError::Checkpoint(_))
    }

    /// 检查是否为空 Channel 错误
    pub fn is_empty_channel(&self) -> bool {
        matches!(self, JunctureError::EmptyChannel { .. })
    }

    /// 检查是否为空输入错误
    pub fn is_empty_input(&self) -> bool {
        matches!(self, JunctureError::EmptyInput)
    }

    /// 检查是否为任务未找到错误
    pub fn is_task_not_found(&self) -> bool {
        matches!(self, JunctureError::TaskNotFound { .. })
    }
}
```

---

## 11. 节点弹性策略

### 11.1 重试策略 (RetryPolicy)

> 参考: `langgraph/types.py:406` — RetryPolicy dataclass
> 参考: `langgraph/pregel/_retry.py` — retry execution logic

```rust
/// 节点级重试策略
///
/// 应用于单个节点，在节点执行失败时自动重试。
/// 通过 `add_node()` 的 `retry_policy` 参数配置。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 初始重试间隔
    pub initial_interval: Duration,
    /// 退避倍数（每次重试间隔乘以此系数）
    pub backoff_factor: f64,
    /// 最大重试间隔
    pub max_interval: Duration,
    /// 最大重试次数（含首次执行）
    pub max_attempts: usize,
    /// 是否添加随机抖动（防止雷群效应）
    pub jitter: bool,
    /// 自定义重试条件：返回 true 时重试
    pub retry_on: Option<Arc<dyn Fn(&JunctureError) -> bool + Send + Sync>>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_millis(500),
            backoff_factor: 2.0,
            max_interval: Duration::from_secs(10),
            max_attempts: 3,
            jitter: true,
            retry_on: None,
        }
    }
}

/// 重试执行逻辑
async fn execute_with_retry<S: State>(
    node: &dyn Node<S>,
    state: S,
    config: &RunnableConfig,
    policy: &RetryPolicy,
) -> Result<Command<S>, JunctureError> {
    let mut last_error = None;
    let mut delay = policy.initial_interval;

    for attempt in 0..policy.max_attempts {
        match node.call(state.clone(), config).await {
            Ok(cmd) => return Ok(cmd),
            Err(e) => {
                // 检查是否应该重试
                let should_retry = match &policy.retry_on {
                    Some(pred) => pred(&e),
                    None => !matches!(e, JunctureError::Cancelled | JunctureError::Interrupted),
                };

                if !should_retry || attempt + 1 >= policy.max_attempts {
                    return Err(e);
                }

                last_error = Some(e);

                // 计算延迟（含抖动）
                let actual_delay = if policy.jitter {
                    let jitter_range = delay.mul_f64(0.1);
                    delay + jitter_range
                } else {
                    delay
                };
                tokio::time::sleep(actual_delay).await;

                // 指数退避
                delay = (delay.mul_f64(policy.backoff_factor)).min(policy.max_interval);
            }
        }
    }
    Err(last_error.unwrap())
}

**重试策略集成**: RetryPolicy 完全集成到节点执行流程。
> `execute_with_retry()` 实现指数退避（exponential backoff）和抖动（jitter）。
> 退避间隔计算：delay = min(delay * backoff_factor, max_interval)。
> 抖动范围：±10% 的延迟，防止雷群效应（thundering herd）。


```rust
impl<S: State> StateGraph<S> {
    /// 添加带重试策略的节点
    ///
    /// 等价于 LangGraph 的 `add_node(name, fn, retry=RetryPolicy(...))`
    pub fn add_node_with_retry(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        retry_policy: RetryPolicy,
    ) -> &mut Self {
        let node_name = name.into();
        let inner = node.into_node(&node_name);
        let wrapped = Arc::new(RetryingNode {
            inner,
            policy: retry_policy,
        });
        self.nodes.insert(node_name, wrapped);
        self
    }
}

/// RetryingNode 包装器：在 Node trait 层面实现重试
struct RetryingNode<S: State> {
    inner: Arc<dyn Node<S>>,
    policy: RetryPolicy,
}

#[async_trait]
impl<S: State> Node<S> for RetryingNode<S> {
    fn call(&self, state: S, config: &RunnableConfig) -> BoxFuture<'_, Result<Command<S>, JunctureError>> {
        let policy = self.policy.clone();
        let inner = self.inner.clone();
        Box::pin(async move {
            execute_with_retry(&*inner, state, config, &policy).await
        })
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}
```

### 11.2 心跳机制 (Heartbeat)

> 参考: 实现备注 (C-03-005) — 空闲超时和心跳信号

心跳机制用于长运行节点发送活跃信号，防止空闲超时误检。通过 `Heartbeat` 和 `HeartbeatWatcher` 类型，节点可以定期发送心跳信号，引擎的空闲超时看门狗每次收到 `ping()` 时重置计时器。

```rust
/// 心跳发送器
///
/// 长运行节点应定期调用 `ping()` 发送活跃信号，防止空闲超时误检。
/// 心跳携带一个无界通道发送端，每次调用 `ping()` 时向引擎的空闲超时看门狗发送信号。
///
/// 创建配对的心跳发送器和监视器：
///
/// ```ignore
/// use juncture_core::Heartbeat;
/// use std::time::Duration;
///
/// let (heartbeat, mut watcher) = Heartbeat::new_pair();
/// heartbeat.ping().unwrap();
/// assert!(watcher.is_alive(Duration::from_secs(10)));
/// ```
pub struct Heartbeat {
    tx: tokio::sync::mpsc::UnboundedSender<()>,
    _rx: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,
}

impl Clone for Heartbeat {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            _rx: None, // 只有原始 Heartbeat 保留接收端
        }
    }
}

impl Heartbeat {
    /// 从无界通道发送端创建心跳
    pub const fn new(tx: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
        Self { tx, _rx: None }
    }

    /// 创建配对的心跳发送器和监视器
    ///
    /// 返回通过无界通道连接的 `(Heartbeat, HeartbeatWatcher)` 对。
    /// 监视器可以通过检查心跳是否在空闲超时内到达来检测停滞。
    pub fn new_pair() -> (Self, HeartbeatWatcher) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = HeartbeatWatcher::new(rx);
        (Self { tx, _rx: None }, watcher)
    }

    /// 发送心跳信号
    ///
    /// # Errors
    ///
    /// 如果通道已关闭（监视器被丢弃），返回错误。
    pub fn ping(&self) -> Result<(), JunctureError> {
        self.tx.send(()).map_err(|_| JunctureError::execution("heartbeat channel closed".into()))
    }
}

/// 心跳监视器
///
/// 监视心跳信号，通过检查最近一次心跳到达时间来检测节点是否停滞。
pub struct HeartbeatWatcher {
    rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    last_ping: std::time::Instant,
}

impl HeartbeatWatcher {
    /// 从无界通道接收端创建监视器
    pub fn new(rx: tokio::sync::mpsc::UnboundedReceiver<()>) -> Self {
        Self {
            rx,
            last_ping: std::time::Instant::now(),
        }
    }

    /// 检查节点是否在给定的空闲超时内存活
    ///
    /// 如果最近一次心跳在 `idle_timeout` 内到达，返回 `true`。
    pub fn is_alive(&mut self, idle_timeout: Duration) -> bool {
        // 尝试接收所有待处理的心跳
        while let Ok(()) = self.rx.try_recv() {
            self.last_ping = std::time::Instant::now();
        }
        self.last_ping.elapsed() < idle_timeout
    }
}
```

**集成到 TimeoutPolicy**: `idle_timeout` 通过 Heartbeat 机制实现。
> 每个 task 创建 Heartbeat Sender 对，节点定期调用 `heartbeat.ping()` 发送心跳。
> tokio::select! 并发检查：超时分支、心跳分支、执行分支。
> 心跳接收时刷新 idle_timer，实现空闲检测而非总运行时间限制。

**Runtime 集成**: `Runtime<C>` 包含 `heartbeat: Heartbeat` 字段，节点通过 `runtime.heartbeat().ping()` 发送心跳信号。

> 参考: `langgraph/types.py:439` — TimeoutPolicy
> 参考: `langgraph/errors.py:167` — NodeTimeoutError

```rust
/// 节点级超时策略
///
/// 防止 LLM 调用或工具执行无限阻塞。
/// 通过 `add_node()` 的 `timeout_policy` 参数配置。
///
/// > **实现备注 (D-03-16)**: 实际实现支持分层超时机制：
/// > cancellation → timeout → idle → retry → interrupt → node.call。
/// > 通过心跳信号检测空闲状态，实现更细粒度的超时控制（C-03-005）。
#[derive(Debug, Clone)]
pub struct TimeoutPolicy {
    /// 单次执行的最大运行时间
    pub run_timeout: Duration,
    /// 空闲超时：如果在此时间内没有进度信号，视为超时
    pub idle_timeout: Option<Duration>,
    /// 收到进度信号时刷新 idle_timeout（通过 heartbeat）
    pub refresh_on: Option<Arc<dyn Fn(&StreamEvent<()>) -> bool + Send + Sync>>,
}

**空闲超时实现**: idle_timeout 通过 Heartbeat 机制实现。
> 每个 task 创建 Heartbeat Sender 对，定期发送心跳信号。
> tokio::select! 并发检查：超时分支、心跳分支、执行分支。
> 心跳接收时刷新 idle_timer，实现空闲检测而非总运行时间限制。
    fn default() -> Self {
        Self {
            run_timeout: Duration::from_secs(300), // 5 分钟
            idle_timeout: None,
            refresh_on: None,
        }
    }
}

/// 节点超时错误
#[derive(Debug, thiserror::Error)]
pub enum NodeTimeoutError {
    #[error("节点 '{node}' 总超时（{timeout_ms}ms）")]
    Timeout { node: String, timeout_ms: u64 },

    #[error("节点 '{node}' 执行超时（{timeout_ms}ms）")]
    RunTimeout { node: String, timeout_ms: u64 },

    #[error("节点 '{node}' 空闲超时（{timeout_ms}ms）")]
    IdleTimeout { node: String, timeout_ms: u64 },

    #[error("节点 '{node}' 截止时间已过（{deadline_ms}ms）")]
    DeadlineExceeded { node: String, deadline_ms: u64 },
}

// > **实现备注 (D-03-7)**: 实际实现中 `NodeTimeoutError` 有 4 个变体而非 2 个：
// > 额外包含 `Timeout { node: String, timeout_ms: u64 }` 和 `DeadlineExceeded { node: String, deadline_ms: u64 }`。
// > 同时使用 `u64` 毫秒表示而非 `Duration`，简化序列化和原子操作。

/// 超时执行逻辑
async fn execute_with_timeout<S: State>(
    node: &dyn Node<S>,
    state: S,
    config: &RunnableConfig,
    policy: &TimeoutPolicy,
) -> Result<Command<S>, JunctureError> {
    let result = tokio::time::timeout(policy.run_timeout, node.call(state, config)).await;

    match result {
        Ok(Ok(cmd)) => Ok(cmd),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(JunctureError::NodeTimeout(NodeTimeoutError::RunTimeout {
            node: node.name().to_string(),
            timeout: policy.run_timeout,
        })),
    }
}
```

### 11.3 持久化模式 (Durability)

> 参考: `langgraph/types.py:87-93` — Durability = Literal["sync", "async", "exit"]

```rust
/// Checkpoint 持久化模式
///
/// 控制何时将 checkpoint 写入持久化后端。
/// 不同模式在性能和持久性之间权衡。
#[derive(Debug, Clone, Default)]
pub enum Durability {
    /// 同步持久化：每个 superstep 结束后立即写入
    /// 最安全，但每次 superstep 增加一次 I/O 延迟
    #[default]
    Sync,

    /// 异步持久化：后台任务写入 checkpoint
    /// 减少延迟，但如果进程崩溃可能丢失最近的 checkpoint
    Async,

    /// 仅在图退出时持久化
    /// 最高性能，中间状态仅在内存中，适合可重放的工作流
    Exit,
}
```

| 模式 | 写入时机 | 崩溃恢复 | 性能影响 | 适用场景 |
|------|---------|----------|---------|---------|
| `Sync` | 每 superstep 后 | 完整恢复 | 每步 +1 I/O | 关键业务工作流 |
| `Async` | 后台异步 | 可能丢失最后一步 | 最小延迟 | 高吞吐场景 |
| `Exit` | 图退出时 | 仅最终状态 | 无中间 I/O | 可重放/批量任务 |

### 11.4 优雅停止 (RunControl)

> 参考: `langgraph/runtime.py:79` — RunControl

```rust
/// 运行控制：允许外部请求优雅停止
///
/// 典型场景：收到 SIGTERM 信号时，在下一个 superstep 边界
/// 安全停止执行，保存 checkpoint 以便后续恢复。
pub struct RunControl {
    /// 停止请求信号
    drain_requested: Arc<AtomicBool>,
    /// 停止原因（用于日志和调试）
    drain_reason: Arc<Mutex<Option<String>>>,
}

impl RunControl {
    pub fn new() -> Self {
        Self {
            drain_requested: Arc::new(AtomicBool::new(false)),
            drain_reason: Arc::new(Mutex::new(None)),
        }
    }

    /// 请求优雅停止
    /// 在当前 superstep 完成后生效，checkpoint 会被保存
    pub fn request_drain(&self, reason: &str) {
        self.drain_requested.store(true, Ordering::Relaxed);
        if let Ok(mut r) = self.drain_reason.lock() {
            *r = Some(reason.to_string());
        }
    }

    pub fn is_drain_requested(&self) -> bool {
        self.drain_requested.load(Ordering::Relaxed)
    }
}
```

在 `PregelLoop::tick()` 中检查 drain 请求：

```rust
// tick() 中，在 prepare_next_tasks 之前检查
if self.run_control.is_drain_requested() {
    // 保存当前 checkpoint
    self.save_checkpoint(CheckpointSource::Loop).await?;
    self.status = LoopStatus::Drained;
    return Ok(false); // 停止循环
}
```

**优雅停止实现**: RunControl 提供完整的优雅停止机制。
> `request_drain()` 设置 drain_requested 标志，`is_drain_requested()` 检查标志。
> 下一次 tick() 循环检测到请求后，保存 checkpoint 并设置 Drained 状态。
> 所有正在执行的 task 完成后（JoinSet.join_all），循环干净退出。

### 11.5 节点级错误处理器

> 参考: `langgraph/pregel/_runner.py:171-173`
> 参考: `langgraph/pregel/_algo.py:1110-1270`

当节点配置了 `error_handler` 时，执行流程变为：

```
节点执行失败
  │
  ├─ 节点有 error_handler?
  │   ├─ Yes → 调用 error_handler(NodeError { node, error })
  │   │         → 返回恢复用 Command（写入保留字段）
  │   │         → 继续执行后续 superstep
  │   └─ No → 取消其他任务，返回错误（现有行为）
  │
  └─ 保留写入字段（reserved write keys）:
      __error__ = serde_json::Value  // 错误信息
      __error_source_node__ = String // 失败节点名
```

**两阶段错误恢复实现**: 错误恢复系统采用两阶段调度：
> 1) 扫描 ERROR_SOURCE_NODE 标记失败节点，2) 为失败节点创建恢复任务。
> TaskOutput 包含 error 字段用于传递错误信息，error_handler_map 维护
> 节点到错误处理器的映射关系。

```rust
/// 保留写入键（与 LangGraph 一致）
pub mod reserved_keys {
    pub const INPUT: &str = "__input__";
    pub const INTERRUPT: &str = "__interrupt__";
    pub const RESUME: &str = "__resume__";
    pub const ERROR: &str = "__error__";
    pub const ERROR_SOURCE_NODE: &str = "__error_source_node__";
}
```

在 tick 循环中，`after_tick()` 阶段需要调用：
- `_resume_error_handlers_if_applicable()` — 检查是否有待处理的错误处理器
- `schedule_error_handler()` — 为失败的节点动态创建恢复任务

#### 两阶段错误恢复算法


```
Phase 1: 扫描与标记（在 after_tick 开始时）
  │
  ├─ 1. 扫描 checkpoint 的 pending_writes
  │     查找 ERROR_SOURCE_NODE 标记键
  │
  ├─ 2. 对每个失败的 task（有 ERROR_SOURCE_NODE 标记的）：
  │     如果该节点配置了 error_handler_node：
  │     ├─ 写入 (ERROR, error_value) 到该 task 的 writes
  │     │  （标记该 task 为"completed with error"）
  │     └─ 写入 (ERROR_SOURCE_NODE, node_name) 到该 task 的 writes

Phase 2: 创建恢复任务
  │
  ├─ 3. 为每个有 error_handler 的失败节点创建新 task：
  │     PendingTask {
  │         node_name: error_handler_node_name,
  │         trigger: Pull,
  │         state_override: None,
  │         // writes 为空——runner 会执行它
  │     }
  │
  └─ 4. error_handler 节点接收 NodeError 并返回 Command
        Command 可以包含恢复性更新和路由指令
```

```rust
/// 错误恢复调度器
fn schedule_error_handlers<S: State>(
    pending_writes: &[PendingWrite],
    nodes: &IndexMap<String, Arc<dyn Node<S>>>,
) -> Vec<PendingTask<S>> {
    let mut recovery_tasks = Vec::new();

    // Phase 1: 扫描 ERROR_SOURCE_NODE 标记
    let failed_nodes: Vec<String> = pending_writes.iter()
        .filter(|w| w.channel == reserved_keys::ERROR_SOURCE_NODE)
        .filter_map(|w| w.value.as_str().map(|s| s.to_string()))
        .collect();

    // Phase 2: 为每个失败节点创建 error_handler task
    for node_name in &failed_nodes {
        if let Some(handler_name) = get_error_handler_node(node_name, nodes) {
            // 标记原 task 为 "completed with error"
            // 创建 error_handler task
            recovery_tasks.push(PendingTask {
                id: uuid::Uuid::new_v4().to_string(),
                node_name: handler_name,
                trigger: TaskTrigger::Pull,
                state_override: None,
            });
        }
    }

    recovery_tasks
}
```

### 11.6 PregelProtocol 接口抽象

```rust
/// Pregel 协议 trait
///
/// 提供图执行的统一接口，支持本地和远程图。
pub trait PregelProtocol<S: State>: Send + Sync + 'static {
    /// 同步执行图
    fn invoke(&self, input: S, config: &RunnableConfig) -> BoxFuture<'_, Result<S, JunctureError>>;

    /// 流式执行图
    fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> BoxFuture<'_, Result<Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>, JunctureError>>;

    /// 获取当前状态
    fn get_state(&self, config: &RunnableConfig) -> BoxFuture<'_, Result<Option<StateSnapshot<S>>, JunctureError>>;

    /// 更新状态
    fn update_state(
        &self,
        config: &RunnableConfig,
        update: S::Update,
        as_node: Option<&str>,
    ) -> BoxFuture<'_, Result<RunnableConfig, JunctureError>>;
}
```

---

## 12. 与 LangGraph 的关键差异


| 维度 | LangGraph Python | Juncture Rust | 理由 |
|------|-----------------|---------------|------|
| 并发模型 | `concurrent.futures` 线程池 | `tokio::spawn` + `JoinSet` | 真异步，work-stealing，更高效 |
| 取消机制 | `asyncio.Task.cancel()` | `CancellationToken` + `select!` | 更可靠，不可被忽略 |
| Channel 系统 | 动态 `dict[str, BaseChannel]` | 静态 struct + proc-macro | 编译期安全，零开销 |
| 调度模型 | 纯 reactive（channel 版本驱动） | 混合（边驱动 + 版本辅助） | 更简单，同等语义 |
| put_writes 时序 | 每个 task 完成后 | 每个 task 完成后 | 一致 |
| Merge 顺序 | 按 task 完成顺序 | 按 path-based sorting（PULL 按名字、PUSH 按索引） | 确定性且与 LangGraph 一致 |
| 错误处理 | 异常 + error handler 节点 | Result + 取消传播 | Rust 惯用法 |
| 预算控制 | 无内置 | 内置 BudgetTracker | Juncture 差异化 |
| 图导出 | `get_graph().draw_mermaid_png()` | `to_mermaid()` / `to_json()` | 一致 |

---

## 13. SyncAsyncFuture 与任务结果处理


> 参考: `langgraph/pregel/functional.py` — SyncAsyncFuture 类

在函数式 API（@task/@entrypoint）中，`SyncAsyncFuture` 表示一个可能同步或异步返回的任务结果。

### 13.1 设计动机

某些任务可能同步返回（缓存命中），也可能异步计算（缓存未命中）。`SyncAsyncFuture` 统一两种情况，允许调用者使用 `.await` 获取结果。

### 13.2 Rust 适配

```rust
/// 可能同步或异步的任务结果
///
/// 在 @task 装饰的函数中，返回值包装为 SyncAsyncFuture。
/// 调用者统一使用 `.await` 获取结果，无论内部是同步还是异步。
pub enum SyncAsyncFuture<T> {
    /// 同步结果（如缓存命中）
    Ready(Option<T>),
    /// 异步结果（如需要计算）
    Future(BoxFuture<'static, T>),
}

impl<T> SyncAsyncFuture<T> {
    /// 阻塞获取结果（如果还未就绪，则等待）
    ///
    /// 返回 `Result<T, JunctureError>` 以支持错误传播，遵循 Rust 错误处理惯用法。
    /// `Ready(None)` 情况返回错误而非 panic，因为这是可恢复的状态（例如任务被取消）。
    pub async fn result(self) -> Result<T, JunctureError> {
        match self {
            SyncAsyncFuture::Ready(Some(value)) => Ok(value),
            SyncAsyncFuture::Ready(None) => Err(JunctureError::Internal("Task result not available".into())),
            SyncAsyncFuture::Future(fut) => fut.await,
        }
    }

    /// 非阻塞检查结果是否就绪
    pub fn is_ready(&self) -> bool {
        matches!(self, SyncAsyncFuture::Ready(_))
    }
}

// > **实现备注 (D-03-8)**: 实际实现中 `result()` 返回 `Result<T, JunctureError>` 而非直接返回 `T`，
// > `Ready(None)` 情况返回 `Err(JunctureError:: ...)` 而非 panic。
// > 这遵循 Rust 的错误处理惯用法，避免不可恢复的 panic。

// 从同步值创建
impl<T> From<T> for SyncAsyncFuture<T> {
    fn from(value: T) -> Self {
        SyncAsyncFuture::Ready(Some(value))
    }
}

// 从 Future 创建
impl<T> From<BoxFuture<'static, T>> for SyncAsyncFuture<T> {
    fn from(fut: BoxFuture<'static, T>) -> Self {
        SyncAsyncFuture::Future(fut)
    }
}
```

### 13.3 使用示例

```rust
#[task(cache = CachePolicy::ttl(Duration::from_secs(300)))]
async fn expensive_computation(input: String) -> Result<Analysis> {
    llm.analyze(&input).await
}

// 在 entrypoint 中使用
#[entrypoint]
async fn workflow(input: Input, runtime: &Runtime<()>) -> Result<Output> {
    // 获取任务 future（可能同步或异步）
    let future: SyncAsyncFuture<Analysis> = expensive_computation(input.data);

    // 统一使用 .await 获取结果
    let analysis = future.result().await?;

    Ok(Output { analysis })
}
```

---

## 14. Previous Result Injection


> 参考: `langgraph/func/__init__.py` — @entrypoint 的 `previous` 参数

在函数式 API 中，entrypoint 可以访问上一次执行的返回值，实现累积模式。

### 14.1 设计

```rust
/// Entrypoint 函数可以访问上一次执行的返回值
///
/// 通过 Runtime.previous 字段注入，类型为 `Option<serde_json::Value>`。
/// 允许工作流基于上次结果进行增量处理。
///
/// 使用场景：
/// - 增量数据处理（只处理新数据）
/// - 累积统计（基于历史结果）
/// - 迭代优化（在上次结果基础上改进）
#[entrypoint(checkpointer = MemorySaver::new())]
async fn incremental_workflow(
    input: Input,
    runtime: &Runtime<()>,
) -> Result<Output> {
    // 获取上一次的返回值
    let previous: Option<Output> = runtime.previous
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let mut result = process(input);

    if let Some(prev) = previous {
        // 基于上次结果增量处理
        result = merge_with_previous(prev, result);
    }

    Ok(result)
}
```

### 14.2 注入时机

`previous` 值在以下时机注入到 Runtime：

1. **从 checkpoint 恢复**：加载最新的 checkpoint，读取其 `__return__` 字段
2. **首次执行**：`previous` 为 `None`
3. **手动 update_state**：不更新 `previous`（只有完整执行才会更新）

### 14.3 序列化格式

`previous` 使用 `serde_json::Value` 存储，支持任意可序列化类型：

```rust
// 存储到 checkpoint
checkpoint.data["__return__"] = serde_json::to_value(&output)?;

// 从 checkpoint 恢复
let previous: Option<Output> = checkpoint
    .data
    .get("__return__")
    .and_then(|v| serde_json::from_value(v.clone()).ok());
```

---

