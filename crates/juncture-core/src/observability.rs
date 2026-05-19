// Observability and metrics types
//
// This module provides metrics registry and cache key types for
// OpenTelemetry integration and observability features.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::llm::{CallOptions, ToolDefinition};
use crate::state::Message;

/// Cache key input for LLM response caching
///
/// Used to generate cache keys for LLM responses.
#[derive(Debug, Clone)]
pub struct CacheKeyInput {
    /// Model name
    pub model: String,
    /// Messages
    pub messages: Vec<Message>,
    /// Tools
    pub tools: Vec<ToolDefinition>,
    /// Call options
    pub config: Option<CallOptions>,
}

impl CacheKeyInput {
    /// Create new cache key input
    ///
    /// # Arguments
    ///
    /// * `model` - Model name
    /// * `messages` - Message list
    /// * `tools` - Tool definitions
    /// * `config` - Optional call options
    pub fn new(
        model: impl Into<String>,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        config: Option<CallOptions>,
    ) -> Self {
        Self {
            model: model.into(),
            messages,
            tools,
            config,
        }
    }

    /// Generate hash for this cache key input
    ///
    /// Returns a stable hash value for use as cache key.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Hash model
        self.model.hash(&mut hasher);

        // Hash messages (excluding IDs and timestamps)
        for msg in &self.messages {
            msg.role.hash(&mut hasher);
            match &msg.content {
                crate::state::Content::Text(text) => {
                    text.hash(&mut hasher);
                }
                crate::state::Content::MultiPart(parts) => {
                    for part in parts {
                        match part {
                            crate::state::ContentPart::Text { text } => {
                                text.hash(&mut hasher);
                            }
                            crate::state::ContentPart::Image(data) => {
                                data.media_type.hash(&mut hasher);
                                match &data.source {
                                    crate::state::ImageSource::Base64(data) => {
                                        data.hash(&mut hasher);
                                    }
                                    crate::state::ImageSource::Url(url) => {
                                        url.hash(&mut hasher);
                                    }
                                }
                            }
                            crate::state::ContentPart::Thinking { text, signature } => {
                                text.hash(&mut hasher);
                                signature.hash(&mut hasher);
                            }
                        }
                    }
                }
            }
            // Hash tool calls
            for call in &msg.tool_calls {
                call.id.hash(&mut hasher);
                call.name.hash(&mut hasher);
                if let Ok(s) = serde_json::to_string(&call.args) {
                    s.hash(&mut hasher);
                }
            }
        }

        // Hash tools
        for tool in &self.tools {
            tool.name.hash(&mut hasher);
            if let Ok(s) = serde_json::to_string(&tool.parameters) {
                s.hash(&mut hasher);
            }
        }

        // Hash config
        if let Some(config) = &self.config {
            if let Some(temp) = config.temperature {
                (temp.to_bits()).hash(&mut hasher);
            }
            if let Some(max_tokens) = config.max_tokens {
                max_tokens.hash(&mut hasher);
            }
            if let Some(top_p) = config.top_p {
                (top_p.to_bits()).hash(&mut hasher);
            }
        }

        hasher.finish()
    }
}

/// Metrics registry for custom metrics
///
/// Provides explicit API for creating and registering custom metrics.
#[allow(missing_debug_implementations, reason = "Contains OpenTelemetry meter which doesn't implement Debug")]
pub struct MetricsRegistry {
    /// OpenTelemetry meter (when available)
    #[cfg(feature = "otel")]
    meter: opentelemetry::metrics::Meter,

    /// Phantom data for when otel feature is not enabled
    #[cfg(not(feature = "otel"))]
    _phantom: std::marker::PhantomData<()>,
}

impl MetricsRegistry {
    /// Create new metrics registry
    #[cfg(feature = "otel")]
    #[must_use]
    pub const fn new(meter: opentelemetry::metrics::Meter) -> Self {
        Self { meter }
    }

    /// Create new metrics registry (non-otel)
    #[cfg(not(feature = "otel"))]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

#[cfg(not(feature = "otel"))]
impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    /// Create a counter metric
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Returns
    ///
    /// Counter instrument
    #[cfg(feature = "otel")]
    pub fn counter(&self, name: impl Into<String>) -> opentelemetry::metrics::Counter<u64> {
        self.meter.u64_counter(name.into()).build()
    }

    /// Create a counter metric (non-otel)
    #[cfg(not(feature = "otel"))]
    pub fn counter(&self, _name: impl Into<String>) -> NoOpCounter {
        NoOpCounter
    }

    /// Create a histogram metric
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Returns
    ///
    /// Histogram instrument
    #[cfg(feature = "otel")]
    pub fn histogram(&self, name: impl Into<String>) -> opentelemetry::metrics::Histogram<f64> {
        self.meter.f64_histogram(name.into()).build()
    }

    /// Create a histogram metric (non-otel)
    #[cfg(not(feature = "otel"))]
    pub fn histogram(&self, _name: impl Into<String>) -> NoOpHistogram {
        NoOpHistogram
    }

    /// Create a gauge metric
    ///
    /// # Arguments
    ///
    /// * `name` - Metric name
    ///
    /// # Returns
    ///
    /// Gauge instrument
    #[cfg(feature = "otel")]
    pub fn gauge(&self, name: impl Into<String>) -> opentelemetry::metrics::Gauge<f64> {
        self.meter.f64_gauge(name.into()).build()
    }

    /// Create a gauge metric (non-otel)
    #[cfg(not(feature = "otel"))]
    pub fn gauge(&self, _name: impl Into<String>) -> NoOpGauge {
        NoOpGauge
    }
}

/// No-op counter for when otel feature is disabled
#[derive(Debug)]
pub struct NoOpCounter;

impl NoOpCounter {
    /// Add value to counter (no-op)
    pub const fn add(&self, _value: u64, _attributes: &[()]) {
        // No-op when otel is disabled
    }
}

/// No-op histogram for when otel feature is disabled
#[derive(Debug)]
pub struct NoOpHistogram;

impl NoOpHistogram {
    /// Record value (no-op)
    pub const fn record(&self, _value: f64, _attributes: &[()]) {
        // No-op when otel is disabled
    }
}

/// No-op gauge for when otel feature is disabled
#[derive(Debug)]
pub struct NoOpGauge;

impl NoOpGauge {
    /// Record value (no-op)
    pub const fn record(&self, _value: f64, _attributes: &[()]) {
        // No-op when otel is disabled
    }

    /// Set value (no-op)
    pub const fn set(&self, _value: f64, _attributes: &[()]) {
        // No-op when otel is disabled
    }
}

/// Server deployment metadata for observability
///
/// Contains deployment information for multi-instance scenarios.
#[derive(Debug, Clone, Default)]
pub struct ServerInfo {
    /// Assistant ID for multi-assistant deployments
    pub assistant_id: Option<String>,
    /// Graph ID
    pub graph_id: Option<String>,
    /// Current authenticated user
    pub user: Option<String>,
    /// Deployment environment identifier
    pub deployment: Option<String>,
    /// Service version
    pub version: Option<String>,
    /// Instance ID
    pub instance_id: Option<String>,
}

impl ServerInfo {
    /// Create new server info
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set assistant ID
    #[must_use]
    pub fn with_assistant_id(mut self, id: impl Into<String>) -> Self {
        self.assistant_id = Some(id.into());
        self
    }

    /// Set graph ID
    #[must_use]
    pub fn with_graph_id(mut self, id: impl Into<String>) -> Self {
        self.graph_id = Some(id.into());
        self
    }

    /// Set user
    #[must_use]
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set deployment
    #[must_use]
    pub fn with_deployment(mut self, deployment: impl Into<String>) -> Self {
        self.deployment = Some(deployment.into());
        self
    }

    /// Set version
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set instance ID
    #[must_use]
    pub fn with_instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }
}

/// Cache policy for LLM response caching
#[derive(Default)]
#[allow(
    missing_debug_implementations,
    clippy::type_complexity,
    reason = "Contains Arc<dyn Fn> which doesn't implement Debug. Complex trait object type is required for dynamic tool configuration."
)]
pub struct CachePolicy {
    /// Custom cache key generation function
    pub key_func: Option<Arc<dyn Fn(&CacheKeyInput) -> String + Send + Sync>>,
}

impl CachePolicy {
    /// Create new cache policy
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom cache key function
    #[must_use]
    pub fn with_key_func(
        mut self,
        f: Arc<dyn Fn(&CacheKeyInput) -> String + Send + Sync>,
    ) -> Self {
        self.key_func = Some(f);
        self
    }

    /// Generate cache key from input
    #[must_use]
    pub fn generate_key(&self, input: &CacheKeyInput) -> String {
        self.key_func
            .as_ref()
            .map_or_else(|| format!("{}:{}", input.model, input.hash()), |func| func(input))
    }
}

// Rust guideline compliant 2026-05-19
