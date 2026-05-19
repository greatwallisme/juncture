// LLM provider implementations
//
// This module provides concrete implementations of ChatModel for various
// LLM providers: Anthropic, OpenAI, and Ollama.

use async_trait::async_trait;
use futures::stream;

use crate::llm::{
    CallOptions, ChatModel, LlmError, MessageChunk, StructuredOutputModel, ToolDefinition,
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
        Self::new("claude-sonnet-4-20250514")
            .with_api_key(std::env::var("ANTHROPIC_API_KEY").unwrap_or_default())
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
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = key.into();
        self
    }

    /// Set base URL
    ///
    /// Compatible with Groq, Together AI, vLLM, Azure `OpenAI` endpoints.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Build the request body for the `OpenAI` Chat Completions API
    fn build_request_body(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> serde_json::Value {
        let opts = options.unwrap_or(&self.default_options);

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages.iter().map(|m| {
                let mut msg = serde_json::json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::Human => "user",
                        Role::Ai => "assistant",
                        Role::Tool => "tool",
                    },
                    "content": match &m.content {
                        Content::Text(t) => serde_json::Value::String(t.clone()),
                        Content::MultiPart(parts) => serde_json::Value::Array(
                            parts.iter().map(|p| match p {
                                crate::state::ContentPart::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text
                                }),
                                _ => serde_json::Value::Null,
                            }).collect()
                        ),
                    },
                });
                if !m.tool_calls.is_empty() {
                    msg["tool_calls"] = serde_json::json!(
                        m.tool_calls.iter().map(|tc| serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.args.to_string(),
                            }
                        })).collect::<Vec<_>>()
                    );
                }
                if let Some(ref tc_id) = m.tool_call_id {
                    msg["tool_call_id"] = serde_json::Value::String(tc_id.clone());
                }
                msg
            }).collect::<Vec<_>>(),
        });

        if let Some(temp) = opts.temperature {
            body["temperature"] = serde_json::Value::from(temp);
        }
        if let Some(max_tokens) = opts.max_tokens {
            body["max_tokens"] = serde_json::Value::from(max_tokens);
        }
        if let Some(top_p) = opts.top_p {
            body["top_p"] = serde_json::Value::from(top_p);
        }
        if let Some(ref stop) = opts.stop_sequences {
            body["stop"] = serde_json::json!(stop);
        }

        if !self.tools.is_empty() {
            body["tools"] = serde_json::json!(
                self.tools
                    .iter()
                    .map(|t| serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    }))
                    .collect::<Vec<_>>()
            );
        }

        body
    }

    /// Parse the `OpenAI` Chat Completions response into a Message
    fn parse_response(resp: &serde_json::Value) -> Result<Message, LlmError> {
        let choice = resp
            .get("choices")
            .and_then(|c| c.get(0))
            .ok_or_else(|| LlmError::InvalidResponse("No choices in response".to_string()))?;

        let message = choice
            .get("message")
            .ok_or_else(|| LlmError::InvalidResponse("No message in choice".to_string()))?;

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let tool_calls: Vec<crate::state::ToolCall> = message
            .get("tool_calls")
            .and_then(|tc| tc.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| {
                        let id = tc.get("id")?.as_str()?.to_string();
                        let func = tc.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let args = func
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Null);
                        Some(crate::state::ToolCall { id, name, args })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = resp.get("usage").and_then(|u| {
            Some(crate::state::TokenUsage {
                input_tokens: u.get("prompt_tokens")?.as_u64()?,
                output_tokens: u.get("completion_tokens")?.as_u64()?,
                total_tokens: u.get("total_tokens")?.as_u64()?,
            })
        });

        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(content),
            tool_calls,
            tool_call_id: None,
            name: None,
            usage,
        })
    }
}

#[async_trait]
impl ChatModel for ChatOpenAI {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let body = self.build_request_body(messages, options);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() || e.is_timeout() {
                    LlmError::NetworkError(e.to_string())
                } else if e.is_status() {
                    match e.status() {
                        Some(reqwest::StatusCode::UNAUTHORIZED) => {
                            LlmError::AuthError("Invalid API key".to_string())
                        }
                        Some(reqwest::StatusCode::TOO_MANY_REQUESTS) => {
                            LlmError::RateLimited { retry_after: None }
                        }
                        _ => LlmError::NetworkError(e.to_string()),
                    }
                } else {
                    LlmError::NetworkError(e.to_string())
                }
            })?;

        let status = response.status();
        let resp_text = response
            .text()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LlmError::AuthError("Invalid API key".to_string()));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::RateLimited { retry_after: None });
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            if resp_text.contains("context_length_exceeded")
                || resp_text.contains("maximum context length")
            {
                return Err(LlmError::ContextLengthExceeded { used: 0, limit: 0 });
            }
            return Err(LlmError::InvalidResponse(resp_text));
        }
        if !status.is_success() {
            return Err(LlmError::InvalidResponse(format!(
                "HTTP {status}: {resp_text}"
            )));
        }

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Invalid JSON: {e}")))?;

        Self::parse_response(&resp_json)
    }

    async fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        // SSE streaming implementation deferred to facade crate
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
