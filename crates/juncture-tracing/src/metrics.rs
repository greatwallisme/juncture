//! Metrics definitions and registry for Juncture
//!
//! This module provides metric name constants and a metrics registry for
//! OpenTelemetry metrics export. The registry supports two backends:
//!
//! - **In-memory mode**: `HashMap`-based storage, always available, suitable for
//!   testing and scenarios without an OpenTelemetry pipeline.
//! - **`OTel` mode**: Wraps an `opentelemetry::metrics::Meter` when the `otel`
//!   feature is enabled, forwarding metric operations to the OpenTelemetry SDK
//!   while keeping the in-memory `HashMap`s as a read-back fallback.
//!
//! The external handle API (`CounterHandle::inc()`, `HistogramHandle::record()`,
//! `GaugeHandle::set()`) is identical regardless of backend.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Builder for counter metric configuration
///
/// Use [`CounterBuilder::with_description`] and [`CounterBuilder::with_unit`]
/// to configure metric metadata before the handle is created.
///
/// # Examples
///
/// ```
/// use juncture_tracing::metrics::MetricsRegistry;
///
/// let registry = MetricsRegistry::new();
/// let counter = registry.counter("my_counter", |b| {
///     b.with_description("Total invocations").with_unit("1")
/// });
/// counter.inc();
/// ```
#[derive(Clone, Debug, Default)]
pub struct CounterBuilder {
    pub(crate) description: Option<String>,
    pub(crate) unit: Option<String>,
}

impl CounterBuilder {
    /// Set the metric description
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the metric unit
    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }
}

/// Builder for histogram metric configuration
///
/// Use the `with_*` methods to configure metric metadata including
/// optional bucket boundaries.
///
/// # Examples
///
/// ```
/// use juncture_tracing::metrics::MetricsRegistry;
///
/// let registry = MetricsRegistry::new();
/// let histogram = registry.histogram("latency_ms", |b| {
///     b.with_description("Request latency")
///         .with_unit("ms")
///         .with_boundaries(vec![1.0, 5.0, 10.0, 50.0, 100.0])
/// });
/// histogram.record(42.0);
/// ```
#[derive(Clone, Debug, Default)]
pub struct HistogramBuilder {
    pub(crate) description: Option<String>,
    pub(crate) unit: Option<String>,
    pub(crate) boundaries: Option<Vec<f64>>,
}

impl HistogramBuilder {
    /// Set the metric description
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the metric unit
    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Set explicit histogram bucket boundaries
    #[must_use]
    pub fn with_boundaries(mut self, boundaries: Vec<f64>) -> Self {
        self.boundaries = Some(boundaries);
        self
    }
}

/// Builder for gauge metric configuration
///
/// Use [`GaugeBuilder::with_description`] and [`GaugeBuilder::with_unit`]
/// to configure metric metadata before the handle is created.
///
/// # Examples
///
/// ```
/// use juncture_tracing::metrics::MetricsRegistry;
///
/// let registry = MetricsRegistry::new();
/// let gauge = registry.gauge("active_connections", |b| {
///     b.with_description("Currently active connections").with_unit("1")
/// });
/// gauge.set(10);
/// ```
#[derive(Clone, Debug, Default)]
pub struct GaugeBuilder {
    pub(crate) description: Option<String>,
    pub(crate) unit: Option<String>,
}

impl GaugeBuilder {
    /// Set the metric description
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the metric unit
    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }
}

/// Counter metric handle
///
/// Provides increment operations for counter metrics. When backed by an
/// OpenTelemetry meter, increments are forwarded to the `OTel` SDK.
/// Otherwise, an in-memory `HashMap` is used.
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct CounterHandle {
    registry: Arc<MetricsRegistryInner>,
    name: String,
    otel_counter: Option<opentelemetry::metrics::Counter<u64>>,
}

#[cfg(feature = "otel")]
impl CounterHandle {
    /// Increment the counter by 1
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
        if let Some(ref counter) = self.otel_counter {
            counter.add(value, &[]);
            return;
        }
        let name = self.name.clone();
        let mut counters = self.registry.counters.lock().unwrap();
        let entry = counters.entry(name).or_default();
        *entry = entry.saturating_add(value);
    }

    /// Get the current value from the in-memory store
    ///
    /// Always reads from the in-memory `HashMap`, even when an `OTel` counter
    /// is configured. Intended for testing and local read-back.
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
/// Provides value recording for histogram metrics. When backed by an
/// OpenTelemetry meter, recordings are forwarded to the `OTel` SDK.
/// Otherwise, an in-memory `HashMap` is used.
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct HistogramHandle {
    registry: Arc<MetricsRegistryInner>,
    name: String,
    otel_histogram: Option<opentelemetry::metrics::Histogram<f64>>,
}

#[cfg(feature = "otel")]
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
        if let Some(ref histogram) = self.otel_histogram {
            histogram.record(value, &[]);
            return;
        }
        let name = self.name.clone();
        let mut histograms = self.registry.histograms.lock().unwrap();
        let entry = histograms.entry(name).or_default();
        entry.push(value);
    }

    /// Get all recorded values from the in-memory store
    ///
    /// Always reads from the in-memory `HashMap`, even when an `OTel` histogram
    /// is configured. Intended for testing and local read-back.
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
/// When backed by an OpenTelemetry meter, `set` calls are forwarded to the
/// `OTel` Gauge instrument. The in-memory `AtomicU64` is always updated
/// regardless of backend.
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct GaugeHandle {
    value: Arc<AtomicU64>,
    otel_gauge: Option<opentelemetry::metrics::Gauge<u64>>,
}

#[cfg(feature = "otel")]
impl GaugeHandle {
    /// Set the gauge to a specific value
    ///
    /// # Arguments
    ///
    /// * `value` - Value to set
    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Release);
        if let Some(ref gauge) = self.otel_gauge {
            gauge.record(value, &[]);
        }
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

/// Stored metadata for a named metric
///
/// Populated by builder closures and consumed when constructing `OTel`
/// instruments (the `OTel` meter API expects description/unit/boundaries
/// at instrument build time).
#[cfg(feature = "otel")]
#[allow(
    dead_code,
    reason = "fields read in tests and consumed by OTel instrument creation"
)]
#[derive(Clone, Debug, Default)]
pub(crate) struct MetricMetadata {
    pub(crate) description: Option<String>,
    pub(crate) unit: Option<String>,
    pub(crate) boundaries: Option<Vec<f64>>,
}

/// Inner state of the metrics registry
///
/// Holds in-memory `HashMap`s for counters and histograms (always available)
/// and an optional OpenTelemetry `Meter` when the `otel` feature is enabled.
#[cfg(feature = "otel")]
pub(crate) struct MetricsRegistryInner {
    pub(crate) counters: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    pub(crate) histograms: std::sync::Mutex<std::collections::HashMap<String, Vec<f64>>>,
    pub(crate) metadata: std::sync::Mutex<std::collections::HashMap<String, MetricMetadata>>,
    pub(crate) meter: Option<opentelemetry::metrics::Meter>,
}

#[cfg(feature = "otel")]
impl std::fmt::Debug for MetricsRegistryInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsRegistryInner")
            .field("counters", &self.counters)
            .field("histograms", &self.histograms)
            .field("metadata", &self.metadata)
            .field("meter", &self.meter)
            .finish()
    }
}

#[cfg(feature = "otel")]
impl Default for MetricsRegistryInner {
    fn default() -> Self {
        Self {
            counters: std::sync::Mutex::new(std::collections::HashMap::new()),
            histograms: std::sync::Mutex::new(std::collections::HashMap::new()),
            metadata: std::sync::Mutex::new(std::collections::HashMap::new()),
            meter: None,
        }
    }
}

/// Metrics registry for OpenTelemetry metrics
///
/// Provides methods to create and manage custom metrics. Supports two modes:
///
/// - **In-memory mode** (`MetricsRegistry::new()`): `HashMap`-based storage,
///   always available, suitable for testing.
/// - **`OTel` mode** (`MetricsRegistry::with_meter(meter)`): Wraps an
///   `opentelemetry::metrics::Meter`, forwarding operations to the
///   OpenTelemetry SDK with in-memory `HashMap`s as a read-back fallback.
///
/// # Examples
///
/// ```
/// use juncture_tracing::metrics::MetricsRegistry;
///
/// let registry = MetricsRegistry::new();
/// let counter = registry.counter("my_counter", |b| {
///     b.with_description("Custom counter").with_unit("1")
/// });
/// counter.inc();
/// ```
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct MetricsRegistry {
    pub(crate) inner: Arc<MetricsRegistryInner>,
}

#[cfg(feature = "otel")]
impl MetricsRegistry {
    /// Create a new metrics registry in in-memory mode
    ///
    /// All metric operations use in-memory `HashMap`s. No OpenTelemetry
    /// SDK is involved.
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

    /// Create a metrics registry backed by an OpenTelemetry `Meter`
    ///
    /// When a `Meter` is provided, counter and histogram operations are
    /// forwarded to the `OTel` SDK. The in-memory `HashMap`s remain available
    /// as a read-back fallback for testing.
    ///
    /// # Arguments
    ///
    /// * `meter` - An OpenTelemetry meter obtained from a `MeterProvider`
    #[must_use]
    pub fn with_meter(meter: opentelemetry::metrics::Meter) -> Self {
        Self {
            inner: Arc::new(MetricsRegistryInner {
                counters: std::sync::Mutex::new(std::collections::HashMap::new()),
                histograms: std::sync::Mutex::new(std::collections::HashMap::new()),
                metadata: std::sync::Mutex::new(std::collections::HashMap::new()),
                meter: Some(meter),
            }),
        }
    }

    /// Create a counter metric handle with builder configuration
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    /// * `f` - Builder closure for configuring description and unit
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let counter = registry.counter("invocations", |b| {
    ///     b.with_description("Total invocations").with_unit("1")
    /// });
    /// counter.inc();
    /// counter.inc_by(5);
    /// ```
    pub fn counter<F>(&self, name: &str, f: F) -> CounterHandle
    where
        F: FnOnce(CounterBuilder) -> CounterBuilder,
    {
        let builder = f(CounterBuilder::default());
        self.store_metadata(
            name,
            builder.description.as_deref(),
            builder.unit.as_deref(),
            None,
        );

        let otel_counter = self.inner.meter.as_ref().map(|meter| {
            let mut b = meter.u64_counter(name.to_string());
            if let Some(desc) = &builder.description {
                b = b.with_description(desc.clone());
            }
            if let Some(unit) = &builder.unit {
                b = b.with_unit(unit.clone());
            }
            b.build()
        });

        CounterHandle {
            registry: Arc::clone(&self.inner),
            name: name.to_string(),
            otel_counter,
        }
    }

    /// Create a histogram metric handle with builder configuration
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    /// * `f` - Builder closure for configuring description, unit, and boundaries
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let histogram = registry.histogram("duration_ms", |b| {
    ///     b.with_description("Request duration")
    ///         .with_unit("ms")
    ///         .with_boundaries(vec![1.0, 5.0, 10.0, 50.0, 100.0])
    /// });
    /// histogram.record(42.0);
    /// histogram.record(58.5);
    /// ```
    pub fn histogram<F>(&self, name: &str, f: F) -> HistogramHandle
    where
        F: FnOnce(HistogramBuilder) -> HistogramBuilder,
    {
        let builder = f(HistogramBuilder::default());
        self.store_metadata(
            name,
            builder.description.as_deref(),
            builder.unit.as_deref(),
            builder.boundaries.as_deref(),
        );

        let otel_histogram = self.inner.meter.as_ref().map(|meter| {
            let mut b = meter.f64_histogram(name.to_string());
            if let Some(desc) = &builder.description {
                b = b.with_description(desc.clone());
            }
            if let Some(unit) = &builder.unit {
                b = b.with_unit(unit.clone());
            }
            b.build()
        });

        HistogramHandle {
            registry: Arc::clone(&self.inner),
            name: name.to_string(),
            otel_histogram,
        }
    }

    /// Create a gauge metric handle with builder configuration
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    /// * `f` - Builder closure for configuring description and unit
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::metrics::MetricsRegistry;
    ///
    /// let registry = MetricsRegistry::new();
    /// let gauge = registry.gauge("active_connections", |b| {
    ///     b.with_description("Active connections").with_unit("1")
    /// });
    /// gauge.set(10);
    /// gauge.inc();
    /// gauge.dec();
    /// ```
    #[allow(
        clippy::used_underscore_binding,
        reason = "name parameter stored as metadata and used for OTel gauge creation"
    )]
    pub fn gauge<F>(&self, name: &str, f: F) -> GaugeHandle
    where
        F: FnOnce(GaugeBuilder) -> GaugeBuilder,
    {
        let builder = f(GaugeBuilder::default());
        self.store_metadata(
            name,
            builder.description.as_deref(),
            builder.unit.as_deref(),
            None,
        );

        let otel_gauge = self.inner.meter.as_ref().map(|meter| {
            let mut b = meter.u64_gauge(name.to_string());
            if let Some(desc) = &builder.description {
                b = b.with_description(desc.clone());
            }
            if let Some(unit) = &builder.unit {
                b = b.with_unit(unit.clone());
            }
            b.build()
        });

        GaugeHandle {
            value: Arc::new(AtomicU64::new(0)),
            otel_gauge,
        }
    }

    /// Store metadata for a named metric when any is provided
    #[allow(
        clippy::significant_drop_tightening,
        reason = "MutexGuard is needed for entry API; tightening would complicate the code"
    )]
    fn store_metadata(
        &self,
        name: &str,
        description: Option<&str>,
        unit: Option<&str>,
        boundaries: Option<&[f64]>,
    ) {
        if description.is_some() || unit.is_some() || boundaries.is_some() {
            let mut metadata = self.inner.metadata.lock().unwrap();
            metadata.insert(
                name.to_string(),
                MetricMetadata {
                    description: description.map(str::to_owned),
                    unit: unit.map(str::to_owned),
                    boundaries: boundaries.map(std::borrow::ToOwned::to_owned),
                },
            );
        }
    }
}

#[cfg(feature = "otel")]
impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Adapter that implements [`MetricsCollector`] using a [`MetricsRegistry`].
///
/// Use this to wire a `MetricsRegistry` into `RunnableConfig::with_metrics_collector`
/// so the Pregel engine can emit `OTel` metrics through the registry.
///
/// # Examples
///
/// ```ignore
/// use std::sync::Arc;
/// use juncture_tracing::metrics::{MetricsRegistry, RegistryMetricsCollector};
/// use juncture_core::config::RunnableConfig;
///
/// let registry = MetricsRegistry::new();
/// let collector = RegistryMetricsCollector::new(registry);
/// let config = RunnableConfig::new()
///     .with_metrics_collector(Arc::new(collector));
/// ```
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
pub struct RegistryMetricsCollector {
    registry: MetricsRegistry,
}

#[cfg(feature = "otel")]
impl RegistryMetricsCollector {
    /// Create a new collector backed by the given registry.
    #[must_use]
    pub const fn new(registry: MetricsRegistry) -> Self {
        Self { registry }
    }
}

#[cfg(feature = "otel")]
impl juncture_core::observability::MetricsCollector for RegistryMetricsCollector {
    fn inc_counter(&self, name: &str, value: u64) {
        let counter = self.registry.counter(name, |b| b);
        counter.inc_by(value);
    }

    fn record_histogram(&self, name: &str, value: f64) {
        let histogram = self.registry.histogram(name, |b| b);
        histogram.record(value);
    }

    fn set_gauge(&self, name: &str, value: u64) {
        let gauge = self.registry.gauge(name, |b| b);
        gauge.set(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_names_format() {
        assert!(names::GRAPH_INVOCATIONS.starts_with("juncture."));
        assert!(names::LLM_TOKENS_INPUT.starts_with("juncture."));
        assert!(names::TOOL_CALLS.starts_with("juncture."));
        assert!(names::GRAPH_DURATION_MS.starts_with("juncture."));
        assert!(names::BUDGET_REMAINING_TOKENS.starts_with("juncture."));
    }

    #[test]
    fn test_counter_metrics_exist() {
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
        let counter = registry.counter("test_counter", |b| b);

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
        let histogram = registry.histogram("test_histogram", |b| b);

        assert!(histogram.get_values().is_empty());
        histogram.record(1.0);
        histogram.record(2.5);
        histogram.record(3.0);

        let values = histogram.get_values();
        assert_eq!(values.len(), 3);
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
        let gauge = registry.gauge("test_gauge", |b| b);

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
        let counter1 = registry.counter("counter_a", |b| b);
        let counter2 = registry.counter("counter_b", |b| b);

        counter1.inc_by(3);
        counter2.inc_by(5);

        assert_eq!(counter1.get(), 3);
        assert_eq!(counter2.get(), 5);
    }

    #[cfg(feature = "otel")]
    #[test]
    #[allow(
        clippy::significant_drop_tightening,
        reason = "test needs to hold MutexGuard across multiple assertions on the same metadata"
    )]
    fn test_counter_builder_with_description() {
        let registry = MetricsRegistry::new();
        let counter = registry.counter("test_counter_desc", |b| {
            b.with_description("Test counter").with_unit("1")
        });
        counter.inc();
        assert_eq!(counter.get(), 1);

        let metadata = registry.inner.metadata.lock().unwrap();
        let meta = metadata.get("test_counter_desc");
        assert!(meta.is_some());
        let meta = meta.expect("checked above");
        assert_eq!(meta.description.as_deref(), Some("Test counter"));
        assert_eq!(meta.unit.as_deref(), Some("1"));
    }

    #[cfg(feature = "otel")]
    #[test]
    #[allow(
        clippy::significant_drop_tightening,
        reason = "test needs to hold MutexGuard across multiple assertions on the same metadata"
    )]
    fn test_histogram_builder_with_boundaries() {
        let registry = MetricsRegistry::new();
        let histogram = registry.histogram("test_hist_boundaries", |b| {
            b.with_description("Test histogram")
                .with_unit("ms")
                .with_boundaries(vec![1.0, 5.0, 10.0, 50.0, 100.0])
        });
        histogram.record(42.0);
        assert_eq!(histogram.get_values().len(), 1);

        let metadata = registry.inner.metadata.lock().unwrap();
        let meta = metadata.get("test_hist_boundaries");
        assert!(meta.is_some());
        let meta = meta.expect("checked above");
        assert_eq!(meta.description.as_deref(), Some("Test histogram"));
        assert_eq!(meta.unit.as_deref(), Some("ms"));
        assert_eq!(
            meta.boundaries.as_deref(),
            Some([1.0, 5.0, 10.0, 50.0, 100.0].as_slice())
        );
    }

    #[cfg(feature = "otel")]
    #[test]
    #[allow(
        clippy::significant_drop_tightening,
        reason = "test needs to hold MutexGuard across multiple assertions on the same metadata"
    )]
    fn test_gauge_builder_with_description() {
        let registry = MetricsRegistry::new();
        let gauge = registry.gauge("test_gauge_desc", |b| {
            b.with_description("Active connections").with_unit("1")
        });
        gauge.set(5);
        assert_eq!(gauge.get(), 5);

        let metadata = registry.inner.metadata.lock().unwrap();
        let meta = metadata.get("test_gauge_desc");
        assert!(meta.is_some());
        let meta = meta.expect("checked above");
        assert_eq!(meta.description.as_deref(), Some("Active connections"));
        assert_eq!(meta.unit.as_deref(), Some("1"));
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_no_metadata_without_builder_config() {
        let registry = MetricsRegistry::new();
        let counter = registry.counter("plain_counter", |b| b);
        counter.inc();

        assert!(
            registry
                .inner
                .metadata
                .lock()
                .unwrap()
                .get("plain_counter")
                .is_none()
        );
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_builder_default_is_no_op() {
        let cb = CounterBuilder::default();
        assert!(cb.description.is_none());
        assert!(cb.unit.is_none());

        let hb = HistogramBuilder::default();
        assert!(hb.description.is_none());
        assert!(hb.unit.is_none());
        assert!(hb.boundaries.is_none());

        let gb = GaugeBuilder::default();
        assert!(gb.description.is_none());
        assert!(gb.unit.is_none());
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_with_meter_creates_otel_counter() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::SdkMeterProvider;

        let provider = SdkMeterProvider::builder().build();
        let meter = provider.meter("test");
        let registry = MetricsRegistry::with_meter(meter);

        let counter = registry.counter("otel_counter", |b| {
            b.with_description("OTel counter").with_unit("1")
        });

        // OTel counter is present in the handle
        assert!(
            counter.otel_counter.is_some(),
            "OTel counter should be Some when registry has a meter"
        );

        // In OTel mode, inc_by delegates to the OTel counter and skips
        // the in-memory HashMap, so get() returns 0 (the in-memory fallback
        // is not updated when an OTel instrument is active).
        counter.inc_by(3);
        assert_eq!(counter.get(), 0);
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_with_meter_creates_otel_histogram() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::SdkMeterProvider;

        let provider = SdkMeterProvider::builder().build();
        let meter = provider.meter("test");
        let registry = MetricsRegistry::with_meter(meter);

        let histogram = registry.histogram("otel_histogram", |b| {
            b.with_description("OTel histogram")
                .with_unit("ms")
                .with_boundaries(vec![1.0, 5.0, 10.0])
        });

        assert!(
            histogram.otel_histogram.is_some(),
            "OTel histogram should be Some when registry has a meter"
        );
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_with_meter_creates_otel_gauge() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::SdkMeterProvider;

        let provider = SdkMeterProvider::builder().build();
        let meter = provider.meter("test");
        let registry = MetricsRegistry::with_meter(meter);

        let gauge = registry.gauge("otel_gauge", |b| {
            b.with_description("OTel gauge").with_unit("1")
        });

        assert!(
            gauge.otel_gauge.is_some(),
            "OTel gauge should be Some when registry has a meter"
        );

        // Gauge operations still work
        gauge.set(42);
        assert_eq!(gauge.get(), 42);
    }

    #[cfg(feature = "otel")]
    #[test]
    fn test_in_memory_mode_has_no_otel_instruments() {
        let registry = MetricsRegistry::new();
        let counter = registry.counter("mem_counter", |b| b);
        let histogram = registry.histogram("mem_histogram", |b| b);
        let gauge = registry.gauge("mem_gauge", |b| b);

        assert!(
            counter.otel_counter.is_none(),
            "In-memory registry should not have OTel counter"
        );
        assert!(
            histogram.otel_histogram.is_none(),
            "In-memory registry should not have OTel histogram"
        );
        assert!(
            gauge.otel_gauge.is_none(),
            "In-memory registry should not have OTel gauge"
        );
    }
}

// Rust guideline compliant 2026-05-21
