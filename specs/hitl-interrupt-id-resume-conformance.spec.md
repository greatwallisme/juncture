spec: task
name: "hitl-interrupt-id-resume-conformance"
tags: [design-conformance, hitl, interrupt, 06-hitl]
---

## 意图

将 design `06-hitl` §2.2（确定性中断 ID 生成）、§3.3（resume 命名空间匹配）、§6（HIDDEN_TAG 过滤）的纯函数语义固化为可验证合约，对 `crates/juncture-core/src/interrupt/mod.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `fn generate_interrupt_id`/`extract_namespace`/`is_hidden_node`/`fn validate_resume_coverage` 等符号名（存在即计覆盖），无法验证「相同 (node,index) 产生相同 ID」（xxh3 确定性）、「`{ns}:{local}` 解析出 ns」、「双下划线命名 + len>4 才隐藏」这类行为。本合约绑定真实测试暴露这些 grep 盲区——尤其 `generate_interrupt_id` 此前**仅有 len doc-test**，其确定性/唯一性（xxh3 的核心保证）零覆盖。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `generate_interrupt_id(node, index)` 相同输入必须产生相同 ID（单 build 内 xxh3 确定性）
- 不同 index 或不同 node 必须产生不同 ID
- ID 必须为 32 字符小写十六进制
- `extract_namespace` 对 `{ns}:{local}` 返回 Some(ns)，无冒号或空 ns 返回 None
- `validate_resume_coverage` 全部 pending id 有 resume 值时 Ok，否则返回未覆盖 id 列表
- `is_hidden_node` 仅当名称同时以 `__` 开头结尾且长度 >4、或带 HIDDEN_TAG 时为真

## 已定决策

- 中断 ID 用 `xxh3::digest128()` → `format!("{:032x}")`，单 build 确定性（跨 build/cross-process 不保证，见 fn 文档）
- `validate_resume_coverage` 返回 `Result<(), Vec<String>>`（未覆盖 id 列表），非结构化 error
- `is_hidden_node` 名称规则含 `len() > 4` 阈值（防止 `__` 本身被误判为隐藏）

## 边界

### Allowed Changes
- specs/hitl-interrupt-id-resume-conformance.spec.md
- crates/juncture-core/src/interrupt/mod.rs（仅 `mod tests` 内新增 3 个 generate_interrupt_id 测试）

### Forbidden
- 不得修改 `interrupt/mod.rs` 中任何函数实现以通过验证
- 不得改动 `crates/juncture-derive/**` 或 `interrupt!` 宏定义

## 完成条件

### Rule: deterministic-interrupt-id — xxh3 确定性 ID

场景: 相同输入产生相同 ID
  测试:
    包: juncture-core
    过滤: generate_interrupt_id_is_deterministic_for_same_input
  当 对相同 (node, index) 调用两次 generate_interrupt_id
  那么 两次结果完全相等，且长度为 32

场景: 不同 index / node 产生不同 ID
  测试:
    包: juncture-core
    过滤: generate_interrupt_id_differs_across_index_and_node
  当 固定 node 改 index、固定 index 改 node
  那么 两种情况下 ID 都不同

场景: ID 为 32 字符小写十六进制
  测试:
    包: juncture-core
    过滤: generate_interrupt_id_is_lowercase_hex
  当 生成任意 ID
  那么 长度 32 且全部为小写十六进制字符

### Rule: namespace-extraction — 命名空间解析

场景: 冒号前缀解析为命名空间
  测试:
    包: juncture-core
    过滤: extract_namespace_with_namespace
  当 extract_namespace 收到 "agent:review#0"
  那么 返回 Some("agent")

场景: 无冒号或空命名空间返回 None
  测试:
    包: juncture-core
    过滤: extract_namespace_without_namespace
  当 extract_namespace 收到无冒号的 id（如 "node#index"）
  那么 返回 None

### Rule: resume-coverage-validation — resume 覆盖校验

场景: 全部 pending 有 resume 值则 Ok
  测试:
    包: juncture-core
    过滤: validate_resume_coverage_complete
  当 pending 中断的 id 全部出现在 resume_values map 中
  那么 返回 Ok(())

场景: 缺失 resume 值返回未覆盖列表
  标签: critical
  测试:
    包: juncture-core
    过滤: validate_resume_coverage_multiple_uncovered
  当 pending 中有 id 不在 resume_values 中
  那么 返回 Err，含全部未覆盖 id

### Rule: hidden-node-filtering — 内部节点过滤

场景: 双下划线命名（len>4）或 HIDDEN_TAG 判为隐藏
  测试:
    包: juncture-core
    过滤: hidden_node_double_underscore_prefix_and_suffix
  当 节点名形如 "__route__"
  那么 is_hidden_node 返回 true（普通名 "my_node" 返回 false，由 normal_nodes_are_not_hidden 覆盖反面）

场景: ByNamespace 按 extract_namespace 真正命名空间匹配
  测试:
    包: juncture-core
    过滤: test_match_by_namespace_namespace_mapping
  当 `match_resume_to_interrupts` 收 `ByNamespace` + "{ns}:{local}" 格式中断 id
  那么 按 namespace 路由（ns_a→A_val、ns_b→B_val、无命名空间前缀→None、命名空间不在 map→None）

## Questions

- [x] **`match_resume_to_interrupts` 未实现**：RESOLVED 2026-07-29 — 经核实该函数**已存在**于 `pregel/runner.rs:838`（原 spec 结论"未实现"已过时），处理 Single/ById/ByNamespace。用户决策 #2：对齐 design §3.3 — (a) 新增 `JunctureError::MissingResumeValue { interrupt_ids }` 变体；(b) `ByNamespace` 从错误的"数字索引匹配"修正为真正用 `extract_namespace(id)` 按命名空间匹配（runner.rs 3 个测试已改为命名空间场景）；(c) 引擎 resume 路径保留 `Vec<Option<Value>>` 返回（partial-resume 再触发，per design §3.4）。
- [x] **`validate_resume_coverage` 返回类型**：RESOLVED 2026-07-29 — 用户决策 #2：`validate_resume_coverage` 返回类型从 `Result<(), Vec<String>>` 改为 `Result<(), JunctureError>`，未覆盖中断返回结构化 `MissingResumeValue { interrupt_ids }`（携带全部未覆盖 id，符合本合约 `validate_resume_coverage_multiple_uncovered` 场景"含全部未覆盖 id"）。测试已更新为 `is_missing_resume_value()` + `missing_resume_ids()` 断言。
