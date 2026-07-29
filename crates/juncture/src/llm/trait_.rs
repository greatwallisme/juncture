//! Canonical LLM traits and value types (re-exported from `juncture-core`).
//!
//! This module is the single source of truth for the LLM surface: it
//! re-exports the `ChatModel` trait, `LlmError`, `CallOptions`,
//! `ToolDefinition`, `ToolChoice`, `ResponseFormat`, `StructuredOutputModel`,
//! and the streaming aliases directly from [`juncture_core::llm`].
//!
//! The former facade-local duplicates of these types (the pre-consolidation
//! `trait_.rs` definitions) were removed so there is exactly one definition
//! of each, matching design `08-llm-tools`. The provider *implementations*
//! (`ChatAnthropic`, `ChatOpenAI`, `ChatOllama`, `MockChatModel`,
//! `RetryingModel`, `MiddlewareModel`) still live in this crate's provider
//! modules and implement the re-exported [`ChatModel`] trait.

pub use juncture_core::llm::{
    BoxStream, CallOptions, ChatModel, DeserializeOwned, JsonSchema, LlmError, MessageChunk,
    ResponseFormat, StructuredOutputModel, ToolCallChunk, ToolChoice, ToolDefinition,
};

// Rust guideline compliant 2026-07-29
