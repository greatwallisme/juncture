//! Langfuse-compatible observability engine for Juncture AI agents.
//!
//! This crate provides a zero-dependency telemetry system that captures
//! traces, observations (LLM calls, tool calls, spans), sessions, and
//! metrics directly within the Juncture process. No external services
//! (otel-collector, Jaeger, Prometheus) are required.
//!
//! # Architecture
//!
//! ```text
//! TelemetryCollector
//!   └── BatchWriter (async, non-blocking)
//!         └── TraceStore (SQLite / PostgreSQL)
//!               └── Web Viewer (Langfuse-compatible API)
//! ```
//!
//! # Quick Start
//!
//! ```ignore
//! use juncture_telemetry::{TelemetryCollector, SqliteStore};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = Arc::new(SqliteStore::new("telemetry.db").await?);
//!     let collector = TelemetryCollector::new(store);
//!
//!     // ... run your graph with collector ...
//!
//!     collector.flush().await?;
//!     Ok(())
//! }
//! ```

pub mod batch_writer;
pub mod collector;
pub mod config;
pub mod langfuse;
pub mod models;
#[cfg(feature = "web")]
pub mod otlp;
#[cfg(feature = "sqlite")]
pub mod sqlite_store;
pub mod trace_store;
#[cfg(feature = "web")]
pub mod web;

// Re-exports for convenience
pub use batch_writer::{BatchWriter, TelemetryItem};
pub use collector::TelemetryCollector;
pub use config::{TelemetryConfig, TelemetryHandle};
pub use langfuse::{LangfuseConfig, LangfuseExporter};

/// Create a new telemetry configuration builder.
///
/// This is the recommended entry point for setting up telemetry.
/// It follows the same pattern as `juncture_tracing::init()`.
///
/// # Examples
///
/// ```ignore
/// use juncture_telemetry::init;
///
/// // Minimal -- in-memory store, no export
/// let telemetry = init().await?;
///
/// // Full setup
/// let telemetry = init()
///     .with_store("telemetry.db")
///     .with_langfuse_from_env()
///     .with_dashboard(8123)
///     .await?;
/// ```
#[must_use]
pub fn init() -> TelemetryConfig {
    TelemetryConfig::new()
}
pub use models::{
    CaptureConfig, EnrichedSession, Id, ModelStats, Observation, ObservationLevel, ObservationType,
    Session, SummaryStats, TokenUsage, Trace,
};
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;
pub use trace_store::{
    DailyStats, PaginatedResponse, StoreError, TraceQuery, TraceStore, TraceWithObservations,
};

// Rust guideline compliant 2026-06-01
