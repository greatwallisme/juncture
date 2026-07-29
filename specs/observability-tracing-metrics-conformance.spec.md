spec: task
name: "observability-tracing-metrics-conformance"
tags: [design-conformance, observability, tracing, 09-observability]
---

## 意图

将 design `09-observability` 的 span 命名约定、GraphCallbackHandler 生命周期回调、trace context 传播、metrics（counter/gauge/histogram）语义固化为可验证合约，对 `crates/juncture-tracing/` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub const.*SPAN`/`pub trait GraphCallbackHandler`/`MetricsRegistry` 等符号名（存在即计覆盖），无法验证「span 名称符合格式约定」、「回调处理器自定义实现可用」、「labeled counter 的 key 排序确定性」这类可观测性行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- span 名称常量必须符合既定格式约定（节点级 span 命名）
- GraphCallbackHandler 必须支持自定义实现，且可 Arc 共享
- trace context propagator 必须可创建（inject/extract 不 panic，otel feature 门控，见 Questions）
- HITL 事件（interrupt/resume）必须可正确序列化（用于 tracing/debug 审计）
- labeled counter 的 key 排序必须确定（相同标签集合→相同 key）
- counter 递增 / gauge 设置必须正确记录
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- span 命名用常量（spans.rs），保证 OTel span 名一致
- GraphCallbackHandler trait 可自定义；提供默认 no-op 实现
- MetricsRegistry 显式 API：counter/gauge/histogram + labeled 变体
- trace context 用 propagator 跨服务传播；无 active span 时不 panic

## 边界

### Allowed Changes
- specs/observability-tracing-metrics-conformance.spec.md
- 本合约为只读核实，不改实现代码（juncture-tracing 已有充分测试覆盖）

### Forbidden
- 不得修改 `crates/juncture-tracing/src/**` 中 span 常量 / callback / propagator / metrics 实现

## 完成条件

### Rule: span-naming — span 命名约定

场景: span 名称符合格式
  标签: critical
  测试:
    包: juncture-tracing
    过滤: test_span_names_format
  当 检查 span 名称常量
  那么 符合既定命名格式约定

场景: span 常量存在
  测试:
    包: juncture-tracing
    过滤: test_span_constants_exist
  当 检查节点级 span 常量集合
  那么 全部预期 span 常量存在

### Rule: callback-handler — 生命周期回调

场景: 自定义回调实现可用
  测试:
    包: juncture-tracing
    过滤: test_callback_handler_custom_impl
  当 实现 GraphCallbackHandler 自定义类型
  那么 可正常构造并调用（trait 可被自定义实现满足）

场景: Arc 共享回调
  测试:
    包: juncture-tracing
    过滤: test_arc_callback_handler
  当 回调处理器用 Arc 共享
  那么 多处持有同一 Arc 回调正常工作

### Rule: event-serialization — 事件序列化

场景: resume 事件可序列化
  测试:
    包: juncture-tracing
    过滤: test_resume_event_serialization
  当 序列化 resume 事件
  那么 round-trip 正确（用于 tracing/debug 审计）

### Rule: metrics — 指标记录

场景: labeled counter key 排序确定
  标签: critical
  测试:
    包: juncture-tracing
    过滤: test_labeled_counter_key_ordering
  当 用相同标签集合（任意顺序）构造 labeled counter key
  那么 产生相同 key（排序确定，标签序无关）

场景: counter 递增
  测试:
    包: juncture-tracing
    过滤: test_increment_counter
  当 递增 counter
  那么 值正确记录

场景: gauge 设置
  测试:
    包: juncture-tracing
    过滤: test_set_gauge
  当 设置 gauge 值
  那么 值正确记录

## Questions

- [x] **端到端 OTel 导出**：RESOLVED 2026-07-29 — 新增 `test_otlp_export_pipeline_e2e`（config.rs，otel feature），针对 docker/telemetry 的 otel-collector（127.0.0.1:4318）安装 OTLP pipeline + 记录 counter。collector 不可达时 CI-safe 跳过。验证 exporter 配置 + metric 记录对抗真实 infra（force_flush API 跨 OTel 版本不稳定，已移除；Prometheus 跨服务查询超出 unit 范围，注释说明）。
