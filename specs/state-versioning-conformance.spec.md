spec: task
name: "state-versioning-conformance"
tags: [design-conformance, state, pregel, scheduler, 01-state-channel]
---

## 意图

将 design `01-state-channel` §2.6（版本追踪与调度）的 `FieldVersionTracker` / `VersionsSeen` / `FieldsChanged` 语义固化为可验证合约。这三者是 Pregel 反应式调度的 correctness 基石：节点是否被激活、字段版本是否推进，全靠它们。

`verify-design-coverage.py` 仅 grep `pub struct FieldVersionTracker`/`VersionsSeen`/`fn should_activate` 等符号名，无法区分「bump 用全局单调计数」与「per-field 自增」、或「严格大于才激活」与「大于等于激活」这类行为差异。这些原语此前**仅有 doc-test、无单元测试**，本合约补齐绑定测试。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `FieldVersionTracker.bump` 必须用全局单调 global_max 策略（连续 bump 不同字段得到递增的全局计数，非 per-field 各自 1）
- `bump_all` 只递增 `FieldsChanged` 标记的字段，未标记字段保持 0
- 构造 >64 字段的 `FieldVersionTracker` 必须 panic
- `VersionsSeen.should_activate` 仅当某 trigger 字段 current version **严格大于** seen 时返回真（等于不激活）
- `mark_consumed` 后，相同 version 不得重复激活该节点
- `FieldsChanged` 用 u64 bitmask，set_field/has_field/merge 语义正确

## 已定决策

- 版本号由引擎侧 `FieldVersionTracker`（scheduler.rs）管理，State 内的 `field_versions()`/`bump_versions()` 为 no-op 默认（trait_.rs 注释：版本追踪委托给引擎）
- global_max 用 `saturating_add` 防溢出；`VersionsSeen` 用 IndexMap 保证确定性迭代顺序
- `should_activate` 对未跟踪节点返回 true（首次激活语义）

## 边界

### Allowed Changes
- specs/state-versioning-conformance.spec.md
- crates/juncture-core/src/pregel/scheduler.rs（仅 `scheduler_tests` 内新增 5 个测试函数）

### Forbidden
- 不得修改 `FieldVersionTracker` / `VersionsSeen` / `FieldsChanged` 的实现逻辑以通过验证
- 不得改动 `crates/juncture-derive/**`

## 完成条件

### Rule: field-version-tracker — 全局单调版本计数

场景: bump 用 global_max 全局单调策略
  测试:
    包: juncture-core
    过滤: field_version_tracker_bump_uses_global_max_strategy
  当 对 3 字段 tracker 先 bump(0) 再 bump(2)
  那么 get(0)=1、get(2)=2（取下一个全局计数，非 per-field 的 1）、global_max=2

场景: bump_all 只递增变化字段
  测试:
    包: juncture-core
    过滤: field_version_tracker_bump_all_only_changed_fields
  当 bump_all(FieldsChanged(0b101))（字段 0 与 2）
  那么 get(0)=1、get(1)=0（未标记保持 0）、get(2)=2

场景: 构造超过 64 字段 panic
  标签: critical
  测试:
    包: juncture-core
    过滤: field_version_tracker_new_rejects_over_64_fields
  当 `FieldVersionTracker::new(65)`
  那么 panic，消息含 "Cannot track more than 64 fields"

### Rule: versions-seen — 反应式激活门控

场景: 仅当 version 严格高于 seen 才激活
  测试:
    包: juncture-core
    过滤: versions_seen_activates_only_when_version_strictly_above_seen
  当 节点 seen=[0,0,0]、current=[1,0,0]
  那么 trigger=[0] 激活（1>0），trigger=[1] 不激活（0==0，严格大于）

场景: mark_consumed 抑制相同 version 的重复激活
  测试:
    包: juncture-core
    过滤: versions_seen_mark_consumed_suppresses_reactivation
  当 节点对 current=[1,0,0] 先激活、再 mark_consumed、再查相同 version
  那么 不再激活（已消费该 version）

### Rule: fields-changed — u64 位掩码

场景: FieldsChanged bitmask set/has/merge
  测试:
    包: juncture-core
    过滤: fields_changed_bitmask
  当 对 FieldsChanged 做 set_field、has_field、merge 操作
  那么 位掩码语义正确（标记/查询/合并按 u64 位运算）

## Questions

- [x] **per-state `FieldVersions` 与引擎 `FieldVersionTracker` 双轨**：RESOLVED 2026-07-29 — per-state `FieldVersions`（struct + `type FieldVersions` + `field_versions()`/`bump_versions()` trait 方法 + derive 生成）经核实**零生产调用点**（死代码），且 design §2.6 明确"字段版本号不存储在 State 本身，而是由 PregelLoop 管理"。用户 2026-07-29 决策 #5：移除死代码。已移除：trait_.rs 定义、state_derive.rs 生成、18 处 `type FieldVersions =` impl、state/mod.rs re-export、design/01 §2.2 trait + §2.6 note、checklists/01-state-channel.json。版本追踪统一由引擎 `FieldVersionTracker` 负责。verify-design-coverage 216/216，guard 14/14 通过。
