# CLAUDE.md -- juncture-telemetry

Langfuse-compatible observability engine for Juncture AI agents.

## Structure

```
src/
  lib.rs              -- crate root, re-exports, init() entry point
  config.rs           -- TelemetryConfig builder, TelemetryHandle (RAII flush)
  models.rs           -- Trace, Observation, Session, TokenUsage, CaptureConfig,
                         ModelStats, SummaryStats, EnrichedSession
  trace_store.rs      -- TraceStore trait, query types, PaginatedResponse
  sqlite_store.rs     -- SQLite implementation (sqlx, feature = "sqlite")
  batch_writer.rs     -- Async batch writer with FK-ordered flush + Langfuse export
  collector.rs        -- TelemetryCollector (observation lifecycle)
  langfuse.rs         -- LangfuseExporter (cloud push via REST API)
  web/
    mod.rs            -- WebServer (start/stop, graceful shutdown, with_bind_addr)
    api.rs            -- Langfuse-compatible REST API handlers
    dashboard.rs      -- Embedded SPA dashboard (dark theme, trace tree, charts)
  otlp/
    mod.rs            -- OTLP span → Trace/Observation conversion
    http.rs           -- OTLP HTTP ingest handler (POST /v1/traces)
```

## Features

- `sqlite` (default) -- SQLite storage via sqlx
- `postgres` -- PostgreSQL storage via sqlx
- `web` -- axum web server + Dashboard + OTLP ingest

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Dashboard UI (SPA) |
| `POST` | `/api/public/ingestion` | Langfuse SDK batch ingest |
| `GET` | `/api/public/traces` | Query traces (paginated/filtered, name uses LIKE) |
| `GET` | `/api/public/traces/:id` | Get trace + observations |
| `GET` | `/api/public/sessions` | Query sessions |
| `GET` | `/api/public/sessions/:id` | Get session |
| `GET` | `/api/public/sessions/enriched` | Sessions with aggregated stats |
| `GET` | `/api/public/stats/daily` | Daily aggregated stats |
| `GET` | `/api/public/stats/models` | Per-model aggregated stats |
| `GET` | `/api/public/stats/summary` | Overall summary + latency percentiles |
| `POST` | `/v1/traces` | OTLP JSON ingest |

## Dashboard Pages

- **Dashboard**: 6 stat cards, traces-over-time chart, model cost bars, latency percentiles, token usage chart
- **Traces**: Name/User/Date filters, enhanced table with token flow notation
- **Trace Detail**: Two-panel layout (30% tree / 70% detail), observation search, type filters (All/Gen/Tool/Span), tabbed detail (Overview/Input/Output)
- **Sessions**: Enriched cards with trace count, cost, tokens, last active

## Data Model

Langfuse-compatible three-level hierarchy:
- **Trace**: top-level container (graph invocation), has session_id, user_id, tags
- **Observation**: nested work unit (Span/Generation/ToolCall/Retrieval), parent_observation_id for tree
- **Session**: groups traces by thread_id

## Quick Start (Recommended)

```rust
use juncture_telemetry::init;

// Minimal -- in-memory SQLite, no export
let telemetry = init().install().await?;

// File persistence + Langfuse cloud + dashboard
let telemetry = init()
    .with_store("telemetry.db")
    .with_langfuse_from_env()  // reads LANGFUSE_* from .env
    .with_dashboard(8123)
    .with_bind_addr([0, 0, 0, 0])  // public access
    .install()
    .await?;

let collector = telemetry.collector();
// ... run your agent with collector ...
telemetry.shutdown().await?;  // flush + stop dashboard
```

`TelemetryHandle` auto-flushes on drop and handles ctrl-c signals.

## Legacy API (Manual Setup)

```rust
use juncture_telemetry::{TelemetryCollector, SqliteStore, web::WebServer};

let store = Arc::new(SqliteStore::new("telemetry.db").await?);
let collector = TelemetryCollector::with_capture_config(store.clone(), config);
let server = WebServer::new(store, 8123).start().await?;
```

## Langfuse SDK Integration

Point Langfuse SDK at the embedded server:

```env
LANGFUSE_SECRET_KEY=sk-lf-...
LANGFUSE_PUBLIC_KEY=pk-lf-...
LANGFUSE_HOST=http://127.0.0.1:8123
```

When `with_auth` is used, the server validates Basic Auth headers.
Without `with_auth`, all requests are accepted (no auth required).

## Multi-Agent Tracing

Use `parent_observation_id` to build nested observation trees:

```rust
let agent_span = collector.begin_span(trace_id, None, "agent_name");
let llm_obs = collector.begin_llm_call(trace_id, Some(agent_span.id), model, prompt);
// ... LLM call ...
collector.end_llm_call(llm_obs, response, usage, cost).await?;
collector.end_span(agent_span, None).await?;
```

## Key Design Decisions

- Batch writer uses FK-ordered flush: sessions -> traces -> observations
- `new_memory()` uses transient temp files with RAII cleanup (TransientDbGuard)
- `CaptureConfig` controls prompt/response truncation and sensitive field filtering
- OTLP ingest supports both `gen_ai.*` and `juncture.*` attribute conventions
- Name filter uses `LIKE` with wildcards for partial matching
- Type filter uses case-insensitive comparison (`.toLowerCase()`)

## Testing

```bash
cargo test -p juncture-telemetry
cargo test -p juncture-telemetry --features "web,sqlite"
```
