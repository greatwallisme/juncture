spec: task
name: "llm-tools-interceptor-error-conformance"
tags: [design-conformance, llm-tools, tool, interceptor, 08-llm-tools]
---

## 意图

将 design `08-llm-tools` §5（ToolInterceptor）与 ToolNode 事件发射/错误处理语义固化为可验证合约，对 `crates/juncture/src/tools/` 做**语义级**核对。

`verify-design-coverage.py` 仅 grep `pub trait ToolInterceptor`/`pub struct ToolNode` 等符号名（存在即计覆盖），无法验证「拦截器组合（composite）的阻塞传播」、「tool 失败时走 error handling 分支」、「ToolNode 按 started→finished 顺序发射事件」这类行为。本合约绑定既有测试把这类 grep 盲区纳入 lifecycle 强制。

## 约束

<!-- lint-ack: coverage — agent-spec coverage linter 分词器不切中文全角标点，对中文约束的"场景覆盖"机械低估；每条约束均由下方绑定测试覆盖，以 lifecycle 为权威证据。 -->

- ToolNode 执行必须按顺序发射 started 与 finished 事件
- tool 执行失败 / tool 未找到时必须走 error handling 分支（不 panic）
- ToolInterceptor 组合（composite）必须按序应用，任一阻塞则整体阻塞
- tools_condition 必须按 config 路由到 tools 节点
- 不得为通过 lifecycle 而修改实现或放松测试

## 已定决策

- ToolInterceptor 支持 pre/post 钩子；composite 按注册序组合，阻塞型拦截器可中止
- ToolNode 失败路径：有 error handling 配置时返回结构化结果，否则传播错误
- 事件发射：tool_started → tool_finished（成功）或带错误的 finished

## 边界

### Allowed Changes
- specs/llm-tools-interceptor-error-conformance.spec.md
- 本合约为只读核实，不改实现代码（facade tools 已有充分测试覆盖）

### Forbidden
- 不得修改 `crates/juncture/src/tools/**` 中 ToolNode / ToolInterceptor / tools_condition 实现逻辑

## 完成条件

### Rule: tool-event-ordering — 事件顺序

场景: ToolNode 按序发射 started 与 finished
  标签: critical
  测试:
    包: juncture
    过滤: test_tool_node_emits_events_in_order
  当 ToolNode 执行一个 tool 调用
  那么 先发射 started 事件、后发射 finished 事件（顺序正确）

### Rule: tool-error-handling — 失败与未找到

场景: tool 执行失败走 error handling
  测试:
    包: juncture
    过滤: test_tool_node_tool_failure_with_error_handling
  当 tool 执行返回错误且有 error handling 配置
  那么 走 error handling 分支（不 panic）

场景: tool 未找到走 error handling
  测试:
    包: juncture
    过滤: test_tool_node_tool_not_found_with_error_handling
  当 请求的 tool 不存在且有 error handling 配置
  那么 走 error handling 分支（不 panic）

### Rule: interceptor-composition — 拦截器组合

场景: 阻塞型拦截器中止
  测试:
    包: juncture
    过滤: test_blocking_interceptor
  当 阻塞型（blocking）拦截器拦截
  那么 tool 调用被中止

场景: 组合拦截器按序应用
  测试:
    包: juncture
    过滤: test_composite_interceptor
  当 多个拦截器组合（composite）
  那么 按注册顺序依次应用

场景: 组合拦截器中阻塞传播
  标签: critical
  测试:
    包: juncture
    过滤: test_composite_interceptor_blocking
  当 组合中含阻塞型拦截器
  那么 整体表现为阻塞（传播）

### Rule: tools-condition-routing — 条件路由

场景: tools_condition 按配置路由
  测试:
    包: juncture
    过滤: test_tool_node_tools_condition_with_config
  当 带 tools_condition 配置
  那么 按配置正确路由到 tools 节点

场景: ToolNode 拦截器 pre→tool→post 严格时序
  测试:
    包: juncture
    过滤: test_tool_node_interceptor_pre_post_timing_around_execution
  当 ToolNode 配 RecordingTool + RecordingInterceptor 执行一次 tool 调用
  那么 调用顺序严格为 [pre, tool, post]

## Questions

- [x] **ToolInterceptor pre/post 钩子时序**：RESOLVED 2026-07-29 — 新增 `test_tool_node_interceptor_pre_post_timing_around_execution`（node.rs），用 RecordingTool（记录 tool 体执行）+ RecordingInterceptor（记录 pre/post）驱动真实 ToolNode，断言执行顺序严格为 [pre, tool, post]。原 interceptor 测试仅在隔离态调 pre/post，本测试 pin 住 ToolNode 的实际时序契约。
