// LLM integration types and traits
//
// This module provides the foundational abstractions for LLM integration,
// including the `ChatModel` trait, message types, and related configurations.
//
// # Design Principles
//
// - Unified abstraction: Single trait covering all LLM providers
// - Streaming-first: Both invoke and stream are first-class operations
// - Type-safe: Leverages Rust's type system for message and tool handling
// - Zero-cost: Abstractions don't add runtime overhead

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::{Message, Role};

/// Re-export `BoxStream` for use in `ChatModel` trait
pub use futures::stream::BoxStream;

/// LLM invocation error types
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Authentication failed
    #[error("authentication failed: {0}")]
    AuthError(String),

    /// Rate limited with optional retry-after duration
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimited {
        /// Optional duration to wait before retrying
        retry_after: Option<std::time::Duration>,
    },

    /// Context length exceeded
    #[error("context length exceeded: {used} tokens used, {limit} limit")]
    ContextLengthExceeded {
        /// Tokens used in request
        used: u64,
        /// Model's context window limit
        limit: u64,
    },

    /// Network error during HTTP request
    #[error("network error: {0}")]
    NetworkError(String),

    /// Invalid response from LLM provider
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Requested model not found
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// Content was filtered by provider
    #[error("content filtered")]
    ContentFiltered,

    /// Request timeout
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),

    /// Other errors
    #[error("llm error: {0}")]
    Other(String),
}

/// Options for LLM invocations
///
/// These options override default settings on the `ChatModel` instance
/// for a single invocation.
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    /// Sampling temperature (0.0 to 1.0)
    pub temperature: Option<f32>,

    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,

    /// Sequences that will stop generation
    pub stop_sequences: Option<Vec<String>>,

    /// Nucleus sampling threshold (0.0 to 1.0)
    pub top_p: Option<f32>,

    /// Override the model name for this call
    pub model_override: Option<String>,

    /// Tool selection strategy
    pub tool_choice: Option<ToolChoice>,

    /// Response format for structured output
    pub response_format: Option<ResponseFormat>,
}

/// Tool selection strategy
#[derive(Clone, Debug)]
pub enum ToolChoice {
    /// Automatically decide whether to call tools
    Auto,
    /// Do not call any tools
    None,
    /// Must call at least one tool
    Required,
    /// Must call the specified tool
    Specific {
        /// Name of the tool to call
        name: String,
    },
}

/// Response format for structured output
#[derive(Clone, Debug)]
pub enum ResponseFormat {
    /// JSON object (model outputs valid JSON)
    JsonObject,
    /// JSON Schema with strict validation
    JsonSchema {
        /// Name of the schema
        name: String,
        /// JSON Schema definition
        schema: serde_json::Value,
        /// Whether to use strict mode
        strict: bool,
    },
}

/// Tool definition for function calling
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for parameters
    pub parameters: serde_json::Value,
}

/// Streaming message chunk
///
/// Represents incremental data from streaming LLM responses.
/// Chunks must be accumulated to reconstruct the complete message.
#[derive(Clone, Debug)]
pub struct MessageChunk {
    /// Message role (may be empty in early chunks)
    pub role: Option<Role>,
    /// Text content delta
    pub content: String,
    /// Tool call chunks (using `args_delta` field name from stream module)
    pub tool_call_chunks: Vec<ToolCallChunk>,
    /// Token usage (only in final chunk)
    pub usage: Option<crate::state::TokenUsage>,
}

/// Streaming tool call chunk
///
/// Re-exported from `crate::stream` for LLM integration.
/// Note: This struct uses `args_delta` as the field name (not `arguments`).
/// Use the stream module's version for consistency.
pub use crate::stream::ToolCallChunk;

/// Unified `ChatModel` trait for all LLM providers
///
/// This trait provides a common interface for interacting with different
/// LLM providers (`Anthropic`, `OpenAI`, `Ollama`, etc.).
///
/// # Type Parameters
///
/// * `'a` - Lifetime for borrowed data in streaming
#[async_trait]
pub trait ChatModel: Send + Sync + Clone + 'static {
    /// Invoke the model with messages
    ///
    /// # Arguments
    ///
    /// * `messages` - Conversation history
    /// * `options` - Optional call settings to override defaults
    ///
    /// # Returns
    ///
    /// The model's response as a complete message
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError>;

    /// Stream the model's response
    ///
    /// # Arguments
    ///
    /// * `messages` - Conversation history
    /// * `options` - Optional call settings to override defaults
    ///
    /// # Returns
    ///
    /// A stream of message chunks that must be accumulated
    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError>;

    /// Bind tools to this model instance
    ///
    /// Returns a new instance with the tools registered for function calling.
    ///
    /// # Arguments
    ///
    /// * `tools` - List of tool definitions
    #[must_use]
    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self;

    /// Convert to structured output model
    ///
    /// Returns a wrapper that forces the model to output structured JSON
    /// matching type T's schema.
    ///
    /// # Type Parameters
    ///
    /// * `T` - Target type with JSON Schema support
    #[must_use]
    fn with_structured_output<T: JsonSchema + DeserializeOwned + Serialize>(
        self,
    ) -> StructuredOutputModel<Self, T>
    where
        Self: Sized;

    /// Get the model name
    fn model_name(&self) -> &str;
}

/// Trait for types with JSON Schema support
pub trait JsonSchema: schemars::JsonSchema {}

/// Blanket implementation for all `schemars::JsonSchema` types
impl<T: schemars::JsonSchema> JsonSchema for T {}

/// Marker for deserializable types
pub trait DeserializeOwned: for<'de> Deserialize<'de> {}

/// Blanket implementation for all deserializable types
impl<T: for<'de> Deserialize<'de>> DeserializeOwned for T {}

/// Wrapper for structured output from LLMs
///
/// Uses function calling to force the model to output JSON matching
/// the schema of type T.
pub struct StructuredOutputModel<M, T>
where
    M: Clone,
{
    /// Inner model
    pub(crate) inner: M,
    /// Phantom data for target type
    pub(crate) _phantom: std::marker::PhantomData<T>,
}

impl<M: Clone, T> Clone for StructuredOutputModel<M, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<M, T> std::fmt::Debug for StructuredOutputModel<M, T>
where
    M: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredOutputModel")
            .field("inner", &"<model>")
            .field("_phantom", &self._phantom)
            .finish()
    }
}

#[async_trait]
impl<M, T> ChatModel for StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: JsonSchema + DeserializeOwned + Serialize + Send + Sync + 'static,
{
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Create a virtual tool with T's schema
        let schema = schemars::schema_for!(T);
        let tool_def = ToolDefinition {
            name: "structured_output".to_string(),
            description: "Output structured data".to_string(),
            parameters: serde_json::to_value(schema)
                .map_err(|e| LlmError::InvalidResponse(e.to_string()))?,
        };

        // Force tool usage
        #[allow(
            clippy::manual_unwrap_or_default,
            clippy::option_if_let_else,
            reason = "project rules prohibit unwrap_or_default; match is explicit and readable"
        )]
        let mut opts = match options.cloned() {
            Some(opts) => opts,
            None => CallOptions::default(),
        };
        opts.tool_choice = Some(ToolChoice::Required);

        // Call inner model with tool bound
        let model_with_tool = self.inner.bind_tools(vec![tool_def]);
        let response = model_with_tool.invoke(messages, Some(&opts)).await?;

        // Extract tool call arguments and parse as T
        if let Some(tool_call) = response.tool_calls.first() {
            let _value: T = serde_json::from_value(tool_call.arguments.clone()).map_err(|e| {
                LlmError::InvalidResponse(format!("Failed to parse structured output: {e}"))
            })?;

            // Return as JSON string in content
            Ok(Message {
                id: response.id,
                role: Role::Ai,
                content: crate::state::Content::Text(serde_json::to_string(&_value).map_err(
                    |e| {
                        LlmError::InvalidResponse(format!(
                            "Failed to serialize structured output: {e}"
                        ))
                    },
                )?),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
                usage: response.usage,
            })
        } else {
            Err(LlmError::InvalidResponse(
                "No tool call in response".to_string(),
            ))
        }
    }

    async fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // Streaming not yet supported for structured output
        Err(LlmError::InvalidResponse(
            "Streaming not supported for structured output".to_string(),
        ))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        Self {
            inner: self.inner.bind_tools(tools),
            _phantom: std::marker::PhantomData,
        }
    }

    fn with_structured_output<U: JsonSchema + DeserializeOwned + Serialize>(
        self,
    ) -> StructuredOutputModel<Self, U>
    where
        Self: Sized,
    {
        StructuredOutputModel {
            inner: self,
            _phantom: std::marker::PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

// Rust guideline compliant 2026-05-20
