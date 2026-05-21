//! OpenTelemetry integration and tracing for Juncture applications
//!
//! This crate provides instrumentation capabilities for Juncture graph execution,
//! including structured logging, span management, and metrics collection.
//!
//! # Feature Flags
//!
//! - `otel` (default: off) - Enable OpenTelemetry trace/metrics export configuration
//!
//! # Basic Usage
//!
//! Initialize tracing for your Juncture application:
//!
//! ```no_run
//! use juncture_tracing::init_tracing;
//!
//! let _ = init_tracing();
//! // Your application code here
//! ```
//!
//! # With OpenTelemetry
//!
//! When the `otel` feature is enabled, you can configure OTLP export:
//!
//! ```no_run
//! use juncture_tracing::{init, config::TracingConfig};
//! use tracing::Level;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! init()
//!     .with_service_name("my-agent-service")
//!     .with_log_level(Level::INFO)
//!     .install()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Span Constants
//!
//! Use the provided span and attribute constants for consistency:
//!
//! ```
//! use juncture_tracing::{spans::names, spans::attrs};
//!
//! assert_eq!(names::GRAPH_INVOKE, "juncture.graph.invoke");
//! assert_eq!(attrs::NODE_NAME, "juncture.node.name");
//! ```

pub mod callback;
pub mod debug;
pub mod spans;
pub mod test_utils;
pub mod types;

#[cfg(feature = "otel")]
pub mod config;
#[cfg(feature = "otel")]
pub mod metrics;

// Re-exports for convenience
pub use callback::{
    CallbackHandlerAdapter, GraphCallbackHandler, GraphInterruptEvent, GraphResumeEvent,
};
pub use debug::DebugEvent;
pub use test_utils::TestMetricsCollector;
pub use types::{LlmCacheKeyInput, LlmCachePolicy, ServerInfo};

#[cfg(feature = "otel")]
pub use config::{TracingConfig, init};
#[cfg(feature = "otel")]
pub use metrics::{
    CounterBuilder, GaugeBuilder, HistogramBuilder, MetricsRegistry, RegistryMetricsCollector,
};

// Re-export span constants
pub use spans::{attrs, names};

/// Initialize basic tracing without OpenTelemetry
///
/// Sets up a simple `tracing-subscriber` for structured logging.
/// Use this when you don't need OTLP export.
///
/// # Panics
///
/// This function panics if a global tracing subscriber is already installed.
///
/// # Examples
///
/// ```no_run
/// use juncture_tracing::init_tracing;
///
/// let _ = init_tracing();
/// // Your application code
/// ```
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .from_env_lossy(),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_constants_exist() {
        // Verify all important span names are defined
        assert_eq!(names::GRAPH_INVOKE, "juncture.graph.invoke");
        assert_eq!(names::NODE_EXECUTE, "juncture.node.execute");
        assert_eq!(names::LLM_CALL, "juncture.llm.call");
        assert_eq!(names::TOOL_CALL, "juncture.tool.call");

        // Verify attributes are defined
        assert_eq!(attrs::THREAD_ID, "juncture.thread.id");
        assert_eq!(attrs::NODE_NAME, "juncture.node.name");
        assert_eq!(attrs::LLM_MODEL, "juncture.llm.model");
    }

    #[test]
    fn test_init_tracing_doesnt_panic() {
        // Calling init_tracing multiple times is safe (it will just fail silently)
        init_tracing();
        init_tracing();
    }

    #[test]
    fn test_debug_event_reexport() {
        // Verify DebugEvent is accessible from the root
        let event = DebugEvent::GraphEnd {
            total_steps: 0,
            total_duration_ms: 0,
        };
        assert!(event.is_graph_end());
    }

    #[test]
    fn test_types_reexport() {
        // Verify types are accessible from the root
        let server_info = ServerInfo::new();
        assert!(server_info.assistant_id.is_none());

        let cache_policy = LlmCachePolicy::new();
        assert!(cache_policy.key_func.is_none());
    }

    #[test]
    fn test_test_utils_reexport() {
        // Verify test utilities are accessible from the root
        let collector = TestMetricsCollector::new();
        assert_eq!(collector.get_counter("test"), 0);
    }
}

// Rust guideline compliant 2026-05-19
