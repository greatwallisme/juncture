//! Core traits and error types for LLM integration.

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;
use thiserror::Error;

use crate::llm::{Message, MessageChunk};

/// Type alias for a boxed, pinned, sendable stream.
///
/// This is the standard return type for streaming LLM responses,
/// erasing the concrete stream implementation while maintaining
/// async iteration capability.
///
/// # Type Parameters
///
/// * `'a` - Lifetime of the stream (typically tied to `&self` in trait methods)
/// * `T` - The item type yielded by the stream
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// LLM error types.
///
/// Comprehensive error types covering all failure modes when interacting
/// with LLM providers.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Authentication failed with the provider.
    ///
    /// This typically indicates an invalid API key or expired credentials.
    #[error("authentication failed: {0}")]
    AuthError(String),

    /// Rate limited by the provider.
    ///
    /// The provider is limiting request rate. The caller should wait
    /// for the specified duration before retrying.
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimited {
        /// Suggested wait duration before retrying.
        retry_after: Option<Duration>,
    },

    /// Context length exceeded.
    ///
    /// The input message count or token count exceeds the model's context window.
    #[error("context length exceeded: {used} tokens used, {limit} limit")]
    ContextLengthExceeded {
        /// Actual token count used.
        used: u64,
        /// Maximum context window size.
        limit: u64,
    },

    /// Network error during request.
    ///
    /// This variant is only available when the `reqwest` feature is enabled.
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// Generic network error.
    ///
    /// Used when no provider features are enabled.
    #[cfg(not(any(feature = "anthropic", feature = "openai", feature = "ollama")))]
    #[error("network error: {0}")]
    NetworkError(String),

    /// Invalid response from provider.
    ///
    /// The provider returned a response that could not be parsed or was malformed.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Model not found.
    ///
    /// The requested model name does not exist or is not available.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// Content filtered by provider.
    ///
    /// The provider refused to process the content due to policy violations.
    #[error("content filtered")]
    ContentFiltered,

    /// Request timeout.
    ///
    /// The provider did not respond within the specified time limit.
    #[error("timeout after {0:?}")]
    Timeout(Duration),

    /// Other error.
    ///
    /// Catch-all for errors not covered by specific variants.
    #[error("LLM error: {0}")]
    Other(String),
}

/// Tool choice strategy for function calling.
///
/// Controls when and how the model should use available tools.
#[derive(Clone, Debug, Default)]
pub enum ToolChoice {
    /// Let the model decide whether to use tools.
    #[default]
    Auto,

    /// Do not use any tools.
    None,

    /// Require the model to use at least one tool.
    Required,

    /// Require the model to use a specific tool.
    Specific {
        /// Name of the tool to use.
        name: String,
    },
}

/// Response format specification.
///
/// Controls the format of the model's response, useful for structured output.
#[derive(Clone, Debug)]
pub enum ResponseFormat {
    /// Request JSON response (no schema validation).
    JsonObject,

    /// Request response matching a specific JSON schema.
    JsonSchema {
        /// Name of the schema (for documentation).
        name: String,

        /// JSON schema definition.
        schema: serde_json::Value,

        /// Whether to enforce strict schema adherence.
        strict: bool,
    },
}

/// Call options for LLM invocation.
///
/// Optional parameters that control model behavior during inference.
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    /// Sampling temperature (0.0 to 1.0).
    ///
    /// Lower values make the model more deterministic, higher values more random.
    pub temperature: Option<f32>,

    /// Maximum tokens to generate.
    ///
    /// Limits the length of the model's response.
    pub max_tokens: Option<u32>,

    /// Sequences that will stop generation.
    ///
    /// When the model generates any of these sequences, it stops generating further tokens.
    pub stop_sequences: Option<Vec<String>>,

    /// Nucleus sampling threshold (0.0 to 1.0).
    ///
    /// Controls the cumulative probability threshold for token sampling.
    pub top_p: Option<f32>,

    /// Override the default model name.
    ///
    /// Allows switching models without creating a new client instance.
    pub model_override: Option<String>,

    /// Tool choice strategy.
    pub tool_choice: Option<ToolChoice>,

    /// Response format specification.
    pub response_format: Option<ResponseFormat>,

    /// Tags for streaming metadata and filtering.
    ///
    /// Tags are propagated into stream events as [`MessageStreamMetadata::tags`].
    /// The `"nostream"` tag causes streaming events to be suppressed for this call.
    pub tags: Vec<String>,
}

/// Tool definition for function calling.
///
/// Describes a tool/function that the model can call during inference.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    /// Tool name (identifier).
    pub name: String,

    /// Tool description (helps the model understand when to use it).
    pub description: String,

    /// JSON schema for tool parameters.
    pub parameters: serde_json::Value,
}

/// Unified interface for LLM providers.
///
/// This trait provides a common abstraction over different LLM providers
/// (Anthropic, `OpenAI`, Ollama, etc.), allowing provider-agnostic code.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel};
/// use juncture::Message;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let messages = vec![Message::human("Hi")];
///
/// let response = model.invoke(&messages, None).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait ChatModel: Send + Sync + Clone + 'static {
    /// Invoke the model and get a complete response.
    ///
    /// This is the simplest way to use an LLM - send messages and get back
    /// a complete response. For real-time streaming, use [`Self::stream`].
    ///
    /// # Errors
    ///
    /// Returns [`LlmError`] if:
    /// - Authentication fails
    /// - Network issues occur
    /// - The provider returns an invalid response
    /// - Rate limits are exceeded
    /// - The context length is exceeded
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError>;

    /// Stream the model response.
    ///
    /// Returns a stream of [`MessageChunk`] values, allowing real-time
    /// processing of the model's output as it's generated.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::ChatModel;
    /// use futures::stream::StreamExt;
    ///
    /// # async fn example(mut stream: impl futures::Stream<Item = Result<String, Box<dyn std::error::Error>>>) -> Result<(), Box<dyn std::error::Error>> {
    /// while let Some(chunk) = stream.next().await {
    ///     let chunk = chunk?;
    ///     // Process chunk
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The stream may yield [`LlmError`] values if errors occur during streaming.
    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> BoxStream<'_, Result<MessageChunk, LlmError>>;

    /// Bind tools to the model for function calling.
    ///
    /// Returns a new instance of the model with the specified tools available.
    /// The model can then call these tools during inference when appropriate.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::{ChatModel, MockChatModel, ToolDefinition};
    /// use serde_json::json;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = MockChatModel::new("gpt-4");
    /// let tools = vec![
    ///     ToolDefinition {
    ///         name: "get_weather".to_string(),
    ///         description: "Get current weather".to_string(),
    ///         parameters: json!({
    ///             "type": "object",
    ///             "properties": {
    ///                 "location": {"type": "string"}
    ///             }
    ///         }),
    ///     },
    /// ];
    /// let model_with_tools = model.bind_tools(tools);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self;

    /// Get the model name.
    ///
    /// Returns the identifier of the model being used (e.g., "gpt-4", "claude-3-opus").
    fn model_name(&self) -> &str;

    /// Wrap this model to extract structured output.
    ///
    /// Returns a [`StructuredOutputModel`] that forces the LLM to output
    /// JSON matching the schema of type `T`, which is then deserialized
    /// into the target type.
    ///
    /// This method is only available when the `structured-output` feature is enabled.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The target type for structured output (must implement [`DeserializeOwned`], [`JsonSchema`], [`Clone`], [`Send`], and [`Sync`])
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::{ChatModel, MockChatModel};
    /// use serde::Deserialize;
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Debug, Clone, Deserialize, JsonSchema)]
    /// struct WeatherReport {
    ///     temperature: f64,
    ///     conditions: String,
    /// }
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = base_model.with_structured_output::<WeatherReport>();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    #[cfg(feature = "structured-output")]
    fn with_structured_output<T>(self) -> crate::llm::StructuredOutputModel<Self, T>
    where
        Self: Sized,
        T: serde::de::DeserializeOwned + schemars::JsonSchema + Clone + Send + Sync + 'static,
    {
        crate::llm::StructuredOutputModel::new(self)
    }
}

// Rust guideline compliant 2026-05-19
