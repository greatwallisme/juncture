spec: task
name: "pregel-budget-retry-conformance"
tags: [design-conformance, pregel, budget, retry, 03-pregel-engine]
---

## 意图

将 design `03-pregel-engine` 的预算追踪（BudgetTracker）与重试退避（compute_delay / RetryPolicy）语义固化为可验证合约，对 `crates/juncture-core/src/pregel/budget.rs` 与 `crates/juncture-core/src/graph/builder.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub struct BudgetTracker`/`fn compute_delay`/`RetryPolicy` 等符号名（存在即计覆盖），无法验证「退避延迟封顶于 max_interval 且 jitter 限制在 ±25%」、「预算任一维度超限 check 返回 Some」这类数学/状态行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- `compute_delay(base, jitter, max)` 无 jitter 时等于 min(base, max)
- 有 jitter 时结果落在 [0.75, 1.25] × capped 区间，且经二次封顶后仍 ≤ max
- `BudgetTracker` 必须累计 report 的 tokens / cost / steps / model_call 用量
- `BudgetTracker.check()` 任一维度超限返回 `Some(BudgetExceededReason)`，未超返回 None
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- compute_delay 用 ±25% jitter（rand 区间 [0.75, 1.25]），cap_delay 二次封顶防超 max
- BudgetTracker 多维度限额（tokens/cost/steps/duration/model_calls），check 返回首个超限原因
- RetryPolicy 默认 backoff_factor 2.0、max_interval 10s、jitter true（builder.rs:153）

## 边界

### Allowed Changes
- specs/pregel-budget-retry-conformance.spec.md
- 本合约为只读核实，不改实现代码（budget.rs / builder.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `budget.rs` / `builder.rs` 中 BudgetTracker / compute_delay / RetryPolicy 的实现逻辑
- 不得改动 `crates/juncture-derive/**`

## 完成条件

### Rule: retry-backoff — 退避与抖动数学

场景: 无 jitter 时延迟等于 base（封顶后）
  测试:
    包: juncture-core
    过滤: test_compute_delay_no_jitter
  当 compute_delay(base, jitter=false, max) 且 base ≤ max
  那么 返回 base（封顶逻辑不影响未超值）

场景: 延迟封顶于 max_interval
  测试:
    包: juncture-core
    过滤: test_compute_delay_caps_at_max
  当 base > max
  那么 返回 max（不得无限增长）

场景: jitter 限制在 ±25% 区间
  标签: critical
  测试:
    包: juncture-core
    过滤: test_compute_delay_with_jitter_stays_within_range
  当 compute_delay(base, jitter=true, max) 多次采样
  那么 所有结果落在 [0.75, 1.25] × capped 区间内（防 thundering herd 的抖动边界）

场景: jitter 后仍封顶于 max
  测试:
    包: juncture-core
    过滤: test_compute_delay_jitter_capped_by_max
  当 jitter 放大后的值超过 max
  那么 二次封顶回 max（jitter 不得突破上限）

### Rule: budget-enforcement — 预算累计与超限检测

场景: 累计 token 用量
  测试:
    包: juncture-core
    过滤: test_budget_tracker_tokens
  当 多次 report_tokens
  那么 current_usage 的 token 总量正确累计

场景: 累计成本用量
  测试:
    包: juncture-core
    过滤: test_budget_tracker_cost
  当 多次 report_cost
  那么 current_usage 的 cost 正确累计

场景: 累计 step 用量
  测试:
    包: juncture-core
    过滤: test_budget_tracker_steps
  当 多次 report_step
  那么 current_usage 的 step 计数正确

场景: 超限时 check 返回原因
  标签: critical
  测试:
    包: juncture-core
    过滤: test_budget_tracker_model_call_exceeds_limit
  当 model_call 累计超过配置上限
  那么 check() 返回 Some(BudgetExceededReason)（未超时为 None）

## Questions

- [ ] **退避算法双实现**：`compute_delay`（builder.rs:708，RetryPolicy 路径）与 `task_compute_delay`（func/mod.rs:491，`#[task]` 宏路径）是两份独立的 ±25% jitter + cap 实现。语义相同但代码重复，需决策是否合并为单一 source of truth。（按用户指示推迟到试点结束后统一决策 2026-07-29）
