# Findings: Telemetry 重构研究

## 1. 当前 Juncture Telemetry 架构分析

### 1.1 代码结构 (crates/juncture-tracing/)
- `config.rs` - TracingConfig builder, OTLP setup (HTTP transport, gRPC 有 bug)
- `spans.rs` - 7 个 span name 常量 + 24 个 attribute key 常量
- `metrics.rs` - MetricsRegistry (counter/histogram/gauge), OTel + in-memory 双模式
- `callback.rs` - GraphCallbackHandler trait (7 个生命周期事件)
- `propagation.rs` - Trace context 传播
- `test_utils.rs` - TestMetricsCollector

### 1.2 关键限制
- **Span 数据有限**: 只有 name/duration/error，缺少 input/output 内容
- **无 Session 概念**: 每次 graph.invoke 是独立的，无法关联多轮对话
- **无 User Tracking**: 不知道哪个用户触发的
- **无 Tags/Metadata**: 无法灵活分类和过滤
- **Metrics 无 Labels**: OTel counter/histogram 没有附加维度标签
- **部署依赖重**: 需要 otel-collector + jaeger + prometheus

### 1.3 OTel 集成问题
- gRPC (tonic) exporter 在 HTTP/2 握手时挂起 → 已切换到 HTTP transport
- 需要手动 flush metrics → 短生命周期应用数据丢失
- 无本地存储 → 必须有外部 collector 才能看到数据

## 2. Langfuse 分析

### 2.1 数据模型 (3 层)
```
Trace (顶层容器)
├── user_id, session_id, tags, metadata, environment, release
├── Observation: Generation (LLM call)
│   ├── model, model_parameters
│   ├── input (prompt), output (completion)
│   ├── usage: {input_tokens, output_tokens, total_tokens, total_cost}
│   ├── metadata, level, status_message
│   └── nested observations
├── Observation: ToolCall
│   ├── name, input, output
│   └── metadata
├── Observation: Retrieval (RAG)
│   ├── query, documents
│   └── metadata
└── Observation: Span (generic)
    ├── name, input, output
    ├── start_time, end_time
    └── metadata
```

### 2.2 核心特性
- **异步批量发送**: 零延迟影响，后台 flush
- **Session 聚合**: 多轮对话可视化
- **User 追踪**: 按用户分析成本/质量
- **Tags/Environment**: 灵活分类 (production/staging/feature-x)
- **成本追踪**: 按 model/user/session 统计
- **Prompt 管理**: 版本化 prompt，A/B 测试
- **Evaluation**: LLM-as-a-Judge 评估
- **Dashboard**: 内置可视化，无需 Grafana

### 2.3 部署模式
- **自托管**: Docker Compose (Langfuse + ClickHouse/PostgreSQL)
- **单进程**: Next.js app + DB，比 otel+jaeger+prometheus 简单得多
- **OTel 兼容**: 可作为 OTLP 后端接收数据

## 3. LangSmith 分析

### 3.1 核心特性
- **Agent-native**: 深度理解 LangGraph 执行流程
- **Trace Tree**: 树形视图展示 agent 决策链
- **长 Trace 分析**: AI 辅助分析复杂 trace
- **Prompt Playground**: 在线调试 prompt
- **实验对比**: A/B 测试不同配置
- **成本监控**: 实时成本仪表盘

### 3.2 与 Langfuse 的区别
- LangSmith 是商业 SaaS，Langfuse 开源可自托管
- LangSmith 对 LangChain/LangGraph 集成更深
- Langfuse 的 OTel 兼容性更好
- LangSmith 的调试工具更强大 (AI 辅助分析)

## 4. 设计方向建议

### 4.1 推荐方案: 内嵌 Langfuse-compatible 系统
**理由**:
- 用户明确要求不部署外部服务
- Langfuse 是开源的，数据模型成熟
- 内嵌 viewer 提供最佳人机工程
- 保留 OTel export 兼容性

### 4.2 技术选型
- **存储**: SQLite (单机) + 可选 PostgreSQL (生产)
- **Web Server**: axum (内嵌)
- **前端**: 内嵌 HTML/JS (单文件或 WASM)
- **协议**: 兼容 Langfuse API (可选直接对接 Langfuse UI)

### 4.3 实施优先级
1. **P0**: 数据模型 + 内嵌存储 + 基础 viewer
2. **P1**: LLM prompt/response 捕获 + cost tracking
3. **P2**: Session + User + Tags 支持
4. **P3**: OTel 双写 + Langfuse API 兼容
