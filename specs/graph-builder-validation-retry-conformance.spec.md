spec: task
name: "graph-builder-validation-retry-conformance"
tags: [design-conformance, graph-builder, topology, retry, 02-graph-builder]
---

## 意图

将 design `02-graph-builder` 的拓扑验证（validate_keys）与节点重试决策（RetryPolicy::should_retry）语义固化为可验证合约，对 `crates/juncture-core/src/graph/builder.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `fn validate_keys`/`fn should_retry`/`RetryPolicy` 等符号名（存在即计覆盖），无法验证「validate_keys 捕获无效 replace field index / 保留字符 / 缺失 entry-finish」、「should_retry 默认允许 execution error 但阻止 cancelled 与 interrupt」这类验证/决策行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `validate_keys` 必须捕获无效 replace field index、保留字符、缺失 entry/finish point
- `add_sequence` 引用不存在的节点必须报错
- `RetryPolicy::should_retry` 默认允许 execution error 重试
- `should_retry` 默认必须阻止 cancelled 与 interrupt（不重试这两类）
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- validate_keys 在 compile 时强制执行：校验节点名、entry/finish 存在、reducer 索引有效、保留字符
- RetryPolicy 默认重试瞬时 execution error；cancelled/interrupt 不可重试（避免无意义重试取消/中断）
- should_retry 支持自定义 predicate 覆盖默认决策

## 边界

### Allowed Changes
- specs/graph-builder-validation-retry-conformance.spec.md
- 本合约为只读核实，不改实现代码（builder.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `juncture-core/src/graph/builder.rs` 中 validate_keys / RetryPolicy / should_retry 实现逻辑
- 不得改动 `crates/juncture-derive/**`

## 完成条件

### Rule: topology-validation — validate_keys 校验

场景: 捕获无效 replace field index
  标签: critical
  测试:
    包: juncture-core
    过滤: test_validate_keys_catches_invalid_replace_field_index
  当 状态声明的 replace field index 越界
  那么 validate_keys 报错（捕获无效索引）

场景: 捕获保留字符
  测试:
    包: juncture-core
    过滤: test_validate_keys_reserved_characters
  当 节点名含保留字符
  那么 validate_keys 报错

场景: entry point 不存在报错
  测试:
    包: juncture-core
    过滤: test_validate_keys_entry_point_not_found
  当 entry point 引用不存在的节点
  那么 validate_keys 报错

场景: finish point 不存在报错
  测试:
    包: juncture-core
    过滤: test_validate_keys_finish_point_not_found
  当 finish point 引用不存在的节点
  那么 validate_keys 报错

### Rule: sequence-validation — add_sequence 校验

场景: 引用不存在的节点报错
  测试:
    包: juncture-core
    过滤: test_add_sequence_missing_node
  当 add_sequence 含不存在的节点
  那么 报错（拒绝构造残缺序列）

### Rule: retry-decision — should_retry 决策

场景: 默认允许 execution error 重试
  测试:
    包: juncture-core
    过滤: test_retry_policy_should_retry_default_allows_execution_errors
  当 错误为 execution error
  那么 should_retry 返回 true（默认重试）

场景: 默认阻止 cancelled 重试
  标签: critical
  测试:
    包: juncture-core
    过滤: test_retry_policy_should_retry_default_blocks_cancelled
  当 错误为 cancelled
  那么 should_retry 返回 false（不重试取消）

场景: 默认阻止 interrupt 重试
  测试:
    包: juncture-core
    过滤: test_retry_policy_should_retry_default_blocks_interrupt
  当 错误为 interrupt
  那么 should_retry 返回 false（不重试中断）

## Questions

- [ ] **拓扑环检测覆盖**：本合约覆盖 validate_keys 的索引/命名/entry-finish 校验；环路检测（tarjan SCC）仅有 test_tarjan_scc_simple/cycle 两个测试，端到端 compile-时环拒绝路径覆盖较薄。（按用户指示推迟到试点结束后统一决策 2026-07-29）
