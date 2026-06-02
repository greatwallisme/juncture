# Juncture Telemetry

[![Crates.io](https://img.shields.io/crates/v/juncture-telemetry.svg)](https://crates.io/crates/juncture-telemetry)
[![Documentation](https://docs.rs/juncture-telemetry/badge.svg)](https://docs.rs/juncture-telemetry)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Langfuse-compatible observability engine for Juncture AI agents. Provides embedded dashboard, cloud export, and OTLP ingest.

## Features

- **Embedded Dashboard**: Dark-theme SPA with trace trees, charts, and real-time updates
- **Langfuse Cloud Export**: Push traces to Langfuse cloud for team collaboration
- **OTLP Ingest**: Receive OpenTelemetry spans via HTTP
- **SQLite/PostgreSQL Storage**: Persistent trace storage
- **Multi-Agent Tracing**: Nested observation trees for complex agent workflows

## Quick Start

```rust
use juncture_telemetry::init;

// Minimal - in-memory SQLite, no export
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

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Dashboard UI |
| `POST` | `/api/public/ingestion` | Langfuse SDK batch ingest |
| `GET` | `/api/public/traces` | Query traces |
| `GET` | `/api/public/sessions` | Query sessions |
| `GET` | `/api/public/stats/summary` | Overall statistics |

## Feature Flags

| Feature | Description |
|---------|-------------|
| `sqlite` | SQLite storage (default) |
| `postgres` | PostgreSQL storage |
| `web` | Web server + dashboard + OTLP ingest |
| `otlp-grpc` | OTLP gRPC ingest |

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
