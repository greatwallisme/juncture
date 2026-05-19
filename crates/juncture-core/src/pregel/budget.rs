//! Budget tracking for Pregel execution
//!
//! This module provides budget tracking for limiting execution based on
//! tokens, cost, duration, and steps.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
#[derive(Debug)]
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
        }
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

// Rust guideline compliant 2026-05-19
