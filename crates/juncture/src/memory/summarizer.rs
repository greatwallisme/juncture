//! Text summarization using LLMs.
//!
//! This module provides the [`Summarizer`] trait for text summarization
//! and [`LlmSummarizer`] which uses any LLM implementing [`ChatModel`](crate::llm::ChatModel)
//! to generate summaries.

use crate::llm::{CallOptions, ChatModel, LlmError};
use async_trait::async_trait;
use juncture_core::state::messages::Message;
use thiserror::Error;

/// Error type for memory operations.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Summarization operation failed.
    #[error("summarization failed: {0}")]
    SummarizationFailed(String),

    /// Store operation failed.
    #[error("store error: {0}")]
    StoreError(String),

    /// Fact extraction operation failed.
    #[error("fact extraction failed: {0}")]
    ExtractionFailed(String),

    /// Content is too short to summarize meaningfully.
    #[error("conversation too short to summarize")]
    InsufficientContent,
}

/// Async trait for text summarization.
///
/// Implementors can use different strategies (LLM-based, extractive, etc.)
/// to generate summaries of input text.
#[async_trait]
pub trait Summarizer: Send + Sync + 'static {
    /// Summarize the provided text.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::SummarizationFailed`] if summarization fails,
    /// or [`MemoryError::InsufficientContent`] if the input is too short.
    async fn summarize(&self, text: &str) -> Result<String, MemoryError>;
}

/// LLM-based summarizer using any [`ChatModel`] implementation.
///
/// This summarizer sends the text to an LLM with a system prompt instructing
/// it to produce a concise summary preserving key facts.
///
/// # Type Parameters
///
/// * `M` - The chat model type (e.g., [`ChatOpenAI`](crate::llm::ChatOpenAI))
///
/// # Example
///
/// ```ignore
/// use juncture::memory::LlmSummarizer;
/// use juncture::llm::ChatOpenAI;
/// use std::sync::Arc;
///
/// let model = ChatOpenAI::new("sk-...");
/// let summarizer = LlmSummarizer::new(model, 500);
/// ```
#[derive(Clone, Debug)]
pub struct LlmSummarizer<M: ChatModel> {
    /// The underlying LLM for generating summaries.
    model: M,
    /// Maximum tokens for the summary output.
    max_output_tokens: u32,
}

impl<M: ChatModel> LlmSummarizer<M> {
    /// Create a new LLM-based summarizer.
    ///
    /// # Arguments
    ///
    /// * `model` - The chat model to use for summarization
    /// * `max_output_tokens` - Maximum tokens in the generated summary
    #[must_use]
    pub const fn new(model: M, max_output_tokens: u32) -> Self {
        Self {
            model,
            max_output_tokens,
        }
    }
}

#[async_trait]
impl<M: ChatModel> Summarizer for LlmSummarizer<M> {
    async fn summarize(&self, text: &str) -> Result<String, MemoryError> {
        // Validate input has meaningful content
        let trimmed = text.trim();
        if trimmed.len() < 50 {
            return Err(MemoryError::InsufficientContent);
        }

        // Create system message for summarization
        let system_msg = Message::system(
            "Summarize the following text concisely, preserving key facts and information. Output only the summary.",
        );

        // Create user message with the text to summarize
        let user_msg = Message::human(trimmed);

        // Configure call options
        let options = CallOptions {
            max_tokens: Some(self.max_output_tokens),
            ..Default::default()
        };

        // Invoke the model
        let response = self
            .model
            .invoke(&[system_msg, user_msg], Some(&options))
            .await
            .map_err(|e| match e {
                LlmError::InvalidResponse(msg) => {
                    MemoryError::SummarizationFailed(format!("invalid response: {msg}"))
                }
                #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
                LlmError::NetworkError(e) => {
                    MemoryError::SummarizationFailed(format!("network error: {e}"))
                }
                _ => MemoryError::SummarizationFailed(format!("LLM error: {e}")),
            })?;

        // Extract the summary text from the response
        let summary = response.content_text();

        if summary.trim().is_empty() {
            return Err(MemoryError::SummarizationFailed(
                "LLM returned empty summary".to_string(),
            ));
        }

        Ok(summary.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_error_display() {
        let err = MemoryError::SummarizationFailed("test error".to_string());
        assert_eq!(err.to_string(), "summarization failed: test error");

        let err = MemoryError::StoreError("db error".to_string());
        assert_eq!(err.to_string(), "store error: db error");

        let err = MemoryError::ExtractionFailed("parse error".to_string());
        assert_eq!(err.to_string(), "fact extraction failed: parse error");

        let err = MemoryError::InsufficientContent;
        assert_eq!(err.to_string(), "conversation too short to summarize");
    }

    // Test LlmSummarizer construction with MockChatModel
    #[test]
    fn test_llm_summarizer_construction() {
        let model = crate::llm::MockChatModel::new("test-model");
        let summarizer = LlmSummarizer::new(model, 500);

        // Verify the summarizer was created with correct max_tokens
        assert_eq!(summarizer.max_output_tokens, 500);
    }
}

// Rust guideline compliant 2026-05-26
