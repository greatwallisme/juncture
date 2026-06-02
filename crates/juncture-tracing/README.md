# Juncture Tracing

[![Crates.io](https://img.shields.io/crates/v/juncture-tracing.svg)](https://crates.io/crates/juncture-tracing)
[![Documentation](https://docs.rs/juncture-tracing/badge.svg)](https://docs.rs/juncture-tracing)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

OpenTelemetry integration and structured tracing for Juncture graph execution.

## Features

- **Structured Spacing**: Consistent span naming (`juncture.{component}.{action}`)
- **OpenTelemetry Export**: OTLP export for distributed tracing (feature `otel`)
- **Metrics Collection**: OpenTelemetry metrics for graph execution
- **Graph Callbacks**: Event handlers for graph lifecycle

## Usage

### Basic Tracing (No OTel)

```rust
use juncture_tracing::init_tracing;

// Initialize tracing subscriber with env filter
init_tracing();
```

### OpenTelemetry Integration

```rust
use juncture_tracing::init;

// Configure and install OTel tracing
init()
    .with_service_name("my-juncture-app")
    .install()
    .await?;
```

## Span Hierarchy

```
juncture.graph.invoke
  juncture.superstep
    juncture.node.execute
      juncture.llm.call
      juncture.tool.call
      juncture.checkpoint.put
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `otel` | OpenTelemetry OTLP export |

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
