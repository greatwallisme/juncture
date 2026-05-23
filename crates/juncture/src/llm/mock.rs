//! Mock LLM provider for testing.
//!
//! Provides a mock implementation of [`ChatModel`] that returns pre-configured
//! responses. Useful for testing agent workflows without making actual API calls.

use async_trait::async_trait;
use futures::stream;

use crate::llm::{
    BoxStream, CallOptions, ChatModel, LlmError, Message, MessageChunk, ToolCall, ToolCallChunk,
    ToolDefinition,
};

/// Mock LLM provider for testing.
///
/// Returns pre-configured responses without making actual API calls.
/// Useful for unit tests and integration tests.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::{ChatModel, MockChatModel};
/// use juncture::Message;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let model = MockChatModel::new("gpt-4")
///     .with_response("Hello, world!");
///
/// let messages = vec![Message::human("Hi")];
/// let response = model.invoke(&messages, None).await?;
/// assert!(matches!(response.role, juncture::llm::Role::Ai));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct MockChatModel {
    /// Model name to report.
    model_name: String,

    /// Pre-configured text response.
    response: Option<String>,

    /// Pre-configured tool calls.
    tool_calls: Vec<ToolCall>,

    /// Available tools.
    tools: Vec<ToolDefinition>,

    /// Whether to return an error.
    should_error: bool,
}

impl MockChatModel {
    /// Create a new mock model with the given name.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::MockChatModel;
    ///
    /// let model = MockChatModel::new("gpt-4");
    /// assert_eq!(model.model_name(), "gpt-4");
    /// ```
    #[must_use]
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            response: None,
            tool_calls: Vec::new(),
            tools: Vec::new(),
            should_error: false,
        }
    }

    /// Set the text response to return.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::MockChatModel;
    ///
    /// let model = MockChatModel::new("gpt-4")
    ///     .with_response("Hello, world!");
    /// ```
    #[must_use]
    pub fn with_response(mut self, response: impl Into<String>) -> Self {
        self.response = Some(response.into());
        self
    }

    /// Set the tool calls to return.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::{MockChatModel, ToolCall};
    /// use serde_json::json;
    ///
    /// let tool_calls = vec![
    ///     ToolCall {
    ///         id: "call_123".to_string(),
    ///         name: "get_weather".to_string(),
    ///         arguments: json!({"location": "NYC"}),
    ///     },
    /// ];
    /// let model = MockChatModel::new("gpt-4")
    ///     .with_tool_calls(tool_calls);
    /// ```
    #[must_use]
    pub fn with_tool_calls(mut self, calls: Vec<ToolCall>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Configure the model to return an error on invoke.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::llm::{ChatModel, MockChatModel};
    /// use juncture::Message;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = MockChatModel::new("gpt-4").with_error();
    /// let messages = vec![Message::human("Hi")];
    ///
    /// let result = model.invoke(&messages, None).await;
    /// assert!(result.is_err());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn with_error(mut self) -> Self {
        self.should_error = true;
        self
    }
}

impl Default for MockChatModel {
    fn default() -> Self {
        Self::new("mock-model")
    }
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn invoke(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        if self.should_error {
            return Err(LlmError::Other("Mock error".to_string()));
        }

        let content = self.response.clone().unwrap_or_default();

        let msg = Message::ai_with_tool_calls(content, self.tool_calls.clone());

        Ok(msg)
    }

    fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> BoxStream<'_, Result<MessageChunk, LlmError>> {
        if self.should_error {
            let error = LlmError::Other("Mock error".to_string());
            return Box::pin(stream::once(async move { Err(error) }));
        }

        let content = self.response.clone().unwrap_or_default();
        let chunk = MessageChunk {
            content,
            tool_call_chunks: self
                .tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| ToolCallChunk {
                    id: Some(call.id.clone()),
                    name: Some(call.name.clone()),
                    args_delta: call.arguments.to_string(),
                    index,
                })
                .collect(),
            usage_delta: None,
        };

        Box::pin(stream::once(async move { Ok(chunk) }))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let mut new_model = self.clone();
        new_model.tools = tools;
        new_model
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

// Rust guideline compliant 2026-05-19
