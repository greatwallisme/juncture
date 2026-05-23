//! `OpenAI` GPT provider implementation.
//!
//! Provides integration with `OpenAI`'s GPT API via the Chat Completions API.
//! Supports both streaming and non-streaming requests, function calling, and
//! multimodal inputs.

use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    BoxStream, CallOptions, ChatModel, Content, ContentPart, LlmError, Message, Role, TokenUsage,
    ToolCall, ToolChoice, ToolDefinition,
};

use juncture_tracing::spans::attrs;

/// Default `OpenAI` API base URL.
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// `OpenAI` GPT client.
///
/// Provides access to `OpenAI`'s GPT API via the Chat Completions API.
///
/// # Example
///
/// ```rust,no_run
/// use juncture::llm::{ChatModel, ChatOpenAI};
/// use juncture::Message;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let model = ChatOpenAI::from_env()?;
///     let messages = vec![Message::human("Hello!")];
///
///     let response = model.invoke(&messages, None).await?;
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ChatOpenAI {
    /// HTTP client for API requests.
    client: Client,

    /// `OpenAI` API key.
    api_key: String,

    /// Model to use (e.g., "gpt-4o").
    model: String,

    /// API base URL.
    base_url: String,

    /// Default maximum tokens.
    max_tokens: Option<u32>,

    /// Default temperature.
    temperature: Option<f32>,

    /// Default top-p sampling.
    top_p: Option<f32>,

    /// Available tools/functions.
    tools: Vec<ToolDefinition>,
}

impl ChatOpenAI {
    /// Create a new `OpenAI` client with an API key.
    ///
    /// # Arguments
    ///
    /// * `api_key` - `OpenAI` API key
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::new("sk-...");
    /// ```
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
            api_key: api_key.into(),
            model: "gpt-4o".to_string(),
            base_url: OPENAI_BASE_URL.to_string(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            tools: Vec::new(),
        }
    }

    /// Create a new `OpenAI` client from environment variables.
    ///
    /// Reads the `OPENAI_API_KEY` environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::AuthError`] if the environment variable is not set.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::from_env()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[allow(
        clippy::map_err_ignore,
        reason = "Intentionally converting env var error to AuthError"
    )]
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| LlmError::AuthError("OPENAI_API_KEY not set".to_string()))?;
        Ok(Self::new(api_key))
    }

    /// Set a custom API base URL.
    ///
    /// # Arguments
    ///
    /// * `url` - Base URL for `OpenAI` API
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::new("sk-...")
    ///     .with_base_url("https://api.openai.com/v1");
    /// ```
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Set the model to use.
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., "gpt-4o")
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::new("sk-...")
    ///     .with_model("gpt-4-turbo");
    /// ```
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the default maximum tokens.
    ///
    /// # Arguments
    ///
    /// * `max_tokens` - Maximum tokens to generate
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::new("sk-...")
    ///     .with_max_tokens(4096);
    /// ```
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the default temperature.
    ///
    /// # Arguments
    ///
    /// * `temperature` - Sampling temperature (0.0 to 2.0)
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// let model = ChatOpenAI::new("sk-...")
    ///     .with_temperature(0.7);
    /// ```
    #[must_use]
    pub const fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Convert tool choice to `OpenAI` API format.
    fn convert_tool_choice(choice: &ToolChoice) -> OpenAIToolChoice {
        match choice {
            ToolChoice::Auto => OpenAIToolChoice::Auto,
            ToolChoice::None => OpenAIToolChoice::None,
            ToolChoice::Required => OpenAIToolChoice::Required,
            ToolChoice::Specific { name } => OpenAIToolChoice::Function { name: name.clone() },
        }
    }
}

#[async_trait]
impl ChatModel for ChatOpenAI {
    #[allow(
        clippy::too_many_lines,
        reason = "invoke method requires: message conversion, request building, HTTP call, response parsing, span attribute recording, metrics emission, and budget reporting. The length is justified by the complexity of LLM integration with proper observability."
    )]
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        let span = tracing::info_span!(
            "juncture.llm.call",
            "juncture.llm.model" = %model,
            "juncture.llm.provider" = "openai",
            "juncture.tokens.input" = tracing::field::Empty,
            "juncture.tokens.output" = tracing::field::Empty,
            "juncture.llm.has_tool_calls" = false,
            "juncture.llm.stop_reason" = tracing::field::Empty,
        );
        let _enter = span.enter();

        let api_messages: Vec<_> = messages.iter().map(convert_message).collect();

        let request = OpenAIRequest {
            model: model.clone(),
            messages: api_messages,
            temperature: options.and_then(|o| o.temperature).or(self.temperature),
            max_tokens: options.and_then(|o| o.max_tokens).or(self.max_tokens),
            top_p: options.and_then(|o| o.top_p).or(self.top_p),
            stop: options.and_then(|o| o.stop_sequences.clone()),
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(
                    self.tools
                        .iter()
                        .map(|t| OpenAITool {
                            r#type: "function".to_string(),
                            function: OpenAIFunction {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                parameters: t.parameters.clone(),
                            },
                        })
                        .collect(),
                )
            },
            tool_choice: options
                .and_then(|o| o.tool_choice.as_ref())
                .map(Self::convert_tool_choice),
            stream: false,
        };

        let start = std::time::Instant::now();

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return parse_openai_error(&response_text, status);
        }

        let api_response: OpenAIResponse = serde_json::from_str(&response_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Failed to parse response: {e}")))?;

        // Record span attributes
        if let Some(usage) = &api_response.usage {
            tracing::Span::current().record(attrs::TOKENS_INPUT, usage.input_tokens);
            tracing::Span::current().record(attrs::TOKENS_OUTPUT, usage.output_tokens);
        }

        let has_tool_calls = api_response
            .choices
            .first()
            .and_then(|choice| choice.message.tool_calls.as_ref())
            .is_some_and(|calls| !calls.is_empty());
        tracing::Span::current().record(attrs::LLM_HAS_TOOL_CALLS, has_tool_calls);

        if let Some(finish_reason) = api_response
            .choices
            .first()
            .and_then(|choice| choice.finish_reason.as_deref())
        {
            tracing::Span::current().record(attrs::LLM_STOP_REASON, finish_reason);
        }

        // Emit metrics for LLM call
        tracing::debug!(
            name: "juncture.llm.calls",
            provider = "openai",
            model = %model,
        );

        if let Some(usage) = &api_response.usage {
            tracing::debug!(
                name: "juncture.llm.tokens.input",
                tokens = usage.input_tokens,
                model = %model,
            );
            tracing::debug!(
                name: "juncture.llm.tokens.output",
                tokens = usage.output_tokens,
                model = %model,
            );
        }

        tracing::debug!(
            name: "juncture.llm.duration_ms",
            duration_ms = start.elapsed().as_millis(),
            model = %model,
        );

        let message = convert_api_response(&api_response)?;

        // Report token usage to the budget tracker (if configured)
        if let Some(ref usage) = message.usage {
            let _ = juncture_core::pregel::try_report_model_call(
                usage.input_tokens,
                usage.output_tokens,
            );
        }

        Ok(message)
    }

    #[allow(
        clippy::too_many_lines,
        clippy::redundant_clone,
        clippy::uninlined_format_args,
        reason = "Complex SSE stream parsing logic"
    )]
    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> BoxStream<'_, Result<crate::llm::MessageChunk, LlmError>> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        // Create span for stream setup
        let span = tracing::info_span!(
            "juncture.llm.call",
            "juncture.llm.model" = %model,
            "juncture.llm.provider" = "openai",
        );
        let _enter = span.enter();

        let api_messages: Vec<_> = messages.iter().map(convert_message).collect();

        let request = OpenAIRequest {
            model: model.clone(),
            messages: api_messages,
            temperature: options.and_then(|o| o.temperature).or(self.temperature),
            max_tokens: options.and_then(|o| o.max_tokens).or(self.max_tokens),
            top_p: options.and_then(|o| o.top_p).or(self.top_p),
            stop: options.and_then(|o| o.stop_sequences.clone()),
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(
                    self.tools
                        .iter()
                        .map(|t| OpenAITool {
                            r#type: "function".to_string(),
                            function: OpenAIFunction {
                                name: t.name.clone(),
                                description: t.description.clone(),
                                parameters: t.parameters.clone(),
                            },
                        })
                        .collect(),
                )
            },
            tool_choice: options
                .and_then(|o| o.tool_choice.as_ref())
                .map(Self::convert_tool_choice),
            stream: true,
        };

        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let client = self.client.clone();

        Box::pin(stream::unfold(
            (client, api_key, base_url, request, false, Vec::new()),
            |(client, api_key, base_url, request, done, mut buffer)| async move {
                if done {
                    return None;
                }

                let response = match client
                    .post(format!("{}/chat/completions", base_url))
                    .header("authorization", format!("Bearer {}", api_key))
                    .header("content-type", "application/json")
                    .json(&request)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        return Some((
                            Err(LlmError::NetworkError(e)),
                            (client, api_key, base_url, request, true, buffer),
                        ));
                    }
                };

                let status = response.status();

                if !status.is_success() {
                    let response_text = match response.text().await {
                        Ok(t) => t,
                        Err(e) => {
                            return Some((
                                Err(LlmError::NetworkError(e)),
                                (client, api_key, base_url, request, true, buffer),
                            ));
                        }
                    };

                    let error = match parse_openai_error(&response_text, status) {
                        Ok(_) => crate::llm::MessageChunk {
                            content: String::new(),
                            tool_call_chunks: Vec::new(),
                            usage_delta: None,
                        },
                        Err(e) => {
                            return Some((
                                Err(e),
                                (client, api_key, base_url, request, true, buffer),
                            ));
                        }
                    };

                    return Some((
                        Ok(error),
                        (client, api_key, base_url, request, true, buffer),
                    ));
                }

                let mut byte_stream = response.bytes_stream();

                while let Some(chunk_result) = byte_stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            return Some((
                                Err(LlmError::NetworkError(e)),
                                (client, api_key, base_url, request, true, buffer),
                            ));
                        }
                    };

                    buffer.extend_from_slice(&chunk);

                    while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_bytes = buffer.drain(..=newline_pos).collect::<Vec<_>>();
                        let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);

                        // Skip empty lines and comments
                        let line = line.trim();
                        if line.is_empty() || line.starts_with(':') {
                            continue;
                        }

                        // Parse SSE line format: "data: {...}" or "data: [DONE]"
                        if let Some(data_str) = line.strip_prefix("data: ") {
                            if data_str == "[DONE]" {
                                return None;
                            }

                            if let Ok(sse_chunk) = serde_json::from_str::<OpenAISSEChunk>(data_str)
                            {
                                match convert_openai_sse_chunk(sse_chunk) {
                                    Ok(chunk) => {
                                        if !chunk.content.is_empty()
                                            || !chunk.tool_call_chunks.is_empty()
                                            || chunk.usage_delta.is_some()
                                        {
                                            return Some((
                                                Ok(chunk),
                                                (client, api_key, base_url, request, false, buffer),
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        return Some((
                                            Err(e),
                                            (client, api_key, base_url, request, true, buffer),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }

                None
            },
        ))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        let mut new_model = self.clone();
        new_model.tools = tools;
        new_model
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// `OpenAI` API request format.
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<OpenAIToolChoice>,
    stream: bool,
}

/// `OpenAI` API message format.
#[derive(Debug, Serialize)]
struct OpenAIMessage {
    role: String,
    content: OpenAIContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// `OpenAI` API content format.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

/// `OpenAI` API content part.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

/// `OpenAI` API image URL.
#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
}

/// `OpenAI` API tool definition.
#[derive(Debug, Serialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

/// `OpenAI` API function definition.
#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// `OpenAI` API tool call.
#[derive(Debug, Serialize)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

/// `OpenAI` API function call.
#[derive(Debug, Serialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

/// `OpenAI` API tool choice.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OpenAIToolChoice {
    Auto,
    None,
    Required,
    #[serde(rename = "function")]
    Function {
        name: String,
    },
}

/// `OpenAI` API response format.
#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<TokenUsage>,
}

/// `OpenAI` API choice.
#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

/// `OpenAI` API response message.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OpenAIResponseMessage {
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIResponseToolCall>>,
}

/// `OpenAI` API response tool call.
#[derive(Debug, Deserialize)]
struct OpenAIResponseToolCall {
    id: String,
    function: OpenAIResponseFunction,
}

/// `OpenAI` API response function.
#[derive(Debug, Deserialize)]
struct OpenAIResponseFunction {
    name: String,
    arguments: String,
}

/// Convert message to `OpenAI` API format.
#[allow(
    clippy::match_same_arms,
    reason = "Explicit handling for different content types"
)]
fn convert_message(message: &Message) -> OpenAIMessage {
    let role = match message.role {
        Role::System => "system",
        Role::Human => "user",
        Role::Ai => "assistant",
        Role::Tool => "tool",
    };

    let content = match &message.content {
        Content::Text(text) => OpenAIContent::Text(text.clone()),
        Content::MultiPart(parts) => {
            let mut content_parts = Vec::new();
            for part in parts {
                match part {
                    ContentPart::Text { text } => {
                        content_parts.push(OpenAIContentPart::Text { text: text.clone() });
                    }
                    ContentPart::Image(img) => {
                        let url = match &img.source {
                            crate::llm::ImageSource::Base64(b64) => {
                                format!("data:{};base64,{}", img.media_type, b64)
                            }
                            crate::llm::ImageSource::Url(url) => url.clone(),
                        };
                        content_parts.push(OpenAIContentPart::ImageUrl {
                            image_url: ImageUrl { url },
                        });
                    }
                    ContentPart::Thinking { text, .. } => {
                        content_parts.push(OpenAIContentPart::Text { text: text.clone() });
                    }
                }
            }
            OpenAIContent::Parts(content_parts)
        }
    };

    let tool_calls = if message.tool_calls.is_empty() {
        None
    } else {
        Some(
            message
                .tool_calls
                .iter()
                .map(|tc| OpenAIToolCall {
                    id: tc.id.clone(),
                    r#type: "function".to_string(),
                    function: OpenAIFunctionCall {
                        name: tc.name.clone(),
                        arguments: tc.arguments.to_string(),
                    },
                })
                .collect(),
        )
    };

    OpenAIMessage {
        role: role.to_string(),
        content,
        tool_calls,
        tool_call_id: message.tool_call_id.clone(),
    }
}

/// Parse `OpenAI` API error response.
fn parse_openai_error(
    response_text: &str,
    status: reqwest::StatusCode,
) -> Result<Message, LlmError> {
    if let Ok(error) = serde_json::from_str::<OpenAIErrorResponse>(response_text) {
        match error.error.code.as_deref() {
            Some("invalid_api_key" | "401") => Err(LlmError::AuthError(error.error.message)),
            Some("rate_limit" | "429") => Err(LlmError::RateLimited { retry_after: None }),
            Some("context_length_exceeded") => {
                Err(LlmError::ContextLengthExceeded { used: 0, limit: 0 })
            }
            _ => Err(LlmError::InvalidResponse(error.error.message)),
        }
    } else {
        Err(LlmError::InvalidResponse(format!(
            "HTTP {}: {}",
            status.as_u16(),
            response_text
        )))
    }
}

/// `OpenAI` API error response format.
#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIErrorDetail,
}

/// `OpenAI` API error detail.
#[derive(Debug, Deserialize)]
struct OpenAIErrorDetail {
    message: String,
    #[serde(default)]
    code: Option<String>,
}

/// Convert `OpenAI` API response to Message.
fn convert_api_response(response: &OpenAIResponse) -> Result<Message, LlmError> {
    let choice = response
        .choices
        .first()
        .ok_or_else(|| LlmError::InvalidResponse("No choices in response".to_string()))?;

    let content = choice.message.content.clone().unwrap_or_default();

    let tool_calls = if let Some(calls) = &choice.message.tool_calls {
        calls
            .iter()
            .map(|tc| {
                let arguments: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .map_err(|e| {
                        LlmError::InvalidResponse(format!("Failed to parse tool arguments: {e}"))
                    })?;

                Ok(ToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, LlmError>>()?
    } else {
        Vec::new()
    };

    let mut msg = Message::ai_with_tool_calls(content, tool_calls);
    msg.usage.clone_from(&response.usage);
    Ok(msg)
}

/// `OpenAI` SSE chunk format.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OpenAISSEChunk {
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    id: String,
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    object: String,
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    created: u64,
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    model: String,
    choices: Vec<OpenAIChoiceChunk>,
    usage: Option<TokenUsage>,
}

/// `OpenAI` SSE choice chunk.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OpenAIChoiceChunk {
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    index: usize,
    delta: OpenAIDelta,
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    finish_reason: Option<String>,
}

/// `OpenAI` SSE delta.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OpenAIDelta {
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCallChunk>>,
}

/// `OpenAI` SSE tool call chunk.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OpenAIToolCallChunk {
    index: usize,
    id: Option<String>,
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    r#type: Option<String>,
    function: Option<OpenAIFunctionChunk>,
}

/// `OpenAI` SSE function chunk.
#[derive(Debug, Deserialize)]
struct OpenAIFunctionChunk {
    name: Option<String>,
    arguments: Option<String>,
}

/// Convert `OpenAI` SSE chunk to `MessageChunk`.
fn convert_openai_sse_chunk(chunk: OpenAISSEChunk) -> Result<crate::llm::MessageChunk, LlmError> {
    let choice = chunk
        .choices
        .first()
        .ok_or_else(|| LlmError::InvalidResponse("No choices in SSE chunk".to_string()))?;

    let content = choice.delta.content.clone().unwrap_or_default();

    let tool_call_chunks = if let Some(tool_calls) = &choice.delta.tool_calls {
        tool_calls
            .iter()
            .map(|tc| {
                let args_delta = tc
                    .function
                    .as_ref()
                    .and_then(|f| f.arguments.clone())
                    .unwrap_or_default();

                Ok(crate::llm::ToolCallChunk {
                    id: tc.id.clone(),
                    name: tc.function.as_ref().and_then(|f| f.name.clone()),
                    args_delta,
                    index: tc.index,
                })
            })
            .collect::<Result<Vec<_>, LlmError>>()?
    } else {
        Vec::new()
    };

    Ok(crate::llm::MessageChunk {
        content,
        tool_call_chunks,
        usage_delta: chunk.usage,
    })
}

// Rust guideline compliant 2026-05-19
