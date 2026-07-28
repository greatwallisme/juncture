// Observability and cache key types
//
// This module provides cache key types for LLM response caching.

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

/// Cache policy for LLM response caching
///
/// Carries the key-generation function, an optional TTL, and a shared
/// in-memory store. `Clone` shares the store (it is `Arc`-backed), so reusing
/// one `CachePolicy` across runs yields cross-run cache hits. The Pregel
/// runner scopes this as the `LLM_CACHE_POLICY` task-local from
/// `RunnableConfig::llm_cache_policy`; providers consult it before the HTTP
/// call (`try_llm_cache_lookup`) and store the response after
/// (`try_llm_cache_store`), per design `09-observability` §4.4 /
/// `03-pregel-engine` §13.3.
#[allow(
    missing_debug_implementations,
    clippy::type_complexity,
    reason = "Contains Arc<dyn Fn> which doesn't implement Debug. Complex trait object type is required for dynamic tool configuration."
)]
#[derive(Clone)]
pub struct CachePolicy {
    /// Custom cache key generation function
    pub key_func: Option<Arc<dyn Fn(&CacheKeyInput) -> String + Send + Sync>>,

    /// Optional time-to-live for cached responses. `None` means entries never
    /// expire; `Some(d)` expires entries older than `d` (design `03-pregel-engine`
    /// §13.3 `CachePolicy::ttl(...)`).
    pub ttl: Option<std::time::Duration>,

    /// Shared in-memory store: cache key -> (response message, insertion time).
    /// `Arc` so `Clone`d policies share one store (cross-run reuse).
    store: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, (crate::Message, crate::time::Instant)>>,
    >,
}

impl std::fmt::Debug for CachePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachePolicy")
            .field("key_func", &self.key_func.as_ref().map(|_| "<fn>"))
            .field("ttl", &self.ttl)
            .field("store", &"<store>")
            .finish()
    }
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            key_func: None,
            ttl: None,
            store: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl CachePolicy {
    /// Create new cache policy
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache policy with the given TTL (design `03-pregel-engine`
    /// §13.3 `CachePolicy::ttl(...)`).
    #[must_use]
    pub fn ttl(ttl: std::time::Duration) -> Self {
        Self {
            ttl: Some(ttl),
            ..Self::default()
        }
    }

    /// Set custom cache key function
    #[must_use]
    pub fn with_key_func(mut self, f: Arc<dyn Fn(&CacheKeyInput) -> String + Send + Sync>) -> Self {
        self.key_func = Some(f);
        self
    }

    /// Set the TTL on this policy.
    #[must_use]
    pub const fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
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

    /// Look up a cached response for `key`, honoring the TTL.
    ///
    /// Returns `None` on a miss or when the entry is older than the policy's
    /// TTL (expired entries are dropped lazily). Lock poisoning is recovered
    /// so a panicking prior holder never disables the cache.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<crate::Message> {
        let store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (msg, inserted) = store.get(key)?;
        if let Some(ttl) = self.ttl {
            if inserted.elapsed() > ttl {
                drop(store);
                self.store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(key);
                return None;
            }
        }
        Some(msg.clone())
    }

    /// Store a response under `key`. Lock poisoning is recovered.
    pub fn put(&self, key: String, message: crate::Message) {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, (message, crate::time::Instant::now()));
    }
}

// ---------------------------------------------------------------------------
// MetricsCollector trait
// ---------------------------------------------------------------------------

/// Trait for collecting metrics during graph execution.
///
/// Implementations can forward to OpenTelemetry, in-memory stores, or any
/// other metrics backend. Injected via `RunnableConfig::with_metrics_collector`.
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
/// provides a blanket impl that forwards `GraphCallbackHandler` to this
/// trait, so any type implementing `GraphCallbackHandler` can be passed
/// to [`RunnableConfig::with_callback_handler`] directly.
///
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

    /// Called when a node raises a HITL interrupt.
    ///
    /// Design `09-observability` §5.1 (C-09-001) promises full lifecycle hooks
    /// including `on_interrupt`. Fired for each interrupt signal at the point
    /// the engine surfaces the interrupt (independent of stream emission), so
    /// callback consumers see interrupt events even when no `stream_tx` is
    /// configured.
    ///
    /// * `node` - name of the node that triggered the interrupt.
    /// * `payload` - the interrupt payload (`{"node": ..., ...}` plus any
    ///   user-supplied data passed to `interrupt!`).
    fn on_interrupt(&self, node: &str, payload: &serde_json::Value) {
        let _ = (node, payload);
    }

    /// Called when the graph is resumed after a HITL interrupt.
    ///
    /// Design `09-observability` §5.1 (C-09-001) promises full lifecycle hooks
    /// including `on_resume`. Fired once per resumed node when the caller
    /// supplies a resume value via `resume()`/`resume_stream()`.
    ///
    /// * `node` - name of the node being resumed.
    /// * `resume_value` - the value supplied by the caller to resume the node.
    fn on_resume(&self, node: &str, resume_value: &serde_json::Value) {
        let _ = (node, resume_value);
    }
}

// Rust guideline compliant 2026-05-21
