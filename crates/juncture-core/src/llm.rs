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

use crate::state::{Content, Message};

/// Re-export `BoxStream` for use in `ChatModel` trait
pub use futures::stream::BoxStream;

/// LLM invocation error types
///
/// Matches the canonical variant set from design `08-llm-tools` (the
/// `LlmError` enum + implementation note D-08-7). This is the single source
/// of truth consumed by both `juncture-core` and the `juncture` facade
/// providers (the former `chat.rs` / `llm/trait_.rs` duplicates are
/// consolidated onto this definition).
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

    /// Network error during HTTP request.
    ///
    /// Carries the typed `reqwest::Error` so callers can inspect the
    /// underlying transport failure. Available whenever the `chat` feature
    /// is enabled (the `llm` module is `chat`-gated and `chat = ["reqwest"]`).
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// Invalid response from LLM provider
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Model not found.
    ///
    /// The requested model name does not exist or is not available.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// Content filtered by provider policy.
    #[error("content filtered")]
    ContentFiltered,

    /// Request timeout.
    ///
    /// The provider did not respond within the specified time limit.
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),

    /// Other errors
    #[error("llm error: {0}")]
    Other(#[source] Box<dyn std::error::Error + Send + Sync>),
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

    /// Tags for streaming metadata and filtering.
    ///
    /// Tags are propagated into stream events as `MessageStreamMetadata::tags`.
    /// The `"nostream"` tag causes `EventEmitter::should_emit` to suppress
    /// streaming events for this call.
    pub tags: Vec<String>,
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

/// Streaming message chunk yielded by [`ChatModel::stream`].
///
/// Re-exported from [`crate::stream`] so the LLM streaming surface and the
/// internal stream/event module share a single `MessageChunk` definition
/// (design `08-llm-tools` §1.3). The implementation converged on a
/// `usage_delta` field (token usage reported on the final chunk) rather than
/// the design sketch's `role`/`usage` pair; the design doc is reconciled to
/// this shape so code and spec agree.
pub use crate::stream::MessageChunk;

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
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
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
    /// matching type T's schema, using tool-based extraction with a text
    /// fallback (see [`StructuredOutputModel`]).
    ///
    /// # Type Parameters
    ///
    /// * `T` - Target type with JSON Schema support
    #[must_use]
    fn with_structured_output<T>(self) -> StructuredOutputModel<Self, T>
    where
        Self: Sized,
        T: JsonSchema + DeserializeOwned + Serialize + Clone + Send + Sync + 'static,
    {
        StructuredOutputModel::new(self)
    }

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

/// Wrapper for extracting structured output from LLM responses.
///
/// Forces the model to return output matching a specific schema, which is
/// then deserialized into the target type `T`.
///
/// By default uses tool-based extraction: creates a virtual tool with `T`'s
/// JSON schema and sets `tool_choice` to require the tool. Falls back to
/// text-based JSON parsing if the model does not return tool calls.
///
/// This is the single source of truth (design `08-llm-tools`); the former
/// facade `structured.rs` duplicate was consolidated onto this definition.
///
/// # Type Parameters
///
/// * `M` - The underlying [`ChatModel`] implementation
/// * `T` - The target type for structured output (must implement [`Clone`],
///   [`Send`], [`Sync`], [`JsonSchema`] and [`DeserializeOwned`])
pub struct StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
{
    /// Inner model to wrap.
    pub(crate) inner: M,

    /// Whether to use tool-based extraction (vs. plain text parsing).
    pub(crate) use_tool_based: bool,

    /// Name of the synthetic extraction tool.
    pub(crate) tool_name: String,

    /// Tool definition built from `T`'s JSON schema.
    pub(crate) tool_definition: ToolDefinition,

    /// Phantom data for the target type.
    pub(crate) _phantom: std::marker::PhantomData<T>,
}

impl<M, T> Clone for StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            use_tool_based: self.use_tool_based,
            tool_name: self.tool_name.clone(),
            tool_definition: self.tool_definition.clone(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<M, T> std::fmt::Debug for StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredOutputModel")
            .field("inner", &"<model>")
            .field("use_tool_based", &self.use_tool_based)
            .field("tool_name", &self.tool_name)
            // `tool_definition` intentionally omitted: it is a bulky JSON
            // schema that would clutter debug output.
            .finish_non_exhaustive()
    }
}

impl<M, T> StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
{
    /// Create a new structured output wrapper.
    ///
    /// Tool-based extraction is enabled by default; disable it with
    /// [`with_tool_based_extraction`](Self::with_tool_based_extraction).
    #[must_use]
    pub fn new(inner: M) -> Self {
        let type_name = std::any::type_name::<T>();
        // Sanitize the type name into a valid tool identifier by replacing
        // Rust-specific characters (::, <, >, ,) with underscores.
        let tool_name = format!(
            "extract_{}",
            type_name
                .replace("::", "_")
                .replace(['<', '>', ','], "_")
                .replace(' ', "")
        );

        let schema = schemars::schema_for!(T);
        // Schema serialization cannot fail for a `RootSchema` (only String /
        // Vec / Map leaves); the fallback purely satisfies the non-fallible
        // constructor signature.
        let parameters =
            serde_json::to_value(&schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}));

        let tool_definition = ToolDefinition {
            name: tool_name.clone(),
            description: format!(
                "Extract structured data conforming to the schema for {type_name}"
            ),
            parameters,
        };

        Self {
            inner,
            use_tool_based: true,
            tool_name,
            tool_definition,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Enable or disable tool-based extraction.
    ///
    /// When enabled (default), the model is forced to use a synthetic tool
    /// whose schema matches `T`; the tool-call arguments are then extracted
    /// and deserialized. When disabled, the model's text response is parsed
    /// as JSON.
    #[must_use]
    pub const fn with_tool_based_extraction(mut self, enabled: bool) -> Self {
        self.use_tool_based = enabled;
        self
    }

    /// Borrow the inner model.
    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "Cannot be const in current Rust version"
    )]
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// Extract structured output from a fully-assembled message.
    ///
    /// Tries tool-call arguments first; falls back to parsing text content
    /// if the tool-call arguments do not match `T`'s schema.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::InvalidResponse`] if neither the tool-call
    /// arguments nor the text content parse as `T`.
    pub fn extract(&self, message: &Message) -> Result<T, LlmError> {
        if !message.tool_calls.is_empty()
            && let Ok(result) = Self::extract_from_tool_call(message)
        {
            return Ok(result);
        }
        // Tool-call arguments did not match the schema; fall back to text.
        Self::extract_from_text(message)
    }

    /// Deserialize `T` from the first tool-call's arguments.
    fn extract_from_tool_call(message: &Message) -> Result<T, LlmError> {
        let tool_call = message.tool_calls.first().ok_or_else(|| {
            LlmError::InvalidResponse(
                "No tool calls found in response for tool-based extraction".to_string(),
            )
        })?;

        serde_json::from_value(tool_call.arguments.clone()).map_err(|e| {
            LlmError::InvalidResponse(format!(
                "Failed to parse tool call arguments as structured output: {e}"
            ))
        })
    }

    /// Deserialize `T` from the message's text content.
    fn extract_from_text(message: &Message) -> Result<T, LlmError> {
        let content = match &message.content {
            Content::Text(text) => text,
            Content::MultiPart(_) => {
                return Err(LlmError::InvalidResponse(
                    "Cannot extract structured output from multipart content".to_string(),
                ));
            }
        };

        serde_json::from_str(content).map_err(|e| {
            LlmError::InvalidResponse(format!(
                "Failed to parse structured output: {e}\nContent: {content}"
            ))
        })
    }
}

impl<M, T> Default for StructuredOutputModel<M, T>
where
    M: ChatModel + Default,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new(M::default())
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl<M, T> ChatModel for StructuredOutputModel<M, T>
where
    M: ChatModel,
    T: JsonSchema + DeserializeOwned + Serialize + Clone + Send + Sync + 'static,
{
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        if self.use_tool_based {
            // Bind the extraction tool and force the model to call it.
            let model_with_tool = self.inner.bind_tools(vec![self.tool_definition.clone()]);

            // Merge options, enforcing tool_choice for the extraction tool.
            // Explicit match (not `unwrap_or_default`) per project style.
            #[allow(
                clippy::manual_unwrap_or_default,
                clippy::option_if_let_else,
                reason = "project rules prohibit unwrap_or_default; match is explicit and readable"
            )]
            let mut merged_opts = match options.cloned() {
                Some(opts) => opts,
                None => CallOptions::default(),
            };
            merged_opts.tool_choice = Some(ToolChoice::Specific {
                name: self.tool_name.clone(),
            });

            let response = model_with_tool.invoke(messages, Some(&merged_opts)).await?;

            // Accept the response if its tool-call arguments deserialize as T;
            // otherwise fall through to the text-content fallback.
            if !response.tool_calls.is_empty() && Self::extract_from_tool_call(&response).is_ok() {
                return Ok(response);
            }
            Self::extract_from_text(&response)?;
            Ok(response)
        } else {
            // Text-based extraction: validate that the response body parses as T.
            let response = self.inner.invoke(messages, options).await?;
            Self::extract_from_text(&response)?;
            Ok(response)
        }
    }

    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // Forward the inner stream unchanged. Streaming does not validate the
        // structured output; callers wanting validation should use `invoke()`
        // or collect the stream and call [`Self::extract`].
        self.inner.stream(messages, options).await
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let inner_with_tools = self.inner.bind_tools(tools);
        Self {
            inner: inner_with_tools,
            use_tool_based: self.use_tool_based,
            tool_name: self.tool_name.clone(),
            tool_definition: self.tool_definition.clone(),
            _phantom: std::marker::PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

// Rust guideline compliant 2026-05-20
