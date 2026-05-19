//! Metrics definitions and registry for Juncture
//!
//! This module provides metric name constants and a metrics registry for
//! OpenTelemetry metrics export. This feature is only available when the
//! `otel` feature is enabled.

/// Metric name constants
///
/// These constants define the standard metric names used throughout Juncture.
/// They follow OpenTelemetry semantic conventions where applicable.
pub mod names {
    // Counter metrics

    /// Total number of graph invocations
    pub const GRAPH_INVOCATIONS: &str = "juncture.graph.invocations";

    /// Total number of graph errors
    pub const GRAPH_ERRORS: &str = "juncture.graph.errors";

    /// Total input tokens consumed
    pub const LLM_TOKENS_INPUT: &str = "juncture.llm.tokens.input";

    /// Total output tokens generated
    pub const LLM_TOKENS_OUTPUT: &str = "juncture.llm.tokens.output";

    /// Total cost in USD for LLM calls
    pub const LLM_COST_USD: &str = "juncture.llm.cost_usd";

    /// Total number of LLM calls
    pub const LLM_CALLS: &str = "juncture.llm.calls";

    /// Total number of tool calls
    pub const TOOL_CALLS: &str = "juncture.tool.calls";

    /// Total number of tool errors
    pub const TOOL_ERRORS: &str = "juncture.tool.errors";

    /// Total number of checkpoint writes
    pub const CHECKPOINT_WRITES: &str = "juncture.checkpoint.writes";

    // Histogram metrics

    /// Graph execution duration in milliseconds
    pub const GRAPH_DURATION_MS: &str = "juncture.graph.duration_ms";

    /// Node execution duration in milliseconds
    pub const NODE_DURATION_MS: &str = "juncture.node.duration_ms";

    /// LLM call duration in milliseconds
    pub const LLM_DURATION_MS: &str = "juncture.llm.duration_ms";

    /// Tokens per LLM call
    pub const LLM_TOKENS_PER_CALL: &str = "juncture.llm.tokens_per_call";

    /// Tool call duration in milliseconds
    pub const TOOL_DURATION_MS: &str = "juncture.tool.duration_ms";

    /// Superstep duration in milliseconds
    pub const SUPERSTEP_DURATION_MS: &str = "juncture.superstep.duration_ms";

    // Gauge metrics

    /// Current number of active graph invocations
    pub const GRAPH_ACTIVE_INVOCATIONS: &str = "juncture.graph.active_invocations";

    /// Remaining token budget
    pub const BUDGET_REMAINING_TOKENS: &str = "juncture.budget.remaining_tokens";

    /// Remaining cost budget in USD
    pub const BUDGET_REMAINING_COST_USD: &str = "juncture.budget.remaining_cost_usd";
}

/// Metrics registry for OpenTelemetry metrics
///
/// Provides methods to create and manage custom metrics.
/// When OpenTelemetry dependencies are added, this will integrate with
/// the OpenTelemetry SDK for metrics export.
///
/// # Examples
///
/// ```
/// use juncture_tracing::metrics::MetricsRegistry;
///
/// let registry = MetricsRegistry::new();
/// // Future: Use registry to create OpenTelemetry metrics
/// ```
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct MetricsRegistry {
    _private: (),
}

#[cfg(feature = "otel")]
impl MetricsRegistry {
    /// Create a new metrics registry
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

#[cfg(feature = "otel")]
impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_names_format() {
        // Verify all metric names follow juncter.* format
        assert!(names::GRAPH_INVOCATIONS.starts_with("juncture."));
        assert!(names::LLM_TOKENS_INPUT.starts_with("juncture."));
        assert!(names::TOOL_CALLS.starts_with("juncture."));
        assert!(names::GRAPH_DURATION_MS.starts_with("juncture."));
        assert!(names::BUDGET_REMAINING_TOKENS.starts_with("juncture."));
    }

    #[test]
    fn test_counter_metrics_exist() {
        // Verify all counter metrics are defined
        assert_eq!(names::GRAPH_INVOCATIONS, "juncture.graph.invocations");
        assert_eq!(names::GRAPH_ERRORS, "juncture.graph.errors");
        assert_eq!(names::LLM_TOKENS_INPUT, "juncture.llm.tokens.input");
        assert_eq!(names::LLM_TOKENS_OUTPUT, "juncture.llm.tokens.output");
        assert_eq!(names::LLM_COST_USD, "juncture.llm.cost_usd");
        assert_eq!(names::LLM_CALLS, "juncture.llm.calls");
        assert_eq!(names::TOOL_CALLS, "juncture.tool.calls");
        assert_eq!(names::TOOL_ERRORS, "juncture.tool.errors");
        assert_eq!(names::CHECKPOINT_WRITES, "juncture.checkpoint.writes");
    }

    #[test]
    fn test_histogram_metrics_exist() {
        // Verify all histogram metrics are defined
        assert_eq!(names::GRAPH_DURATION_MS, "juncture.graph.duration_ms");
        assert_eq!(names::NODE_DURATION_MS, "juncture.node.duration_ms");
        assert_eq!(names::LLM_DURATION_MS, "juncture.llm.duration_ms");
        assert_eq!(names::LLM_TOKENS_PER_CALL, "juncture.llm.tokens_per_call");
        assert_eq!(names::TOOL_DURATION_MS, "juncture.tool.duration_ms");
        assert_eq!(
            names::SUPERSTEP_DURATION_MS,
            "juncture.superstep.duration_ms"
        );
    }

    #[test]
    fn test_gauge_metrics_exist() {
        // Verify all gauge metrics are defined
        assert_eq!(
            names::GRAPH_ACTIVE_INVOCATIONS,
            "juncture.graph.active_invocations"
        );
        assert_eq!(
            names::BUDGET_REMAINING_TOKENS,
            "juncture.budget.remaining_tokens"
        );
        assert_eq!(
            names::BUDGET_REMAINING_COST_USD,
            "juncture.budget.remaining_cost_usd"
        );
    }
}

// Rust guideline compliant 2026-05-19
