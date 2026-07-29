spec: task
name: "streaming-emission-batching-conformance"
tags: [design-conformance, streaming, 05-streaming]
---

## 意图

将 design `05-streaming` 的事件发射门控（StreamMode + nostream tag 抑制）与消息批处理（Batch transformer）语义固化为可验证合约，对 `crates/juncture-core/src/stream.rs` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub enum StreamMode`/`fn should_emit`/`Batch` 等符号名（存在即计覆盖），无法验证「nostream tag 抑制 messages 事件但 end 事件始终发射」、「批量阈值达上限才发射、size=0 钳位为 1」这类发射/批处理行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- 带 nostream tag 的 messages 事件必须被抑制；end 事件在 messages 模式始终发射
- tools 模式必须发射 tools 事件（tool_output_delta / tool_finished）
- Batch transformer 达到 max_size 才发射；低于阈值返回 None
- Batch size = 0 必须钳位为 1（不得零批）
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- should_emit 按 (StreamMode, tags) 门控：nostream tag 抑制 messages，但不抑制 end
- Batch transformer 累积消息，达 max_size 发射一批；clamp size 到 ≥1
- StreamResumption.should_skip 控制断点续传跳过已执行 step

## 边界

### Allowed Changes
- specs/streaming-emission-batching-conformance.spec.md
- 本合约为只读核实，不改实现代码（stream.rs 已有充分测试覆盖）

### Forbidden
- 不得修改 `juncture-core/src/stream.rs` 中 should_emit / Batch / StreamResumption 实现逻辑

## 完成条件

### Rule: emission-gating — 事件发射门控

场景: nostream 抑制 messages 事件
  标签: critical
  测试:
    包: juncture-core
    过滤: should_emit_messages_event_with_nostream_suppressed
  当 messages 事件带 nostream tag
  那么 should_emit 返回 false（被抑制）

场景: end 事件在 messages 模式始终发射
  测试:
    包: juncture-core
    过滤: should_emit_end_event_always_in_messages_mode
  当 messages 模式下的 end 事件
  那么 should_emit 返回 true（不被 nostream 抑制）

场景: tools 模式发射 tools 事件
  测试:
    包: juncture-core
    过滤: should_emit_tools_event_in_tools_mode
  当 tools 模式下的 tools 事件
  那么 should_emit 返回 true

### Rule: batch-transformer — 消息批处理

场景: 达 max_size 发射一批
  标签: critical
  测试:
    包: juncture-core
    过滤: batch_transformer_emits_batch_when_max_size_reached
  当 Batch 累积达到 max_size
  那么 发射一批消息

场景: 低于阈值不发射
  测试:
    包: juncture-core
    过滤: batch_transformer_returns_none_below_threshold
  当 Batch 累积低于 max_size
  那么 返回 None（未达阈值）

场景: size=0 钳位为 1
  测试:
    包: juncture-core
    过滤: batch_transformer_size_zero_clamped_to_one
  当 Batch 配置 size=0
  那么 钳位为 1（不得零批，每条立即发射）

### Rule: stream-resumption — 断点续传

场景: step 超过 last_step 不跳过
  测试:
    包: juncture-core
    过滤: resumption_should_skip_returns_false_when_step_after_last_step
  当 当前 step 超过 last_step
  那么 should_skip 返回 false（继续执行）

## Questions

- [ ] **9 种 StreamMode 端到端覆盖**：本合约验证 should_emit 门控与 Batch；9 种模式（Values/Updates/Messages/Custom/Debug/Tools/Checkpoints/Tasks/Multi）的端到端流输出需集成测试。（按用户指示推迟到试点结束后统一决策 2026-07-29）
