//! Ollama provider implementation.
//!
//! Provides integration with Ollama's local model API.
//! Supports both streaming and non-streaming requests.

use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    BoxStream, CallOptions, ChatModel, Content, ContentPart, LlmError, Message, Role, TokenUsage,
    ToolDefinition,
};

use juncture_tracing::spans::attrs;

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

    /// Tools bound via `bind_tools` (sent to tool-capable Ollama models).
    tools: Vec<ToolDefinition>,

    /// Whether to stream responses by default.
    #[allow(dead_code, reason = "configured but not directly accessed")]
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
    /// Panics if the `reqwest` TLS backend fails to initialize (e.g., a broken
    /// system OpenSSL/rustls install). A functioning TLS stack is a hard
    /// environment requirement for any HTTP LLM client, so this surfaces at
    /// construction. `new()` is intentionally infallible by signature; a
    /// fallible `new() -> Result` would be a breaking API change and is
    /// tracked as a known, accepted limitation (audit #7).
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
            client: {
                #[cfg(not(target_family = "wasm"))]
                {
                    Client::builder()
                        .timeout(Duration::from_secs(300))
                        .build()
                        .expect("Failed to create HTTP client: reqwest TLS backend initialization failed (audit #7: accepted environment-level fault)")
                }
                #[cfg(target_family = "wasm")]
                {
                    Client::new()
                }
            },
            model: model.into(),
            base_url: OLLAMA_BASE_URL.to_string(),
            temperature: None,
            top_p: None,
            tools: Vec::new(),
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

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl ChatModel for ChatOllama {
    #[allow(
        clippy::too_many_lines,
        reason = "invoke handles request construction, tools binding, HTTP, response/tool_call parsing, usage + budget reporting, and span instrumentation"
    )]
    async fn invoke(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<Message, LlmError> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        // LLM response cache lookup (design 09-observability §4.4): consult the
        // cache policy scoped by the runner from `RunnableConfig::llm_cache_policy`
        // before the HTTP call; a miss proceeds and stores the fresh response below.
        let cache_key_input = crate::llm::cache_key_input(model, messages, &self.tools, options);
        if let Some(cached) = juncture_core::pregel::try_llm_cache_lookup(&cache_key_input) {
            return Ok(cached);
        }

        #[cfg(not(target_family = "wasm"))]
        let span = tracing::info_span!(
            "juncture.llm.call",
            "juncture.llm.model" = %model,
            "juncture.llm.provider" = "ollama",
            "juncture.tokens.input" = tracing::field::Empty,
            "juncture.tokens.output" = tracing::field::Empty,
            "juncture.llm.has_tool_calls" = false,
            "juncture.llm.stop_reason" = tracing::field::Empty,
        );
        #[cfg(not(target_family = "wasm"))]
        let _enter = span.enter();

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
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(self.tools.iter().map(OllamaTool::from).collect())
            },
        };

        #[cfg(not(target_family = "wasm"))]
        let start = std::time::Instant::now();

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

        // Parse any tool calls the model requested into the unified ToolCall
        // representation (design 08 §3.3 tool-capable models).
        let tool_calls: Vec<crate::llm::ToolCall> = api_response
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(crate::llm::ToolCall::from)
            .collect();
        let has_tool_calls = !tool_calls.is_empty();

        // Record span attributes
        tracing::Span::current().record(attrs::LLM_HAS_TOOL_CALLS, has_tool_calls);
        // Record "unknown" as Ollama API doesn't return stop_reason in responses
        tracing::Span::current().record(attrs::LLM_STOP_REASON, "unknown");

        // Ollama reports token counts on the final response: prompt_eval_count
        // (input, absent when prompt is cached) and eval_count (output).
        let usage = ollama_usage(api_response.prompt_eval_count, api_response.eval_count);
        if let Some(ref u) = usage {
            tracing::Span::current().record(attrs::TOKENS_INPUT, u.input_tokens);
            tracing::Span::current().record(attrs::TOKENS_OUTPUT, u.output_tokens);
        }

        // Emit metrics for LLM call
        tracing::debug!(
            name: "juncture.llm.calls",
            provider = "ollama",
            model = %model,
        );

        #[cfg(not(target_family = "wasm"))]
        tracing::debug!(
            name: "juncture.llm.duration_ms",
            duration_ms = start.elapsed().as_millis(),
            model = %model,
        );

        // Report token usage to the budget tracker (if configured) so Ollama-
        // backed agents participate in budget enforcement like OpenAI/Anthropic.
        if let Some(ref usage) = usage {
            let _ = juncture_core::pregel::try_report_model_call(
                usage.input_tokens,
                usage.output_tokens,
            );
            let _ = juncture_core::pregel::BUDGET_TRACKER
                .try_with(|tracker| tracker.report_output_tokens(usage.output_tokens));
        }

        // Report LLM call and duration metrics
        #[cfg(not(target_family = "wasm"))]
        {
            let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = juncture_core::pregel::try_report_llm_duration(duration_ms);
        }
        let _ = juncture_core::pregel::try_report_llm_call();

        let mut msg = Message::ai_with_tool_calls(api_response.message.content, tool_calls);
        msg.usage = usage;

        // Store the fresh response in the cache for subsequent identical requests
        // (design 09 §4.4); no-op when no cache policy is scoped.
        juncture_core::pregel::try_llm_cache_store(&cache_key_input, &msg);

        Ok(msg)
    }

    #[allow(
        clippy::redundant_clone,
        clippy::uninlined_format_args,
        clippy::too_many_lines,
        reason = "Complex SSE stream parsing logic with full Ollama protocol handling"
    )]
    async fn stream(
        &self,
        messages: &[Message],
        options: Option<&CallOptions>,
    ) -> Result<BoxStream<'_, Result<crate::llm::MessageChunk, LlmError>>, LlmError> {
        let model = options
            .and_then(|o| o.model_override.as_ref())
            .unwrap_or(&self.model);

        // Create span for stream setup
        #[cfg(not(target_family = "wasm"))]
        let span = tracing::info_span!(
            "juncture.llm.call",
            "juncture.llm.model" = %model,
            "juncture.llm.provider" = "ollama",
        );
        #[cfg(not(target_family = "wasm"))]
        let _enter = span.enter();

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
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(self.tools.iter().map(OllamaTool::from).collect())
            },
        };

        let base_url = self.base_url.clone();
        let client = self.client.clone();

        Ok(Box::pin(stream::unfold(
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
                    Err(e) => {
                        return Some((
                            Err(LlmError::NetworkError(e)),
                            (client, base_url, request, true, buffer),
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
                                (client, base_url, request, true, buffer),
                            ));
                        }
                    };

                    return Some((
                        Err(LlmError::InvalidResponse(format!(
                            "HTTP {}: {}",
                            status.as_u16(),
                            response_text
                        ))),
                        (client, base_url, request, true, buffer),
                    ));
                }

                let mut byte_stream = response.bytes_stream();

                while let Some(chunk_result) = byte_stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            return Some((
                                Err(LlmError::NetworkError(e)),
                                (client, base_url, request, true, buffer),
                            ));
                        }
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
                        if let Ok(ollama_response) =
                            serde_json::from_str::<OllamaStreamResponse>(line)
                        {
                            // The final (`done: true`) chunk carries the
                            // cumulative token counters; report them as a
                            // usage_delta so Ollama streaming participates in
                            // budget tracking like OpenAI/Anthropic streaming.
                            let usage_delta = if ollama_response.done {
                                ollama_usage(
                                    ollama_response.prompt_eval_count,
                                    ollama_response.eval_count,
                                )
                            } else {
                                None
                            };

                            // Ollama returns complete tool_calls on the final
                            // (done) chunk for tool-capable models. Map them to
                            // ToolCallChunk with the full arguments object as
                            // args_delta (Ollama does not stream partial args).
                            let tool_call_chunks: Vec<crate::llm::ToolCallChunk> = ollama_response
                                .message
                                .tool_calls
                                .unwrap_or_default()
                                .into_iter()
                                .enumerate()
                                .map(|(index, tc)| {
                                    let call = crate::llm::ToolCall::from(tc);
                                    crate::llm::ToolCallChunk {
                                        id: Some(call.id),
                                        name: Some(call.name),
                                        args_delta: call.arguments.to_string(),
                                        index,
                                    }
                                })
                                .collect();

                            let chunk = crate::llm::MessageChunk {
                                content: ollama_response.message.content,
                                tool_call_chunks,
                                usage_delta,
                            };

                            if ollama_response.done {
                                // Report the final usage to the budget tracker,
                                // then end the stream. Emit a trailing chunk
                                // only if it carries content or usage data.
                                if let Some(ref u) = chunk.usage_delta {
                                    let _ = juncture_core::pregel::try_report_model_call(
                                        u.input_tokens,
                                        u.output_tokens,
                                    );
                                    let _ =
                                        juncture_core::pregel::BUDGET_TRACKER.try_with(|tracker| {
                                            tracker.report_output_tokens(u.output_tokens);
                                        });
                                }
                                if !chunk.content.is_empty() || chunk.usage_delta.is_some() {
                                    return Some((
                                        Ok(chunk),
                                        (client, base_url, request, true, buffer),
                                    ));
                                }
                                // Stream is complete with no trailing payload
                                return None;
                            }

                            if !chunk.content.is_empty() {
                                return Some((
                                    Ok(chunk),
                                    (client, base_url, request, false, buffer),
                                ));
                            }
                        }
                    }
                }

                None
            },
        )))
    }

    fn bind_tools(&self, tools: Vec<ToolDefinition>) -> Self {
        // Ollama's /api/chat accepts an OpenAI-compatible `tools` array for
        // tool-capable models (llama3.1+, qwen2.5+, etc.), so bind_tools is a
        // real binding, not a no-op (design 08 §3.3: "工具支持取决于模型能力" --
        // we send the definitions and let the model decide; models without
        // function-calling simply ignore them).
        let mut new_model = self.clone();
        new_model.tools = tools;
        new_model
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
    /// OpenAI-compatible tool definitions for tool-capable Ollama models.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
}

/// Ollama API tool definition (OpenAI-compatible).
#[derive(Debug, Serialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaFunction,
}

/// Ollama API function definition.
#[derive(Debug, Serialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl From<&ToolDefinition> for OllamaTool {
    fn from(def: &ToolDefinition) -> Self {
        Self {
            r#type: "function".to_string(),
            function: OllamaFunction {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters: def.parameters.clone(),
            },
        }
    }
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
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OllamaResponse {
    message: OllamaResponseMessage,
    #[serde(default)]
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    done: bool,
    /// Number of prompt (input) tokens the model evaluated. Ollama omits this
    /// field (defaults to 0) when the prompt is served from cache.
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    /// Number of generated (output) tokens.
    #[serde(default)]
    eval_count: Option<u64>,
}

/// Ollama API response message.
#[derive(Debug, Deserialize)]
#[allow(dead_code, reason = "deserialization target, fields read indirectly")]
struct OllamaResponseMessage {
    #[allow(dead_code, reason = "deserialization target, fields read indirectly")]
    role: String,
    content: String,
    /// Tool calls requested by the model (present for tool-capable models).
    #[serde(default)]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

/// Ollama API tool call (model's request to invoke a tool).
#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

/// Ollama API tool-call function payload (name + arguments object).
#[derive(Debug, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    /// Ollama returns arguments as a JSON object (not a string).
    #[serde(default)]
    arguments: serde_json::Value,
}

impl From<OllamaToolCall> for crate::llm::ToolCall {
    fn from(tc: OllamaToolCall) -> Self {
        // Ollama does not return a tool-call id; synthesize one from the
        // function name so downstream ToolNode dispatch has a stable key.
        Self {
            id: format!("ollama-{}", tc.function.name),
            name: tc.function.name,
            arguments: tc.function.arguments,
        }
    }
}

/// Ollama API streaming response format.
#[derive(Debug, Deserialize)]
struct OllamaStreamResponse {
    message: OllamaResponseMessage,
    #[serde(default)]
    done: bool,
    /// Present on the final (`done: true`) chunk: prompt tokens evaluated.
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    /// Present on the final (`done: true`) chunk: generated tokens.
    #[serde(default)]
    eval_count: Option<u64>,
}

/// Build a [`crate::llm::TokenUsage`] from Ollama's optional token counters.
///
/// Ollama reports `prompt_eval_count` (input) and `eval_count` (output) on the
/// final response; `prompt_eval_count` is absent (treated as 0) when the
/// prompt is cached. Returns `None` only when both counters are absent, so a
/// cached-prompt response still records output tokens.
fn ollama_usage(prompt_eval_count: Option<u64>, eval_count: Option<u64>) -> Option<TokenUsage> {
    let input = prompt_eval_count.unwrap_or(0);
    let output = eval_count?;
    Some(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_usage_both_counts() {
        let usage = ollama_usage(Some(10), Some(5)).expect("usage with both counts");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_ollama_usage_cached_prompt_defaults_input_to_zero() {
        // prompt_eval_count absent (cached prompt) -> input 0, output recorded.
        let usage = ollama_usage(None, Some(7)).expect("usage with only output");
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 7);
    }

    #[test]
    fn test_ollama_usage_none_without_output() {
        assert!(ollama_usage(Some(10), None).is_none());
        assert!(ollama_usage(None, None).is_none());
    }

    #[test]
    fn test_ollama_tool_from_definition() {
        let def = ToolDefinition {
            name: "echo".to_string(),
            description: "Echo a message".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}}),
        };
        let tool = OllamaTool::from(&def);
        assert_eq!(tool.r#type, "function");
        assert_eq!(tool.function.name, "echo");
        assert_eq!(tool.function.description, "Echo a message");
        assert!(tool.function.parameters.is_object());
    }

    #[test]
    fn test_ollama_tool_call_to_tool_call_synthesizes_id() {
        let otc = OllamaToolCall {
            function: OllamaToolCallFunction {
                name: "echo".to_string(),
                arguments: serde_json::json!({"msg": "hi"}),
            },
        };
        let call = crate::llm::ToolCall::from(otc);
        assert_eq!(call.name, "echo");
        assert_eq!(call.id, "ollama-echo");
        assert_eq!(call.arguments, serde_json::json!({"msg": "hi"}));
    }

    #[test]
    fn test_ollama_response_parses_tool_calls() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {"function": {"name": "echo", "arguments": {"msg": "world"}}}
                ]
            },
            "done": true,
            "prompt_eval_count": 12,
            "eval_count": 3
        }"#;
        let resp: OllamaResponse = serde_json::from_str(json).expect("parse response");
        let calls = resp.message.tool_calls.expect("tool_calls present");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "echo");
        let usage = ollama_usage(resp.prompt_eval_count, resp.eval_count).expect("usage");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 3);
    }

    #[test]
    fn test_bind_tools_sets_tools() {
        let model = ChatOllama::new("llama3.2");
        assert!(model.tools.is_empty());
        let bound = model.bind_tools(vec![ToolDefinition {
            name: "echo".to_string(),
            description: "Echo".to_string(),
            parameters: serde_json::json!({}),
        }]);
        assert_eq!(bound.tools.len(), 1);
        assert_eq!(bound.tools[0].name, "echo");
    }
}

// Rust guideline compliant 2026-05-19
