spec: task
name: "pregel-scheduling-correctness-conformance"
tags: [design-conformance, pregel, scheduler, 03-pregel-engine]
---

## 意图

将 design `03-pregel-engine` 的调度正确性语义固化为可验证合约，覆盖 `apply_writes` 合并确定性、replace 多写入冲突检测、error-handler 恢复调度、fallback 熔断，对 `crates/juncture-core/src/pregel/scheduler.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `fn apply_writes`/`check_replace_conflicts`/`schedule_error_handlers` 等符号名（存在即计覆盖），无法验证「并发同字段写入顺序无关（确定性合并）」、「失败节点触发已注册 handler 且去重」、「熔断节点跳过 fallback」这类调度行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `apply_writes` 对同一字段的并发写入必须**顺序无关**（确定性合并，相同输入任意完成序→相同结果）
- `check_replace_conflicts` 对无冲突的 superstep 返回 Ok
- `schedule_error_handlers` 为失败节点调度其注册的恢复 handler，且去重、排除已处理
- `schedule_fallback_tasks` 对 `circuit_blocked` 的节点跳过 fallback 调度
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- apply_writes 先 `check_replace_conflicts_from_state` 再合并；并发同字段写入按确定性顺序合并（**非**完成序——这实际修正了 Reducer trait 文档注释的"完成序"措辞）
- error-handler 调度做去重，防止同一失败被多次恢复
- fallback 任务带熔断（circuit_blocked）+ 自引用守卫 + 循环守卫，防无限调度

## 边界

### Allowed Changes
- specs/pregel-scheduling-correctness-conformance.spec.md
- 本合约为只读核实，不改实现代码（scheduler.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `scheduler.rs` 中 apply_writes / check_replace_conflicts / schedule_error_handlers / schedule_fallback_tasks 的实现逻辑
- 不得改动 `crates/juncture-derive/**`

## 完成条件

### Rule: deterministic-merge — apply_writes 确定性

场景: 并发同字段写入顺序无关
  标签: critical
  测试:
    包: juncture-core
    过滤: test_apply_writes_concurrent_same_field_is_order_independent
  当 三个节点并发写同一 append 字段，输入序分别为 abc 与 cba
  那么 两次合并结果完全相等（顺序无关，确定性合并）

场景: 空输出合并成功
  测试:
    包: juncture-core
    过滤: test_apply_writes_empty_outputs
  当 apply_writes 收到空 task 输出
  那么 返回 Ok（无写入可合并）

### Rule: replace-conflict-check — replace 多写入检测

场景: 无冲突返回 Ok
  测试:
    包: juncture-core
    过滤: test_check_replace_conflicts_no_conflicts
  当 superstep 内无 replace 字段被多节点写入
  那么 check_replace_conflicts 返回 Ok

### Rule: error-handler-recovery — 失败恢复调度

场景: 无失败时不调度恢复
  测试:
    包: juncture-core
    过滤: test_schedule_error_handlers_no_failures
  当 task_outputs 中无失败节点
  那么 恢复任务列表为空

场景: 有失败节点调度其注册 handler
  标签: critical
  测试:
    包: juncture-core
    过滤: test_schedule_error_handlers_with_failure
  当 失败节点注册了 error handler
  那么 调度该 handler 为恢复任务

场景: 排除已处理并去重
  测试:
    包: juncture-core
    过滤: test_schedule_error_handlers_from_writes_excludes_already_handled_and_dedupes
  当 多个失败指向同一 handler 或已被处理
  那么 恢复任务去重且排除已处理项（不重复恢复）

### Rule: fallback-circuit — fallback 熔断

场景: 熔断节点跳过 fallback
  测试:
    包: juncture-core
    过滤: test_schedule_fallback_tasks_skips_circuit_blocked
  当 节点标记为 circuit_blocked
  那么 不为该节点调度 fallback 任务

## Questions

- [ ] **replace 冲突-检测(Err)路径无测试**：`check_replace_conflicts`（返回 `JunctureError::Execution`）与 `check_replace_conflicts_from_state`（返回 `JunctureError::multiple_writers`）都只测了 Ok 路径（_empty/_no_conflicts），多写入触发 Err 的正面路径零测试。需补冲突检测测试。（按用户指示推迟到试点结束后统一决策 2026-07-29）
- [ ] **两个冲突检查器返回不同错误类型**：`Execution`（通用）vs `multiple_writers`（结构化）。与 deviation #2（`InvalidUpdateError::MultipleWriters` 从未构造）相关，三者错误模型需统一。（同上推迟）
