//! Middleware system for LLM invocation interception.
//!
//! Provides a flexible middleware pattern that wraps [`ChatModel::invoke()`]
//! calls with pre/post processing hooks. Middleware can be used for logging,
//! metrics collection, request modification, error recovery, and more.
//!
//! # Architecture
//!
//! Middleware is executed in a pipeline:
//!
//! 1. **Pre-invoke** (forward order): All middleware `pre_invoke()` methods run
//!    in the order they were added. Each can modify messages and options or abort
//!    the call by returning an error.
//!
//! 2. **LLM call**: The inner model's `invoke()` method executes.
//!
//! 3. **Post-invoke** (reverse order): All middleware `post_invoke()` methods run
//!    in **reverse order** (last added runs first). Each can modify the result or
//!    convert errors into successes.
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
//! use juncture::llm::middleware::{LoggingMiddleware, MetricsMiddleware};
//! use juncture::Message;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
//!
//! // Add logging and metrics middleware
//! let model = MiddlewareModel::new(base_model)
//!     .with_middleware(LoggingMiddleware::new())
//!     .with_middleware(MetricsMiddleware::new());
//!
//! let messages = vec![Message::human("Hi")];
//! let response = model.invoke(&messages, None).await?;
//! # Ok(())
//! # }
//! ```

use std::cell::Cell;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use futures::Stream;
use tracing::{Level, event};

use crate::llm::{CallOptions, ChatModel, LlmError, Message, MessageChunk, ToolDefinition};

// Thread-local storage for middleware timing data.
// Each middleware that needs timing data can use this to store a start time
// in `pre_invoke()` and retrieve it in `post_invoke()`. This works because
// both methods are called sequentially in the same async task context.
thread_local! {
    /// Start time for middleware execution tracking.
    static MIDDLEWARE_START_TIME: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Middleware for intercepting LLM invocations.
///
/// Implementations can modify requests and responses, collect metrics,
/// add logging, or implement custom retry/error handling logic.
///
/// # Lifecycle
///
/// For each [`ChatModel::invoke()`] call:
///
/// 1. [`Self::pre_invoke()`] is called in the order middleware were added
/// 2. The inner LLM call executes
/// 3. [`Self::post_invoke()`] is called in **reverse order**
///
/// # Example
///
/// ```
/// use juncture::llm::middleware::LlmMiddleware;
/// use juncture::llm::{CallOptions, LlmError, Message};
/// use async_trait::async_trait;
///
/// struct UppercaseMiddleware;
///
/// #[async_trait]
/// impl LlmMiddleware for UppercaseMiddleware {
///     async fn pre_invoke(
///         &self,
///         _messages: &mut Vec<Message>,
///         _options: &mut CallOptions,
///     ) -> Result<(), LlmError> {
///         // Implementation here
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait LlmMiddleware: Send + Sync + 'static {
    /// Called before the LLM invocation.
    ///
    /// This method runs in the order middleware were added (first added runs first).
    /// It can modify the `messages` and `options` parameters, or abort the call
    /// by returning an error.
    ///
    /// # Parameters
    ///
    /// * `messages` - The messages to send to the LLM (mutable for modification)
    /// * `options` - The call options (mutable for modification)
    ///
    /// # Errors
    ///
    /// Returns an error to abort the LLM call. The error is propagated back
    /// to the caller without invoking the LLM.
    ///
    /// # Default Implementation
    ///
    /// The default implementation does nothing and always returns `Ok(())`.
    async fn pre_invoke(
        &self,
        _messages: &mut Vec<Message>,
        _options: &mut CallOptions,
    ) -> Result<(), LlmError> {
        Ok(())
    }

    /// Called after the LLM invocation completes.
    ///
    /// This method runs in **reverse order** (last added middleware runs first).
    /// It can inspect and modify the result, or convert errors into successes.
    ///
    /// # Parameters
    ///
    /// * `result` - The result from the LLM call (mutable for modification)
    ///
    /// # Errors
    ///
    /// Returns an error to replace the original result. This can be used to
    /// implement error recovery or transformation.
    ///
    /// # Default Implementation
    ///
    /// The default implementation does nothing and always returns `Ok(())`.
    async fn post_invoke(&self, _result: &mut Result<Message, LlmError>) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Wrapper that applies middleware to a [`ChatModel`].
///
/// Executes middleware in a pipeline around the inner model's `invoke()` calls.
/// Middleware can modify requests/responses, collect metrics, add logging, etc.
///
/// # Type Parameters
///
/// * `M` - The inner [`ChatModel`] type to wrap
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
/// use juncture::llm::middleware::LoggingMiddleware;
/// use juncture::Message;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let model = MiddlewareModel::new(base_model)
///     .with_middleware(LoggingMiddleware::new().with_model_name("gpt-4"));
///
/// let messages = vec![Message::human("Hi")];
/// let response = model.invoke(&messages, None).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct MiddlewareModel<M: ChatModel> {
    /// Inner model to wrap.
    inner: M,

    /// Middleware to apply (in execution order).
    middleware: Vec<Arc<dyn LlmMiddleware>>,
}

// Manual Debug implementation since dyn LlmMiddleware doesn't implement Debug
impl<M: ChatModel> fmt::Debug for MiddlewareModel<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MiddlewareModel")
            .field("inner", &self.inner.model_name())
            .field("middleware_count", &self.middleware.len())
            .finish()
    }
}

impl<M: ChatModel> MiddlewareModel<M> {
    /// Create a new middleware wrapper with no middleware.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying model to wrap
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = MiddlewareModel::new(base_model);
    /// ```
    #[must_use]
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            middleware: Vec::new(),
        }
    }

    /// Add a middleware to the pipeline.
    ///
    /// Middleware are executed in the order they are added. The `pre_invoke()`
    /// method runs in forward order, while `post_invoke()` runs in reverse order.
    ///
    /// # Arguments
    ///
    /// * `middleware` - The middleware to add
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, MiddlewareModel};
    /// use juncture::llm::middleware::LoggingMiddleware;
    /// use std::sync::Arc;
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = MiddlewareModel::new(base_model)
    ///     .with_middleware(LoggingMiddleware::new());
    /// ```
    #[must_use]
    pub fn with_middleware(mut self, middleware: impl LlmMiddleware) -> Self {
        self.middleware.push(Arc::new(middleware));
        self
    }

    /// Add multiple middleware to the pipeline.
    ///
    /// Middleware are executed in the order they appear in the slice.
    ///
    /// # Arguments
    ///
    /// * `middleware` - Slice of middleware to add
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, MiddlewareModel};
    /// use juncture::llm::middleware::{LoggingMiddleware, MetricsMiddleware};
    /// use std::sync::Arc;
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = MiddlewareModel::new(base_model)
    ///     .with_middlewares(&[
    ///         LoggingMiddleware::new(),
    ///         MetricsMiddleware::new(),
    ///     ]);
    /// ```
    #[must_use]
    pub fn with_middlewares(mut self, middleware: &[Arc<dyn LlmMiddleware>]) -> Self {
        self.middleware.extend_from_slice(middleware);
        self
    }
}

impl<M: ChatModel + Default> Default for MiddlewareModel<M> {
    fn default() -> Self {
        Self::new(M::default())
    }
}

#[async_trait]
impl<M: ChatModel> ChatModel for MiddlewareModel<M> {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Clone messages and options for modification
        let mut messages = messages.to_vec();
        let mut options = options.cloned().unwrap_or_default();

        // Run pre_invoke in forward order
        for mw in &self.middleware {
            mw.pre_invoke(&mut messages, &mut options).await?;
        }

        // Execute the inner LLM call
        let mut result = self.inner.invoke(&messages, Some(&options)).await;

        // Run post_invoke in reverse order
        for mw in self.middleware.iter().rev() {
            mw.post_invoke(&mut result).await?;
        }

        result
    }

    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Pin<Box<dyn Stream<Item = Result<MessageChunk, LlmError>> + Send + '_>> {
        // For streaming, we pass through to the inner model
        // Running async pre_invoke in the non-async stream() method is not feasible
        // For production use, consider a different design for streaming middleware
        self.inner.stream(messages, options)
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let inner_with_tools = self.inner.bind_tools(tools);
        Self {
            inner: inner_with_tools,
            middleware: self.middleware.clone(),
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

/// Middleware that logs LLM invocation using structured logging.
///
/// Emits OpenTelemetry-compatible events for request starts and completions,
/// making it easy to trace LLM calls in distributed systems.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
/// use juncture::llm::middleware::LoggingMiddleware;
/// use juncture::Message;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let model = MiddlewareModel::new(base_model)
///     .with_middleware(LoggingMiddleware::new().with_model_name("gpt-4"));
///
/// let messages = vec![Message::human("Hi")];
/// let response = model.invoke(&messages, None).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct LoggingMiddleware {
    /// Model name to include in logs (optional).
    model_name: String,
}

impl LoggingMiddleware {
    /// Create a new logging middleware with no model name.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::middleware::LoggingMiddleware;
    ///
    /// let middleware = LoggingMiddleware::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            model_name: String::new(),
        }
    }

    /// Set the model name for logging purposes.
    ///
    /// # Arguments
    ///
    /// * `model_name` - The model name to include in log events
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::middleware::LoggingMiddleware;
    ///
    /// let middleware = LoggingMiddleware::new().with_model_name("gpt-4");
    /// ```
    #[must_use]
    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = model_name.into();
        self
    }
}

impl Default for LoggingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmMiddleware for LoggingMiddleware {
    async fn pre_invoke(
        &self,
        messages: &mut Vec<Message>,
        _options: &mut CallOptions,
    ) -> Result<(), LlmError> {
        event!(
            name: "llm.invoke.started",
            Level::INFO,
            model_name = %self.model_name,
            message_count = messages.len(),
            "LLM invoke started",
        );
        Ok(())
    }

    async fn post_invoke(&self, result: &mut Result<Message, LlmError>) -> Result<(), LlmError> {
        let status = if result.is_ok() { "ok" } else { "error" };

        event!(
            name: "llm.invoke.completed",
            Level::INFO,
            model_name = %self.model_name,
            status,
            "LLM invoke completed",
        );
        Ok(())
    }
}

/// Metrics collected by [`MetricsMiddleware`].
///
/// Contains aggregated statistics about LLM invocations.
#[derive(Debug, Clone, Copy)]
pub struct LlmMetrics {
    /// Total number of successful invocations.
    pub invoke_count: u64,

    /// Total number of failed invocations.
    pub error_count: u64,

    /// Average duration of successful invocations (in milliseconds).
    pub avg_duration_ms: u64,

    /// Total duration of all successful invocations (in milliseconds).
    pub total_duration_ms: u64,
}

/// Middleware that tracks LLM invocation metrics.
///
/// Records invocation counts, error counts, and timing statistics using
/// lock-free atomic operations for thread-safe performance.
///
/// # Thread Safety
///
/// This middleware uses [`AtomicU64`] for all counters, making it safe
/// to share across threads without locking.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel, MiddlewareModel};
/// use juncture::llm::middleware::MetricsMiddleware;
/// use juncture::Message;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let middleware = MetricsMiddleware::new();
///
/// let model = MiddlewareModel::new(base_model)
///     .with_middleware(middleware.clone());
///
/// let messages = vec![Message::human("Hi")];
/// let response = model.invoke(&messages, None).await?;
///
/// let metrics = middleware.metrics();
/// assert!(metrics.invoke_count > 0);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct MetricsMiddleware {
    /// Total number of successful invocations.
    invoke_count: Arc<AtomicU64>,

    /// Total number of failed invocations.
    error_count: Arc<AtomicU64>,

    /// Total duration of all successful invocations (in milliseconds).
    total_duration_ms: Arc<AtomicU64>,
}

impl MetricsMiddleware {
    /// Create a new metrics middleware with zeroed counters.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::middleware::MetricsMiddleware;
    ///
    /// let middleware = MetricsMiddleware::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            invoke_count: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            total_duration_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get the current metrics.
    ///
    /// Returns a snapshot of the collected metrics at this moment in time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::middleware::MetricsMiddleware;
    ///
    /// let middleware = MetricsMiddleware::new();
    /// let metrics = middleware.metrics();
    /// assert_eq!(metrics.invoke_count, 0);
    /// ```
    #[must_use]
    pub fn metrics(&self) -> LlmMetrics {
        let invoke_count = self.invoke_count.load(Ordering::Relaxed);
        let error_count = self.error_count.load(Ordering::Relaxed);
        let total_duration_ms = self.total_duration_ms.load(Ordering::Relaxed);

        let avg_duration_ms = if invoke_count > 0 {
            total_duration_ms / invoke_count
        } else {
            0
        };

        LlmMetrics {
            invoke_count,
            error_count,
            avg_duration_ms,
            total_duration_ms,
        }
    }

    /// Reset all metrics to zero.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::middleware::MetricsMiddleware;
    ///
    /// let middleware = MetricsMiddleware::new();
    /// middleware.reset();
    /// assert_eq!(middleware.metrics().invoke_count, 0);
    /// ```
    pub fn reset(&self) {
        self.invoke_count.store(0, Ordering::Relaxed);
        self.error_count.store(0, Ordering::Relaxed);
        self.total_duration_ms.store(0, Ordering::Relaxed);
    }
}

impl Default for MetricsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmMiddleware for MetricsMiddleware {
    async fn pre_invoke(
        &self,
        _messages: &mut Vec<Message>,
        _options: &mut CallOptions,
    ) -> Result<(), LlmError> {
        // Store the start time in thread-local storage
        MIDDLEWARE_START_TIME.set(Some(Instant::now()));
        Ok(())
    }

    async fn post_invoke(&self, result: &mut Result<Message, LlmError>) -> Result<(), LlmError> {
        // Calculate duration from stored start time
        let duration_ms = MIDDLEWARE_START_TIME.with(|start| {
            start.take().map_or(0, |s| {
                s.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
            })
        });

        // Update counters based on result
        match result {
            Ok(_) => {
                self.invoke_count.fetch_add(1, Ordering::Relaxed);
                self.total_duration_ms
                    .fetch_add(duration_ms, Ordering::Relaxed);
            }
            Err(_) => {
                self.error_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::mock::MockChatModel;

    #[test]
    fn test_middleware_model_new() {
        let base_model = MockChatModel::new("gpt-4");
        let model = MiddlewareModel::new(base_model);

        assert!(model.middleware.is_empty());
        assert_eq!(model.model_name(), "gpt-4");
    }

    #[test]
    fn test_middleware_model_builder() {
        let base_model = MockChatModel::new("gpt-4");
        let log_mw = LoggingMiddleware::new();
        let metrics_mw = MetricsMiddleware::new();

        let model = MiddlewareModel::new(base_model)
            .with_middleware(log_mw)
            .with_middleware(metrics_mw);

        assert_eq!(model.middleware.len(), 2);
    }

    #[test]
    fn test_logging_middleware() {
        let middleware = LoggingMiddleware::new();
        assert_eq!(middleware.model_name, "");

        let with_name = LoggingMiddleware::new().with_model_name("gpt-4");
        assert_eq!(with_name.model_name, "gpt-4");
    }

    #[test]
    fn test_metrics_middleware_new() {
        let middleware = MetricsMiddleware::new();
        let metrics = middleware.metrics();

        assert_eq!(metrics.invoke_count, 0);
        assert_eq!(metrics.error_count, 0);
        assert_eq!(metrics.total_duration_ms, 0);
        assert_eq!(metrics.avg_duration_ms, 0);
    }

    #[test]
    fn test_metrics_middleware_reset() {
        let middleware = MetricsMiddleware::new();

        // Simulate some metrics
        middleware.invoke_count.fetch_add(5, Ordering::Relaxed);
        middleware.error_count.fetch_add(2, Ordering::Relaxed);
        middleware
            .total_duration_ms
            .fetch_add(100, Ordering::Relaxed);

        let metrics = middleware.metrics();
        assert_eq!(metrics.invoke_count, 5);
        assert_eq!(metrics.error_count, 2);

        // Reset
        middleware.reset();
        let metrics_after = middleware.metrics();
        assert_eq!(metrics_after.invoke_count, 0);
        assert_eq!(metrics_after.error_count, 0);
        assert_eq!(metrics_after.total_duration_ms, 0);
    }

    /// Test middleware that aborts in `pre_invoke`
    struct AbortMiddleware;

    #[async_trait]
    impl LlmMiddleware for AbortMiddleware {
        async fn pre_invoke(
            &self,
            _messages: &mut Vec<Message>,
            _options: &mut CallOptions,
        ) -> Result<(), LlmError> {
            Err(LlmError::Other(Box::new(std::io::Error::other(
                "aborted by middleware",
            ))))
        }
    }

    #[tokio::test]
    async fn test_pre_invoke_abort() {
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model).with_middleware(AbortMiddleware);

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("aborted by middleware")
        );
    }

    /// Test middleware that modifies the result in `post_invoke`
    struct ResultModifierMiddleware;

    #[async_trait]
    impl LlmMiddleware for ResultModifierMiddleware {
        async fn post_invoke(
            &self,
            result: &mut Result<Message, LlmError>,
        ) -> Result<(), LlmError> {
            // Convert errors to successes with a fallback message
            if result.is_err() {
                *result = Ok(Message::ai("Fallback response"));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_post_invoke_modifies_result() {
        let base_model = MockChatModel::new("gpt-4").with_error();
        let model = MiddlewareModel::new(base_model).with_middleware(ResultModifierMiddleware);

        let messages = vec![Message::human("Hi")];
        let result = model.invoke(&messages, None).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(matches!(response.role, crate::llm::Role::Ai));
    }

    /// Test middleware that records execution order
    struct OrderRecorder {
        order: Arc<std::sync::Mutex<Vec<String>>>,
        name: String,
    }

    #[async_trait]
    impl LlmMiddleware for OrderRecorder {
        async fn pre_invoke(
            &self,
            _messages: &mut Vec<Message>,
            _options: &mut CallOptions,
        ) -> Result<(), LlmError> {
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{}_pre", self.name));
            Ok(())
        }

        async fn post_invoke(
            &self,
            _result: &mut Result<Message, LlmError>,
        ) -> Result<(), LlmError> {
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{}_post", self.name));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_post_invoke_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mw1 = OrderRecorder {
            order: Arc::clone(&order),
            name: "first".to_string(),
        };
        let mw2 = OrderRecorder {
            order: Arc::clone(&order),
            name: "second".to_string(),
        };

        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let model = MiddlewareModel::new(base_model)
            .with_middleware(mw1)
            .with_middleware(mw2);

        let messages = vec![Message::human("Hi")];
        let _ = model.invoke(&messages, None).await;

        let order_data = {
            let order_guard = order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            order_guard.clone()
        };
        assert_eq!(
            order_data,
            vec![
                "first_pre".to_string(),
                "second_pre".to_string(),
                "second_post".to_string(),
                "first_post".to_string(),
            ]
        );
    }

    #[test]
    fn test_bind_tools_preserves_middleware() {
        let base_model = MockChatModel::new("gpt-4");
        let _log_mw = LoggingMiddleware::new();
        let _metrics_mw = MetricsMiddleware::new();

        let model = MiddlewareModel::new(base_model)
            .with_middleware(LoggingMiddleware::new())
            .with_middleware(MetricsMiddleware::new());

        let model_with_tools = model.bind_tools(vec![]);

        assert_eq!(model_with_tools.middleware.len(), 2);
    }

    #[tokio::test]
    async fn test_metrics_middleware_tracks_invocations() {
        let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
        let middleware = MetricsMiddleware::new();

        let model = MiddlewareModel::new(base_model).with_middleware(middleware.clone());

        let messages = vec![Message::human("Hi")];
        let _ = model.invoke(&messages, None).await;

        let metrics = middleware.metrics();
        assert_eq!(metrics.invoke_count, 1);
        assert_eq!(metrics.error_count, 0);
        // Duration should be recorded, but may be 0 for very fast operations
        let _ = metrics.total_duration_ms;
        let _ = metrics.avg_duration_ms;
    }

    #[tokio::test]
    async fn test_metrics_middleware_tracks_errors() {
        let base_model = MockChatModel::new("gpt-4").with_error();
        let middleware = MetricsMiddleware::new();

        let model = MiddlewareModel::new(base_model).with_middleware(middleware.clone());

        let messages = vec![Message::human("Hi")];
        let _ = model.invoke(&messages, None).await;

        let metrics = middleware.metrics();
        assert_eq!(metrics.invoke_count, 0);
        assert_eq!(metrics.error_count, 1);
        assert_eq!(metrics.total_duration_ms, 0);
        assert_eq!(metrics.avg_duration_ms, 0);
    }
}

// Rust guideline compliant 2026-05-26
