// Observability and cache key types
//
// This module provides cache key types for LLM response caching
// and server metadata for observability features.

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
                if let Ok(s) = serde_json::to_string(&call.arguments) {
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
#[derive(Clone)]
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
    pub fn with_key_func(mut self, f: Arc<dyn Fn(&CacheKeyInput) -> String + Send + Sync>) -> Self {
        self.key_func = Some(f);
        self
    }

    /// Generate cache key from input
    #[must_use]
    pub fn generate_key(&self, input: &CacheKeyInput) -> String {
        self.key_func.as_ref().map_or_else(
            || format!("{}:{}", input.model, input.hash()),
            |func| func(input),
        )
    }
}

// ---------------------------------------------------------------------------
// MetricsCollector trait
// ---------------------------------------------------------------------------

/// Trait for collecting metrics during graph execution.
///
/// Implementations can forward to OpenTelemetry, in-memory stores, or any
/// other metrics backend. Injected via [`RunnableConfig::with_metrics_collector`].
///
/// The trait lives in `juncture-core` so the Pregel engine can emit metrics
/// without depending on `juncture-tracing`. The `juncture-tracing` crate
/// provides concrete implementations (`TestMetricsCollector`, `RegistryMetricsCollector`).
///
/// # Examples
///
/// ```ignore
/// use std::sync::Arc;
/// use juncture_core::observability::MetricsCollector;
/// use juncture_core::config::RunnableConfig;
///
/// let collector: Arc<dyn MetricsCollector> = /* ... */;
/// let config = RunnableConfig::new()
///     .with_metrics_collector(collector);
/// ```
pub trait MetricsCollector: Send + Sync + 'static {
    /// Increment a counter metric by `value`.
    fn inc_counter(&self, name: &str, value: u64);

    /// Record `value` to a histogram metric.
    fn record_histogram(&self, name: &str, value: f64);

    /// Set a gauge metric to `value`.
    fn set_gauge(&self, name: &str, value: u64);
}

// ---------------------------------------------------------------------------
// GraphLifecycleCallback trait
// ---------------------------------------------------------------------------

/// Callback trait for graph lifecycle events.
///
/// Implementations receive notifications at key points during graph execution.
/// All methods have default no-op implementations. Injected via
/// [`RunnableConfig::with_callback_handler`].
///
/// The trait lives in `juncture-core` so the Pregel engine can emit callbacks
/// without depending on `juncture-tracing`. The `juncture-tracing` crate
/// provides a blanket impl that forwards [`GraphCallbackHandler`] to this
/// trait, so any type implementing [`GraphCallbackHandler`] can be passed
/// to [`RunnableConfig::with_callback_handler`] directly.
///
/// [`GraphCallbackHandler`]: juncture_tracing::callback::GraphCallbackHandler
/// [`RunnableConfig::with_callback_handler`]: crate::config::RunnableConfig::with_callback_handler
///
/// # Examples
///
/// ```ignore
/// use std::sync::Arc;
/// use juncture_core::observability::GraphLifecycleCallback;
/// use juncture_core::config::RunnableConfig;
///
/// let handler: Arc<dyn GraphLifecycleCallback> = /* ... */;
/// let config = RunnableConfig::new()
///     .with_callback_handler(handler);
/// ```
pub trait GraphLifecycleCallback: Send + Sync + 'static {
    /// Called when a node starts execution.
    fn on_node_start(&self, node: &str, task_id: &str) {
        let _ = (node, task_id);
    }

    /// Called when a node completes execution successfully.
    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        let _ = (node, task_id, duration_ms);
    }

    /// Called when a node encounters an error.
    fn on_node_error(&self, node: &str, error: &crate::JunctureError) {
        let _ = (node, error);
    }

    /// Called when the graph execution completes.
    fn on_graph_end(&self, result: &Result<(), crate::JunctureError>) {
        let _ = result;
    }

    /// Called when a checkpoint is saved.
    fn on_checkpoint_saved(&self, checkpoint_id: &str, step: usize) {
        let _ = (checkpoint_id, step);
    }
}

// Rust guideline compliant 2026-05-21
