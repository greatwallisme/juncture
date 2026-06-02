# Task Plan: Telemetry 重构 - 对标 Langfuse/LangSmith

## Goal
将 Juncture 的 telemetry 从"需要部署 3 个外部服务 + 数据不丰富 + 查看体验差"改造为"零部署、数据丰富、查看体验优秀"的 LLM-native 可观测性系统，对标 Langfuse/LangSmith 的能力。

## 用户痛点 (2026-06-01)
1. **部署复杂**: 需要同时部署 otel-collector、jaeger、prometheus 三个服务
2. **数据不丰富**: 缺少 LLM prompt/response 内容、缺少 session 概念、缺少 user tracking
3. **人机工程差**: Jaeger/Prometheus UI 面向基础设施监控，不适合 AI agent 调试

## 对标分析

### Langfuse 核心特性 (开源, 可自托管)
- **数据模型**: Trace → Observation (Generation/ToolCall/Retrieval/Span) 三层嵌套
- **Session**: 多轮对话聚合
- **User/Tags/Metadata/Environment**: 丰富的上下文标注
- **LLM-native**: 原生理解 token usage, model params, prompt/completion, cost
- **异步批量**: 后台批量发送，零延迟影响
- **OTel 兼容**: 基于 OpenTelemetry 标准

### LangSmith 核心特性 (商业)
- **Agent-native**: 深度集成 LangGraph，理解 agent 决策流程
- **分布式追踪**: 跨服务、跨 agent 的完整调用链
- **调试工具**: 长 trace 分析、prompt 优化、实验对比

### Juncture 现状
- OTel span 层次结构已定义 (graph → superstep → node → llm/tool)
- Metrics 已定义 (counter/histogram/gauge)
- 缺少: prompt/response 捕获、session、user tracking、内嵌 viewer
- 部署: 需要 otel-collector + jaeger + prometheus

## Phase 1: 数据模型 + SQLite 存储 + Collector [status: complete]
- [x] 1.1 创建 juncture-telemetry crate (Cargo.toml, lib.rs)
- [x] 1.2 定义 Langfuse-compatible 数据模型 (models.rs: Trace, Observation, Session, TokenUsage, CaptureConfig)
- [x] 1.3 实现 TraceStore trait (trace_store.rs)
- [x] 1.4 实现 SqliteStore (sqlite_store.rs, sqlx)
- [x] 1.5 实现 BatchWriter (batch_writer.rs, FK 排序 flush)
- [x] 1.6 实现 TelemetryCollector (collector.rs)
- [x] 1.7 29 个测试全部通过
- [x] 1.8 clippy 零错误零警告

## Phase 2: 内嵌存储层 [status: complete]
- [x] 2.1 TraceStore trait (trace_store.rs)
- [x] 2.2 SqliteStore (sqlite_store.rs, sqlx)
- [x] 2.3 BatchWriter (batch_writer.rs, FK 排序 flush)
- [x] 2.4 TelemetryCollector (collector.rs)

## Phase 3: Langfuse API + Web Server + Dashboard [status: complete]
- [x] 3.0 创建 web 模块结构 (mod.rs, api.rs, dashboard.rs)
- [x] 3.1 Langfuse ingestion API (POST /api/public/ingestion)
- [x] 3.2 traces 查询 API (GET /api/public/traces, GET /api/public/traces/:id)
- [x] 3.3 sessions 查询 API (GET /api/public/sessions, GET /api/public/sessions/:id)
- [x] 3.4 daily stats API (GET /api/public/stats/daily)
- [x] 3.5 嵌入式 Dashboard UI (SPA, 暗色主题, trace tree, session timeline)
- [x] 3.6 WebServer 入口 (start/stop, graceful shutdown)
- [x] 3.7 测试 + clippy 零错误零警告

## Phase 4: OTel 兼容层 [status: complete]
- [x] 4.1 实现 OTLP HTTP ingest (POST /v1/traces, JSON 格式)
- [x] 4.2 OTLP span → Trace/Observation 转换 (支持 gen_ai.* 和 juncture.* 属性)
- [x] 4.3 集成到 WebServer router
- [x] 4.4 测试 + clippy 零错误零警告

## Phase 5: 集成与文档 [status: complete]
- [x] 5.1 更新 design/09-observability.md (新增 Section 11: juncture-telemetry)
- [x] 5.2 更新 juncture-telemetry/CLAUDE.md
- [x] 5.3 最终验证 (workspace test 56 passed, clippy zero errors)

## Decisions Log
| Decision | Rationale | Date |
|----------|-----------|------|
| 用户确认: longfuse = Langfuse, longsmith = LangSmith | 搜索结果确认 | 2026-06-01 |
| 存储: SQLite + 可选 PostgreSQL | feature gate 切换，SQLite 零依赖默认 | 2026-06-01 |
| Viewer: 兼容 Langfuse API | 复用 Langfuse 前端 UI，最强大 | 2026-06-01 |
| OTel: OTLP ingest (不仅导出，还能接收) | 统一数据入口，支持混合场景 | 2026-06-01 |
| 捕获: 完整 prompt/response + 可配置截断 | 开发全量，生产可限制 | 2026-06-01 |
