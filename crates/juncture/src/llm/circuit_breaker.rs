//! Circuit breaker middleware for LLM invocation resilience.
//!
//! Implements the circuit breaker pattern to prevent cascading failures
//! when the LLM provider is experiencing issues. The circuit breaker has
//! three states: `Closed` (normal), `Open` (failing), and `HalfOpen` (testing recovery).
//!
//! # State Machine
//!
//! ```text
//! Closed --(failure_threshold reached)--> Open
//! Open --(recovery_timeout elapsed)--> HalfOpen
//! HalfOpen --(success)--> Closed
//! HalfOpen --(failure)--> Open
//! ```
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
//! use juncture::llm::middleware::CircuitBreaker;
//! use juncture::llm::circuit_breaker::{CircuitBreakerConfig, CircuitState};
//! use std::time::Duration;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
//!
//! // Configure circuit breaker with custom thresholds
//! let config = CircuitBreakerConfig {
//!     failure_threshold: 3,
//!     recovery_timeout: Duration::from_secs(30),
//!     half_open_max_calls: 1,
//! };
//! let breaker = CircuitBreaker::new(config);
//!
//! let model = MiddlewareModel::new(base_model)
//!     .with_middleware(breaker.clone());
//!
//! let messages = vec![Message::human("Hi")];
//! let response = model.invoke(&messages, None).await?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::llm::middleware::LlmMiddleware;
use crate::llm::{CallOptions, LlmError, Message};

// Circuit state constants (used as u8 values)
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Circuit breaker state.
///
/// Represents the current state of the circuit breaker in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed, allowing requests through.
    ///
    /// This is the normal operating state where all requests are allowed.
    Closed,

    /// Circuit is open, rejecting all requests.
    ///
    /// The circuit has detected too many failures and is blocking requests
    /// to prevent cascading failures. After the recovery timeout, it will
    /// transition to `HalfOpen` to test if the service has recovered.
    Open,

    /// Circuit is half-open, allowing limited test requests.
    ///
    /// The circuit is testing whether the service has recovered by allowing
    /// a limited number of requests through. If these succeed, it transitions
    /// to Closed; if they fail, it returns to Open.
    HalfOpen,
}

#[allow(
    clippy::match_same_arms,
    reason = "Each enum variant maps to its corresponding state value"
)]
impl CircuitState {
    const fn from_u8(value: u8) -> Self {
        match value {
            STATE_CLOSED => Self::Closed,
            STATE_OPEN => Self::Open,
            STATE_HALF_OPEN => Self::HalfOpen,
            _ => Self::Closed, // Default to Closed for invalid values
        }
    }

    #[allow(
        dead_code,
        reason = "Method provided for API completeness, may be used in future"
    )]
    const fn as_u8(self) -> u8 {
        match self {
            Self::Closed => STATE_CLOSED,
            Self::Open => STATE_OPEN,
            Self::HalfOpen => STATE_HALF_OPEN,
        }
    }
}

/// Circuit breaker configuration.
///
/// Defines the thresholds and timeouts that control circuit breaker behavior.
#[derive(Clone, Debug)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    ///
    /// Default: 5
    pub failure_threshold: u32,

    /// Duration to wait before transitioning from `Open` to `HalfOpen`.
    ///
    /// Default: 30 seconds
    pub recovery_timeout: Duration,

    /// Maximum number of calls allowed in `HalfOpen` state for testing.
    ///
    /// Default: 1
    pub half_open_max_calls: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        }
    }
}

/// Error returned when the circuit breaker is open.
///
/// This error is returned by [`CircuitBreaker::pre_invoke()`] when the circuit
/// is in the `Open` state, preventing the LLM call from executing.
#[derive(thiserror::Error)]
#[error("circuit breaker is open")]
pub struct CircuitBreakerOpenError;

/// Circuit breaker middleware for LLM invocation resilience.
///
/// Prevents cascading failures by blocking requests to an unhealthy LLM provider.
/// The circuit tracks failure counts and transitions between states based on
/// configured thresholds.
///
/// # Thread Safety
///
/// This implementation uses lock-free atomic operations for all state and counters,
/// making it safe to share across threads without locking. Cloning this struct
/// creates a new handle to the same underlying atomics.
///
/// # State Management
///
/// - [`Closed`]: Normal operation, all requests allowed
/// - [`Open`]: Failure detected, all requests blocked
/// - [`HalfOpen`]: Testing recovery, limited requests allowed
///
/// [`Closed`]: CircuitState::Closed
/// [`Open`]: CircuitState::Open
/// [`HalfOpen`]: CircuitState::HalfOpen
///
/// # Example
///
/// ```rust
/// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
/// use std::time::Duration;
///
/// let config = CircuitBreakerConfig {
///     failure_threshold: 3,
///     recovery_timeout: Duration::from_secs(10),
///     half_open_max_calls: 2,
/// };
/// let breaker = CircuitBreaker::new(config);
/// ```
#[derive(Clone)]
pub struct CircuitBreaker {
    /// Circuit breaker configuration.
    config: CircuitBreakerConfig,

    /// Current circuit state (0=Closed, 1=Open, 2=HalfOpen).
    state: Arc<AtomicU8>,

    /// Number of consecutive failures.
    failure_count: Arc<AtomicU32>,

    /// Number of consecutive successes (used in `HalfOpen`).
    success_count: Arc<AtomicU32>,

    /// Timestamp of last failure (epoch milliseconds).
    last_failure_time: Arc<AtomicU64>,

    /// Number of calls made in `HalfOpen` state.
    half_open_calls: Arc<AtomicU32>,
}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "Atomic fields are internal implementation details"
)]
impl fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("config", &self.config)
            .field("state", &self.state())
            .field("failure_count", &self.failure_count())
            .finish()
    }
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Circuit breaker configuration
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    /// use std::time::Duration;
    ///
    /// let config = CircuitBreakerConfig {
    ///     failure_threshold: 5,
    ///     recovery_timeout: Duration::from_secs(30),
    ///     half_open_max_calls: 1,
    /// };
    /// let breaker = CircuitBreaker::new(config);
    /// ```
    #[must_use]
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(AtomicU8::new(STATE_CLOSED)),
            failure_count: Arc::new(AtomicU32::new(0)),
            success_count: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
            half_open_calls: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Get the current circuit state.
    ///
    /// This method handles the automatic `Open` → `HalfOpen` transition based on
    /// the recovery timeout. If the circuit is `Open` and the recovery timeout
    /// has elapsed, it transitions to `HalfOpen`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    /// assert_eq!(breaker.state(), CircuitState::Closed);
    /// ```
    #[must_use]
    pub fn state(&self) -> CircuitState {
        // Load current state with Acquire to synchronize with Release stores
        let current_state = self.state.load(Ordering::Acquire);

        // Check if we need to transition from Open to HalfOpen
        if current_state == STATE_OPEN {
            let last_failure = self.last_failure_time.load(Ordering::Relaxed);
            #[allow(
                clippy::cast_possible_truncation,
                reason = "Recovery timeout fits in u64 for realistic values"
            )]
            let recovery_ms = self.config.recovery_timeout.as_millis() as u64;
            let now = current_time_millis();

            // If recovery timeout has elapsed, transition to HalfOpen
            if last_failure > 0 && now.saturating_sub(last_failure) >= recovery_ms {
                // Try to transition to HalfOpen
                if self
                    .state
                    .compare_exchange(
                        current_state,
                        STATE_HALF_OPEN,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // Reset counters for HalfOpen state
                    self.half_open_calls.store(0, Ordering::Relaxed);
                    return CircuitState::HalfOpen;
                }
            }
        }

        CircuitState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Get the current failure count.
    ///
    /// Returns the number of consecutive failures that have occurred.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    /// assert_eq!(breaker.failure_count(), 0);
    /// ```
    #[must_use]
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Relaxed)
    }

    /// Reset the circuit breaker to `Closed` state.
    ///
    /// Clears all counters and resets the state to `Closed`, allowing all
    /// requests through. This can be used to manually reset the circuit
    /// after maintenance or recovery procedures.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    /// breaker.reset();
    /// assert_eq!(breaker.state(), CircuitState::Closed);
    /// assert_eq!(breaker.failure_count(), 0);
    /// ```
    pub fn reset(&self) {
        self.state.store(STATE_CLOSED, Ordering::Release);
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        self.last_failure_time.store(0, Ordering::Relaxed);
        self.half_open_calls.store(0, Ordering::Relaxed);
    }

    /// Set the last failure time (for testing purposes).
    ///
    /// This method allows tests to control the timing of `Open` → `HalfOpen`
    /// transitions without actually waiting for the recovery timeout.
    ///
    /// # Arguments
    ///
    /// * `time` - Epoch milliseconds to set as the last failure time
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    ///
    /// let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
    /// breaker.set_last_failure_time(0); // Set to epoch
    /// ```
    #[cfg(test)]
    fn set_last_failure_time(&self, time: u64) {
        self.last_failure_time.store(time, Ordering::Relaxed);
    }

    /// Handle a successful LLM invocation.
    ///
    /// Resets failure counters and potentially transitions from `HalfOpen` to `Closed`.
    fn handle_success(&self) {
        // Reset failure count on success
        self.failure_count.store(0, Ordering::Relaxed);

        let current_state = self.state.load(Ordering::Acquire);

        if current_state == STATE_HALF_OPEN {
            // Increment success count
            let successes = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;

            // If we've had enough successes in HalfOpen, transition to Closed
            if successes >= self.config.half_open_max_calls
                && self
                    .state
                    .compare_exchange(
                        current_state,
                        STATE_CLOSED,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                // Reset counters
                self.success_count.store(0, Ordering::Relaxed);
                self.half_open_calls.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Handle a failed LLM invocation.
    ///
    /// Increments failure counters and potentially transitions to `Open` state.
    fn handle_failure(&self) {
        // Record failure time
        self.last_failure_time
            .store(current_time_millis(), Ordering::Relaxed);

        // Increment failure count
        let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;

        let current_state = self.state.load(Ordering::Acquire);

        // Check if we should open the circuit
        if failures >= self.config.failure_threshold {
            // Only transition from Closed or HalfOpen to Open
            if current_state != STATE_OPEN
                && self
                    .state
                    .compare_exchange(
                        current_state,
                        STATE_OPEN,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                // Reset success count when opening
                self.success_count.store(0, Ordering::Relaxed);
            }
        } else if current_state == STATE_HALF_OPEN
            && self
                .state
                .compare_exchange(
                    current_state,
                    STATE_OPEN,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            // In HalfOpen with low failure count - still transition to Open
            // Reset counters
            self.success_count.store(0, Ordering::Relaxed);
            self.half_open_calls.store(0, Ordering::Relaxed);
        }
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }
}

#[async_trait]
impl LlmMiddleware for CircuitBreaker {
    async fn pre_invoke(
        &self,
        _messages: &mut Vec<Message>,
        _options: &mut CallOptions,
    ) -> Result<(), LlmError> {
        let current_state = self.state();

        match current_state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => Err(LlmError::Other(Box::new(CircuitBreakerOpenError))),
            CircuitState::HalfOpen => {
                // Check if we've exceeded the half-open call limit
                let calls = self.half_open_calls.fetch_add(1, Ordering::Relaxed);

                if calls >= self.config.half_open_max_calls {
                    // Revert the increment and reject
                    self.half_open_calls.fetch_sub(1, Ordering::Relaxed);
                    Err(LlmError::Other(Box::new(CircuitBreakerOpenError)))
                } else {
                    Ok(())
                }
            }
        }
    }

    async fn post_invoke(&self, result: &mut Result<Message, LlmError>) -> Result<(), LlmError> {
        match result {
            Ok(_) => self.handle_success(),
            Err(_) => self.handle_failure(),
        }
        Ok(())
    }
}

// Manual Debug implementation for CircuitBreakerOpenError
impl fmt::Debug for CircuitBreakerOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CircuitBreakerOpenError").finish()
    }
}

/// Get the current time as epoch milliseconds.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Milliseconds fit in u64 for realistic time ranges"
)]
fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatModel;
    use crate::llm::middleware::MiddlewareModel;
    use crate::llm::mock::MockChatModel;

    #[test]
    fn test_circuit_breaker_new() {
        let config = CircuitBreakerConfig::default();
        let breaker = CircuitBreaker::new(config);

        assert_eq!(breaker.state(), CircuitState::Closed);
        assert_eq!(breaker.failure_count(), 0);
    }

    #[test]
    fn test_circuit_breaker_default_config() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.recovery_timeout, Duration::from_secs(30));
        assert_eq!(config.half_open_max_calls, 1);
    }

    #[test]
    fn test_circuit_breaker_default() {
        let breaker = CircuitBreaker::default();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_closed_allows_calls() {
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let breaker = CircuitBreaker::default();

        let model = MiddlewareModel::new(base_model).with_middleware(breaker);

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        let _ = result.unwrap();
    }

    #[tokio::test]
    async fn test_circuit_breaker_transitions_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_millis(1),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Simulate failures until threshold is reached
        for _ in 0..3 {
            breaker.handle_failure();
        }

        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(breaker.failure_count(), 3);
    }

    #[tokio::test]
    async fn test_circuit_breaker_open_rejects_calls() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();

        assert_eq!(breaker.state(), CircuitState::Open);

        // Try to invoke - should be rejected
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model).with_middleware(breaker);

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("circuit breaker is open")
        );
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_allows_limited_calls() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            half_open_max_calls: 2,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Set last failure time to trigger HalfOpen transition
        breaker.set_last_failure_time(current_time_millis().saturating_sub(100));

        // Check that state is now HalfOpen
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // First call should succeed and stay in HalfOpen (1/2 successes)
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model.clone()).with_middleware(breaker.clone());

        let messages = vec![Message::human("Hi")];
        let result1 = model.invoke(&messages, None).await;
        let _ = result1.unwrap();
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // Second call should succeed and transition to Closed (2/2 successes)
        let model2 = MiddlewareModel::new(base_model.clone()).with_middleware(breaker.clone());
        let result2 = model2.invoke(&messages, None).await;
        let _ = result2.unwrap();
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Third call should succeed because circuit is now Closed
        let model3 = MiddlewareModel::new(base_model).with_middleware(breaker.clone());
        let result3 = model3.invoke(&messages, None).await;
        let _ = result3.unwrap();
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_success_closes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();

        // Set last failure time to trigger HalfOpen transition
        breaker.set_last_failure_time(current_time_millis().saturating_sub(100));

        // Should be in HalfOpen now
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // Successful call should transition to Closed
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model).with_middleware(breaker.clone());

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        let _ = result.unwrap();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert_eq!(breaker.failure_count(), 0);
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_failure_opens() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();

        // Set last failure time to trigger HalfOpen transition
        breaker.set_last_failure_time(current_time_millis().saturating_sub(100));

        // Should be in HalfOpen now
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // Failed call should transition back to Open
        let base_model = MockChatModel::new("gpt-4").with_error();
        let model = MiddlewareModel::new(base_model).with_middleware(breaker.clone());

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        let _ = result.unwrap_err();
        assert_eq!(breaker.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Reset should return to Closed
        breaker.reset();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert_eq!(breaker.failure_count(), 0);
    }

    #[tokio::test]
    async fn test_circuit_breaker_success_in_closed_resets_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Add some failures
        breaker.handle_failure();
        breaker.handle_failure();
        assert_eq!(breaker.failure_count(), 2);

        // Successful call should reset failures
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model).with_middleware(breaker.clone());

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        let _ = result.unwrap();
        assert_eq!(breaker.failure_count(), 0);
    }

    #[test]
    fn test_circuit_state_constants() {
        assert_eq!(CircuitState::Closed.as_u8(), 0);
        assert_eq!(CircuitState::Open.as_u8(), 1);
        assert_eq!(CircuitState::HalfOpen.as_u8(), 2);

        assert_eq!(CircuitState::from_u8(0), CircuitState::Closed);
        assert_eq!(CircuitState::from_u8(1), CircuitState::Open);
        assert_eq!(CircuitState::from_u8(2), CircuitState::HalfOpen);
        // Invalid values default to Closed
        assert_eq!(CircuitState::from_u8(255), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_open_to_half_open_transition() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Force the circuit open
        breaker.handle_failure();
        breaker.handle_failure();
        assert_eq!(breaker.state(), CircuitState::Open);

        // Set last failure time to trigger HalfOpen transition
        breaker.set_last_failure_time(current_time_millis().saturating_sub(100));

        // Check state - should transition to HalfOpen
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_breaker_multiple_failures_then_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Add failures below threshold
        breaker.handle_failure();
        breaker.handle_failure();
        assert_eq!(breaker.failure_count(), 2);
        assert_eq!(breaker.state(), CircuitState::Closed);

        // Successful call should reset failures
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model).with_middleware(breaker.clone());

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        let _ = result.unwrap();
        assert_eq!(breaker.failure_count(), 0);
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_clone() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        };
        let breaker = CircuitBreaker::new(config);

        // Add some failures
        breaker.handle_failure();
        breaker.handle_failure();

        // Clone the breaker
        let cloned = breaker.clone();

        // Both should have the same state (shared Arc atomics)
        assert_eq!(breaker.state(), cloned.state());
        assert_eq!(breaker.failure_count(), cloned.failure_count());
    }
}

// Rust guideline compliant 2026-05-27
