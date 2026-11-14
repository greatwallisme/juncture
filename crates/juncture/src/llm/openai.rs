//! `OpenAI` GPT provider implementation.
//!
//! Provides integration with `OpenAI`'s GPT API via the Chat Completions API.
//! Supports both streaming and non-streaming requests, function calling, and
//! multimodal inputs.

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    BoxStream, CallOptions, ChatModel, Content, ContentPart, LlmError, Message, PromptCacheMode,
    PromptCacheTtl, Role, TokenUsage, ToolCall, ToolChoice, ToolDefinition,
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
    /// # Errors
    ///
    /// Returns [`LlmError::Other`] if the `reqwest` TLS backend fails to
    /// initialize (e.g., a broken system OpenSSL/rustls install). A functioning
    /// TLS stack is a hard environment requirement for any HTTP LLM client, so
    /// this surfaces at construction rather than panicking.
    ///
    /// # Example
    ///
    /// ```rust
    /// use juncture::llm::ChatOpenAI;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = ChatOpenAI::new("sk-...")?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(api_key: impl Into<String>) -> Result<Self, LlmError> {
        let client = {
            #[cfg(not(target_family = "wasm"))]
            {
                Client::builder()
                    .timeout(Duration::from_secs(120))
                    .build()
                    .map_err(|e| LlmError::Other(Box::new(e)))?
            }
            #[cfg(target_family = "wasm")]
            {
                Client::new()
            }
        };
        Ok(Self {
            client,
            api_key: api_key.into(),
            model: "gpt-4o".to_string(),
            base_url: OPENAI_BASE_URL.to_string(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            tools: Vec::new(),
        })
    }

    /// Create a new `OpenAI` client from environment variables.
    ///
    /// Reads `OPENAI_API_KEY` (required), and optionally `OPENAI_BASE_URL`
    /// (for OpenAI-compatible endpoints such as `vLLM`, `Together AI`, `Groq`,
    /// or `Azure OpenAI`; defaults to `https://api.openai.com/v1`) and `OPENAI_MODEL`
    /// (defaults to `gpt-4o`). This matches the variables documented in
    /// `.env.example` and read by `examples/src/common.rs`, so a configured
    /// `.env` yields a working compatible client without manual chaining.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::AuthError`] if `OPENAI_API_KEY` is not set.
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
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| OPENAI_BASE_URL.to_string());
        Ok(Self::new(api_key)?
            .with_model(model)
            .with_base_url(base_url))
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
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = ChatOpenAI::new("sk-...")?
    ///     .with_base_url("https://api.openai.com/v1");
    /// # Ok(())
    /// # }
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
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = ChatOpenAI::new("sk-...")?
    ///     .with_model("gpt-4-turbo");
    /// # Ok(())
    /// # }
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
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = ChatOpenAI::new("sk-...")?
    ///     .with_max_tokens(4096);
    /// # Ok(())
    /// # }
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
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let model = ChatOpenAI::new("sk-...")?
    ///     .with_temperature(0.7);
    /// # Ok(())
    /// # }
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

/// Boxed byte stream from an `OpenAI` SSE response.
/// On WASM, reqwest types are `!Send`, so we omit the `Send` bound.
#[cfg(not(target_family = "wasm"))]
type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
#[cfg(target_family = "wasm")]
type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>>>>;

/// State machine for the streaming unfold.
///
/// - `Pending`: initial state, need to send the HTTP request
/// - `Active`: byte stream is open, reading SSE chunks from the same connection
/// - `Done`: terminal state, no more data
enum StreamState {
    Pending {
        client: Client,
        api_key: String,
        base_url: String,
        /// Boxed so the largest enum variant stays close to the others in
        /// size; the request body is sent exactly once and never inspected.
        request: Box<OpenAIRequest>,
    },
    Active(ByteStream),
    Done,
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
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

        // LLM response cache lookup (design 09-observability §4.4): consult the
        // cache policy scoped by the runner from `RunnableConfig::llm_cache_policy`
        // before the HTTP call. A hit returns the cached response with no
        // network round-trip; a miss proceeds and stores the fresh response below.
        let cache_key_input = crate::llm::cache_key_input(model, messages, &self.tools, options);
        if let Some(cached) = juncture_core::pregel::try_llm_cache_lookup(&cache_key_input) {
            return Ok(cached);
        }

        #[cfg(not(target_family = "wasm"))]
        let span = tracing::info_span!(
            "juncture.llm.call",
            "juncture.llm.model" = %model,
            "juncture.llm.provider" = "openai",
            "juncture.tokens.input" = tracing::field::Empty,
            "juncture.tokens.output" = tracing::field::Empty,
            "juncture.llm.has_tool_calls" = false,
            "juncture.llm.stop_reason" = tracing::field::Empty,
        );
        #[cfg(not(target_family = "wasm"))]
        let _enter = span.enter();

        let breakpoints = breakpoints_enabled(options);
        let api_messages: Vec<_> = messages
            .iter()
            .map(|m| convert_message(m, breakpoints))
            .collect();
        let cache_options = resolve_cache_options(options);

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
            prompt_cache_key: options.and_then(|o| o.prompt_cache_key.clone()),
            prompt_cache_options: cache_options,
            stream: false,
        };

        #[cfg(not(target_family = "wasm"))]
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
            tracing::Span::current().record(attrs::TOKENS_INPUT, usage.prompt_tokens);
            tracing::Span::current().record(attrs::TOKENS_OUTPUT, usage.completion_tokens);
            // Cache counters live in `prompt_tokens_details`; the converted
            // `TokenUsage` normalizes the hit-rate denominator per provider.
            let normalized = TokenUsage::from(usage.clone());
            tracing::Span::current().record(attrs::CACHE_READ_TOKENS, normalized.cached_tokens);
            tracing::Span::current()
                .record(attrs::CACHE_WRITE_TOKENS, normalized.cache_write_tokens);
            tracing::Span::current().record(attrs::CACHE_HIT_RATE, normalized.cache_hit_rate());
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
                tokens = usage.prompt_tokens,
                model = %model,
            );
            tracing::debug!(
                name: "juncture.llm.tokens.output",
                tokens = usage.completion_tokens,
                model = %model,
            );
        }

        #[cfg(not(target_family = "wasm"))]
        tracing::debug!(
            name: "juncture.llm.duration_ms",
            duration_ms = start.elapsed().as_millis(),
            model = %model,
        );

        let message = convert_api_response(&api_response)?;

        // Store the fresh response in the cache for subsequent identical requests
        // (design 09 §4.4); no-op when no cache policy is scoped.
        juncture_core::pregel::try_llm_cache_store(&cache_key_input, &message);

        // Report token usage to the budget tracker (if configured)
        if let Some(ref usage) = message.usage {
            let _ = juncture_core::pregel::try_report_model_call(
                usage.input_tokens,
                usage.output_tokens,
            );

            // Report output tokens separately for metrics
            let _ = juncture_core::pregel::BUDGET_TRACKER.try_with(|tracker| {
                tracker.report_output_tokens(usage.output_tokens);
            });
        }

        // Report LLM call and duration metrics
        #[cfg(not(target_family = "wasm"))]
        {
            let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = juncture_core::pregel::try_report_llm_duration(duration_ms);
        }
        let _ = juncture_core::pregel::try_report_llm_call();

        Ok(message)
    }

    #[allow(
        clippy::too_many_lines,
        clippy::redundant_clone,
        clippy::uninlined_format_args,
        reason = "Complex SSE stream parsing logic"
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
            "juncture.llm.provider" = "openai",
        );
        #[cfg(not(target_family = "wasm"))]
        let _enter = span.enter();

        let breakpoints = breakpoints_enabled(options);
        let api_messages: Vec<_> = messages
            .iter()
            .map(|m| convert_message(m, breakpoints))
            .collect();
        let cache_options = resolve_cache_options(options);

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
            prompt_cache_key: options.and_then(|o| o.prompt_cache_key.clone()),
            prompt_cache_options: cache_options,
            stream: true,
        };

        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let client = self.client.clone();

        // Send ONE HTTP request, then keep the byte_stream in unfold state so all
        // SSE chunks are read from the same connection.
        let init: StreamState = StreamState::Pending {
            client,
            api_key,
            base_url,
            request: Box::new(request),
        };

        let stream = stream::unfold((init, Vec::new()), |(state, mut buffer)| async move {
            let mut byte_stream = match state {
                StreamState::Pending {
                    client,
                    api_key,
                    base_url,
                    request,
                } => {
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
                                (StreamState::Done, buffer),
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
                                    (StreamState::Done, buffer),
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
                                return Some((Err(e), (StreamState::Done, buffer)));
                            }
                        };

                        return Some((Ok(error), (StreamState::Done, buffer)));
                    }

                    Box::pin(response.bytes_stream())
                }
                StreamState::Active(s) => s,
                StreamState::Done => return None,
            };

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        return Some((Err(LlmError::NetworkError(e)), (StreamState::Done, buffer)));
                    }
                };

                buffer.extend_from_slice(&chunk);

                while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                    let line_bytes = buffer.drain(..=newline_pos).collect::<Vec<_>>();
                    let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);

                    let line = line.trim();
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data_str) = line.strip_prefix("data: ") {
                        if data_str == "[DONE]" {
                            return None;
                        }

                        if let Ok(sse_chunk) = serde_json::from_str::<OpenAISSEChunk>(data_str) {
                            match convert_openai_sse_chunk(sse_chunk) {
                                Ok(chunk) => {
                                    if !chunk.content.is_empty()
                                        || !chunk.tool_call_chunks.is_empty()
                                        || chunk.usage_delta.is_some()
                                    {
                                        return Some((
                                            Ok(chunk),
                                            (StreamState::Active(byte_stream), buffer),
                                        ));
                                    }
                                }
                                Err(e) => {
                                    return Some((Err(e), (StreamState::Done, buffer)));
                                }
                            }
                        }
                    }
                }
            }

            None
        });

        // On WASM, wrap the !Send stream with force_send_stream for ChatModel trait compatibility.
        #[cfg(target_family = "wasm")]
        {
            Ok(Box::pin(juncture_core::wasm_send::force_send_stream(
                stream,
            )))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            Ok(Box::pin(stream))
        }
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

/// Resolve the `prompt_cache_options` object for a call.
///
/// `Disabled` intentionally emits `mode = "explicit"` with no breakpoints
/// anywhere in the request: that combination produces no cache writes and
/// therefore no cache-write billing on GPT-5.6+ (design `12` §2.6).
fn resolve_cache_options(options: Option<&CallOptions>) -> Option<OpenAIPromptCacheOptions> {
    let mode = options
        .and_then(|o| o.prompt_cache_mode)
        .unwrap_or(PromptCacheMode::Implicit);

    let ttl = options
        .and_then(|o| o.prompt_cache_ttl)
        .map(PromptCacheTtl::as_openai_str);

    match mode {
        // Implicit caching is automatic; sending the object adds nothing.
        PromptCacheMode::Implicit => None,
        PromptCacheMode::Explicit | PromptCacheMode::Disabled => Some(OpenAIPromptCacheOptions {
            mode: "explicit",
            ttl,
        }),
    }
}

/// Whether explicit content-level breakpoints should be serialized.
fn breakpoints_enabled(options: Option<&CallOptions>) -> bool {
    !matches!(
        options.and_then(|o| o.prompt_cache_mode),
        Some(PromptCacheMode::Disabled)
    )
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
    /// Stable routing key for prompt-cache prefix locality.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    /// Explicit cache mode/TTL; omitted under implicit caching.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_options: Option<OpenAIPromptCacheOptions>,
    stream: bool,
}

/// `OpenAI` `prompt_cache_options` object.
#[derive(Debug, Serialize)]
struct OpenAIPromptCacheOptions {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<&'static str>,
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
///
/// `prompt_cache_breakpoint` is a content-level marker (`{"mode":"explicit"}`),
/// which is why the breakpoint abstraction lives on `ContentPart` rather than in
/// call options (design `12` §2.5, §2.6).
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_breakpoint: Option<OpenAICacheBreakpoint>,
    },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

/// `OpenAI` explicit cache breakpoint marker.
#[derive(Debug, Serialize)]
struct OpenAICacheBreakpoint {
    mode: &'static str,
}

impl OpenAICacheBreakpoint {
    /// Marker for an explicit breakpoint.
    const EXPLICIT: Self = Self { mode: "explicit" };
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
    usage: Option<OpenAIUsage>,
}

/// `OpenAI` API usage object.
///
/// `OpenAI` uses `prompt_tokens` / `completion_tokens` (not `input_tokens` / `output_tokens`).
#[derive(Debug, Clone, Deserialize)]
struct OpenAIUsage {
    #[allow(dead_code, reason = "deserialization target, field read indirectly")]
    prompt_tokens: u64,
    #[allow(dead_code, reason = "deserialization target, field read indirectly")]
    completion_tokens: u64,
    total_tokens: u64,
    /// Cache counters. Present only on cached requests; GPT-5.6+ adds
    /// `cache_write_tokens` alongside `cached_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_tokens_details: Option<OpenAIPromptTokensDetails>,
}

/// `OpenAI` nested `prompt_tokens_details` object.
#[derive(Debug, Clone, Default, Deserialize)]
struct OpenAIPromptTokensDetails {
    /// Prompt tokens served from cache (already included in `prompt_tokens`).
    #[serde(default)]
    cached_tokens: u64,
    /// Prompt tokens written to cache this request (GPT-5.6+).
    #[serde(default)]
    cache_write_tokens: u64,
}

impl From<OpenAIUsage> for TokenUsage {
    fn from(val: OpenAIUsage) -> Self {
        let details = val.prompt_tokens_details.unwrap_or_default();
        Self {
            input_tokens: val.prompt_tokens,
            output_tokens: val.completion_tokens,
            total_tokens: val.total_tokens,
            cached_tokens: details.cached_tokens,
            cache_write_tokens: details.cache_write_tokens,
            ..Self::default()
        }
    }
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
fn convert_message(message: &Message, breakpoints_enabled: bool) -> OpenAIMessage {
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
                    ContentPart::Text {
                        text,
                        cache_control,
                    } => {
                        content_parts.push(OpenAIContentPart::Text {
                            text: text.clone(),
                            prompt_cache_breakpoint: cache_control
                                .filter(|_| breakpoints_enabled)
                                .map(|_| OpenAICacheBreakpoint::EXPLICIT),
                        });
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
                        content_parts.push(OpenAIContentPart::Text {
                            text: text.clone(),
                            prompt_cache_breakpoint: None,
                        });
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
    msg.usage = response.usage.clone().map(TokenUsage::from);
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
    usage: Option<OpenAIUsage>,
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
        usage_delta: chunk.usage.map(TokenUsage::from),
    })
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "tests assert on JSON shapes they constructed themselves"
)]
mod cache_wire_tests {
    use super::*;
    use serde_json::json;

    fn call_options(mode: PromptCacheMode) -> CallOptions {
        CallOptions {
            prompt_cache_mode: Some(mode),
            prompt_cache_key: Some("tenant-a".to_string()),
            prompt_cache_ttl: Some(PromptCacheTtl::Min30),
            ..CallOptions::default()
        }
    }

    #[test]
    fn test_breakpoint_serialized_when_explicit() {
        let message = Message {
            id: "m1".to_string(),
            role: Role::Human,
            content: Content::MultiPart(vec![ContentPart::text_with_cache_control(
                "history",
                juncture_core::prefix::CacheControl::ephemeral(),
            )]),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            usage: None,
        };

        let converted = convert_message(&message, true);
        let serialized = serde_json::to_value(&converted).expect("serialize");
        assert_eq!(
            serialized["content"][0]["prompt_cache_breakpoint"]["mode"],
            "explicit"
        );
    }

    #[test]
    fn test_breakpoint_omitted_when_disabled() {
        let message = Message {
            id: "m1".to_string(),
            role: Role::Human,
            content: Content::MultiPart(vec![ContentPart::text_with_cache_control(
                "history",
                juncture_core::prefix::CacheControl::ephemeral(),
            )]),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            usage: None,
        };

        let converted = convert_message(&message, false);
        let serialized = serde_json::to_value(&converted).expect("serialize");
        assert!(
            serialized["content"][0]
                .get("prompt_cache_breakpoint")
                .is_none(),
            "Disabled mode must emit no breakpoint (no cache-write billing)"
        );
    }

    #[test]
    fn test_no_breakpoint_when_none_attached() {
        let message = Message {
            id: "m1".to_string(),
            role: Role::Human,
            content: Content::MultiPart(vec![ContentPart::text("plain")]),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            usage: None,
        };

        let converted = convert_message(&message, true);
        let serialized = serde_json::to_value(&converted).expect("serialize");
        assert!(
            serialized["content"][0]
                .get("prompt_cache_breakpoint")
                .is_none()
        );
    }

    #[test]
    fn test_implicit_mode_sends_no_cache_options() {
        let options = CallOptions::default();
        assert!(resolve_cache_options(Some(&options)).is_none());
    }

    #[test]
    fn test_explicit_mode_sends_mode_and_ttl() {
        let options = call_options(PromptCacheMode::Explicit);
        let resolved = resolve_cache_options(Some(&options)).expect("options present");
        let serialized = serde_json::to_value(&resolved).expect("serialize");
        assert_eq!(serialized["mode"], "explicit");
        assert_eq!(serialized["ttl"], "30m");
    }

    #[test]
    fn test_disabled_mode_sends_explicit_without_breakpoints() {
        let options = call_options(PromptCacheMode::Disabled);
        let resolved = resolve_cache_options(Some(&options)).expect("options present");
        let serialized = serde_json::to_value(&resolved).expect("serialize");
        assert_eq!(serialized["mode"], "explicit");
        assert!(!breakpoints_enabled(Some(&options)));
    }

    #[test]
    fn test_prompt_cache_ttl_wire_spelling() {
        assert_eq!(PromptCacheTtl::Min30.as_openai_str(), "30m");
        assert_eq!(PromptCacheTtl::Hour12.as_openai_str(), "12h");
    }

    #[test]
    fn test_prompt_cache_key_is_routed_through_request() {
        // The key must be present in the request body when configured; it is
        // what keeps a warm prefix pinned to one machine.
        let options = call_options(PromptCacheMode::Explicit);
        let request = OpenAIRequest {
            model: "gpt-5.6".to_string(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            prompt_cache_key: options.prompt_cache_key.clone(),
            prompt_cache_options: resolve_cache_options(Some(&options)),
            stream: false,
        };
        let serialized = serde_json::to_value(&request).expect("serialize");
        assert_eq!(serialized["prompt_cache_key"], "tenant-a");
        assert_eq!(serialized["prompt_cache_options"]["mode"], "explicit");
    }

    #[test]
    fn test_usage_cache_counters_deserialize() {
        let json = json!({
            "prompt_tokens": 1000,
            "completion_tokens": 20,
            "total_tokens": 1020,
            "prompt_tokens_details": {"cached_tokens": 800, "cache_write_tokens": 0}
        });
        let usage: OpenAIUsage = serde_json::from_value(json).expect("parse usage");
        let mapped = TokenUsage::from(usage);
        assert_eq!(mapped.cached_tokens, 800);
        assert_eq!(mapped.cache_write_tokens, 0);
        assert!((mapped.cache_hit_rate() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_usage_without_details_still_parses() {
        let json = json!({
            "prompt_tokens": 100,
            "completion_tokens": 10,
            "total_tokens": 110
        });
        let usage: OpenAIUsage = serde_json::from_value(json).expect("parse usage");
        let mapped = TokenUsage::from(usage);
        assert_eq!(mapped.cached_tokens, 0);
        assert!((mapped.cache_hit_rate() - 0.0).abs() < f64::EPSILON);
    }
}

// Rust guideline compliant 2026-05-19
