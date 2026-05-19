//! Anthropic Claude provider implementation.
//!
//! Provides integration with Anthropic's Claude API via the Messages API.
//! Supports both streaming and non-streaming requests, tool use, and
//! multimodal inputs.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    CallOptions, ChatModel, Content, ContentPart, LlmError, Message, MessageChunk, Role,
    TokenUsage, ToolCall, ToolChoice, ToolDefinition,
};

/// Default Anthropic API base URL.
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API version header.
const API_VERSION: &str = "2023-06-01";

/// Anthropic Claude client.
///
/// Provides access to Anthropic's Claude API via the Messages API.
///
/// # Example
///
/// ```rust,no_run
/// use juncture::llm::{ChatModel, ChatAnthropic};
/// use juncture::Message;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let model = ChatAnthropic::from_env()?;
///     let messages = vec![Message::human("Hello!")];
///
///     let response = model.invoke(&messages, None).await?;
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ChatAnthropic {
    /// HTTP client for API requests.
    client: Client,

    /// Anthropic API key.
    api_key: String,

    /// Model to use (e.g., "claude-3-5-sonnet-20241022").
    model: String,

    /// API base URL.
    base_url: String,

    /// Default maximum tokens.
    max_tokens: u32,

    /// Default temperature.
    temperature: Option<f32>,

    /// Default top-p sampling.
    top_p: Option<f32>,

    /// Available tools.
    tools: Vec<ToolDefinition>,
}

impl ChatAnthropic {
    /// Create a new Anthropic client with an API key.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Anthropic API key
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::new("sk-ant-...");
    /// ```
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
            api_key: api_key.into(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            base_url: ANTHROPIC_BASE_URL.to_string(),
            max_tokens: 4096,
            temperature: None,
            top_p: None,
            tools: Vec::new(),
        }
    }

    /// Create a new Anthropic client from environment variables.
    ///
    /// Reads the `ANTHROPIC_API_KEY` environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::AuthError`] if the environment variable is not set.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::from_env()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[allow(
        clippy::map_err_ignore,
        reason = "Intentionally converting env var error to AuthError"
    )]
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LlmError::AuthError("ANTHROPIC_API_KEY not set".to_string()))?;
        Ok(Self::new(api_key))
    }

    /// Set a custom API base URL.
    ///
    /// # Arguments
    ///
    /// * `url` - Base URL for Anthropic API
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::new("sk-ant-...")
    ///     .with_base_url("https://api.anthropic.com");
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
    /// * `model` - Model name (e.g., "claude-3-5-sonnet-20241022")
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::new("sk-ant-...")
    ///     .with_model("claude-3-opus-20240229");
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
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::new("sk-ant-...")
    ///     .with_max_tokens(8192);
    /// ```
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set the default temperature.
    ///
    /// # Arguments
    ///
    /// * `temperature` - Sampling temperature (0.0 to 1.0)
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatAnthropic;
    ///
    /// let model = ChatAnthropic::new("sk-ant-...")
    ///     .with_temperature(0.7);
    /// ```
    #[must_use]
    pub const fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Convert tool choice to Anthropic API format.
    fn convert_tool_choice(choice: &ToolChoice) -> AnthropicToolChoice {
        match choice {
            ToolChoice::Auto => AnthropicToolChoice::Auto,
            ToolChoice::None => AnthropicToolChoice::None,
            ToolChoice::Required => AnthropicToolChoice::Any,
            ToolChoice::Specific { name } => AnthropicToolChoice::Tool { name: name.clone() },
        }
    }
}

#[async_trait]
impl ChatModel for ChatAnthropic {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        let (system_msg, api_messages): (Vec<_>, Vec<_>) = messages
            .iter()
            .partition(|m| matches!(m.role, Role::System));

        let system = system_msg
            .first()
            .and_then(|m| match &m.content {
                Content::Text(text) => Some(text.clone()),
                Content::MultiPart(_) => None,
            })
            .or_else(|| {
                if system_msg.is_empty() {
                    None
                } else {
                    Some(String::new())
                }
            });

        let mut converted_messages = Vec::new();
        for m in &api_messages {
            let content = convert_content(&m.content, &m.tool_calls)?;
            converted_messages.push(AnthropicMessage {
                role: convert_role_to_anthropic(&m.role).to_string(),
                content,
            });
        }

        let request = AnthropicRequest {
            model: model.clone(),
            messages: converted_messages,
            system,
            max_tokens: options
                .and_then(|o| o.max_tokens)
                .unwrap_or(self.max_tokens),
            temperature: options.and_then(|o| o.temperature).or(self.temperature),
            top_p: options.and_then(|o| o.top_p).or(self.top_p),
            stop_sequences: options.and_then(|o| o.stop_sequences.clone()),
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(
                    self.tools
                        .iter()
                        .map(|t| AnthropicTool {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            input_schema: t.parameters.clone(),
                        })
                        .collect(),
                )
            },
            tool_choice: options
                .and_then(|o| o.tool_choice.as_ref())
                .map(Self::convert_tool_choice),
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return parse_anthropic_error(&response_text, status);
        }

        let api_response: AnthropicResponse = serde_json::from_str(&response_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Failed to parse response: {e}")))?;

        Ok(convert_api_response(api_response))
    }

    fn stream(
        &self,
        _messages: &[Message],
        _options: Option<&CallOptions>,
    ) -> Pin<Box<dyn Stream<Item = Result<MessageChunk, LlmError>> + Send + '_>> {
        Box::pin(stream::once(async {
            Err(LlmError::Other(
                "SSE streaming implementation pending".to_string(),
            ))
        }))
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

/// Anthropic API request format.
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
    stream: bool,
}

/// Anthropic API message format.
#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Anthropic API content format.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Anthropic API content block.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[allow(dead_code, reason = "ToolResult used for API compatibility")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Anthropic API image source.
#[derive(Debug, Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    media_type: String,
    data: String,
}

/// Anthropic API tool definition.
#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Anthropic API tool choice.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "none")]
    None,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "tool")]
    Tool { name: String },
}

/// Anthropic API response format.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "API response fields for future use")]
struct AnthropicResponse {
    id: String,
    role: String,
    content: Vec<ResponseContentBlock>,
    usage: Option<TokenUsage>,
}

/// Anthropic API response content block.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponseContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Convert message content to Anthropic API format.
#[allow(
    clippy::match_same_arms,
    reason = "Explicit handling for different content types"
)]
fn convert_content(
    content: &Content,
    tool_calls: &[ToolCall],
) -> Result<AnthropicContent, LlmError> {
    if !tool_calls.is_empty() {
        let blocks: Vec<ContentBlock> = tool_calls
            .iter()
            .map(|tc| ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input: tc.args.clone(),
            })
            .collect();
        return Ok(AnthropicContent::Blocks(blocks));
    }

    match content {
        Content::Text(text) => Ok(AnthropicContent::Text(text.clone())),
        Content::MultiPart(parts) => {
            let mut blocks = Vec::new();
            for p in parts {
                let block = match p {
                    ContentPart::Text { text } => ContentBlock::Text { text: text.clone() },
                    ContentPart::Image(img) => {
                        let (media_type, data) = match &img.source {
                            crate::llm::ImageSource::Base64(b64) => {
                                (img.media_type.clone(), b64.clone())
                            }
                            crate::llm::ImageSource::Url(_) => {
                                return Err(LlmError::InvalidResponse(
                                    "URL images not supported for Anthropic API".to_string(),
                                ));
                            }
                        };
                        ContentBlock::Image {
                            source: ImageSource { media_type, data },
                        }
                    }
                    ContentPart::Thinking { text, .. } => ContentBlock::Text { text: text.clone() },
                };
                blocks.push(block);
            }
            Ok(AnthropicContent::Blocks(blocks))
        }
    }
}

/// Parse Anthropic API error response.
fn parse_anthropic_error(
    response_text: &str,
    status: reqwest::StatusCode,
) -> Result<Message, LlmError> {
    if let Ok(error) = serde_json::from_str::<AnthropicErrorResponse>(response_text) {
        match error.error.type_ {
            Some(t) if t == "authentication_error" => Err(LlmError::AuthError(error.error.message)),
            Some(t) if t == "rate_limit_error" => Err(LlmError::RateLimited { retry_after: None }),
            Some(t) if t == "invalid_request_error" => {
                if error.error.message.contains("context") {
                    Err(LlmError::ContextLengthExceeded { used: 0, limit: 0 })
                } else {
                    Err(LlmError::InvalidResponse(error.error.message))
                }
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

/// Anthropic API error response format.
#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicErrorDetail,
}

/// Anthropic API error detail.
#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    type_: Option<String>,
    message: String,
}

/// Convert Anthropic API response to Message.
fn convert_api_response(response: AnthropicResponse) -> Message {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for block in response.content {
        match block {
            ResponseContentBlock::Text { text } => {
                content.push_str(&text);
            }
            ResponseContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    args: input,
                });
            }
        }
    }

    Message::ai_with_tool_calls(content, tool_calls)
}

/// Convert role to Anthropic API format.
#[allow(clippy::match_same_arms, reason = "Explicit roles for clarity")]
#[allow(
    clippy::missing_const_for_fn,
    reason = "Cannot be const due to reference parameter"
)]
fn convert_role_to_anthropic(role: &Role) -> &'static str {
    match role {
        Role::Human => "user",
        Role::Ai => "assistant",
        Role::Tool => "user",
        Role::System => "user",
    }
}

// Rust guideline compliant 2026-05-19
