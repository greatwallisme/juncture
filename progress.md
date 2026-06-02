# Progress: Telemetry 重构

## Session 2026-06-01

### Phase 1: 数据模型 + SQLite 存储 + Collector
- [x] 分析当前 juncture-tracing 架构
- [x] 研究 Langfuse/LangSmith 数据模型和核心特性
- [x] 读取 design/09-observability.md 设计文档
- [x] 设计方案确认 (存储: SQLite+PG, Viewer: Langfuse API, OTel: ingest, Capture: 完整+截断)
- [x] 创建 juncture-telemetry crate
- [x] 实现数据模型 (Trace, Observation, Session, TokenUsage, CaptureConfig)
- [x] 实现 TraceStore trait + SqliteStore (sqlx)
- [x] 实现 BatchWriter (异步批量写入，FK 排序)
- [x] 实现 TelemetryCollector (主入口 API)
- [x] 29 个测试全部通过
- [ ] 修复 clippy 错误 (agent 处理中)

### Phase 2: Langfuse API + Web Server + Dashboard [complete]
- [x] 创建 web 模块结构 (mod.rs, api.rs, dashboard.rs)
- [x] 实现 Langfuse ingestion API (POST /api/public/ingestion)
- [x] 实现 traces 查询 API (GET /api/public/traces, GET /api/public/traces/:id)
- [x] 实现 sessions 查询 API (GET /api/public/sessions, GET /api/public/sessions/:id)
- [x] 实现 daily stats API (GET /api/public/stats/daily)
- [x] 嵌入式 Dashboard UI (SPA, 暗色主题, trace tree, session timeline)
- [x] WebServer 入口 (start/stop, graceful shutdown)
- [x] 31 个测试全部通过, clippy 零错误零警告

### Phase 3-8: 待做
