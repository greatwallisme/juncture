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

// Re-export the message value types from juncture-core. The `ChatModel`
// trait, `LlmError`, `CallOptions`, `ToolDefinition`, `ToolChoice`,
// `ResponseFormat`, `StructuredOutputModel`, and the streaming aliases
// (`MessageChunk`, `ToolCallChunk`, `BoxStream`) are re-exported via
// `trait_` from the canonical `juncture_core::llm` definitions.
pub use juncture_core::state::messages::{
    Content, ContentPart, ImageData, ImageSource, Message, Role, ToolCall,
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

// `StructuredOutputModel` is re-exported via `trait_::*` from the canonical
// `juncture_core::llm::StructuredOutputModel` definition.

/// Build a [`juncture_core::observability::CacheKeyInput`] for LLM response
/// cache lookup/store (design `09-observability` §4.4).
///
/// Now that the facade re-exports the canonical `juncture_core` types, this is
/// a direct construction (the former field-by-field `to_core_cache_key_input`
/// bridge is no longer needed). `CacheKeyInput::hash` keys on `model`, the
/// full `messages` and `tools`, and the scalar `temperature`/`max_tokens`/
/// `top_p` options, so `tool_choice`/`response_format`/`tags` do not affect
/// the generated key.
#[must_use]
pub(crate) fn cache_key_input(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    config: Option<&CallOptions>,
) -> juncture_core::observability::CacheKeyInput {
    juncture_core::observability::CacheKeyInput {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools: tools.to_vec(),
        config: config.cloned(),
    }
}

// Rust guideline compliant 2026-07-29
