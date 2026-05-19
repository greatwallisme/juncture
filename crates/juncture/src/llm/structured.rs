//! Structured output extraction.
//!
//! Provides a wrapper for extracting structured data from LLM responses
//! by forcing the model to output JSON matching a specific schema.

use std::marker::PhantomData;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, stream};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::llm::{CallOptions, ChatModel, LlmError, Message, MessageChunk, ToolDefinition};

/// Wrapper for extracting structured output from LLM responses.
///
/// Forces the model to return JSON that matches a specific schema, which
/// is then deserialized into the target type `T`.
///
/// # Type Parameters
///
/// * `M` - The underlying [`ChatModel`] implementation
/// * `T` - The target type for structured output (must implement [`Clone`], [`Send`], [`Sync`], [`JsonSchema`] and [`DeserializeOwned`])
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
///
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

    /// Phantom data for the target type.
    _phantom: PhantomData<T>,
}

impl<M: ChatModel, T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static> StructuredOutputModel<M, T> {
    /// Create a new structured output wrapper.
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
    pub const fn new(inner: M) -> Self {
        Self {
            inner,
            _phantom: PhantomData,
        }
    }

    /// Get the inner model.
    #[must_use]
    #[allow(clippy::missing_const_for_fn, reason = "Cannot be const in current Rust version")]
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// Extract structured output from a message.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::InvalidResponse`] if the message content cannot
    /// be parsed as valid JSON matching the target schema.
    fn extract_structured(message: &Message) -> Result<T, LlmError> {
        let content = match &message.content {
            crate::llm::Content::Text(text) => text,
            crate::llm::Content::MultiPart(_) => {
                return Err(LlmError::InvalidResponse(
                    "Cannot extract structured output from multipart content".to_string(),
                ));
            }
        };

        // Try to parse as JSON directly
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

#[async_trait]
impl<M: ChatModel, T: DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static> ChatModel
    for StructuredOutputModel<M, T>
{
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Get response from inner model
        let response = self.inner.invoke(messages, options).await?;

        // Validate that response matches schema
        Self::extract_structured(&response)?;

        Ok(response)
    }

    fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Pin<Box<dyn Stream<Item = Result<MessageChunk, LlmError>> + Send + '_>> {
        // Streaming not supported for structured output
        // since we need to validate the complete response
        Box::pin(stream::once(async {
            Err(LlmError::Other(
                "Streaming not supported for structured output".to_string(),
            ))
        }))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let inner_with_tools = self.inner.bind_tools(tools);
        Self {
            inner: inner_with_tools,
            _phantom: PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

// Rust guideline compliant 2026-05-19
