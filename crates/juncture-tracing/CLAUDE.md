# CLAUDE.md -- juncture-tracing

OpenTelemetry integration and structured tracing for Juncture graph execution.

## Structure

```
src/
  lib.rs          -- crate root, re-exports
  spans.rs        -- Span name constants and attribute key constants
  callback.rs     -- GraphCallbackHandler, GraphInterruptEvent, GraphResumeEvent
  test_utils.rs   -- TestMetricsCollector for asserting metrics in tests (gated by feature `test-util` / `cfg(test)`)
  types.rs        -- LlmCacheKeyInput, LlmCachePolicy, ServerInfo
  config.rs       -- TracingConfig, init() builder for OTLP setup (feature `otel`)
  metrics.rs      -- MetricsRegistry for OpenTelemetry metrics (feature `otel`)
  propagation.rs  -- Trace context propagation helpers for cross-service span linking
```

## Span Naming Convention

All spans follow `juncture.{component}.{action}`:
- `juncture.graph.invoke` / `juncture.graph.complete`
- `juncture.superstep`
- `juncture.node.execute`
- `juncture.llm.call`
- `juncture.tool.call`
- `juncture.checkpoint.put`

## Usage

Basic (no OTel): call `init_tracing()` to set up `tracing-subscriber` with env filter.
With OTel: use `init().with_service_name("...").install()` (requires `otel` feature).

## Features

- `otel` -- OpenTelemetry OTLP export (config, metrics modules)
- `test-util` -- (default-off) test-only utilities such as `TestMetricsCollector`; the `test_utils` module and `pub use test_utils::TestMetricsCollector` are gated by `#[cfg(any(test, feature = "test-util"))]` so test doubles never ship in release builds

## Testing

```bash
cargo test -p juncture-tracing
cargo test -p juncture-tracing --features otel
```
