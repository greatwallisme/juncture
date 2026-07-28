//! LLM integration module.
//!
//! This module provides a unified interface for working with various LLM providers
//! including Anthropic, `OpenAI`, and `Ollama`. It implements the `ChatModel` trait
//! which abstracts over different provider APIs while providing a consistent interface.
//!
//! # Features
//!
//! - **Unified API**: Single `ChatModel` trait for all providers
//! - **Streaming support**: Real-time token streaming via `MessageChunk`
//! - **Tool calling**: Native support for function/tool calling
//! - **Error handling**: Comprehensive error types with retry logic
//! - **Provider-specific**: Optimized implementations for each provider
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::{ChatModel, ChatAnthropic};
//! use juncture::Message;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let model = ChatAnthropic::from_env()?;
//!     let messages = vec![
//!         Message::human("Hello, how are you?"),
//!     ];
//!
//!     let response = model.invoke(&messages, None).await?;
//!     // Process response
//!     Ok(())
//! }
//! ```
//!
//! # Module Structure
//!
//! - `trait_`: Core `ChatModel` trait and error types
//! - `message`: Message type definitions and re-exports
//! - `anthropic`: Anthropic Claude provider implementation
//! - `openai`: `OpenAI` GPT provider implementation
//! - `ollama`: Ollama local model provider implementation
//! - `mock`: Mock implementation for testing
//! - `pricing`: Model pricing information
//! - `retry`: Retry wrapper for resilient LLM calls
//! - `structured`: Structured output extraction

// Re-export core types from juncture-core
pub use juncture_core::{
    state::messages::{Content, ContentPart, ImageData, ImageSource, Message, Role, ToolCall},
    stream::{MessageChunk, ToolCallChunk},
};

mod circuit_breaker;
mod message;
mod middleware;
mod mock;
mod pricing;
mod retry;
mod trait_;

// Provider implementations (feature-gated)
#[cfg(feature = "anthropic")]
mod anthropic;

#[cfg(feature = "openai")]
mod openai;

#[cfg(feature = "ollama")]
mod ollama;

#[cfg(feature = "structured-output")]
mod structured;

// Public exports
pub use circuit_breaker::*;
pub use message::*;
pub use middleware::*;
pub use mock::MockChatModel;
pub use pricing::{ModelPricing, PricingTable};
pub use retry::RetryingModel;
pub use trait_::*;

// Provider exports
#[cfg(feature = "anthropic")]
pub use anthropic::ChatAnthropic;

#[cfg(feature = "openai")]
pub use openai::ChatOpenAI;

#[cfg(feature = "ollama")]
pub use ollama::ChatOllama;

#[cfg(feature = "structured-output")]
pub use structured::StructuredOutputModel;

/// Build a core [`juncture_core::observability::CacheKeyInput`] from a
/// provider's facade-typed inputs for LLM cache lookup/store (design
/// `09-observability` §4.4).
///
/// The facade's `ToolDefinition`/`CallOptions` are structurally identical to
/// `juncture_core`'s (see the chat.rs duplicate note in `findings.md`), so the
/// conversion is a faithful field-by-field copy. `Message` is already the core
/// type. The core `CacheKeyInput::hash` keys on `model`, the full `messages`
/// and `tools`, and the scalar `temperature`/`max_tokens`/`top_p` options, so
/// `tool_choice`/`response_format`/`tags` are left as their `None`/empty
/// defaults (they do not affect the generated key; a custom `key_func` can
/// capture them if finer-grained keys are needed).
#[must_use]
pub(crate) fn to_core_cache_key_input(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: Option<&CallOptions>,
) -> juncture_core::observability::CacheKeyInput {
    let core_tools = tools
        .iter()
        .map(|t| juncture_core::ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect();
    let core_config = config.map(|c| juncture_core::CallOptions {
        temperature: c.temperature,
        max_tokens: c.max_tokens,
        stop_sequences: c.stop_sequences.clone(),
        top_p: c.top_p,
        model_override: c.model_override.clone(),
        tool_choice: None,
        response_format: None,
        tags: Vec::new(),
    });
    juncture_core::observability::CacheKeyInput {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools: core_tools,
        config: core_config,
    }
}

// Rust guideline compliant 2026-05-19
