// LLM provider implementations
//
// This module provides concrete implementations of ChatModel for various
// LLM providers: Anthropic, OpenAI, and Ollama.

use async_trait::async_trait;
use futures::stream::{self, StreamExt};

use crate::llm::{
    CallOptions, ChatModel, LlmError, MessageChunk, StructuredOutputModel, ToolDefinition,
};
use crate::state::{Content, Message, Role};

/// Anthropic Claude implementation
#[derive(Clone, Debug)]
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
    ///
    /// # Panics
    ///
    /// Panics if `ANTHROPIC_API_KEY` environment variable is not set.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new("claude-sonnet-4-20250514").with_api_key(
            std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set"),
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

    /// Build the request body for the Anthropic Messages API
    fn build_request_body(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> serde_json::Value {
        let opts = options.unwrap_or(&self.default_options);

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": messages.iter().map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        Role::System | Role::Human | Role::Tool => "user",
                        Role::Ai => "assistant",
                    },
                    "content": match &m.content {
                        Content::Text(t) => serde_json::Value::Array(vec![
                            serde_json::json!({"type": "text", "text": t})
                        ]),
                        Content::MultiPart(parts) => serde_json::Value::Array(
                            parts.iter().map(|p| match p {
                                crate::state::ContentPart::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text
                                }),
                                _ => serde_json::json!({"type": "text", "text": ""}),
                            }).collect()
                        ),
                    },
                })
            }).collect::<Vec<_>>(),
        });

        if let Some(temp) = opts.temperature {
            body["temperature"] = serde_json::Value::from(temp);
        }
        if let Some(top_p) = opts.top_p {
            body["top_p"] = serde_json::Value::from(top_p);
        }
        if let Some(ref stop) = opts.stop_sequences {
            body["stop_sequences"] = serde_json::json!(stop);
        }
        if let Some(max_tokens) = opts.max_tokens {
            body["max_tokens"] = serde_json::Value::from(max_tokens);
        }

        if !self.tools.is_empty() {
            body["tools"] = serde_json::json!(
                self.tools
                    .iter()
                    .map(|t| serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    }))
                    .collect::<Vec<_>>()
            );
        }

        body
    }

    /// Send an HTTP request to the Anthropic API and return the raw response
    async fn send_request(&self, body: &serde_json::Value) -> Result<reqwest::Response, LlmError> {
        let url = format!("{}/v1/messages", self.base_url);

        self.http_client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(body)
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
            })
    }

    /// Check HTTP status and return an appropriate error if not successful
    async fn check_status(response: reqwest::Response) -> Result<reqwest::Response, LlmError> {
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::AuthError(format!("Invalid API key: {body}")));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::RateLimited { retry_after: None });
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            let body = response.text().await.unwrap_or_default();
            if body.contains("context_length_exceeded") || body.contains("maximum context length") {
                return Err(LlmError::ContextLengthExceeded { used: 0, limit: 0 });
            }
            return Err(LlmError::InvalidResponse(body));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::InvalidResponse(format!("HTTP {status}: {body}")));
        }
        Ok(response)
    }
}

#[async_trait]
impl ChatModel for ChatAnthropic {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let body = self.build_request_body(messages, options);
        let response = self.send_request(&body).await?;
        let response = Self::check_status(response).await?;

        let resp_text = response
            .text()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Invalid JSON: {e}")))?;

        // Parse Anthropic response
        let content_array = resp_json
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| LlmError::InvalidResponse("No content in response".to_string()))?;

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for content_block in content_array {
            if let Some(block_type) = content_block.get("type").and_then(|t| t.as_str()) {
                match block_type {
                    "text" => {
                        if let Some(text) = content_block.get("text").and_then(|t| t.as_str()) {
                            text_content.push_str(text);
                        }
                    }
                    "tool_use" => {
                        let id = content_block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = content_block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = content_block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        tool_calls.push(crate::state::ToolCall {
                            id,
                            name,
                            arguments,
                        });
                    }
                    _ => {}
                }
            }
        }

        let usage = resp_json.get("usage").and_then(|u| {
            Some(crate::state::TokenUsage {
                input_tokens: u.get("input_tokens")?.as_u64()?,
                output_tokens: u.get("output_tokens")?.as_u64()?,
                total_tokens: u.get("input_tokens")?.as_u64()?
                    + u.get("output_tokens")?.as_u64()?,
            })
        });

        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(text_content),
            tool_calls,
            tool_call_id: None,
            name: None,
            usage,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "SSE stream parsing requires detailed event type handling"
    )]
    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        let mut body = self.build_request_body(messages, options);
        body["stream"] = serde_json::Value::Bool(true);

        let response = self.send_request(&body).await?;
        let response = Self::check_status(response).await?;

        // Convert the response body into a channel-based stream to sever
        // the borrow from the Response object, which is required because
        // async_trait boxes the future and self-referential borrows are
        // not possible across that boundary.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<tokio_util::bytes::Bytes, LlmError>>(8);
        tokio::spawn(async move {
            let mut reader = response;
            loop {
                match reader.chunk().await {
                    Ok(Some(chunk)) => {
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::NetworkError(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        let receiver_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let byte_stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<tokio_util::bytes::Bytes, LlmError>> + Send>,
        > = Box::pin(receiver_stream);
        let sse_buffer = String::new();
        let pending_chunks: Vec<Result<MessageChunk, LlmError>> = Vec::new();

        let chunk_stream = stream::unfold(
            (byte_stream, sse_buffer, pending_chunks),
            |(mut byte_stream, mut sse_buffer, mut pending_chunks)| async move {
                loop {
                    if let Some(chunk) = pending_chunks.pop() {
                        return Some((chunk, (byte_stream, sse_buffer, pending_chunks)));
                    }

                    match byte_stream.next().await {
                        Some(Ok(bytes)) => {
                            sse_buffer.push_str(&String::from_utf8_lossy(&bytes));

                            let mut new_chunks = Vec::new();
                            while let Some(pos) = sse_buffer.find("\n\n") {
                                let raw_event = sse_buffer[..pos].to_string();
                                sse_buffer.drain(..pos + 2);
                                new_chunks.push(parse_sse_event(&raw_event));
                            }
                            // Reverse so we can pop() in order
                            new_chunks.reverse();
                            pending_chunks = new_chunks;
                        }
                        Some(Err(e)) => {
                            return Some((Err(e), (byte_stream, sse_buffer, pending_chunks)));
                        }
                        None => return None,
                    }
                }
            },
        );

        Ok(Box::pin(chunk_stream))
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
                                "arguments": tc.arguments.to_string(),
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
                        let arguments = func
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Null);
                        Some(crate::state::ToolCall {
                            id,
                            name,
                            arguments,
                        })
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
pub struct ChatOllama {
    /// Model name
    model: String,
    /// Base URL
    base_url: String,
    /// Default call options
    default_options: CallOptions,
    /// Registered tools (reserved for Ollama tool calling support)
    #[expect(
        dead_code,
        reason = "Ollama tool calling not yet integrated into build_request_body"
    )]
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

    /// Build the request body for the Ollama Chat API
    fn build_request_body(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
        stream: bool,
    ) -> serde_json::Value {
        let opts = options.unwrap_or(&self.default_options);

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages.iter().map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        Role::System => "system",
                        Role::Human => "user",
                        Role::Ai => "assistant",
                        Role::Tool => "tool",
                    },
                    "content": match &m.content {
                        Content::Text(t) => t.clone(),
                        Content::MultiPart(parts) => {
                            parts.iter()
                                .filter_map(|p| match p {
                                    crate::state::ContentPart::Text { text } => Some(text.clone()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        }
                    },
                })
            }).collect::<Vec<_>>(),
            "stream": stream,
        });

        if let Some(temp) = opts.temperature {
            body["options"] = serde_json::json!({"temperature": temp});
        }
        if let Some(top_p) = opts.top_p {
            if body.get("options").is_none() {
                body["options"] = serde_json::json!({});
            }
            body["options"]["top_p"] = serde_json::json!(top_p);
        }
        if let Some(ref stop) = opts.stop_sequences {
            body["stop"] = serde_json::json!(stop);
        }
        if let Some(max_tokens) = opts.max_tokens {
            if body.get("options").is_none() {
                body["options"] = serde_json::json!({});
            }
            body["options"]["num_predict"] = serde_json::json!(max_tokens);
        }

        body
    }

    /// Send an HTTP request to the Ollama API
    async fn send_request(&self, body: &serde_json::Value) -> Result<reqwest::Response, LlmError> {
        let url = format!("{}/api/chat", self.base_url);

        self.http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() || e.is_timeout() {
                    LlmError::NetworkError(format!(
                        "Failed to connect to Ollama at {}: {}. Is Ollama running?",
                        self.base_url, e
                    ))
                } else {
                    LlmError::NetworkError(e.to_string())
                }
            })
    }

    /// Check HTTP status and return an appropriate error if not successful
    async fn check_status(
        response: reqwest::Response,
        model: &str,
    ) -> Result<reqwest::Response, LlmError> {
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if body.contains("model") && body.contains("not found") {
                return Err(LlmError::InvalidResponse(format!(
                    "Model '{model}' not found in Ollama. Run: ollama pull {model}"
                )));
            }
            return Err(LlmError::InvalidResponse(format!("HTTP {status}: {body}")));
        }
        Ok(response)
    }
}

#[async_trait]
impl ChatModel for ChatOllama {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let body = self.build_request_body(messages, options, false);
        let response = self.send_request(&body).await?;
        let response = Self::check_status(response, &self.model).await?;

        let resp_text = response
            .text()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Invalid JSON: {e}")))?;

        let message = resp_json
            .get("message")
            .ok_or_else(|| LlmError::InvalidResponse("No message in response".to_string()))?;

        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let usage = resp_json.get("prompt_eval_count").and_then(|p| {
            Some(crate::state::TokenUsage {
                input_tokens: p.as_u64()?,
                output_tokens: resp_json.get("eval_count")?.as_u64()?,
                total_tokens: p.as_u64()? + resp_json.get("eval_count")?.as_u64()?,
            })
        });

        Ok(Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: Role::Ai,
            content: Content::Text(content),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage,
        })
    }

    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<crate::llm::BoxStream<'_, Result<MessageChunk, LlmError>>, LlmError> {
        let body = self.build_request_body(messages, options, true);
        let response = self.send_request(&body).await?;
        let response = Self::check_status(response, &self.model).await?;

        // Decouple the response body from the Response object via an mpsc
        // channel, same pattern as ChatAnthropic. This is required because
        // async_trait boxes the future and self-referential borrows across
        // that boundary are not possible.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<tokio_util::bytes::Bytes, LlmError>>(8);
        tokio::spawn(async move {
            let mut reader = response;
            loop {
                match reader.chunk().await {
                    Ok(Some(chunk)) => {
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::NetworkError(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        let receiver_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let byte_stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<tokio_util::bytes::Bytes, LlmError>> + Send>,
        > = Box::pin(receiver_stream);
        let ndjson_buffer = String::new();
        let pending_chunks: Vec<Result<MessageChunk, LlmError>> = Vec::new();

        let chunk_stream = stream::unfold(
            (byte_stream, ndjson_buffer, pending_chunks),
            |(mut byte_stream, mut ndjson_buffer, mut pending_chunks)| async move {
                loop {
                    if let Some(chunk) = pending_chunks.pop() {
                        return Some((chunk, (byte_stream, ndjson_buffer, pending_chunks)));
                    }

                    match byte_stream.next().await {
                        Some(Ok(bytes)) => {
                            ndjson_buffer.push_str(&String::from_utf8_lossy(&bytes));

                            let mut new_chunks = Vec::new();
                            while let Some(pos) = ndjson_buffer.find('\n') {
                                let line = ndjson_buffer[..pos].trim().to_string();
                                ndjson_buffer.drain(..=pos);
                                if line.is_empty() {
                                    continue;
                                }
                                new_chunks.push(parse_ollama_ndjson_line(&line));
                            }
                            new_chunks.reverse();
                            pending_chunks = new_chunks;
                        }
                        Some(Err(e)) => {
                            return Some((Err(e), (byte_stream, ndjson_buffer, pending_chunks)));
                        }
                        None => return None,
                    }
                }
            },
        );

        Ok(Box::pin(chunk_stream))
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

/// Parse a raw SSE event string into event type and data JSON
///
/// SSE format is: `event: <type>\ndata: <json>\n`
fn extract_sse_fields(raw: &str) -> Option<(String, &str)> {
    let mut event_type = None;
    let mut data_line = None;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_line = Some(rest.trim());
        }
    }

    event_type.map(|et| (et, data_line.unwrap_or("{}")))
}

/// Construct an empty [`MessageChunk`] with no content.
const fn empty_chunk() -> MessageChunk {
    MessageChunk {
        role: None,
        content: String::new(),
        tool_call_chunks: vec![],
        usage: None,
    }
}

/// Parse a single raw SSE event (between `\n\n` boundaries) into a
/// `Result<MessageChunk, LlmError>` suitable for the output stream.
fn parse_sse_event(raw: &str) -> Result<MessageChunk, LlmError> {
    let Some((event_type, data)) = extract_sse_fields(raw) else {
        return Ok(empty_chunk());
    };

    let json: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| LlmError::InvalidResponse(format!("Invalid SSE data JSON: {e}")))?;

    match event_type.as_str() {
        "message_start" => {
            let usage = json
                .get("message")
                .and_then(|m| m.get("usage"))
                .and_then(|u| {
                    Some(crate::state::TokenUsage {
                        input_tokens: u.get("input_tokens")?.as_u64()?,
                        output_tokens: u.get("output_tokens")?.as_u64()?,
                        total_tokens: u.get("input_tokens")?.as_u64()?
                            + u.get("output_tokens")?.as_u64()?,
                    })
                });
            Ok(MessageChunk {
                role: Some(Role::Ai),
                content: String::new(),
                tool_call_chunks: vec![],
                usage,
            })
        }
        "content_block_start" => Ok(parse_content_block_start(&json)),
        "content_block_delta" => Ok(parse_content_block_delta(&json)),
        "message_delta" => Ok(parse_message_delta(&json)),
        "error" => {
            let error_msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown streaming error")
                .to_string();
            Err(LlmError::InvalidResponse(error_msg))
        }
        _ => Ok(empty_chunk()),
    }
}

/// Extract a content block index from an SSE event JSON payload.
///
/// Anthropic always sends index as a small non-negative integer, so
/// truncation from u64 to usize is safe in practice.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Anthropic index is always small"
)]
fn extract_index(json: &serde_json::Value) -> usize {
    json.get("index")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize
}

/// Parse a `content_block_start` SSE event.
fn parse_content_block_start(json: &serde_json::Value) -> MessageChunk {
    let index = extract_index(json);
    let content_block = json
        .get("content_block")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let block_type = content_block
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    if block_type == "tool_use" {
        let id = content_block
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = content_block
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        MessageChunk {
            role: None,
            content: String::new(),
            tool_call_chunks: vec![crate::llm::ToolCallChunk {
                id: Some(id),
                name: Some(name),
                args_delta: String::new(),
                index,
            }],
            usage: None,
        }
    } else {
        empty_chunk()
    }
}

/// Parse a `content_block_delta` SSE event.
fn parse_content_block_delta(json: &serde_json::Value) -> MessageChunk {
    let index = extract_index(json);
    let delta = json
        .get("delta")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let delta_type = delta
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    match delta_type {
        "text_delta" => {
            let text = delta
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            MessageChunk {
                role: None,
                content: text,
                tool_call_chunks: vec![],
                usage: None,
            }
        }
        "input_json_delta" => {
            let partial = delta
                .get("partial_json")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            MessageChunk {
                role: None,
                content: String::new(),
                tool_call_chunks: vec![crate::llm::ToolCallChunk {
                    id: None,
                    name: None,
                    args_delta: partial,
                    index,
                }],
                usage: None,
            }
        }
        _ => empty_chunk(),
    }
}

/// Parse a `message_delta` SSE event (usage and stop reason).
fn parse_message_delta(json: &serde_json::Value) -> MessageChunk {
    let output_tokens = json
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let usage = crate::state::TokenUsage {
        input_tokens: 0,
        output_tokens,
        total_tokens: output_tokens,
    };
    MessageChunk {
        role: None,
        content: String::new(),
        tool_call_chunks: vec![],
        usage: Some(usage),
    }
}

/// Parse a single NDJSON line from the Ollama streaming API into a
/// `Result<MessageChunk, LlmError>`.
///
/// Ollama streaming format: each line is a complete JSON object with fields:
/// - `message.content` - text delta
/// - `done` - whether this is the final chunk
/// - `eval_count` / `prompt_eval_count` - token counts (final chunk only)
fn parse_ollama_ndjson_line(line: &str) -> Result<MessageChunk, LlmError> {
    let json: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| LlmError::InvalidResponse(format!("Invalid Ollama NDJSON: {e}")))?;

    if let Some(error_msg) = json.get("error").and_then(serde_json::Value::as_str) {
        return Err(LlmError::InvalidResponse(error_msg.to_string()));
    }

    let content = json
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    let done = json
        .get("done")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let usage = done.then(|| {
        let input = json
            .get("prompt_eval_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let output = json
            .get("eval_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        crate::state::TokenUsage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
        }
    });

    Ok(MessageChunk {
        role: done.then_some(Role::Ai),
        content,
        tool_call_chunks: vec![],
        usage,
    })
}

// Rust guideline compliant 2026-05-21

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sse_fields_valid_event() {
        let raw = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\"}";
        let (event_type, data) = extract_sse_fields(raw).expect("should parse");
        assert_eq!(event_type, "content_block_delta");
        assert_eq!(data, "{\"type\":\"content_block_delta\"}");
    }

    #[test]
    fn test_extract_sse_fields_no_event_prefix() {
        let raw = "data: {}";
        assert!(extract_sse_fields(raw).is_none());
    }

    #[test]
    fn test_extract_sse_fields_empty() {
        assert!(extract_sse_fields("").is_none());
    }

    #[test]
    fn test_parse_sse_event_text_delta() {
        let raw = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.content, "Hello");
        assert!(chunk.tool_call_chunks.is_empty());
    }

    #[test]
    fn test_parse_sse_event_message_start() {
        let raw = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-opus\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.role, Some(Role::Ai));
        let usage = chunk.usage.expect("should have usage");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 1);
        assert_eq!(usage.total_tokens, 11);
    }

    #[test]
    fn test_parse_sse_event_tool_use_start() {
        let raw = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_abc\",\"name\":\"get_weather\"}}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.tool_call_chunks.len(), 1);
        assert_eq!(chunk.tool_call_chunks[0].id, Some("toolu_abc".to_string()));
        assert_eq!(
            chunk.tool_call_chunks[0].name,
            Some("get_weather".to_string())
        );
        assert_eq!(chunk.tool_call_chunks[0].index, 1);
        assert!(chunk.tool_call_chunks[0].args_delta.is_empty());
    }

    #[test]
    fn test_parse_sse_event_input_json_delta() {
        let raw = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"SF\\\"}\"}}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.tool_call_chunks.len(), 1);
        assert_eq!(chunk.tool_call_chunks[0].args_delta, r#"{"city":"SF"}"#);
        assert_eq!(chunk.tool_call_chunks[0].index, 1);
    }

    #[test]
    fn test_parse_sse_event_message_delta_usage() {
        let raw = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":15}}";
        let chunk = parse_sse_event(raw).expect("should parse");
        let usage = chunk.usage.expect("should have usage");
        assert_eq!(usage.output_tokens, 15);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_parse_sse_event_error() {
        let raw = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"API is overloaded\"}}";
        let result = parse_sse_event(raw);
        assert!(result.is_err());
        match result {
            Err(LlmError::InvalidResponse(msg)) => {
                assert_eq!(msg, "API is overloaded");
            }
            _ => panic!("expected InvalidResponse error"),
        }
    }

    #[test]
    fn test_parse_sse_event_ping() {
        let raw = "event: ping\ndata: {}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.content, "");
        assert!(chunk.tool_call_chunks.is_empty());
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn test_parse_sse_event_unknown_event() {
        let raw = "event: unknown_type\ndata: {}";
        let chunk = parse_sse_event(raw).expect("should parse");
        assert_eq!(chunk.content, "");
    }

    #[test]
    fn test_empty_chunk() {
        let chunk = empty_chunk();
        assert!(chunk.role.is_none());
        assert!(chunk.content.is_empty());
        assert!(chunk.tool_call_chunks.is_empty());
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn test_extract_sse_fields_whitespace_in_prefix() {
        let raw = "event: content_block_start\ndata: {\"type\":\"content_block_start\"}";
        let (event_type, data) = extract_sse_fields(raw).expect("should parse");
        assert_eq!(event_type, "content_block_start");
        assert_eq!(data, "{\"type\":\"content_block_start\"}");
    }

    #[test]
    fn test_parse_ollama_ndjson_content_chunk() {
        let line = r#"{"model":"llama3","created_at":"2024-01-01T00:00:00Z","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        let chunk = parse_ollama_ndjson_line(line).expect("should parse");
        assert_eq!(chunk.content, "Hello");
        assert!(chunk.role.is_none());
        assert!(chunk.tool_call_chunks.is_empty());
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn test_parse_ollama_ndjson_final_chunk() {
        let line = r#"{"model":"llama3","created_at":"2024-01-01T00:00:02Z","message":{"role":"assistant","content":""},"done":true,"total_duration":123456789,"eval_count":10,"prompt_eval_count":20}"#;
        let chunk = parse_ollama_ndjson_line(line).expect("should parse");
        assert!(chunk.content.is_empty());
        assert_eq!(chunk.role, Some(Role::Ai));
        assert!(chunk.tool_call_chunks.is_empty());
        let usage = chunk.usage.expect("final chunk should have usage");
        assert_eq!(usage.input_tokens, 20);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.total_tokens, 30);
    }

    #[test]
    fn test_parse_ollama_ndjson_error() {
        let line = r#"{"error":"model not found"}"#;
        let result = parse_ollama_ndjson_line(line);
        assert!(result.is_err());
        match result {
            Err(LlmError::InvalidResponse(msg)) => {
                assert_eq!(msg, "model not found");
            }
            _ => panic!("expected InvalidResponse error"),
        }
    }

    #[test]
    fn test_parse_ollama_ndjson_invalid_json() {
        let line = "not json at all";
        let result = parse_ollama_ndjson_line(line);
        assert!(result.is_err());
        match result {
            Err(LlmError::InvalidResponse(msg)) => {
                assert!(msg.starts_with("Invalid Ollama NDJSON:"));
            }
            _ => panic!("expected InvalidResponse error"),
        }
    }

    #[test]
    fn test_parse_ollama_ndjson_mid_stream_chunk() {
        let line = r#"{"model":"llama3","created_at":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":" world"},"done":false}"#;
        let chunk = parse_ollama_ndjson_line(line).expect("should parse");
        assert_eq!(chunk.content, " world");
        assert!(chunk.usage.is_none());
    }

    #[test]
    fn test_parse_ollama_ndjson_final_chunk_no_usage() {
        let line = r#"{"model":"llama3","created_at":"2024-01-01T00:00:02Z","message":{"role":"assistant","content":""},"done":true}"#;
        let chunk = parse_ollama_ndjson_line(line).expect("should parse");
        let usage = chunk
            .usage
            .expect("final chunk should have usage even without eval fields");
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn test_ollama_build_request_body_stream() {
        let ollama = ChatOllama::new("llama3");
        let messages = vec![Message {
            id: "1".to_string(),
            role: Role::Human,
            content: Content::Text("hello".to_string()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
            usage: None,
        }];
        let body = ollama.build_request_body(&messages, None, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "llama3");
    }
}
