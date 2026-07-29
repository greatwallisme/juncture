spec: task
name: "functional-api-macros-conformance"
tags: [design-conformance, functional-api, macro, entrypoint, task]
---

## 意图

固化 design `03-pregel-engine` §13.3（`#[task]` / `#[entrypoint]` 属性宏）与 §14（Previous Result Injection）的语义要求为可验证合约，对 `433c7d8` 引入的宏实现做**语义级**（非符号级）核对。目的是暴露 `scripts/verify-design-coverage.py`（仅 grep 符号名，216/216）无法检测的行为偏差——典型如"宏符号存在但其签名使设计文档承诺的能力（§14 previous 访问）对用户不可用"。

## 约束

- `#[task]` 必须将 `async fn(...) -> Result<O, E>` 转为返回 `SyncAsyncFuture<O>` 的可调用包装器
- `#[task(cache = ...)]` 命中时必须**同步返回**（`SyncAsyncFuture::Ready`），不执行函数体
- `#[task(retry = ...)]` 必须对瞬时失败重试（指数退避 + 抖动）
- `#[task(timeout = ...)]` 超时必须转为 `Err`
- `#[entrypoint]` 必须生成 `compile()` 访问器，返回可执行的 `CompiledGraph`
- `#[entrypoint]` 标注的函数**必须能访问 design §14 的 `previous`**（上一次执行的返回值），以支持累积/增量模式——这是 §14 的核心能力
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- 宏由 `juncture-derive` 实现为属性宏（design §13.3）
- `#[task]` cache 用 per-task `OnceLock<CachePolicy>`（自包含；图级 cache 的"后续扩展"批准证据丢失，见审计任务4）
- `#[entrypoint]` 当前用节点兼容签名 `async fn(&S) -> Result<S::Update, JunctureError>`，经 `NodeFnUpdate` 包装（D-03-13）

## 边界

### Allowed Changes
- specs/functional-api-macros-conformance.spec.md
- 本合约为只读核实，不改实现代码

### Forbidden
- 不得为通过 verification 而修改 `crates/juncture-derive/**` 或 `crates/juncture-core/src/func/**`
- 不得放松 §14 previous 可访问性要求

## 完成条件

场景: task cache 命中同步返回
  测试:
    包: juncture-core
    过滤: task_cache_hits_on_second_call
  当 同一 `#[task(cache = ...)]` 被相同参数调用两次
  那么 第二次返回同步结果（缓存命中）且函数体只执行一次

场景: task retry 对瞬时失败重试
  测试:
    包: juncture-core
    过滤: task_retry_succeeds_on_second_attempt
  当 `#[task(retry = ...)]` 首次失败、第二次成功
  那么最终返回成功且至少重试一次

场景: task timeout 转为错误
  测试:
    包: juncture-core
    过滤: task_timeout_errors
  当 `#[task(timeout = ...)]` 的函数体超过超时时长
  那么返回 Err

场景: entrypoint 编译并可调用
  测试:
    包: juncture-core
    过滤: entrypoint_compiles_and_invokes
  当 `#[entrypoint]` 标注的函数调用 `compile()` 后 `invoke_async`
  那么图成功执行并返回 Ok

场景: entrypoint 可访问 previous 以支持 §14 累积模式
  标签: critical
  测试:
    包: juncture-core
    过滤: entrypoint_accesses_previous_return_value
  当 带 checkpointer 的 `#[entrypoint]` 第二次执行
  那么函数体内能读取到上一次执行的返回值（previous 非 None）

场景: entrypoint 可访问 runtime 上下文（store/checkpointer）
  测试:
    包: juncture-core
    过滤: entrypoint_accesses_runtime_context
  当 `#[entrypoint]` 函数执行
  那么函数体内能通过 runtime 访问 store 与 checkpointer 上下文

场景: task 图级 cache 命中（runner-scoped TASK_CACHE_POLICY）
  测试:
    包: juncture-core
    过滤: task_hits_graph_level_cache_via_task_local
  当 `TASK_CACHE_POLICY` task-local 被 scope 且同一 `#[task]` 被相同参数调用两次
  那么第二次命中图级 cache，函数体只执行一次

## Questions

- [x] 图级/跨任务 cache：RESOLVED 2026-07-29 — 已实现。`TASK_CACHE_POLICY` task-local，runner 从 `RunnableConfig::task_cache_policy`（源自 `CompileConfig.cache_policy`）scope；`#[task]` 查图级优先，per-task `OnceLock` fallback。原 plan 文件 compiled-tumbling-locket.md 已丢失，用户在审计中明确批准实现。
- [x] `#[entrypoint]` 升级 `NodeFnUpdateWithRuntime`：RESOLVED 2026-07-29 — 已实现。签名 `async fn(&S, &Runtime<()>) -> Result<S::Update>`，引擎把动态 `previous` 合并进 `runtime.previous`（§14 恢复）。用户在审计中明确批准。剩余偏差：I/O 类型解耦（I,O ≠ S）需 IntoState/FromState 桥接，仍为后续。
