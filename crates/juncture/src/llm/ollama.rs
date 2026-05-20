//! Ollama provider implementation.
//!
//! Provides integration with Ollama's local model API.
//! Supports both streaming and non-streaming requests.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    CallOptions, ChatModel, Content, ContentPart, LlmError, Message, Role,
    ToolDefinition,
};

/// Default Ollama API base URL.
const OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// Ollama client.
///
/// Provides access to Ollama's local model API.
///
/// # Example
///
/// ```rust,no_run
/// use juncture::llm::{ChatModel, ChatOllama};
/// use juncture::Message;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let model = ChatOllama::new("llama3.2");
///     let messages = vec![Message::human("Hello!")];
///
///     let response = model.invoke(&messages, None).await?;
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ChatOllama {
    /// HTTP client for API requests.
    client: Client,

    /// Model to use (e.g., "llama3.2").
    model: String,

    /// API base URL.
    base_url: String,

    /// Default temperature.
    temperature: Option<f32>,

    /// Default top-p sampling.
    top_p: Option<f32>,

    /// Whether to stream responses by default.
    #[allow(dead_code, reason = "Reserved for future streaming feature")]
    stream: bool,
}

impl ChatOllama {
    /// Create a new Ollama client.
    ///
    /// # Arguments
    ///
    /// * `model` - Model name (e.g., "llama3.2")
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOllama;
    ///
    /// let model = ChatOllama::new("llama3.2");
    /// ```
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("Failed to create HTTP client"),
            model: model.into(),
            base_url: OLLAMA_BASE_URL.to_string(),
            temperature: None,
            top_p: None,
            stream: false,
        }
    }

    /// Set a custom API base URL.
    ///
    /// # Arguments
    ///
    /// * `url` - Base URL for Ollama API
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOllama;
    ///
    /// let model = ChatOllama::new("llama3.2")
    ///     .with_base_url("http://localhost:11434");
    /// ```
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
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
    /// use juncture::llm::ChatOllama;
    ///
    /// let model = ChatOllama::new("llama3.2")
    ///     .with_temperature(0.7);
    /// ```
    #[must_use]
    pub const fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

#[async_trait]
impl ChatModel for ChatOllama {
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        let api_messages: Vec<_> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    Role::System => "system",
                    Role::Human => "user",
                    Role::Ai => "assistant",
                    Role::Tool => "tool",
                }
                .to_string(),
                content: extract_text_content(&m.content),
                images: extract_images(&m.content),
            })
            .collect();

        let request = OllamaRequest {
            model: model.clone(),
            messages: api_messages,
            stream: false,
            options: Some(OllamaOptions {
                temperature: options.and_then(|o| o.temperature).or(self.temperature),
                top_p: options.and_then(|o| o.top_p).or(self.top_p),
            }),
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(LlmError::InvalidResponse(format!(
                "HTTP {}: {}",
                status.as_u16(),
                response_text
            )));
        }

        let api_response: OllamaResponse = serde_json::from_str(&response_text)
            .map_err(|e| LlmError::InvalidResponse(format!("Failed to parse response: {e}")))?;

        Ok(Message::ai_with_tool_calls(
            api_response.message.content,
            Vec::new(),
        ))
    }

    #[allow(clippy::redundant_clone, clippy::uninlined_format_args, reason = "Complex SSE stream parsing logic")]
    fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Pin<Box<dyn Stream<Item = Result<crate::llm::MessageChunk, LlmError>> + Send + '_>> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        let api_messages: Vec<_> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    Role::System => "system",
                    Role::Human => "user",
                    Role::Ai => "assistant",
                    Role::Tool => "tool",
                }
                .to_string(),
                content: extract_text_content(&m.content),
                images: extract_images(&m.content),
            })
            .collect();

        let request = OllamaRequest {
            model: model.clone(),
            messages: api_messages,
            stream: true,
            options: Some(OllamaOptions {
                temperature: options.and_then(|o| o.temperature).or(self.temperature),
                top_p: options.and_then(|o| o.top_p).or(self.top_p),
            }),
        };

        let base_url = self.base_url.clone();
        let client = self.client.clone();

        Box::pin(stream::unfold(
            (client, base_url, request, false, Vec::new()),
            |(client, base_url, request, done, mut buffer)| async move {
                if done {
                    return None;
                }

                let response = match client
                    .post(format!("{}/api/chat", base_url))
                    .header("content-type", "application/json")
                    .json(&request)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => return Some((Err(LlmError::NetworkError(e)), (client, base_url, request, true, buffer))),
                };

                let status = response.status();

                if !status.is_success() {
                    let response_text = match response.text().await {
                        Ok(t) => t,
                        Err(e) => return Some((Err(LlmError::NetworkError(e)), (client, base_url, request, true, buffer))),
                    };

                    return Some((
                        Err(LlmError::InvalidResponse(format!("HTTP {}: {}", status.as_u16(), response_text))),
                        (client, base_url, request, true, buffer),
                    ));
                }

                let mut byte_stream = response.bytes_stream();

                while let Some(chunk_result) = byte_stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => return Some((Err(LlmError::NetworkError(e)), (client, base_url, request, true, buffer))),
                    };

                    buffer.extend_from_slice(&chunk);

                    while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_bytes = buffer.drain(..=newline_pos).collect::<Vec<_>>();
                        let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);

                        // Skip empty lines
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }

                        // Parse JSON line
                        if let Ok(ollama_response) = serde_json::from_str::<OllamaStreamResponse>(line) {
                            let chunk = crate::llm::MessageChunk {
                                content: ollama_response.message.content,
                                tool_call_chunks: Vec::new(),
                                usage_delta: None,
                            };

                            if ollama_response.done {
                                // Stream is complete
                                return None;
                            }

                            if !chunk.content.is_empty() {
                                return Some((Ok(chunk), (client, base_url, request, false, buffer)));
                            }
                        }
                    }
                }

                None
            },
        ))
    }

    fn bind_tools(&self, _tools: Vec<ToolDefinition>) -> Self {
        // Ollama doesn't support tools in the same way
        // Return self unchanged
        self.clone()
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Extract plain text content from Content.
#[allow(
    clippy::match_same_arms,
    reason = "Explicit handling for different content types"
)]
fn extract_text_content(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        Content::MultiPart(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Thinking { text, .. } => Some(text.as_str()),
                ContentPart::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Extract base64 images from Content.
fn extract_images(content: &Content) -> Vec<String> {
    match content {
        Content::Text(_) => Vec::new(),
        Content::MultiPart(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Image(img) => match &img.source {
                    crate::llm::ImageSource::Base64(b64) => Some(b64.clone()),
                    crate::llm::ImageSource::Url(_) => None,
                },
                _ => None,
            })
            .collect(),
    }
}

/// Ollama API request format.
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

/// Ollama API message format.
#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
}

/// Ollama generation options.
#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
}

/// Ollama API response format.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "API response fields for future use")]
struct OllamaResponse {
    message: OllamaResponseMessage,
    #[serde(default)]
    done: bool,
}

/// Ollama API response message.
#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    #[allow(dead_code, reason = "API response field for future use")]
    role: String,
    content: String,
}

/// Ollama API streaming response format.
#[derive(Debug, Deserialize)]
struct OllamaStreamResponse {
    message: OllamaResponseMessage,
    #[serde(default)]
    done: bool,
}

// Rust guideline compliant 2026-05-19
