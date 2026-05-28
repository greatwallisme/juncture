//! Retry wrapper for resilient LLM calls.
//!
//! Provides automatic retry logic with exponential backoff for transient errors
//! like rate limiting and network timeouts.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;

use crate::llm::{CallOptions, ChatModel, LlmError, Message, MessageChunk, ToolDefinition};

/// Simple error type for retry-related errors
#[derive(Debug, thiserror::Error)]
#[error("Max retries exceeded with unknown error")]
struct RetryExhaustedError;

/// Wrapper that adds retry logic to any [`ChatModel`].
///
/// Implements exponential backoff retry for transient errors like rate limiting
/// and network timeouts. Permanent errors (authentication, invalid requests) are
/// not retried.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel, RetryingModel};
/// use juncture::Message;
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let base_model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let model = RetryingModel::new(base_model)
///     .max_retries(3)
///     .initial_backoff(Duration::from_secs(1));
///
/// let messages = vec![Message::human("Hi")];
/// let response = model.invoke(&messages, None).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct RetryingModel<M: ChatModel> {
    /// Inner model to wrap.
    inner: M,

    /// Maximum number of retry attempts.
    max_retries: usize,

    /// Initial backoff duration.
    initial_backoff: Duration,

    /// Maximum backoff duration.
    ///
    /// Exponential backoff is capped at this value to prevent excessive delays.
    max_backoff: Duration,

    /// Whether to respect the `retry_after` field from rate limit errors.
    ///
    /// If true, uses the server-suggested retry delay when available.
    /// If false, always calculates backoff using exponential backoff.
    respect_retry_after: bool,
}

impl<M: ChatModel> RetryingModel<M> {
    /// Create a new retry wrapper with default settings.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying model to wrap
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{ChatModel, MockChatModel, RetryingModel};
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = RetryingModel::new(base_model);
    /// ```
    #[must_use]
    pub const fn new(inner: M) -> Self {
        Self {
            inner,
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            respect_retry_after: true,
        }
    }

    /// Set the maximum number of retry attempts.
    ///
    /// # Arguments
    ///
    /// * `max_retries` - Maximum number of retries (0 = no retries)
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, RetryingModel};
    /// use std::time::Duration;
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = RetryingModel::new(base_model)
    ///     .max_retries(5);
    /// ```
    #[must_use]
    pub const fn max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the initial backoff duration.
    ///
    /// The backoff duration doubles with each retry attempt (exponential backoff).
    ///
    /// # Arguments
    ///
    /// * `backoff` - Initial backoff duration
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, RetryingModel};
    /// use std::time::Duration;
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = RetryingModel::new(base_model)
    ///     .initial_backoff(Duration::from_secs(2));
    /// ```
    #[must_use]
    pub const fn initial_backoff(mut self, backoff: Duration) -> Self {
        self.initial_backoff = backoff;
        self
    }

    /// Set the maximum backoff duration.
    ///
    /// Exponential backoff is capped at this value to prevent excessive delays.
    ///
    /// # Arguments
    ///
    /// * `max_backoff` - Maximum backoff duration
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, RetryingModel};
    /// use std::time::Duration;
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = RetryingModel::new(base_model)
    ///     .max_backoff(Duration::from_secs(60));
    /// ```
    #[must_use]
    pub const fn max_backoff(mut self, max_backoff: Duration) -> Self {
        self.max_backoff = max_backoff;
        self
    }

    /// Set whether to respect the `retry_after` field from rate limit errors.
    ///
    /// # Arguments
    ///
    /// * `respect` - If true, uses server-suggested retry delay when available
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::{MockChatModel, RetryingModel};
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = RetryingModel::new(base_model)
    ///     .respect_retry_after(false);
    /// ```
    #[must_use]
    pub const fn respect_retry_after(mut self, respect: bool) -> Self {
        self.respect_retry_after = respect;
        self
    }

    /// Check if an error is retryable.
    ///
    /// Only rate limits and timeouts are retried. All other errors are considered permanent.
    const fn is_retryable(error: &LlmError) -> bool {
        matches!(error, LlmError::RateLimited { .. } | LlmError::Timeout(_))
    }

    /// Calculate backoff duration for a given retry attempt.
    fn backoff_duration(&self, attempt: usize) -> Duration {
        // Exponential backoff: 2^attempt * initial_backoff
        let multiplier = 2_u32.pow(u32::try_from(attempt).unwrap_or(u32::MAX));
        let exponential = self.initial_backoff.saturating_mul(multiplier);
        exponential.min(self.max_backoff)
    }

    /// Extract suggested retry delay from error if available.
    const fn extract_retry_delay(&self, error: &LlmError) -> Option<Duration> {
        if let LlmError::RateLimited { retry_after } = error {
            if self.respect_retry_after {
                *retry_after
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl<M: ChatModel + Default> Default for RetryingModel<M> {
    fn default() -> Self {
        Self::new(M::default())
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl<M: ChatModel> ChatModel for RetryingModel<M> {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            let result = self.inner.invoke(messages, options).await;

            match result {
                Ok(response) => return Ok(response),
                Err(error) if Self::is_retryable(&error) && attempt < self.max_retries => {
                    last_error = Some(error);

                    // Use suggested retry delay if available, otherwise calculate backoff
                    let delay = self
                        .extract_retry_delay(last_error.as_ref().unwrap())
                        .unwrap_or_else(|| self.backoff_duration(attempt));

                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::Other(Box::new(RetryExhaustedError))))
    }

    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Pin<Box<dyn Stream<Item = Result<MessageChunk, LlmError>> + Send + '_>> {
        // For streaming, we don't implement retry logic since the stream
        // may already be partially consumed. Return the inner stream directly.
        self.inner.stream(messages, options)
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let inner_with_tools = self.inner.bind_tools(tools);
        Self {
            inner: inner_with_tools,
            max_retries: self.max_retries,
            initial_backoff: self.initial_backoff,
            max_backoff: self.max_backoff,
            respect_retry_after: self.respect_retry_after,
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::mock::MockChatModel;

    #[test]
    fn test_retry_model_new() {
        let base_model = MockChatModel::new("gpt-4");
        let model = RetryingModel::new(base_model);

        assert_eq!(model.max_retries, 3);
        assert_eq!(model.initial_backoff, Duration::from_millis(500));
        assert_eq!(model.max_backoff, Duration::from_secs(30));
        assert!(model.respect_retry_after);
    }

    #[test]
    fn test_retry_model_builder_methods() {
        let base_model = MockChatModel::new("gpt-4");
        let model = RetryingModel::new(base_model)
            .max_retries(5)
            .initial_backoff(Duration::from_secs(2))
            .max_backoff(Duration::from_secs(60))
            .respect_retry_after(false);

        assert_eq!(model.max_retries, 5);
        assert_eq!(model.initial_backoff, Duration::from_secs(2));
        assert_eq!(model.max_backoff, Duration::from_secs(60));
        assert!(!model.respect_retry_after);
    }

    #[test]
    fn test_backoff_duration_capping() {
        let base_model = MockChatModel::new("gpt-4");
        let model = RetryingModel::new(base_model)
            .initial_backoff(Duration::from_secs(1))
            .max_backoff(Duration::from_secs(10));

        // Test exponential backoff with capping
        assert_eq!(model.backoff_duration(0), Duration::from_secs(1));
        assert_eq!(model.backoff_duration(1), Duration::from_secs(2));
        assert_eq!(model.backoff_duration(2), Duration::from_secs(4));
        assert_eq!(model.backoff_duration(3), Duration::from_secs(8));
        assert_eq!(model.backoff_duration(4), Duration::from_secs(10)); // Capped at max_backoff
        assert_eq!(model.backoff_duration(5), Duration::from_secs(10)); // Stays capped
    }

    #[test]
    fn test_extract_retry_delay_respects_flag() {
        let base_model = MockChatModel::new("gpt-4");

        // Test with respect_retry_after = true (default)
        let model_respect = RetryingModel::new(base_model.clone()).respect_retry_after(true);
        let retry_after = Duration::from_secs(5);
        let rate_limited_error = LlmError::RateLimited {
            retry_after: Some(retry_after),
        };

        assert_eq!(
            model_respect.extract_retry_delay(&rate_limited_error),
            Some(retry_after)
        );

        // Test with respect_retry_after = false
        let model_ignore = RetryingModel::new(base_model).respect_retry_after(false);
        assert_eq!(model_ignore.extract_retry_delay(&rate_limited_error), None);
    }

    #[test]
    fn test_extract_retry_delay_non_rate_limited() {
        let base_model = MockChatModel::new("gpt-4");
        let model = RetryingModel::new(base_model);

        // Test with non-rate-limited error
        let timeout_error = LlmError::Timeout(Duration::from_secs(30));
        assert_eq!(model.extract_retry_delay(&timeout_error), None);
    }

    #[test]
    fn test_bind_tools_preserves_new_fields() {
        let base_model = MockChatModel::new("gpt-4");
        let model = RetryingModel::new(base_model)
            .max_backoff(Duration::from_secs(60))
            .respect_retry_after(false);

        let model_with_tools = model.bind_tools(vec![]);

        assert_eq!(model_with_tools.max_backoff, Duration::from_secs(60));
        assert!(!model_with_tools.respect_retry_after);
    }
}

// Rust guideline compliant 2026-05-19
