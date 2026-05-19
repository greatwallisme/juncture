//! Metrics definitions and registry for Juncture
//!
//! This module provides metric name constants and a metrics registry for
//! OpenTelemetry metrics export. This feature is only available when the
//! `otel` feature is enabled.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Counter metric handle
///
/// Provides increment operations for counter metrics.
#[derive(Clone, Debug)]
pub struct CounterHandle {
    registry: Arc<MetricsRegistryInner>,
    name: String,
}

impl CounterHandle {
    /// Increment the counter by 1
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (which indicates another
    /// thread panicked while holding the lock).
    pub fn inc(&self) {
        self.inc_by(1);
    }

    /// Increment the counter by a specific amount
    ///
    /// # Arguments
    ///
    /// * `value` - Amount to increment by
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (which indicates another
    /// thread panicked while holding the lock).
    #[allow(
        clippy::significant_drop_tightening,
        reason = "MutexGuard is needed for entry API; tightening would complicate the code"
    )]
    pub fn inc_by(&self, value: u64) {
        let name = self.name.clone();
        let mut counters = self.registry.counters.lock().unwrap();
        let entry = counters.entry(name).or_default();
        *entry = entry.saturating_add(value);
    }

    /// Get the current value
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (which indicates another
    /// thread panicked while holding the lock).
    #[must_use]
    pub fn get(&self) -> u64 {
        let name = self.name.clone();
        let counters = self.registry.counters.lock().unwrap();
        counters.get(&name).copied().unwrap_or(0)
    }
}

/// Histogram metric handle
///
/// Provides value recording for histogram metrics.
#[derive(Clone, Debug)]
pub struct HistogramHandle {
    registry: Arc<MetricsRegistryInner>,
    name: String,
}

impl HistogramHandle {
    /// Record a value
    ///
    /// # Arguments
    ///
    /// * `value` - Value to record
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (which indicates another
    /// thread panicked while holding the lock).
    #[allow(
        clippy::significant_drop_tightening,
        reason = "MutexGuard is needed for entry API; tightening would complicate the code"
    )]
    pub fn record(&self, value: f64) {
        let name = self.name.clone();
        let mut histograms = self.registry.histograms.lock().unwrap();
        let entry = histograms.entry(name).or_default();
        entry.push(value);
    }

    /// Get all recorded values
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (which indicates another
    /// thread panicked while holding the lock).
    #[must_use]
    pub fn get_values(&self) -> Vec<f64> {
        let name = self.name.clone();
        let histograms = self.registry.histograms.lock().unwrap();
        histograms.get(&name).cloned().unwrap_or_default()
    }
}

/// Gauge metric handle
///
/// Provides set and increment/decrement operations for gauge metrics.
#[derive(Clone, Debug)]
pub struct GaugeHandle {
    value: Arc<AtomicU64>,
}

impl GaugeHandle {
    /// Set the gauge to a specific value
    ///
    /// # Arguments
    ///
    /// * `value` - Value to set
    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Release);
    }

    /// Increment the gauge by 1
    pub fn inc(&self) {
        self.inc_by(1);
    }

    /// Increment the gauge by a specific amount
    ///
    /// # Arguments
    ///
    /// * `value` - Amount to increment by
    pub fn inc_by(&self, value: u64) {
        self.value.fetch_add(value, Ordering::Release);
    }

    /// Decrement the gauge by 1
    pub fn dec(&self) {
        self.dec_by(1);
    }

    /// Decrement the gauge by a specific amount
    ///
    /// # Arguments
    ///
    /// * `value` - Amount to decrement by
    pub fn dec_by(&self, value: u64) {
        self.value.fetch_sub(value, Ordering::Release);
    }

    /// Get the current value
    #[must_use]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }
}

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
/// let counter = registry.counter("my_counter");
/// counter.inc();
/// ```
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct MetricsRegistry {
    inner: Arc<MetricsRegistryInner>,
}

/// Inner state of the metrics registry
#[derive(Debug, Default)]
struct MetricsRegistryInner {
    counters: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    histograms: std::sync::Mutex<std::collections::HashMap<String, Vec<f64>>>,
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
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MetricsRegistryInner::default()),
        }
    }

    /// Create a counter metric handle
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let counter = registry.counter("invocations");
    /// counter.inc();
    /// counter.inc_by(5);
    /// ```
    #[must_use]
    pub fn counter(&self, name: &str) -> CounterHandle {
        CounterHandle {
            registry: Arc::clone(&self.inner),
            name: name.to_string(),
        }
    }

    /// Create a histogram metric handle
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let histogram = registry.histogram("duration_ms");
    /// histogram.record(42.0);
    /// histogram.record(58.5);
    /// ```
    #[must_use]
    pub fn histogram(&self, name: &str) -> HistogramHandle {
        HistogramHandle {
            registry: Arc::clone(&self.inner),
            name: name.to_string(),
        }
    }

    /// Create a gauge metric handle
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let gauge = registry.gauge("active_connections");
    /// gauge.set(10);
    /// gauge.inc();
    /// gauge.dec();
    /// ```
    #[must_use]
    pub fn gauge(&self, _name: &str) -> GaugeHandle {
        GaugeHandle {
            value: Arc::new(AtomicU64::new(0)),
        }
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

    #[cfg(feature = "otel")]
    #[test]
    fn test_counter_handle() {
        let registry = MetricsRegistry::new();
        let counter = registry.counter("test_counter");

        assert_eq!(counter.get(), 0);
        counter.inc();
        assert_eq!(counter.get(), 1);
        counter.inc_by(5);
        assert_eq!(counter.get(), 6);
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_histogram_handle() {
        let registry = MetricsRegistry::new();
        let histogram = registry.histogram("test_histogram");

        assert!(histogram.get_values().is_empty());
        histogram.record(1.0);
        histogram.record(2.5);
        histogram.record(3.0);

        let values = histogram.get_values();
        assert_eq!(values.len(), 3);
        // Use approximate comparison for floating point values
        #[allow(
            clippy::float_cmp,
            reason = "test values are exact binary fractions, safe to compare"
        )]
        {
            assert_eq!(values[0], 1.0);
            assert_eq!(values[1], 2.5);
            assert_eq!(values[2], 3.0);
        }
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_gauge_handle() {
        let registry = MetricsRegistry::new();
        let gauge = registry.gauge("test_gauge");

        assert_eq!(gauge.get(), 0);
        gauge.set(10);
        assert_eq!(gauge.get(), 10);
        gauge.inc();
        assert_eq!(gauge.get(), 11);
        gauge.inc_by(5);
        assert_eq!(gauge.get(), 16);
        gauge.dec();
        assert_eq!(gauge.get(), 15);
        gauge.dec_by(3);
        assert_eq!(gauge.get(), 12);
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_multiple_counter_handles() {
        let registry = MetricsRegistry::new();
        let counter1 = registry.counter("counter_a");
        let counter2 = registry.counter("counter_b");

        counter1.inc_by(3);
        counter2.inc_by(5);

        assert_eq!(counter1.get(), 3);
        assert_eq!(counter2.get(), 5);
    }
}

// Rust guideline compliant 2026-05-19
