//! Budget tracking for Pregel execution
//!
//! This module provides budget tracking for limiting execution based on
//! tokens, cost, duration, and steps.
//!
//! # Task-Local Budget Tracking
//!
//! The budget tracker is available as a task-local variable during node
//! execution, allowing LLM providers to report token usage without explicit
//! parameter passing. Use [`try_report_model_call`] to report usage from
//! within LLM implementations.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// Task-local budget tracker for LLM usage reporting.
//
// This task-local variable is set by the Pregel execution engine during
// node execution, allowing LLM providers to report token usage without
// requiring explicit parameter passing through the ChatModel trait.
tokio::task_local! {
    pub static BUDGET_TRACKER: Arc<BudgetTracker>;
}

/// Action to take when a budget limit is exceeded
///
/// Controls how the Pregel engine responds when resource usage
/// surpasses the configured budget limits.
#[derive(Clone, Default)]
pub enum BudgetExceededAction {
    /// Terminate execution immediately, returning current state
    #[default]
    Terminate,

    /// Raise a human-in-the-loop interrupt, waiting for manual confirmation
    Interrupt,

    /// Custom handler that decides the action based on current usage
    Custom(std::sync::Arc<dyn Fn(BudgetUsage) -> BudgetExceededAction + Send + Sync>),
}

impl std::fmt::Debug for BudgetExceededAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminate => write!(f, "Terminate"),
            Self::Interrupt => write!(f, "Interrupt"),
            Self::Custom(_) => write!(f, "Custom(<fn>)"),
        }
    }
}

/// Budget configuration limits
///
/// Defines the maximum limits for various budget dimensions.
#[derive(Clone, Default)]
pub struct BudgetConfig {
    /// Maximum tokens to consume (None = unlimited)
    pub max_tokens: Option<u64>,

    /// Maximum cost in USD (None = unlimited)
    pub max_cost_usd: Option<f64>,

    /// Maximum execution duration (None = unlimited)
    pub max_duration: Option<Duration>,

    /// Maximum number of steps (None = unlimited)
    pub max_steps: Option<usize>,

    /// Action to take when budget is exceeded
    pub on_exceeded: BudgetExceededAction,
}

impl std::fmt::Debug for BudgetConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetConfig")
            .field("max_tokens", &self.max_tokens)
            .field("max_cost_usd", &self.max_cost_usd)
            .field("max_duration", &self.max_duration)
            .field("max_steps", &self.max_steps)
            .field("on_exceeded", &self.on_exceeded)
            .finish()
    }
}

impl BudgetConfig {
    /// Create a new budget configuration with no limits
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_tokens: None,
            max_cost_usd: None,
            max_duration: None,
            max_steps: None,
            on_exceeded: BudgetExceededAction::default(),
        }
    }

    /// Set maximum tokens
    #[must_use]
    pub const fn with_max_tokens(mut self, tokens: u64) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set maximum cost in USD
    #[must_use]
    pub const fn with_max_cost_usd(mut self, cost: f64) -> Self {
        self.max_cost_usd = Some(cost);
        self
    }

    /// Set maximum duration
    #[must_use]
    pub const fn with_max_duration(mut self, duration: Duration) -> Self {
        self.max_duration = Some(duration);
        self
    }

    /// Set maximum steps
    #[must_use]
    pub const fn with_max_steps(mut self, steps: usize) -> Self {
        self.max_steps = Some(steps);
        self
    }

    /// Check if this config has any limits set
    #[must_use]
    pub const fn has_limits(&self) -> bool {
        self.max_tokens.is_some()
            || self.max_cost_usd.is_some()
            || self.max_duration.is_some()
            || self.max_steps.is_some()
    }
}

/// Budget tracker for execution limits
///
/// Tracks resource usage during execution and checks against configured limits.
pub struct BudgetTracker {
    /// Tokens used (`AtomicU64` for thread-safe updates)
    tokens_used: AtomicU64,

    /// Cost in micros-USD (scaled by 1M to avoid floats in atomic)
    cost_usd_micros: AtomicU64,

    /// Start time of execution
    start_time: Instant,

    /// Number of steps completed
    steps_completed: AtomicUsize,

    /// Budget configuration
    config: BudgetConfig,

    /// Optional metrics collector for emitting usage metrics
    metrics_collector: Option<std::sync::Arc<dyn crate::observability::MetricsCollector>>,
}

impl std::fmt::Debug for BudgetTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetTracker")
            .field("tokens_used", &self.tokens_used)
            .field("cost_usd_micros", &self.cost_usd_micros)
            .field("start_time", &self.start_time)
            .field("steps_completed", &self.steps_completed)
            .field("config", &self.config)
            .field(
                "metrics_collector",
                &self.metrics_collector.as_ref().map(|_| "<Arc>"),
            )
            .finish()
    }
}

impl BudgetTracker {
    /// Create a new budget tracker with the given configuration
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    /// use std::time::Duration;
    ///
    /// let config = BudgetConfig::new()
    ///     .with_max_tokens(1000)
    ///     .with_max_duration(Duration::from_secs(60));
    /// let tracker = BudgetTracker::new(config);
    /// ```
    #[must_use]
    pub fn new(config: BudgetConfig) -> Self {
        Self {
            tokens_used: AtomicU64::new(0),
            cost_usd_micros: AtomicU64::new(0),
            start_time: Instant::now(),
            steps_completed: AtomicUsize::new(0),
            config,
            metrics_collector: None,
        }
    }

    /// Set the metrics collector for emitting usage metrics
    #[must_use]
    pub fn with_metrics_collector(
        mut self,
        collector: Option<std::sync::Arc<dyn crate::observability::MetricsCollector>>,
    ) -> Self {
        self.metrics_collector = collector;
        self
    }

    /// Report token usage
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_tokens(100);
    /// assert_eq!(tracker.current_usage().tokens_used, 100);
    /// ```
    pub fn report_tokens(&self, tokens: u64) {
        self.tokens_used.fetch_add(tokens, Ordering::Relaxed);

        // Emit token input metric
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.llm.tokens.input", tokens);
        }
    }

    /// Report token output (generated tokens)
    ///
    /// This is called separately to distinguish between input and output tokens
    /// for metrics purposes.
    pub fn report_output_tokens(&self, tokens: u64) {
        // Emit token output metric
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.llm.tokens.output", tokens);
        }
    }

    /// Report cost in USD
    ///
    /// Cost is stored internally in micros-USD (1/1,000,000 of a USD) to avoid
    /// floating-point arithmetic in atomic operations.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_cost(0.001); // 0.001 USD
    /// assert_eq!(tracker.current_usage().cost_usd, 0.001);
    /// ```
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "cost values are expected to be positive and within reasonable bounds"
    )]
    pub fn report_cost(&self, cost_usd: f64) {
        // Convert to micros-USD (multiply by 1M)
        let cost_micros = (cost_usd * 1_000_000.0) as u64;
        self.cost_usd_micros
            .fetch_add(cost_micros, Ordering::Relaxed);

        // Emit cost metric (convert to integer micro-units for counter)
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.llm.cost_usd", cost_micros);
        }
    }

    /// Report a completed step
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_step();
    /// tracker.report_step();
    /// assert_eq!(tracker.current_usage().steps_completed, 2);
    /// ```
    pub fn report_step(&self) {
        self.steps_completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Report an LLM call
    ///
    /// This should be called when an LLM invocation completes successfully.
    pub fn report_llm_call(&self) {
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.llm.calls", 1);
        }
    }

    /// Report LLM call duration in milliseconds
    ///
    /// This should be called when an LLM invocation completes.
    #[allow(
        clippy::cast_precision_loss,
        reason = "milliseconds as f64 is sufficient for histogram metrics"
    )]
    pub fn report_llm_duration(&self, duration_ms: u64) {
        if let Some(ref collector) = self.metrics_collector {
            collector.record_histogram("juncture.llm.duration_ms", duration_ms as f64);
        }
    }

    /// Report tool call
    ///
    /// This should be called when a tool invocation completes.
    pub fn report_tool_call(&self) {
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.tool.calls", 1);
        }
    }

    /// Report tool error
    ///
    /// This should be called when a tool invocation fails.
    pub fn report_tool_error(&self) {
        if let Some(ref collector) = self.metrics_collector {
            collector.inc_counter("juncture.tool.errors", 1);
        }
    }

    /// Report tool execution duration in milliseconds
    ///
    /// This should be called when a tool invocation completes.
    #[allow(
        clippy::cast_precision_loss,
        reason = "milliseconds as f64 is sufficient for histogram metrics"
    )]
    pub fn report_tool_duration(&self, duration_ms: u64) {
        if let Some(ref collector) = self.metrics_collector {
            collector.record_histogram("juncture.tool.duration_ms", duration_ms as f64);
        }
    }

    /// Report token and cost usage from a model call
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_usage(1000, 0.001);
    /// assert_eq!(tracker.current_usage().tokens_used, 1000);
    /// assert!((tracker.current_usage().cost_usd - 0.001).abs() < 0.0001);
    /// ```
    pub fn report_usage(&self, tokens: u64, cost_usd: f64) {
        self.report_tokens(tokens);
        self.report_cost(cost_usd);
    }

    /// Report token usage from a model call with separate input/output counts
    ///
    /// Adds the sum of input and output tokens to the total token counter.
    /// This is the primary method for integrating LLM provider usage reporting
    /// with budget enforcement.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_model_call(50, 150);
    /// assert_eq!(tracker.current_usage().tokens_used, 200);
    /// ```
    pub fn report_model_call(&self, input_tokens: u64, output_tokens: u64) {
        self.tokens_used
            .fetch_add(input_tokens + output_tokens, Ordering::Relaxed);
    }

    /// Check if any budget limit has been exceeded
    ///
    /// Returns `Some(BudgetExceededReason)` if a limit was exceeded,
    /// or `None` if all limits are within bounds.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    /// use std::time::Duration;
    ///
    /// let config = BudgetConfig::new().with_max_tokens(100);
    /// let tracker = BudgetTracker::new(config);
    ///
    /// tracker.report_tokens(150);
    /// assert!(tracker.check().is_some());
    /// ```
    #[must_use]
    pub fn check(&self) -> Option<BudgetExceededReason> {
        // Check token limit
        if let Some(max_tokens) = self.config.max_tokens
            && self.tokens_used.load(Ordering::Relaxed) > max_tokens
        {
            return Some(BudgetExceededReason::Tokens {
                used: self.tokens_used.load(Ordering::Relaxed),
                limit: max_tokens,
            });
        }

        // Check cost limit
        if let Some(max_cost) = self.config.max_cost_usd {
            #[allow(
                clippy::cast_precision_loss,
                reason = "precision loss is acceptable for cost comparison"
            )]
            let cost_micros = self.cost_usd_micros.load(Ordering::Relaxed);
            #[allow(
                clippy::cast_precision_loss,
                reason = "precision loss is acceptable for cost comparison"
            )]
            let cost_usd = cost_micros as f64 / 1_000_000.0;
            if cost_usd > max_cost {
                return Some(BudgetExceededReason::Cost {
                    used: cost_usd,
                    limit: max_cost,
                });
            }
        }

        // Check duration limit
        if let Some(max_duration) = self.config.max_duration
            && self.start_time.elapsed() > max_duration
        {
            return Some(BudgetExceededReason::Duration {
                used: self.start_time.elapsed(),
                limit: max_duration,
            });
        }

        // Check step limit
        if let Some(max_steps) = self.config.max_steps
            && self.steps_completed.load(Ordering::Relaxed) > max_steps
        {
            return Some(BudgetExceededReason::Steps {
                used: self.steps_completed.load(Ordering::Relaxed),
                limit: max_steps,
            });
        }

        None
    }

    /// Get current usage statistics
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::{BudgetTracker, BudgetConfig};
    ///
    /// let tracker = BudgetTracker::new(BudgetConfig::new());
    /// tracker.report_tokens(100);
    /// tracker.report_cost(0.01);
    /// tracker.report_step();
    ///
    /// let usage = tracker.current_usage();
    /// assert_eq!(usage.tokens_used, 100);
    /// assert_eq!(usage.cost_usd, 0.01);
    /// assert_eq!(usage.steps_completed, 1);
    /// assert!(usage.duration.as_secs() < 1);
    /// ```
    #[must_use]
    pub fn current_usage(&self) -> BudgetUsage {
        let cost_micros = self.cost_usd_micros.load(Ordering::Relaxed);
        #[allow(
            clippy::cast_precision_loss,
            reason = "precision loss is acceptable for cost display"
        )]
        BudgetUsage {
            tokens_used: self.tokens_used.load(Ordering::Relaxed),
            cost_usd: cost_micros as f64 / 1_000_000.0,
            duration: self.start_time.elapsed(),
            steps_completed: self.steps_completed.load(Ordering::Relaxed),
        }
    }
}

/// Reason why budget was exceeded
#[derive(Clone, Debug)]
pub enum BudgetExceededReason {
    /// Token limit exceeded
    Tokens { used: u64, limit: u64 },

    /// Cost limit exceeded
    Cost { used: f64, limit: f64 },

    /// Duration limit exceeded
    Duration { used: Duration, limit: Duration },

    /// Step limit exceeded
    Steps { used: usize, limit: usize },
}

impl std::fmt::Display for BudgetExceededReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tokens { used, limit } => {
                write!(f, "Token budget exceeded: {used} > {limit}")
            }
            Self::Cost { used, limit } => {
                write!(f, "Cost budget exceeded: ${used:.6} > ${limit:.6}")
            }
            Self::Duration { used, limit } => {
                write!(f, "Duration budget exceeded: {used:?} > {limit:?}")
            }
            Self::Steps { used, limit } => {
                write!(f, "Step budget exceeded: {used} > {limit}")
            }
        }
    }
}

/// Current budget usage statistics
#[derive(Clone, Debug)]
pub struct BudgetUsage {
    /// Tokens consumed
    pub tokens_used: u64,

    /// Cost in USD
    pub cost_usd: f64,

    /// Time elapsed since start
    pub duration: Duration,

    /// Number of steps completed
    pub steps_completed: usize,
}

/// Attempt to report model call token usage to the current budget tracker.
///
/// This function accesses the task-local budget tracker set by the Pregel
/// execution engine and reports token usage from an LLM call. It returns
/// an error if called outside of a task-local budget tracker context.
///
/// This is the preferred method for LLM providers to report usage, as it
/// integrates seamlessly with the budget tracking system without requiring
/// explicit parameter passing.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a task-local
/// budget tracker context (e.g., during direct LLM calls without graph execution).
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::budget::try_report_model_call;
///
/// // In an LLM provider's invoke() method
/// if let Some(usage) = &response.usage {
///     let _ = try_report_model_call(usage.input_tokens, usage.output_tokens);
/// }
/// ```
pub fn try_report_model_call(
    input_tokens: u64,
    output_tokens: u64,
) -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_model_call(input_tokens, output_tokens);
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Report an LLM call completion (for metrics)
///
/// This function reports that an LLM call completed successfully,
/// incrementing the call counter metric.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a graph execution context.
pub fn try_report_llm_call() -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_llm_call();
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Report LLM call duration in milliseconds (for metrics)
///
/// This function records the duration of an LLM call in a histogram metric.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a graph execution context.
pub fn try_report_llm_duration(duration_ms: u64) -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_llm_duration(duration_ms);
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Report a tool call (for metrics)
///
/// This function reports that a tool was invoked, incrementing the call counter.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a graph execution context.
pub fn try_report_tool_call() -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_tool_call();
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Report a tool error (for metrics)
///
/// This function reports that a tool invocation failed, incrementing the error counter.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a graph execution context.
pub fn try_report_tool_error() -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_tool_error();
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Report tool execution duration in milliseconds (for metrics)
///
/// This function records the duration of a tool call in a histogram metric.
///
/// # Errors
///
/// Returns `BudgetReportError::NoTracker` if called outside of a graph execution context.
pub fn try_report_tool_duration(duration_ms: u64) -> Result<(), BudgetReportError> {
    BUDGET_TRACKER
        .try_with(|tracker| {
            tracker.report_tool_duration(duration_ms);
        })
        .map_err(|_err| BudgetReportError::NoTracker)
}

/// Error returned when budget reporting fails.
///
/// This error indicates that budget reporting was attempted outside of
/// a task-local budget tracker context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetReportError {
    /// No budget tracker is available in the current context.
    ///
    /// This typically means the LLM call is happening outside of graph
    /// execution, where no budget tracker has been configured.
    NoTracker,
}

impl std::fmt::Display for BudgetReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTracker => write!(
                f,
                "Cannot report budget usage: no budget tracker in current context"
            ),
        }
    }
}

impl std::error::Error for BudgetReportError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_config_no_limits() {
        let config = BudgetConfig::new();
        assert!(!config.has_limits());
    }

    #[test]
    fn test_budget_config_with_limits() {
        let config = BudgetConfig::new().with_max_tokens(1000).with_max_steps(10);

        assert!(config.has_limits());
    }

    #[test]
    fn test_budget_tracker_tokens() {
        let config = BudgetConfig::new().with_max_tokens(100);
        let tracker = BudgetTracker::new(config);

        tracker.report_tokens(50);
        assert!(tracker.check().is_none());

        tracker.report_tokens(60);
        assert!(tracker.check().is_some());

        let usage = tracker.current_usage();
        assert_eq!(usage.tokens_used, 110);
    }

    #[test]
    fn test_budget_tracker_cost() {
        let config = BudgetConfig::new().with_max_cost_usd(0.01);
        let tracker = BudgetTracker::new(config);

        tracker.report_cost(0.005);
        assert!(tracker.check().is_none());

        tracker.report_cost(0.006);
        assert!(tracker.check().is_some());

        let usage = tracker.current_usage();
        assert!((usage.cost_usd - 0.011).abs() < 0.0001);
    }

    #[test]
    fn test_budget_tracker_steps() {
        let config = BudgetConfig::new().with_max_steps(5);
        let tracker = BudgetTracker::new(config);

        for _ in 0..5 {
            tracker.report_step();
        }
        assert!(tracker.check().is_none());

        tracker.report_step();
        assert!(tracker.check().is_some());

        let usage = tracker.current_usage();
        assert_eq!(usage.steps_completed, 6);
    }

    #[test]
    fn test_budget_tracker_model_call() {
        let tracker = BudgetTracker::new(BudgetConfig::new());
        assert_eq!(tracker.current_usage().tokens_used, 0);

        tracker.report_model_call(50, 100);
        assert_eq!(tracker.current_usage().tokens_used, 150);

        tracker.report_model_call(10, 20);
        assert_eq!(tracker.current_usage().tokens_used, 180);
    }

    #[test]
    fn test_budget_tracker_model_call_exceeds_limit() {
        let config = BudgetConfig::new().with_max_tokens(100);
        let tracker = BudgetTracker::new(config);

        assert!(tracker.check().is_none());
        tracker.report_model_call(60, 50);
        assert!(tracker.check().is_some());
    }

    #[test]
    fn test_budget_tracker_duration() {
        let config = BudgetConfig::new().with_max_duration(Duration::from_millis(100));
        let tracker = BudgetTracker::new(config);

        assert!(tracker.check().is_none());
        std::thread::sleep(Duration::from_millis(150));
        assert!(tracker.check().is_some());
    }

    #[test]
    fn test_budget_exceeded_reason_display() {
        let reason = BudgetExceededReason::Tokens {
            used: 150,
            limit: 100,
        };
        assert!(reason.to_string().contains("Token budget exceeded"));

        let reason = BudgetExceededReason::Steps { used: 10, limit: 5 };
        assert!(reason.to_string().contains("Step budget exceeded"));
    }
}

// Rust guideline compliant 2026-05-20
