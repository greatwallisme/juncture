spec: task
name: "subgraph-namespace-persistence-conformance"
tags: [design-conformance, subgraph, 07-subgraph]
---

## 意图

将 design `07-subgraph` 的 SubgraphPersistence（Stateless/PerThread/Inherit）命名空间隔离策略、SubgraphTransformer 节点/命名空间前缀、嵌套命名空间链接固化为可验证合约，对 `crates/juncture-core/src/subgraph.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub enum SubgraphPersistence`/`pub struct SubgraphTransformer` 等符号名（存在即计覆盖），无法验证「Stateless 命名空间为 None、PerThread 用 thread_id、Inherit 用 uuid 且每次调用不同」这类持久化模式→命名空间策略的行为映射。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `SubgraphPersistence::Stateless` 命名空间必须为 None
- `PerThread` 命名空间必须基于 thread_id
- `Inherit` 命名空间必须基于 uuid，且不同调用产生不同命名空间
- `SubgraphTransformer` 必须给节点名与命名空间加前缀
- 嵌套子图的命名空间必须按层级正确链接
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- 三种持久化模式对应三种命名空间策略：Stateless（无）、PerThread（thread_id 隔离）、Inherit（uuid 隔离，每次独立）
- SubgraphTransformer 统一为子图事件加 namespace 前缀，支持 filter 按类型过滤
- 嵌套命名空间用 separator 链接，支持多层

## 边界

### Allowed Changes
- specs/subgraph-namespace-persistence-conformance.spec.md
- 本合约为只读核实，不改实现代码（subgraph.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `juncture-core/src/subgraph.rs` 中 SubgraphPersistence / SubgraphTransformer / namespace 逻辑
- 不得改动 `crates/juncture-derive/**`（StateSubset 代码生成）

## 完成条件

### Rule: persistence-namespace-strategy — 持久化模式→命名空间

场景: Stateless 命名空间为 None
  测试:
    包: juncture-core
    过滤: test_stateless_namespace_is_none
  当 SubgraphPersistence 为 Stateless
  那么 计算出的命名空间为 None

场景: PerThread 命名空间基于 thread_id
  测试:
    包: juncture-core
    过滤: test_perthread_namespace_uses_thread_id
  当 SubgraphPersistence 为 PerThread
  那么 命名空间基于 thread_id（不同 thread 不同 ns）

场景: Inherit 命名空间基于 uuid
  测试:
    包: juncture-core
    过滤: test_inherit_namespace_is_uuid_based
  当 SubgraphPersistence 为 Inherit
  那么 命名空间基于 uuid

场景: Inherit 每次调用命名空间不同
  标签: critical
  测试:
    包: juncture-core
    过滤: test_inherit_namespace_differs_between_invocations
  当 同一 Inherit 子图被多次调用
  那么 每次产生不同命名空间（uuid 隔离）

### Rule: transformer-prefix — 事件前缀转换

场景: transformer 为节点名加前缀
  测试:
    包: juncture-core
    过滤: transform_updates_prefixes_node_name
  当 SubgraphTransformer 转换子图事件
  那么 节点名被加上命名空间前缀

场景: filter 拒绝非匹配类型
  测试:
    包: juncture-core
    过滤: transform_filter_rejects_non_matching_type
  当 transformer 配置了类型 filter 且事件类型不匹配
  那么 该事件被拒绝（不输出）

### Rule: nested-namespace — 嵌套命名空间

场景: 三层嵌套命名空间正确链接
  标签: critical
  测试:
    包: juncture-core
    过滤: nested_namespace_three_levels_deep
  当 子图嵌套三层
  那么 命名空间按层级正确链接（每层独立 uuid）

场景: StateSubset 共享状态循环（extract→map_update→parent.apply）
  测试:
    包: juncture-core
    过滤: subset_shared_state_cycle_applies_child_changes_to_parent
  当 子图 extract 父状态→产生 delta→map_update→父 apply
  那么 子图变更传播到父（name replace、messages append、age 父专属字段不动）

## Questions

- [x] **StateSubset 共享状态语义**：RESOLVED 2026-07-29 — 新增 `subset_shared_state_cycle_applies_child_changes_to_parent`（derive_state.rs），覆盖完整共享状态循环：`extract` 子状态 → 子图产生 delta → `map_update` 投影回父 Update → 父 `apply` 使子图变更可见（name replace、messages append、age 父专属字段不动）。补齐了原 extract/map_update 单元测试缺失的 apply-back 步骤。
