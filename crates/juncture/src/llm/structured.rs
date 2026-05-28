//! Structured output extraction.
//!
//! Provides a wrapper for extracting structured data from LLM responses
//! using tool-based extraction. Creates a virtual tool with the target
//! type's JSON schema, forces the model to use it via `tool_choice`,
//! and extracts the result from tool call arguments. Falls back to
//! text-based JSON parsing if the model does not return tool calls.

use std::marker::PhantomData;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::llm::{
    BoxStream, CallOptions, ChatModel, Content, LlmError, Message, MessageChunk, ToolChoice,
    ToolDefinition,
};

/// Wrapper for extracting structured output from LLM responses.
///
/// Forces the model to return output that matches a specific schema,
/// which is then deserialized into the target type `T`.
///
/// By default uses tool-based extraction: creates a virtual tool with
/// `T`'s JSON schema and sets `tool_choice` to require the tool.
/// Falls back to text-based JSON parsing if the model does not return
/// tool calls.
///
/// # Type Parameters
///
/// * `M` - The underlying [`ChatModel`] implementation
/// * `T` - The target type for structured output (must implement [`Clone`],
///   [`Send`], [`Sync`], [`JsonSchema`] and [`DeserializeOwned`])
///
/// # Example
///
/// ```rust,ignore
/// use juncture::llm::{ChatModel, MockChatModel, StructuredOutputModel};
/// use serde::Deserialize;
/// use schemars::JsonSchema;
///
/// #[derive(Debug, Clone, Deserialize, JsonSchema)]
/// struct WeatherReport {
///     temperature: f64,
///     conditions: String,
/// }
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let base_model = MockChatModel::new("gpt-4");
/// let model = StructuredOutputModel::<_, WeatherReport>::new(base_model);
/// // The model will return a WeatherReport
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct StructuredOutputModel<
    M: ChatModel,
    T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static,
> {
    /// Inner model to wrap.
    inner: M,

    /// Whether to use tool-based extraction.
    use_tool_based: bool,

    /// Name of the synthetic extraction tool.
    tool_name: String,

    /// Tool definition for the target type.
    tool_definition: ToolDefinition,

    /// Phantom data for the target type.
    _phantom: PhantomData<T>,
}

impl<M: ChatModel, T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static>
    StructuredOutputModel<M, T>
{
    /// Create a new structured output wrapper.
    ///
    /// Tool-based extraction is enabled by default.
    ///
    /// # Arguments
    ///
    /// * `inner` - The underlying model to wrap
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use juncture::llm::{ChatModel, MockChatModel, StructuredOutputModel};
    ///
    /// let base_model = MockChatModel::new("gpt-4");
    /// let model = StructuredOutputModel::<_, MyType>::new(base_model);
    /// ```
    #[must_use]
    pub fn new(inner: M) -> Self {
        let type_name = std::any::type_name::<T>();
        // Sanitize type name for use as a tool identifier by replacing
        // Rust-specific characters (::, <, >, ,) with underscores
        let tool_name = format!(
            "extract_{}",
            type_name
                .replace("::", "_")
                .replace(['<', '>', ','], "_")
                .replace(' ', "")
        );

        let schema = schemars::schema_for!(T);
        // Schema serialization should never fail because RootSchema only
        // contains types that are always JSON-serializable (String, Vec,
        // HashMap with String keys, etc.). A minimal fallback is used
        // purely to satisfy the non-fallible constructor signature.
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
            _phantom: PhantomData,
        }
    }

    /// Configure whether to use tool-based extraction.
    ///
    /// When enabled (default), the model is forced to use a synthetic tool
    /// whose schema matches `T`. The tool call arguments are then extracted
    /// and deserialized. Falls back to text-based JSON parsing if the model
    /// does not return tool calls.
    #[must_use]
    pub const fn with_tool_based_extraction(mut self, enabled: bool) -> Self {
        self.use_tool_based = enabled;
        self
    }

    /// Get the inner model.
    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "Cannot be const in current Rust version"
    )]
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// Extract structured output from a message.
    ///
    /// Attempts to parse tool calls first; falls back to text content if
    /// tool call arguments are not valid for the target type.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::InvalidResponse`] if neither tool call arguments
    /// nor text content can be parsed as `T`.
    pub fn extract(&self, message: &Message) -> Result<T, LlmError> {
        if !message.tool_calls.is_empty()
            && let Ok(result) = Self::extract_from_tool_call(message)
        {
            return Ok(result);
        }
        // Tool call arguments did not match schema; fall through to text
        Self::extract_from_text(message)
    }

    /// Extract structured output from tool call arguments.
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

    /// Extract structured output from message text content.
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

impl<M: ChatModel + Default, T: DeserializeOwned + JsonSchema + Clone + Send + Sync> Default
    for StructuredOutputModel<M, T>
{
    fn default() -> Self {
        Self::new(M::default())
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl<M: ChatModel, T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static> ChatModel
    for StructuredOutputModel<M, T>
{
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        if self.use_tool_based {
            // Create a model instance with the extraction tool bound
            let model_with_tool = self.inner.bind_tools(vec![self.tool_definition.clone()]);

            // Merge options, enforcing tool_choice for the extraction tool
            let mut merged_opts = options.cloned().unwrap_or_default();
            merged_opts.tool_choice = Some(ToolChoice::Specific {
                name: self.tool_name.clone(),
            });

            let response = model_with_tool.invoke(messages, Some(&merged_opts)).await?;

            // Try tool call extraction first
            if !response.tool_calls.is_empty() && Self::extract_from_tool_call(&response).is_ok() {
                return Ok(response);
            }
            // Tool call arguments were invalid; fall through to text fallback

            // Fall back to text-based extraction
            Self::extract_from_text(&response)?;
            Ok(response)
        } else {
            // Use text-based extraction (original behavior)
            let response = self.inner.invoke(messages, options).await?;
            Self::extract_from_text(&response)?;
            Ok(response)
        }
    }

    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> BoxStream<'_, Result<MessageChunk, LlmError>> {
        // Streaming support for structured output.
        //
        // Streams directly from the inner model without binding the extraction tool.
        // This provides real-time streaming capability for progress monitoring,
        // but unlike `invoke()`, does NOT validate the structured output.
        //
        // Consumers should use `invoke()` for validated structured output,
        // or collect all chunks from the stream and call `extract()` on the
        // accumulated result.
        //
        // Note: If you need tool-based extraction with streaming, bind the
        // StructuredOutputModel to the inner model first:
        // ```ignore
        // let model = base_model
        //     .bind_tools(vec![extraction_tool])
        //     .with_structured_output::<MyType>();
        // ```

        self.inner.stream(messages, options)
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let inner_with_tools = self.inner.bind_tools(tools);
        Self {
            inner: inner_with_tools,
            use_tool_based: self.use_tool_based,
            tool_name: self.tool_name.clone(),
            tool_definition: self.tool_definition.clone(),
            _phantom: PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{MockChatModel, ToolCall};
    use futures::stream::StreamExt;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Clone, Deserialize, JsonSchema)]
    struct WeatherReport {
        temperature: f64,
        conditions: String,
    }

    #[tokio::test]
    async fn test_tool_based_extraction_success() {
        let tool_calls = vec![ToolCall {
            id: "call_extract".to_string(),
            name: "weather_tool".to_string(),
            arguments: json!({"temperature": 22.5, "conditions": "sunny"}),
        }];

        let base = MockChatModel::new("gpt-4")
            .with_response("")
            .with_tool_calls(tool_calls);

        let model = StructuredOutputModel::<_, WeatherReport>::new(base);

        let messages = vec![Message::human("What's the weather?")];
        let response = model.invoke(&messages, None).await.unwrap();

        assert!(!response.tool_calls.is_empty());
        let extracted: WeatherReport = model.extract(&response).unwrap();
        assert!((extracted.temperature - 22.5).abs() < f64::EPSILON);
        assert_eq!(extracted.conditions, "sunny");
    }

    #[tokio::test]
    async fn test_text_based_extraction_fallback() {
        // Mock model returns text instead of tool calls
        let base = MockChatModel::new("gpt-4")
            .with_response(r#"{"temperature": 18.0, "conditions": "cloudy"}"#);

        let model = StructuredOutputModel::<_, WeatherReport>::new(base);

        let messages = vec![Message::human("What's the weather?")];
        let response = model.invoke(&messages, None).await.unwrap();

        let extracted: WeatherReport = model.extract(&response).unwrap();
        assert!((extracted.temperature - 18.0).abs() < f64::EPSILON);
        assert_eq!(extracted.conditions, "cloudy");
    }

    #[tokio::test]
    async fn test_disabled_tool_based_extraction() {
        // With tool-based extraction disabled, model returns text
        // and should be parsed as JSON
        let base = MockChatModel::new("gpt-4")
            .with_response(r#"{"temperature": 25.0, "conditions": "hot"}"#);

        let model =
            StructuredOutputModel::<_, WeatherReport>::new(base).with_tool_based_extraction(false);

        let messages = vec![Message::human("What's the weather?")];
        let response = model.invoke(&messages, None).await.unwrap();

        let extracted: WeatherReport = model.extract(&response).unwrap();
        assert!((extracted.temperature - 25.0).abs() < f64::EPSILON);
        assert_eq!(extracted.conditions, "hot");
    }

    #[tokio::test]
    async fn test_invalid_tool_call_falls_back_to_text() {
        // Model returns invalid tool call arguments but valid text
        let tool_calls = vec![ToolCall {
            id: "call_bad".to_string(),
            name: "structured_output".to_string(),
            arguments: json!({"temperature": "not_a_number", "conditions": 42}),
        }];

        let base = MockChatModel::new("gpt-4")
            .with_response(r#"{"temperature": 30.0, "conditions": "warm"}"#)
            .with_tool_calls(tool_calls);

        let model = StructuredOutputModel::<_, WeatherReport>::new(base);

        let messages = vec![Message::human("What's the weather?")];
        let response = model.invoke(&messages, None).await.unwrap();

        // Should successfully fall back to text extraction
        let extracted: WeatherReport = model.extract(&response).unwrap();
        assert!((extracted.temperature - 30.0).abs() < f64::EPSILON);
        assert_eq!(extracted.conditions, "warm");
    }

    #[tokio::test]
    async fn test_stream_returns_chunks() {
        // Test that streaming returns chunks from the inner model
        let base = MockChatModel::new("gpt-4")
            .with_response(r#"{"temperature": 21.0, "conditions": "rainy"}"#);

        let model = StructuredOutputModel::<_, WeatherReport>::new(base);

        let messages = vec![Message::human("What's the weather?")];
        let mut stream = model.stream(&messages, None);

        // Should receive at least one chunk
        let chunk_result = stream.next().await;
        assert!(chunk_result.is_some());

        let chunk = chunk_result.unwrap().unwrap();
        assert!(!chunk.content.is_empty());
    }

    #[tokio::test]
    async fn test_stream_with_tool_based_extraction() {
        // Test that streaming works with tool-based extraction enabled
        let tool_calls = vec![ToolCall {
            id: "call_stream".to_string(),
            name: "weather_tool".to_string(),
            arguments: json!({"temperature": 19.5, "conditions": "windy"}),
        }];

        let base = MockChatModel::new("gpt-4")
            .with_response("")
            .with_tool_calls(tool_calls);

        let model = StructuredOutputModel::<_, WeatherReport>::new(base);

        let messages = vec![Message::human("What's the weather?")];
        let mut stream = model.stream(&messages, None);

        // Should receive at least one chunk
        let chunk_result = stream.next().await;
        assert!(chunk_result.is_some());

        let chunk = chunk_result.unwrap().unwrap();
        // Chunk may be empty if using tool-based extraction
        assert!(chunk.content.is_empty() || !chunk.tool_call_chunks.is_empty());
    }
}
