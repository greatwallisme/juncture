// LLM provider implementations
//
// This module provides concrete implementations of ChatModel for various
// LLM providers: Anthropic, OpenAI, and Ollama.

use async_trait::async_trait;
use futures::stream;

use crate::llm::{
    CallOptions, ChatModel, LlmError, MessageChunk, StructuredOutputModel,
    ToolDefinition,
};
use crate::state::{Content, Message, Role};

/// Anthropic Claude implementation
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "Fields reserved for future API implementation")]
pub struct ChatAnthropic {
    /// Model name
    model: String,
    /// API key
    api_key: String,
    /// Base URL
    base_url: String,
    /// Default call options
    default_options: CallOptions,
    /// Registered tools
    tools: Vec<ToolDefinition>,
    /// HTTP client
    http_client: reqwest::Client,
    /// Max tokens
    max_tokens: u32,
}

impl ChatAnthropic {
    /// Create new Anthropic client
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., "claude-sonnet-4-20250514")
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: String::new(),
            base_url: "https://api.anthropic.com".to_string(),
            default_options: CallOptions::default(),
            tools: vec![],
            http_client: reqwest::Client::new(),
            max_tokens: 4096,
        }
    }

    /// Create from environment variables
    ///
    /// Reads `ANTHROPIC_API_KEY` from environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new("claude-sonnet-4-20250514").with_api_key(
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        )
    }

    /// Set API key
    ///
    /// # Arguments
    ///
    /// * `key` - API key
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = key.into();
        self
    }

    /// Set base URL
    ///
    /// # Arguments
    ///
    /// * `url` - Base URL
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Set max tokens
    ///
    /// # Arguments
    ///
    /// * `n` - Maximum tokens to generate
    #[must_use]
    pub const fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Set temperature
    ///
    /// # Arguments
    ///
    /// * `t` - Temperature (0.0 to 1.0)
    #[must_use]
    pub const fn with_temperature(mut self, t: f32) -> Self {
        self.default_options.temperature = Some(t);
        self
    }
}

#[async_trait]
impl ChatModel for ChatAnthropic {
    async fn invoke(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Basic implementation returning an empty AI message.
        // Full implementation would make HTTP request to Anthropic Messages API.
        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        })
    }

    async fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // Basic implementation returning an empty stream.
        // Full implementation would use SSE to stream from Anthropic API.
        Ok(Box::pin(stream::empty()))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        Self {
            tools,
            ..self.clone()
        }
    }

    fn with_structured_output<T: crate::llm::JsonSchema + crate::llm::DeserializeOwned>(
        self,
    ) -> StructuredOutputModel<Self, T>
    where
        Self: Sized,
    {
        StructuredOutputModel {
            inner: self,
            _phantom: std::marker::PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// `OpenAI` implementation
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "Fields reserved for future API implementation")]
pub struct ChatOpenAI {
    /// Model name
    model: String,
    /// API key
    api_key: String,
    /// Base URL
    base_url: String,
    /// Default call options
    default_options: CallOptions,
    /// Registered tools
    tools: Vec<ToolDefinition>,
    /// HTTP client
    http_client: reqwest::Client,
}

impl ChatOpenAI {
    /// Create new `OpenAI` client
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., `gpt-4o`)
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            default_options: CallOptions::default(),
            tools: vec![],
            http_client: reqwest::Client::new(),
        }
    }

    /// Create from environment variables
    ///
    /// Reads `OPENAI_API_KEY` from environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new("gpt-4o").with_api_key(std::env::var("OPENAI_API_KEY").unwrap_or_default())
    }

    /// Set API key
    ///
    /// # Arguments
    ///
    /// * `key` - API key
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = key.into();
        self
    }

    /// Set base URL
    ///
    /// # Arguments
    ///
    /// * `url` - Base URL (for compatible APIs)
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait]
impl ChatModel for ChatOpenAI {
    async fn invoke(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Basic implementation returning an empty AI message.
        // Full implementation would make HTTP request to OpenAI Chat Completions API.
        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        })
    }

    async fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // Basic implementation returning an empty stream.
        // Full implementation would use SSE to stream from OpenAI API.
        Ok(Box::pin(stream::empty()))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        Self {
            tools,
            ..self.clone()
        }
    }

    fn with_structured_output<T: crate::llm::JsonSchema + crate::llm::DeserializeOwned>(
        self,
    ) -> StructuredOutputModel<Self, T>
    where
        Self: Sized,
    {
        StructuredOutputModel {
            inner: self,
            _phantom: std::marker::PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Ollama implementation
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "Fields reserved for future API implementation")]
pub struct ChatOllama {
    /// Model name
    model: String,
    /// Base URL
    base_url: String,
    /// Default call options
    default_options: CallOptions,
    /// Registered tools
    tools: Vec<ToolDefinition>,
    /// HTTP client
    http_client: reqwest::Client,
}

impl ChatOllama {
    /// Create new Ollama client
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., "llama3")
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: "http://localhost:11434".to_string(),
            default_options: CallOptions::default(),
            tools: vec![],
            http_client: reqwest::Client::new(),
        }
    }

    /// Set base URL
    ///
    /// # Arguments
    ///
    /// * `url` - Ollama server URL
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait]
impl ChatModel for ChatOllama {
    async fn invoke(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        // Basic implementation returning an empty AI message.
        // Full implementation would make HTTP request to Ollama API.
        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(String::new()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        })
    }

    async fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // Basic implementation returning an empty stream.
        // Full implementation would stream from Ollama API.
        Ok(Box::pin(stream::empty()))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        Self {
            tools,
            ..self.clone()
        }
    }

    fn with_structured_output<T: crate::llm::JsonSchema + crate::llm::DeserializeOwned>(
        self,
    ) -> StructuredOutputModel<Self, T>
    where
        Self: Sized,
    {
        StructuredOutputModel {
            inner: self,
            _phantom: std::marker::PhantomData,
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

// Rust guideline compliant 2026-05-19
