//! Configuration types for graph execution
//!
//! Provides [`RunnableConfig`] for controlling graph execution behavior,
//! including concurrency limits, checkpoint settings, caching, and
//! cancellation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::checkpoint::CheckpointSaver;
use crate::interrupt::ResumeValue;
use crate::observability::{
    CachePolicy as LlmCachePolicy, GraphLifecycleCallback, MetricsCollector,
};
use crate::pregel::BudgetConfig;
use crate::pregel::Durability;
use crate::runtime::RuntimeStore;

/// Configuration for graph execution
#[derive(Clone, Default)]
pub struct RunnableConfig {
    /// Thread ID for checkpoint isolation
    pub thread_id: Option<String>,

    /// Checkpoint ID to resume from (time-travel)
    pub checkpoint_id: Option<String>,

    /// Maximum superstep count (default 25)
    pub recursion_limit: usize,

    /// Maximum parallel tasks (for bounded concurrency)
    pub max_parallel_tasks: usize,

    /// Run name for observability
    pub run_name: Option<String>,

    /// Graph name for observability (specified at graph construction time)
    pub graph_name: Option<String>,

    /// Unique run identifier for logging, stream resumption, and cancellation.
    ///
    /// When `None`, the execution layer (`CompiledGraph::stream`, `invoke`, etc.)
    /// generates a new `UUIDv4` automatically before creating the Pregel loop.
    /// Callers may set this explicitly to correlate multiple operations with
    /// the same run ID (e.g., for stream resumption or distributed tracing).
    pub run_id: Option<String>,

    /// Checkpoint namespace (for subgraph isolation)
    pub checkpoint_ns: Option<crate::checkpoint::CheckpointNamespace>,

    /// Cache configuration
    pub cache: Option<CacheConfig>,

    /// Tags for filtering
    pub tags: Vec<String>,

    /// User metadata
    pub metadata: HashMap<String, serde_json::Value>,

    /// Cancellation token for aborting execution
    pub cancellation_token: Option<tokio_util::sync::CancellationToken>,

    /// Budget configuration for execution limits
    pub budget: Option<BudgetConfig>,

    /// Checkpoint durability mode
    pub durability: Option<Durability>,

    /// Callback invoked when a node finishes execution
    #[allow(
        clippy::type_complexity,
        reason = "trait object callback requires full signature"
    )]
    pub node_finished_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,

    /// Resume value for HITL interrupt continuation
    ///
    /// Supports single value, ID-based resume, and namespace-based resume
    /// for multi-interrupt workflows.
    pub resume_value: Option<ResumeValue>,

    /// Nodes that should interrupt before execution (HITL)
    pub interrupt_before: Option<Vec<String>>,

    /// Nodes that should interrupt after execution (HITL)
    pub interrupt_after: Option<Vec<String>>,

    /// Optional metrics collector for OpenTelemetry or in-memory metrics
    pub metrics_collector: Option<Arc<dyn MetricsCollector>>,

    /// Optional callback handler for graph lifecycle events
    ///
    /// Receives notifications at key points during graph execution:
    /// node start/end/error, graph completion, and checkpoint saves.
    /// All methods have default no-op implementations.
    pub callback_handler: Option<Arc<dyn GraphLifecycleCallback>>,

    /// LLM response cache policy for controlling key generation and TTL
    pub llm_cache_policy: Option<LlmCachePolicy>,
}

impl std::fmt::Debug for RunnableConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnableConfig")
            .field("thread_id", &self.thread_id)
            .field("checkpoint_id", &self.checkpoint_id)
            .field("recursion_limit", &self.recursion_limit)
            .field("max_parallel_tasks", &self.max_parallel_tasks)
            .field("run_name", &self.run_name)
            .field("graph_name", &self.graph_name)
            .field("run_id", &self.run_id)
            .field("checkpoint_ns", &self.checkpoint_ns)
            .field("cache", &self.cache)
            .field("tags", &self.tags)
            .field("metadata", &self.metadata)
            .field(
                "cancellation_token",
                &self
                    .cancellation_token
                    .as_ref()
                    .map(|_| "CancellationToken"),
            )
            .field("budget", &self.budget)
            .field("durability", &self.durability)
            .field(
                "node_finished_callback",
                &self.node_finished_callback.as_ref().map(|_| "<fn>"),
            )
            .field("resume_value", &self.resume_value)
            .field("interrupt_before", &self.interrupt_before)
            .field("interrupt_after", &self.interrupt_after)
            .field(
                "metrics_collector",
                &self
                    .metrics_collector
                    .as_ref()
                    .map(|_| "<MetricsCollector>"),
            )
            .field(
                "callback_handler",
                &self
                    .callback_handler
                    .as_ref()
                    .map(|_| "<GraphLifecycleCallback>"),
            )
            .field(
                "llm_cache_policy",
                &self.llm_cache_policy.as_ref().map(|_| "<CachePolicy>"),
            )
            .finish()
    }
}

impl RunnableConfig {
    /// Create a new configuration with sensible defaults
    #[must_use]
    pub fn new() -> Self {
        Self {
            recursion_limit: 25,
            max_parallel_tasks: 100,
            ..Default::default()
        }
    }

    /// Set the thread ID for checkpoint isolation
    #[must_use]
    pub fn with_thread_id(mut self, id: impl Into<String>) -> Self {
        self.thread_id = Some(id.into());
        self
    }

    /// Set the checkpoint ID for time-travel resume
    #[must_use]
    pub fn with_checkpoint_id(mut self, id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(id.into());
        self
    }

    /// Set the recursion limit (maximum superstep count)
    #[must_use]
    pub const fn with_recursion_limit(mut self, limit: usize) -> Self {
        self.recursion_limit = limit;
        self
    }

    /// Set the maximum number of parallel tasks
    #[must_use]
    pub const fn with_max_parallel_tasks(mut self, max: usize) -> Self {
        self.max_parallel_tasks = max;
        self
    }

    /// Set the run name for observability
    #[must_use]
    pub fn with_run_name(mut self, name: impl Into<String>) -> Self {
        self.run_name = Some(name.into());
        self
    }

    /// Set the graph name for observability
    #[must_use]
    pub fn with_graph_name(mut self, name: impl Into<String>) -> Self {
        self.graph_name = Some(name.into());
        self
    }

    /// Set the checkpoint namespace for subgraph isolation
    #[must_use]
    pub fn with_checkpoint_ns(mut self, ns: crate::checkpoint::CheckpointNamespace) -> Self {
        self.checkpoint_ns = Some(ns);
        self
    }

    /// Set cache configuration
    #[must_use]
    pub fn with_cache(mut self, cache: CacheConfig) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Add a tag for filtering
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add metadata key-value pair
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Set the cancellation token for aborting execution
    #[must_use]
    pub fn with_cancellation_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.cancellation_token = Some(token);
        self
    }

    /// Set the budget configuration for execution limits
    #[must_use]
    pub fn with_budget(mut self, budget: BudgetConfig) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set `interrupt_before` nodes (HITL - interrupt before node execution)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::config::RunnableConfig;
    ///
    /// let config = RunnableConfig::new()
    ///     .with_interrupt_before(vec!["human_input".to_string()]);
    /// ```
    #[must_use]
    pub fn with_interrupt_before(mut self, nodes: Vec<String>) -> Self {
        self.interrupt_before = Some(nodes);
        self
    }

    /// Set `interrupt_after` nodes (HITL - interrupt after node execution)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::config::RunnableConfig;
    ///
    /// let config = RunnableConfig::new()
    ///     .with_interrupt_after(vec!["confirmation".to_string()]);
    /// ```
    #[must_use]
    pub fn with_interrupt_after(mut self, nodes: Vec<String>) -> Self {
        self.interrupt_after = Some(nodes);
        self
    }

    /// Set the metrics collector for observability
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use juncture_core::config::RunnableConfig;
    /// use juncture_core::observability::MetricsCollector;
    ///
    /// let collector: Arc<dyn MetricsCollector> = /* ... */;
    /// let config = RunnableConfig::new()
    ///     .with_metrics_collector(collector);
    /// ```
    #[must_use]
    pub fn with_metrics_collector(mut self, collector: Arc<dyn MetricsCollector>) -> Self {
        self.metrics_collector = Some(collector);
        self
    }

    /// Set the callback handler for graph lifecycle events
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use juncture_core::config::RunnableConfig;
    /// use juncture_core::observability::GraphLifecycleCallback;
    ///
    /// let handler: Arc<dyn GraphLifecycleCallback> = /* ... */;
    /// let config = RunnableConfig::new()
    ///     .with_callback_handler(handler);
    /// ```
    #[must_use]
    pub fn with_callback_handler(mut self, handler: Arc<dyn GraphLifecycleCallback>) -> Self {
        self.callback_handler = Some(handler);
        self
    }

    /// Set the LLM response cache policy
    #[must_use]
    pub fn with_llm_cache_policy(mut self, policy: LlmCachePolicy) -> Self {
        self.llm_cache_policy = Some(policy);
        self
    }
}

/// Cache configuration for node results
#[derive(Clone, Debug)]
pub struct CacheConfig {
    /// Cache policy
    pub policy: CachePolicy,
}

/// Cache policy controlling how node results are cached
///
/// Supports default (state-hash-based), TTL-based, and custom key
/// function caching strategies.
#[derive(Clone)]
pub struct CachePolicy {
    /// Optional custom key function for cache key generation
    ///
    /// When set, this function computes the cache key from the current
    /// state value and execution config. When unset, the default
    /// state-hash-based key is used.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub key_func: Option<Arc<dyn Fn(&serde_json::Value, &RunnableConfig) -> String + Send + Sync>>,

    /// Optional time-to-live for cached entries
    pub ttl: Option<Duration>,

    /// Optional maximum number of cache entries
    pub max_entries: Option<usize>,
}

impl std::fmt::Debug for CachePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachePolicy")
            .field("key_func", &self.key_func.as_ref().map(|_| "<fn>"))
            .field("ttl", &self.ttl)
            .field("max_entries", &self.max_entries)
            .finish()
    }
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

impl CachePolicy {
    /// Create the default cache policy (state-hash-based, no TTL)
    #[must_use]
    pub fn default_policy() -> Self {
        Self {
            key_func: None,
            ttl: None,
            max_entries: None,
        }
    }

    /// Create a TTL-based cache policy
    ///
    /// Cached entries expire after the specified duration.
    #[must_use]
    pub fn ttl(duration: Duration) -> Self {
        Self {
            key_func: None,
            ttl: Some(duration),
            max_entries: None,
        }
    }

    /// Create a custom-key cache policy
    ///
    /// Uses the provided function to compute cache keys instead of
    /// the default state-hash-based approach.
    #[must_use]
    pub fn custom_key(
        key_func: impl Fn(&serde_json::Value, &RunnableConfig) -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            key_func: Some(Arc::new(key_func)),
            ttl: None,
            max_entries: None,
        }
    }
}

/// Task-level configuration for node execution
///
/// Overrides or supplements the graph-level [`RunnableConfig`] for
/// individual tasks, providing per-node retry, caching, and timeout
/// settings.
#[derive(Clone, Debug, Default)]
pub struct TaskConfig {
    /// Retry policy for this task
    pub retry_policy: Option<crate::graph::RetryPolicy>,

    /// Cache policy for this task
    pub cache_policy: Option<CachePolicy>,

    /// Timeout duration for this task
    pub timeout: Option<Duration>,

    /// Optional task name override
    pub name: Option<String>,
}

/// Entry point configuration for graph execution
///
/// Specifies the checkpointer and store to use when starting
/// a graph execution, enabling persistence and cross-thread
/// state management.
#[derive(Clone, Default)]
pub struct EntrypointConfig {
    /// Optional checkpointer for state persistence
    pub checkpointer: Option<Arc<dyn CheckpointSaver>>,

    /// Optional store for cross-thread state
    pub store: Option<Arc<dyn RuntimeStore>>,
}

impl std::fmt::Debug for EntrypointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntrypointConfig")
            .field(
                "checkpointer",
                &self.checkpointer.as_ref().map(|_| "<CheckpointSaver>"),
            )
            .field("store", &self.store.as_ref().map(|_| "<RuntimeStore>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runnable_config_new() {
        let config = RunnableConfig::new();
        assert_eq!(config.recursion_limit, 25);
        assert_eq!(config.max_parallel_tasks, 100);
        assert!(config.thread_id.is_none());
        assert!(config.checkpoint_id.is_none());
        assert!(config.cancellation_token.is_none());
        assert!(config.budget.is_none());
        assert!(config.durability.is_none());
        assert!(config.resume_value.is_none());
    }

    #[test]
    fn test_runnable_config_with_cancellation_token() {
        let token = tokio_util::sync::CancellationToken::new();
        let config = RunnableConfig::new().with_cancellation_token(token);
        assert!(config.cancellation_token.is_some());
    }

    #[test]
    fn test_runnable_config_with_budget() {
        let budget = BudgetConfig::new().with_max_tokens(1000);
        let config = RunnableConfig::new().with_budget(budget);
        assert!(config.budget.is_some());
        assert_eq!(config.budget.as_ref().unwrap().max_tokens, Some(1000));
    }

    #[test]
    fn test_cache_policy_default() {
        let policy = CachePolicy::default_policy();
        assert!(policy.key_func.is_none());
        assert!(policy.ttl.is_none());
        assert!(policy.max_entries.is_none());
    }

    #[test]
    fn test_cache_policy_ttl() {
        let policy = CachePolicy::ttl(Duration::from_secs(60));
        assert!(policy.key_func.is_none());
        assert_eq!(policy.ttl, Some(Duration::from_secs(60)));
        assert!(policy.max_entries.is_none());
    }

    #[test]
    fn test_cache_policy_custom_key() {
        let policy =
            CachePolicy::custom_key(|val, _cfg| format!("key-{}", val.as_str().unwrap_or("")));
        assert!(policy.key_func.is_some());
        assert!(policy.ttl.is_none());
        assert!(policy.max_entries.is_none());

        // Verify the key function works
        let config = RunnableConfig::new();
        let key = (policy.key_func.as_ref().unwrap())(&serde_json::json!("test"), &config);
        assert_eq!(key, "key-test");
    }

    #[test]
    fn test_cache_policy_default_trait() {
        let policy = CachePolicy::default();
        assert!(policy.key_func.is_none());
        assert!(policy.ttl.is_none());
        assert!(policy.max_entries.is_none());
    }

    #[test]
    fn test_cache_policy_debug() {
        let policy = CachePolicy::ttl(Duration::from_secs(30));
        let debug_str = format!("{policy:?}");
        assert!(debug_str.contains("ttl"));
        assert!(debug_str.contains("30s"));
    }

    #[test]
    fn test_task_config_default() {
        let config = TaskConfig::default();
        assert!(config.retry_policy.is_none());
        assert!(config.cache_policy.is_none());
        assert!(config.timeout.is_none());
        assert!(config.name.is_none());
    }

    #[test]
    fn test_entrypoint_config_default() {
        let config = EntrypointConfig::default();
        assert!(config.checkpointer.is_none());
        assert!(config.store.is_none());
    }

    #[test]
    fn test_runnable_config_debug_format() {
        let config = RunnableConfig::new()
            .with_thread_id("t1")
            .with_run_name("test-run");
        let debug = format!("{config:?}");
        assert!(debug.contains("t1"));
        assert!(debug.contains("test-run"));
    }
}

// Rust guideline compliant 2026-05-19
