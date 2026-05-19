//! Execution context and configuration for Pregel engine
//!
//! Provides the mutable execution state ([`ExecutionContext`]) and
//! immutable configuration ([`ExecutionConfig`]) used by the Pregel
//! loop during graph execution.

use crate::graph::RetryPolicy;
use crate::pregel::budget::BudgetConfig;
use crate::pregel::durability::Durability;
use crate::pregel::scheduler::{FieldVersionTracker, VersionsSeen};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Mutable execution context: holds state and version tracking
///
/// Encapsulates the mutable portions of the Pregel loop that change
/// during execution, separating them from the immutable configuration.
pub struct ExecutionContext<S: crate::State> {
    /// Current state
    pub state: S,

    /// Field version tracker (equivalent to `channel_versions`)
    pub field_versions: FieldVersionTracker,

    /// Versions seen by each node (equivalent to `versions_seen`)
    pub versions_seen: VersionsSeen,

    /// Pending writes for checkpoint recovery
    pub pending_writes: Vec<crate::checkpoint::PendingWrite>,
}

impl<S: crate::State> std::fmt::Debug for ExecutionContext<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionContext")
            .field("state", &"<state>")
            .field("field_versions", &self.field_versions)
            .field("versions_seen", &self.versions_seen)
            .field("pending_writes", &self.pending_writes.len())
            .finish()
    }
}

/// Immutable execution configuration
///
/// Holds the runtime parameters that do not change during execution,
/// including recursion limits, interrupt settings, and per-node policies.
pub struct ExecutionConfig {
    /// Maximum superstep count
    pub recursion_limit: usize,

    /// Nodes to interrupt before execution
    pub interrupt_before: HashSet<String>,

    /// Nodes to interrupt after execution
    pub interrupt_after: HashSet<String>,

    /// Budget configuration
    pub budget: Option<BudgetConfig>,

    /// Checkpoint durability mode
    pub durability: Durability,

    /// Per-node retry policies
    pub retry_policies: HashMap<String, RetryPolicy>,

    /// Per-node timeout policies
    pub timeout_policies: HashMap<String, TimeoutPolicy>,
}

impl std::fmt::Debug for ExecutionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionConfig")
            .field("recursion_limit", &self.recursion_limit)
            .field("interrupt_before", &self.interrupt_before)
            .field("interrupt_after", &self.interrupt_after)
            .field("budget", &self.budget)
            .field("durability", &self.durability)
            .field(
                "retry_policies",
                &self.retry_policies.keys().collect::<Vec<_>>(),
            )
            .field(
                "timeout_policies",
                &self.timeout_policies.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            recursion_limit: 25,
            interrupt_before: HashSet::new(),
            interrupt_after: HashSet::new(),
            budget: None,
            durability: Durability::default(),
            retry_policies: HashMap::new(),
            timeout_policies: HashMap::new(),
        }
    }
}

impl ExecutionConfig {
    /// Create a new execution config with defaults
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the recursion limit
    #[must_use]
    pub const fn with_recursion_limit(mut self, limit: usize) -> Self {
        self.recursion_limit = limit;
        self
    }

    /// Set `interrupt_before` nodes
    #[must_use]
    pub fn with_interrupt_before(mut self, nodes: HashSet<String>) -> Self {
        self.interrupt_before = nodes;
        self
    }

    /// Set `interrupt_after` nodes
    #[must_use]
    pub fn with_interrupt_after(mut self, nodes: HashSet<String>) -> Self {
        self.interrupt_after = nodes;
        self
    }

    /// Set budget configuration
    #[must_use]
    pub fn with_budget(mut self, budget: BudgetConfig) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set durability mode
    #[must_use]
    pub const fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }
}

/// Node-level timeout policy
///
/// Prevents LLM calls or tool execution from blocking indefinitely.
/// Configured per-node through the graph builder.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::context::TimeoutPolicy;
/// use std::time::Duration;
///
/// let policy = TimeoutPolicy::default()
///     .with_run_timeout(Duration::from_secs(60));
/// ```
#[derive(Clone)]
pub struct TimeoutPolicy {
    /// Maximum runtime for a single execution
    pub run_timeout: Duration,

    /// Idle timeout: if no progress signal within this time, consider timed out
    pub idle_timeout: Option<Duration>,

    /// Progress signal detector (refreshes `idle_timeout` on heartbeat)
    ///
    /// Receives a serializable event representation containing event type and node name.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub refresh_on: Option<std::sync::Arc<dyn Fn(&serde_json::Value) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for TimeoutPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutPolicy")
            .field("run_timeout", &self.run_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("refresh_on", &self.refresh_on.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            run_timeout: Duration::from_secs(300),
            idle_timeout: None,
            refresh_on: None,
        }
    }
}

impl TimeoutPolicy {
    /// Create a new timeout policy with defaults
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the run timeout
    #[must_use]
    pub const fn with_run_timeout(mut self, timeout: Duration) -> Self {
        self.run_timeout = timeout;
        self
    }

    /// Set the idle timeout
    #[must_use]
    pub const fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_config_default() {
        let config = ExecutionConfig::default();
        assert_eq!(config.recursion_limit, 25);
        assert!(config.interrupt_before.is_empty());
        assert!(config.interrupt_after.is_empty());
        assert!(config.budget.is_none());
        assert!(config.retry_policies.is_empty());
        assert!(config.timeout_policies.is_empty());
    }

    #[test]
    fn test_execution_config_builder() {
        let config = ExecutionConfig::new()
            .with_recursion_limit(50)
            .with_interrupt_before(HashSet::from(["node_a".to_string()]))
            .with_durability(Durability::Async);

        assert_eq!(config.recursion_limit, 50);
        assert!(config.interrupt_before.contains("node_a"));
        assert!(matches!(config.durability, Durability::Async));
    }

    #[test]
    fn test_execution_config_debug() {
        let config = ExecutionConfig::new();
        let debug = format!("{config:?}");
        assert!(debug.contains("recursion_limit"));
        assert!(debug.contains("25"));
    }

    #[test]
    fn test_timeout_policy_default() {
        let policy = TimeoutPolicy::default();
        assert_eq!(policy.run_timeout, Duration::from_secs(300));
        assert!(policy.idle_timeout.is_none());
        assert!(policy.refresh_on.is_none());
    }

    #[test]
    fn test_timeout_policy_builder() {
        let policy = TimeoutPolicy::new()
            .with_run_timeout(Duration::from_secs(60))
            .with_idle_timeout(Duration::from_secs(10));

        assert_eq!(policy.run_timeout, Duration::from_secs(60));
        assert_eq!(policy.idle_timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn test_timeout_policy_debug() {
        let policy = TimeoutPolicy::default();
        let debug = format!("{policy:?}");
        assert!(debug.contains("run_timeout"));
        assert!(debug.contains("300s"));
    }
}

// Rust guideline compliant 2026-05-20
