# Subgraph 组合系统

## 概述

Subgraph 允许将复杂图拆分为独立的、可复用的子图。每个子图有自己的 State 类型、节点、边和 checkpoint 命名空间。Juncture 提供两种组合模式：共享状态模式和显式映射模式，均在编译期保证类型安全。

---

## 1. LangGraph 参考架构

### 两种组合模式

**模式 1：编译后子图作为节点（共享状态键）**

```python
# 子图的 state 字段是父图 state 的子集
builder.add_node("review", compiled_subgraph)
```

子图自动读写父图中同名的 state channel。无需手动映射。

**模式 2：包装节点内调用子图（不同 schema）**

```python
def wrapper(state: ParentState):
    result = subgraph.invoke({"bar": state["foo"]})
    return {"foo": result["bar"]}

builder.add_node("review", wrapper)
```

手动在包装函数中做状态转换。

### Checkpoint 命名空间

- 根图：`""`（空字符串）
- 子图：`"node_name:uuid"`
- 嵌套子图：`"outer:uuid|inner:uuid"`

每个子图实例有独立的 checkpoint 历史，不与父图混用。

### 持久化模式

| 模式 | 配置 | 行为 |
|------|------|------|
| Per-invocation | `checkpointer=None`（默认） | 每次调用全新开始；继承父图 checkpointer 用于 interrupt |
| Per-thread | `checkpointer=True` | 同一 thread 上跨调用累积状态 |
| Stateless | `checkpointer=False` | 无 checkpoint，不支持 interrupt |

### 中断传播

子图内的 `interrupt()` 冒泡到顶层图。父图的 checkpoint 包含子图的中断状态。Resume 时信号从父图向下传递到子图。

---

## 2. Juncture 子图设计

### 2.1 模式 1：共享状态（SubState 是 ParentState 的子集）

```rust
use juncture::prelude::*;

#[derive(State, Clone, Serialize, Deserialize)]
struct ParentState {
    #[reducer(append)]
    messages: Vec<Message>,
    draft: String,
    review_result: Option<String>,
    final_output: String,
}

#[derive(State, Clone, Serialize, Deserialize)]
struct ReviewState {
    #[reducer(append)]
    messages: Vec<Message>,
    draft: String,
    review_result: Option<String>,
}

// 构建子图
let review_graph = {
    let mut g = StateGraph::<ReviewState>::new();
    g.add_node("reviewer", review_node);
    g.add_edge(START, "reviewer");
    g.add_edge("reviewer", END);
    g.compile_ephemeral()?
};

// 父图中添加子图节点（共享状态模式）
let mut parent = StateGraph::<ParentState>::new();
parent.add_subgraph_node("review", review_graph);
```

#### 编译期约束

共享状态模式要求 `SubState` 的每个字段在 `ParentState` 中都存在且类型相同。通过 trait bound 实现：

```rust
pub trait StateSubset<Parent: State>: State {
    /// 从父状态提取子状态
    fn extract(parent: &Parent) -> Self;
    /// 将子状态更新映射回父状态更新
    fn map_update(update: Self::Update) -> Parent::Update;
}
```

`#[derive(State)]` 可以生成此 trait 的实现（通过属性标注）：

```rust
#[derive(State, Clone, Serialize, Deserialize)]
#[subset_of(ParentState)]  // proc-macro 验证字段存在性并生成 StateSubset impl
struct ReviewState {
    #[reducer(append)]
    messages: Vec<Message>,
    draft: String,
    review_result: Option<String>,
}
```

如果 `ReviewState` 包含 `ParentState` 中不存在的字段，编译报错。

#### add_subgraph_node 签名

```rust
impl<S: State> StateGraph<S> {
    pub fn add_subgraph_node<Sub: StateSubset<S>>(
        &mut self,
        name: &str,
        subgraph: CompiledGraph<Sub>,
    ) -> &mut Self;
}
```

### 2.2 模式 2：显式映射（不同 State 类型）

```rust
#[derive(State, Clone, Serialize, Deserialize)]
struct ParentState {
    #[reducer(append)]
    messages: Vec<Message>,
    document: String,
    review_verdict: Option<String>,
}

#[derive(State, Clone, Serialize, Deserialize)]
struct ReviewState {
    content: String,           // 来自 parent.document
    criteria: Vec<String>,     // 子图独有
    verdict: Option<String>,   // 映射回 parent.review_verdict
}

let mut parent = StateGraph::<ParentState>::new();
parent.add_subgraph(
    "review",
    review_graph,  // CompiledGraph<ReviewState>
    // input_map: 从父状态构建子状态
    |parent: &ParentState| ReviewState {
        content: parent.document.clone(),
        criteria: vec!["accuracy".into(), "clarity".into()],
        verdict: None,
    },
    // output_map: 将子图更新映射回父图更新
    |sub_update: ReviewStateUpdate| ParentStateUpdate {
        review_verdict: sub_update.verdict,
        ..Default::default()
    },
);
```

#### add_subgraph 签名

```rust
impl<S: State> StateGraph<S> {
    pub fn add_subgraph<Sub>(
        &mut self,
        name: &str,
        subgraph: CompiledGraph<Sub>,
        input_map: impl Fn(&S) -> Sub + Send + Sync + 'static,
        output_map: impl Fn(Sub::Update) -> S::Update + Send + Sync + 'static,
    ) -> &mut Self
    where
        Sub: State + Serialize + DeserializeOwned;
}
```

#### 类型安全保证

- `input_map` 闭包的签名 `Fn(&S) -> Sub` 在编译期检查字段访问
- `output_map` 闭包的签名 `Fn(Sub::Update) -> S::Update` 在编译期检查字段映射
- 字段名拼错、类型不匹配均为编译错误

---

## 3. Checkpoint 命名空间隔离

### 命名空间格式

```
{parent_namespace}|{subgraph_name}:{invocation_uuid}
```

示例：
- 根图：`""`
- 一级子图 `review`：`"|review:550e8400-e29b-41d4-a716-446655440000"`
- 嵌套子图 `review` 内的 `detail`：`"|review:550e8400...|detail:6ba7b810..."`

### 实现

```rust
#[derive(Clone, Debug)]
pub struct CheckpointNamespace {
    segments: Vec<NamespaceSegment>,
}

#[derive(Clone, Debug)]
pub struct NamespaceSegment {
    pub node_name: String,
    pub invocation_id: String,  // UUID
}

impl CheckpointNamespace {
    pub fn root() -> Self {
        Self { segments: vec![] }
    }

    pub fn child(&self, node_name: &str, invocation_id: &str) -> Self {
        let mut ns = self.clone();
        ns.segments.push(NamespaceSegment {
            node_name: node_name.to_string(),
            invocation_id: invocation_id.to_string(),
        });
        ns
    }

    pub fn to_string(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("{}:{}", s.node_name, s.invocation_id))
            .collect::<Vec<_>>()
            .join("|")
    }
}
```

### Checkpoint 存储

子图的 checkpoint 通过 namespace 前缀与父图隔离：

```rust
// CheckpointSaver 接口中，config 携带 namespace
pub struct RunnableConfig {
    pub thread_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub checkpoint_ns: CheckpointNamespace,  // 命名空间
    // ...
}
```

CheckpointSaver 实现使用 `(thread_id, checkpoint_ns, checkpoint_id)` 三元组作为唯一键。

---

## 4. 子图持久化模式

### 配置

```rust
pub enum SubgraphPersistence {
    /// 默认：每次调用全新开始
    /// 继承父图的 checkpointer 用于 interrupt 支持
    Inherit,

    /// 同一 thread 上跨调用累积状态
    PerThread,

    /// 无 checkpoint，不支持 interrupt
    Stateless,
}
```

### 行为差异

| 模式 | Checkpoint | Interrupt 支持 | 跨调用状态 |
|------|-----------|---------------|-----------|
| `Inherit` | 使用父图 checkpointer | 支持 | 不保留 |
| `PerThread` | 使用父图 checkpointer | 支持 | 保留 |
| `Stateless` | 无 | 不支持 | 不保留 |

### Inherit 模式（默认）

```rust
// 子图每次被父图调用时从 input_map 的输出开始
// 但如果子图内发生 interrupt，checkpoint 会被保存
// resume 时从 checkpoint 恢复（而非重新调用 input_map）
parent.add_subgraph_with_config(
    "review",
    review_graph,
    input_map,
    output_map,
    SubgraphConfig {
        persistence: SubgraphPersistence::Inherit,
        ..Default::default()
    },
);
```

### PerThread 模式

```rust
// 子图在同一 thread 上保持状态
// 适用于：子图代表一个长期运行的子任务
parent.add_subgraph_with_config(
    "memory_agent",
    memory_graph,
    input_map,
    output_map,
    SubgraphConfig {
        persistence: SubgraphPersistence::PerThread,
        ..Default::default()
    },
);
```

---

## 5. 中断传播

### 子图 → 父图冒泡

```
父图执行
  │
  ├─ 进入子图节点 "review"
  │     │
  │     ├─ 子图执行 reviewer 节点
  │     │     │
  │     │     └─ interrupt!(payload)
  │     │           ├─ 子图引擎捕获 Interrupted
  │     │           ├─ 保存子图 checkpoint（namespace: "|review:uuid"）
  │     │           └─ 返回 Err(JunctureError::SubgraphInterrupted { ... })
  │     │
  │     └─ 子图节点包装器捕获 SubgraphInterrupted
  │           ├─ 保存父图 checkpoint（source: Interrupt）
  │           │     checkpoint.subgraph_state = Some(子图 checkpoint 引用)
  │           └─ 向上返回 Err(JunctureError::Interrupted { ... })
  │
  └─ 父图引擎处理 Interrupted
        └─ 发送 StreamEvent::Interrupt { node: "review", ... }
```

### ParentCommand 传播

<!-- Addresses finding: H-14 -->

> 参考: `langgraph/errors.py:128` — ParentCommand

除了中断冒泡，子图节点还可以通过 `ParentCommand` 直接向父图发送路由命令：

```
子图节点执行
  │
  ├─ 返回 Err(ParentCommand(Command::goto("publish")))
  │
  ├─ 子图引擎捕获 ParentCommand
  │     ├─ 提取内部的 Command<S>
  │     ├─ 子图提前终止（不等待其他节点）
  │     └─ 将 Command 包装为 NodeOutput 返回给父图
  │
  └─ 父图 PregelLoop 处理
        ├─ 读取 Command.goto
        ├─ 使用 GraphTarget::Parent 路由
        └─ 父图调度到指定节点
```

与 `GraphTarget::Parent` 的区别：
- `GraphTarget::Parent`：由节点主动设置，作为 Command 的字段返回
- `ParentCommand`：作为异常抛出，允许在子图任意深度冒泡到父图

### Resume 向下传递

```
graph.resume(values, config)
  │
  ├─ 加载父图 checkpoint
  │     next = ["review"]  // 子图节点
  │
  ├─ 重新执行 "review" 子图节点
  │     │
  │     ├─ 检测到子图有中断 checkpoint
  │     ├─ 从子图 checkpoint 恢复（而非重新调用 input_map）
  │     ├─ 将 resume values 传递给子图引擎
  │     │
  │     └─ 子图 resume
  │           ├─ 重新执行被中断的子图节点
  │           ├─ interrupt!() 返回 resume value
  │           └─ 子图正常完成，返回最终 SubState
  │
  └─ output_map(sub_update) → ParentStateUpdate
```

### 实现要点

```rust
/// 子图节点的内部执行逻辑
async fn execute_subgraph_node<S, Sub>(
    parent_state: &S,
    subgraph: &CompiledGraph<Sub>,
    input_map: &dyn Fn(&S) -> Sub,
    output_map: &dyn Fn(Sub::Update) -> S::Update,
    config: &RunnableConfig,
) -> Result<NodeOutput<S>, JunctureError>
where
    S: State,
    Sub: State + Serialize + DeserializeOwned,
{
    // 构建子图命名空间
    let child_ns = config.checkpoint_ns.child(
        "review",
        &uuid::Uuid::new_v4().to_string(),
    );
    let child_config = config.clone().with_namespace(child_ns);

    // 检查是否有子图中断 checkpoint（resume 场景）
    if let Some(sub_checkpoint) = subgraph.checkpointer().get_checkpoint(&child_config).await? {
        if sub_checkpoint.metadata.source == CheckpointSource::Interrupt {
            // Resume 路径：从子图 checkpoint 恢复
            let sub_result = subgraph.resume(
                config.resume_values.clone().unwrap_or_default(),
                &child_config,
            ).await?;
            let sub_update = compute_update(sub_result);
            return Ok(NodeOutput::Update(output_map(sub_update)));
        }
    }

    // 正常路径：从 input_map 开始执行
    let sub_input = input_map(parent_state);
    match subgraph.invoke(sub_input, &child_config).await {
        Ok(sub_result) => {
            let sub_update = compute_update(sub_result);
            Ok(NodeOutput::Update(output_map(sub_update)))
        }
        Err(JunctureError::Interrupted { .. }) => {
            // 子图中断：冒泡到父图
            Err(JunctureError::SubgraphInterrupted {
                subgraph_name: "review".into(),
                namespace: child_config.checkpoint_ns,
            })
        }
        Err(e) => Err(e),
    }
}
```

---

## 6. Subgraph 作为 Runnable

### CompiledGraph 实现 Node trait

通过映射函数，`CompiledGraph<Sub>` 可以作为父图的节点使用：

```rust
/// 子图节点包装器
pub struct SubgraphNode<S: State, Sub: State> {
    subgraph: CompiledGraph<Sub>,
    input_map: Arc<dyn Fn(&S) -> Sub + Send + Sync>,
    output_map: Arc<dyn Fn(Sub::Update) -> S::Update + Send + Sync>,
    config: SubgraphConfig,
}

impl<S: State, Sub: State + Serialize + DeserializeOwned> Node<S> for SubgraphNode<S, Sub> {
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> BoxFuture<'_, Result<NodeOutput<S>, JunctureError>> {
        Box::pin(async move {
            execute_subgraph_node(
                &state,
                &self.subgraph,
                self.input_map.as_ref(),
                self.output_map.as_ref(),
                config,
            ).await
        })
    }

    fn name(&self) -> &str {
        "subgraph"  // 实际名称由 add_subgraph 时指定
    }
}
```

### 递归组合

子图可以包含子图，形成任意深度的嵌套：

```rust
// 三层嵌套
let inner = StateGraph::<InnerState>::new();
// ... 构建 inner ...
let inner_compiled = inner.compile_ephemeral()?;

let middle = StateGraph::<MiddleState>::new();
middle.add_subgraph("inner", inner_compiled, input_map, output_map);
let middle_compiled = middle.compile_ephemeral()?;

let outer = StateGraph::<OuterState>::new();
outer.add_subgraph("middle", middle_compiled, input_map, output_map);
let app = outer.compile(config)?;
```

命名空间自动嵌套：`"|middle:uuid1|inner:uuid2"`

### 取消传播

父图的 `CancellationToken` 自动传递给子图：

```rust
// 子图执行时继承父图的 cancellation token
let child_config = config.clone();
// child_config.cancellation_token 已经是父图的 token
// 父图取消 → 子图自动取消
```

---

## 7. Send API / 动态 Fan-out

### Send 与子图结合

Send 目标可以是子图节点，每个 Send 创建独立的执行实例：

```rust
#[derive(State, Clone, Serialize, Deserialize)]
struct OrchestratorState {
    tasks: Vec<Task>,
    #[reducer(append)]
    results: Vec<TaskResult>,
}

#[derive(State, Clone, Serialize, Deserialize)]
struct WorkerState {
    task: Task,
    result: Option<TaskResult>,
}

// Worker 子图
let worker_graph = {
    let mut g = StateGraph::<WorkerState>::new();
    g.add_node("process", process_task);
    g.add_edge(START, "process");
    g.add_edge("process", END);
    g.compile_ephemeral()?
};

// 父图
let mut orchestrator = StateGraph::<OrchestratorState>::new();
orchestrator.add_node("distribute", distribute_tasks);
orchestrator.add_subgraph(
    "worker",
    worker_graph,
    |parent: &OrchestratorState| WorkerState {
        task: parent.tasks[0].clone(),  // 由 Send 覆盖
        result: None,
    },
    |update: WorkerStateUpdate| OrchestratorStateUpdate {
        results: update.result.map(|r| vec![r]),
        ..Default::default()
    },
);
orchestrator.add_edge(START, "distribute");
orchestrator.add_edge("worker", END);
```

### Fan-out 实现

```rust
async fn distribute_tasks(state: OrchestratorState) -> Result<NodeOutput<OrchestratorState>> {
    // 动态创建 N 个并行 worker
    let sends: Vec<Send<OrchestratorState>> = state.tasks.iter()
        .map(|task| {
            Send::new("worker", OrchestratorState {
                tasks: vec![task.clone()],
                results: vec![],
            })
        })
        .collect();

    Ok(NodeOutput::Send(sends))
}
```

### Fan-in 语义

所有 Send 目标在同一 superstep 内并行执行。它们的输出通过 reducer 合并：

```
distribute → Send([worker×3])
                │
                ├─ worker(task_1) → results: [result_1]  ─┐
                ├─ worker(task_2) → results: [result_2]  ─┼─ append reducer
                └─ worker(task_3) → results: [result_3]  ─┘
                                                           │
                                                           ▼
                                              results: [result_1, result_2, result_3]
```

### Send 与子图的交互

当 Send 目标是子图节点时：
1. 每个 Send 创建独立的子图执行实例
2. 每个实例有独立的 checkpoint namespace
3. 所有实例真正并行执行（tokio::spawn）
4. 子图内的 interrupt 会中断整个 fan-out（所有并行实例暂停）

---

## 8. 完整示例：多级审核工作流

```rust
use juncture::prelude::*;

// === 子图：单个审核员 ===

#[derive(State, Clone, Serialize, Deserialize)]
struct ReviewerState {
    document: String,
    reviewer_name: String,
    #[reducer(append)]
    comments: Vec<String>,
    verdict: Option<String>,
}

async fn do_review(state: ReviewerState) -> Result<ReviewerStateUpdate> {
    // 中断等待审核员输入
    let input = interrupt!(json!({
        "reviewer": state.reviewer_name,
        "document": state.document,
        "instruction": "请审核此文档"
    }))?;

    Ok(ReviewerStateUpdate {
        comments: Some(vec![input["comment"].as_str().unwrap_or("").to_string()]),
        verdict: Some(Some(input["verdict"].as_str().unwrap_or("pending").to_string())),
        ..Default::default()
    })
}

let reviewer_graph = {
    let mut g = StateGraph::<ReviewerState>::new();
    g.add_node("review", do_review);
    g.add_edge(START, "review");
    g.add_edge("review", END);
    g.compile_ephemeral()?
};

// === 父图：编排多个审核员 ===

#[derive(State, Clone, Serialize, Deserialize)]
struct ApprovalState {
    document: String,
    reviewers: Vec<String>,
    #[reducer(append)]
    all_comments: Vec<String>,
    #[reducer(append)]
    verdicts: Vec<String>,
    final_decision: Option<String>,
}

async fn fan_out_reviews(state: ApprovalState) -> Result<NodeOutput<ApprovalState>> {
    let sends = state.reviewers.iter()
        .map(|name| Send::new("single_review", ApprovalState {
            document: state.document.clone(),
            reviewers: vec![name.clone()],
            all_comments: vec![],
            verdicts: vec![],
            final_decision: None,
        }))
        .collect();
    Ok(NodeOutput::Send(sends))
}

async fn aggregate(state: ApprovalState) -> Result<ApprovalStateUpdate> {
    let approved_count = state.verdicts.iter()
        .filter(|v| v.as_str() == "approved")
        .count();
    let decision = if approved_count > state.verdicts.len() / 2 {
        "approved"
    } else {
        "rejected"
    };
    Ok(ApprovalStateUpdate {
        final_decision: Some(Some(decision.to_string())),
        ..Default::default()
    })
}

let mut parent = StateGraph::<ApprovalState>::new();
parent.add_node("fan_out", fan_out_reviews);
parent.add_subgraph(
    "single_review",
    reviewer_graph,
    |p: &ApprovalState| ReviewerState {
        document: p.document.clone(),
        reviewer_name: p.reviewers[0].clone(),
        comments: vec![],
        verdict: None,
    },
    |update: ReviewerStateUpdate| ApprovalStateUpdate {
        all_comments: update.comments,
        verdicts: update.verdict.flatten().map(|v| vec![v]),
        ..Default::default()
    },
);
parent.add_node("aggregate", aggregate);

parent.add_edge(START, "fan_out");
parent.add_edge("single_review", "aggregate");
parent.add_edge("aggregate", END);

let app = parent.compile(CompileConfig {
    checkpointer: Some(Box::new(MemorySaver::new())),
    ..Default::default()
})?;
```

---

## 9. 实现清单

| 组件 | 位置 | 职责 |
|------|------|------|
| `StateSubset<P>` trait | `juncture-core/src/state/subset.rs` | 共享状态模式的编译期约束 |
| `#[subset_of(..)]` 属性 | `juncture-derive/src/subset.rs` | 自动生成 StateSubset impl |
| `SubgraphNode<S, Sub>` | `juncture-core/src/subgraph/node.rs` | 子图节点包装器 |
| `SubgraphConfig` | `juncture-core/src/subgraph/config.rs` | 持久化模式等配置 |
| `CheckpointNamespace` | `juncture-checkpoint/src/namespace.rs` | 命名空间管理 |
| `add_subgraph` / `add_subgraph_node` | `juncture-core/src/graph/builder.rs` | StateGraph 构建方法 |
| Send + Subgraph 集成 | `juncture-core/src/pregel/executor.rs` | fan-out 时创建子图实例 |
| 中断传播 | `juncture-core/src/subgraph/interrupt.rs` | 子图中断冒泡 + resume 传递 |

---

## 6. SubgraphTransformer（<!-- Addresses finding: M-8 -->）

> 参考: `langgraph/channels/transform.py` — SubgraphTransformer 类

SubgraphTransformer 是用于嵌套子图流式输出的事件转换器。当子图产生 StreamEvent 时，SubgraphTransformer 将子图命名空间添加到事件的 `ns` 字段，并可选择过滤和转换事件。

### 6.1 设计

```rust
/// 子图事件转换器
///
/// 负责将子图的 StreamEvent 转换为父图的事件格式，
/// 添加命名空间前缀并可选地过滤事件。
pub struct SubgraphTransformer<S: State> {
    /// 子图名称
    subgraph_name: String,
    /// 子图命名空间路径
    ns: Vec<String>,
    /// 事件过滤条件（可选）
    filter: Option<Box<dyn Fn(&StreamEvent<S>) -> bool + Send + Sync>>,
    /// 是否包含子图的内部事件
    include_internal: bool,
}

impl<S: State + Serialize + DeserializeOwned> SubgraphTransformer<S> {
    /// 创建新的子图转换器
    pub fn new(
        subgraph_name: String,
        parent_ns: Vec<String>,
    ) -> Self {
        let mut ns = parent_ns;
        ns.push(subgraph_name.clone());
        Self {
            subgraph_name,
            ns,
            filter: None,
            include_internal: false,
        }
    }

    /// 设置事件过滤器
    pub fn with_filter<F>(mut self, filter: F) -> Self
    where
        F: Fn(&StreamEvent<S>) -> bool + Send + Sync + 'static
    {
        self.filter = Some(Box::new(filter));
        self
    }

    /// 设置是否包含内部事件（debug, checkpoints 等）
    pub fn with_internal(mut self, include: bool) -> Self {
        self.include_internal = include;
        self
    }

    /// 转换子图事件
    pub fn transform(&self, event: StreamEvent<S>) -> Option<StreamEvent<S>> {
        // 应用过滤器
        if let Some(ref filter) = self.filter {
            if !filter(&event) {
                return None;
            }
        }

        // 过滤内部事件
        if !self.include_internal {
            match &event {
                StreamEvent::Debug(_) |
                StreamEvent::CheckpointSaved { .. } |
                StreamEvent::TaskDetail { .. } => return None,
                _ => {}
            }
        }

        // 添加命名空间前缀
        Some(self.add_namespace(event))
    }

    /// 为事件添加子图命名空间
    fn add_namespace(&self, event: StreamEvent<S>) -> StreamEvent<S> {
        match event {
            StreamEvent::Values { state, step } => StreamEvent::Values { state, step },
            StreamEvent::Updates { node, update, step } => {
                // 更新节点名为完全限定名
                let qualified_node = format!("{}:{}", self.ns.join("|"), node);
                StreamEvent::Updates { node: qualified_node, update, step }
            },
            StreamEvent::Custom { mut ns, .. } => {
                // 添加子图命名空间
                ns.extend(self.ns.clone());
                event
            },
            _ => event,
        }
    }
}
```

### 6.2 使用示例

```rust
// 在父图的流式执行中，为子图事件添加转换器
let transformer = SubgraphTransformer::new(
    "review".to_string(),
    vec![],
)
.with_filter(|event| {
    // 只传递 Values 和 Updates 事件
    matches!(event, StreamEvent::Values { .. } | StreamEvent::Updates { .. })
})
.with_internal(false);

// 子图执行时，事件通过转换器传递到父图
let subgraph_stream = subgraph.stream(input, &config, StreamMode::Updates).await?
    .filter_map(|event| event.ok())
    .filter_map(|event| transformer.transform(event));
```

---

## 源码参考索引

| 概念 | LangGraph 源码位置 | 说明 |
|------|-------------------|------|
| 子图作为节点 | `langgraph/graph/state.py:1443` | attach_node() 处理 CompiledGraph 作为节点 |
| checkpoint_ns 格式 | `langgraph/_internal/_constants.py` (NS_SEP, NS_END) | 命名空间分隔符 `:`、嵌套分隔符 `\|` |
| 子图 checkpoint 隔离 | `langgraph/pregel/_loop.py` | 子图使用独立 checkpoint_ns |
| 子图 streaming | `langgraph-doc/use-subgraphs.md` | subgraphs=True 参数 |
| 子图持久化模式 | `langgraph-doc/use-subgraphs.md` | checkpointer=None/True/False |
| 中断传播 | `langgraph/errors.py:101` | GraphInterrupt 从子图冒泡到父图 |
| Send + 子图 | `langgraph/types.py:654` | Send 目标可以是子图节点 |
| 子图 state 映射 | `langgraph-doc/use-subgraphs.md` | 两种模式：共享 key / 包装函数 |
| CONFIG_KEY_CHECKPOINT_NS | `langgraph/_internal/_constants.py` | checkpoint 命名空间配置键 |
| 子图 input/output schema | `langgraph/graph/state.py:130` | StateGraph 支持 input_schema/output_schema |
