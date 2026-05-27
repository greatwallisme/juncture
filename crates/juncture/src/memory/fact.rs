//! Fact extraction from unstructured text.
//!
//! This module provides types for extracting structured facts from text,
//! including the [`Fact`] struct representing a single fact and the
//! [`FactExtractor`] trait for extraction implementations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use juncture_core::state::messages::Message;
use serde::{Deserialize, Serialize};

use crate::llm::{CallOptions, ChatModel, LlmError};
use crate::memory::MemoryError;

/// A structured fact extracted from text.
///
/// Represents a single piece of information with metadata about its
/// topic, claim, source, confidence, and timestamp.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    /// The topic or subject area of this fact.
    pub topic: String,

    /// The actual claim or statement extracted from the source text.
    pub claim: String,

    /// The original text or document this fact was extracted from.
    pub source: String,

    /// Confidence score for this fact (0.0 to 1.0).
    ///
    /// Higher values indicate greater confidence in the accuracy
    /// of the extracted fact.
    pub confidence: f64,

    /// When this fact was extracted.
    pub timestamp: DateTime<Utc>,
}

impl Fact {
    /// Create a new fact with the current timestamp.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic or subject area
    /// * `claim` - The actual claim or statement
    /// * `source` - The source text or document identifier
    /// * `confidence` - Confidence score (0.0 to 1.0)
    ///
    /// # Panics
    ///
    /// Panics if `confidence` is not in the range [0.0, 1.0].
    #[must_use]
    pub fn new(topic: String, claim: String, source: String, confidence: f64) -> Self {
        assert!(
            (0.0..=1.0).contains(&confidence),
            "confidence must be between 0.0 and 1.0, got {confidence}"
        );

        Self {
            topic,
            claim,
            source,
            confidence,
            timestamp: Utc::now(),
        }
    }
}

/// Async trait for extracting structured facts from text.
///
/// Implementors can use different strategies (LLM-based, NLP, pattern matching)
/// to extract structured facts from unstructured input.
#[async_trait]
pub trait FactExtractor: Send + Sync + 'static {
    /// Extract facts from the provided text.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ExtractionFailed`] if fact extraction fails.
    async fn extract(&self, text: &str) -> Result<Vec<Fact>, MemoryError>;
}

/// LLM-based fact extractor using any [`ChatModel`] implementation.
///
/// This extractor sends text to an LLM with a system prompt instructing it
/// to extract key facts as structured JSON. The LLM response is parsed
/// into [`Fact`] objects with current timestamps.
///
/// # Type Parameters
///
/// * `M` - The chat model type (e.g., [`ChatOpenAI`](crate::llm::ChatOpenAI))
///
/// # Example
///
/// ```ignore
/// use juncture::memory::LlmFactExtractor;
/// use juncture::llm::ChatOpenAI;
///
/// let model = ChatOpenAI::new("sk-...");
/// let extractor = LlmFactExtractor::new(model);
/// ```
#[derive(Clone, Debug)]
pub struct LlmFactExtractor<M: ChatModel> {
    /// The underlying LLM for fact extraction.
    model: M,
}

/// Helper struct for deserializing LLM fact extraction responses.
#[derive(Deserialize)]
struct RawFact {
    topic: String,
    claim: String,
    #[serde(default)]
    source: String,
    confidence: f64,
}

impl<M: ChatModel> LlmFactExtractor<M> {
    /// Create a new LLM-based fact extractor.
    ///
    /// # Arguments
    ///
    /// * `model` - The chat model to use for extraction
    #[must_use]
    pub const fn new(model: M) -> Self {
        Self { model }
    }

    /// Clean JSON response from LLM by removing markdown code blocks.
    ///
    /// LLMs often wrap JSON responses in markdown code blocks like:
    /// ```json
    /// [...]
    /// ```
    /// This function strips those wrappers to return clean JSON.
    fn clean_json_response(raw: &str) -> String {
        let mut s = raw.trim().to_string();

        // Remove ```json prefix
        if let Some(stripped) = s.strip_prefix("```json") {
            s = stripped.to_string();
        } else if let Some(stripped) = s.strip_prefix("```") {
            s = stripped.to_string();
        }

        // Remove ``` suffix
        if let Some(stripped) = s.strip_suffix("```") {
            s = stripped.to_string();
        }

        s.trim().to_string()
    }
}

#[async_trait]
impl<M: ChatModel> FactExtractor for LlmFactExtractor<M> {
    async fn extract(&self, text: &str) -> Result<Vec<Fact>, MemoryError> {
        // Validate input
        let trimmed = text.trim();
        if trimmed.len() < 50 {
            return Ok(Vec::new());
        }

        // Create system message for fact extraction
        let system_prompt = Message::system(
            "Extract key facts from the following text. Return a JSON array of objects \
             with fields: topic, claim, source, confidence (0.0-1.0). Output ONLY the \
             JSON array, no other text.",
        );

        // Create user message with the text to analyze
        let user_msg = Message::human(trimmed);

        // Configure call options
        let options = CallOptions {
            max_tokens: Some(2000),
            ..Default::default()
        };

        // Invoke the model
        let response = self
            .model
            .invoke(&[system_prompt, user_msg], Some(&options))
            .await
            .map_err(|e| match e {
                LlmError::InvalidResponse(msg) => {
                    MemoryError::ExtractionFailed(format!("invalid response: {msg}"))
                }
                #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
                LlmError::NetworkError(e) => {
                    MemoryError::ExtractionFailed(format!("network error: {e}"))
                }
                _ => MemoryError::ExtractionFailed(format!("LLM error: {e}")),
            })?;

        // Extract and clean JSON response
        let raw_response = response.content_text();
        let json_str = Self::clean_json_response(raw_response);

        // Parse JSON array of raw facts
        let raw_facts: Vec<RawFact> = serde_json::from_str(&json_str).map_err(|e| {
            MemoryError::ExtractionFailed(format!(
                "failed to parse facts as JSON array: {e}\nResponse: {json_str}"
            ))
        })?;

        // Convert raw facts to Fact structs with timestamps
        let facts = raw_facts
            .into_iter()
            .map(|raw| Fact::new(raw.topic, raw.claim, raw.source, raw.confidence))
            .collect();

        Ok(facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fact_construction() {
        let fact = Fact::new(
            "Geography".to_string(),
            "Paris is the capital of France".to_string(),
            "test_document.txt".to_string(),
            0.95,
        );

        assert_eq!(fact.topic, "Geography");
        assert_eq!(fact.claim, "Paris is the capital of France");
        assert_eq!(fact.source, "test_document.txt");
        assert!((fact.confidence - 0.95).abs() < f64::EPSILON);
        assert!(fact.timestamp <= Utc::now());
    }

    #[test]
    #[should_panic(expected = "confidence must be between 0.0 and 1.0")]
    fn test_fact_invalid_confidence_high() {
        let _ = Fact::new(
            "Test".to_string(),
            "Claim".to_string(),
            "source".to_string(),
            1.5,
        );
    }

    #[test]
    #[should_panic(expected = "confidence must be between 0.0 and 1.0")]
    fn test_fact_invalid_confidence_low() {
        let _ = Fact::new(
            "Test".to_string(),
            "Claim".to_string(),
            "source".to_string(),
            -0.1,
        );
    }

    #[test]
    fn test_fact_serialization() {
        let fact = Fact::new(
            "Science".to_string(),
            "Water boils at 100°C".to_string(),
            "chemistry.txt".to_string(),
            0.99,
        );

        let json = serde_json::to_string(&fact).expect("serialization failed");
        let deserialized: Fact = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.topic, fact.topic);
        assert_eq!(deserialized.claim, fact.claim);
        assert_eq!(deserialized.source, fact.source);
        assert!((deserialized.confidence - fact.confidence).abs() < f64::EPSILON);
    }

    // Test LlmFactExtractor construction
    #[test]
    fn test_llm_fact_extractor_construction() {
        let model = crate::llm::MockChatModel::new("test-model");
        let extractor = LlmFactExtractor::new(model);

        // Verify the extractor was created - just check it exists
        let _ = &extractor;
    }

    #[test]
    fn test_clean_json_response() {
        // Test with ```json wrapper
        let raw = "```json\n[{\"topic\": \"test\"}]\n```";
        let cleaned = LlmFactExtractor::<crate::llm::MockChatModel>::clean_json_response(raw);
        assert_eq!(cleaned, "[{\"topic\": \"test\"}]");

        // Test with ``` wrapper only
        let raw = "```\n[{\"topic\": \"test\"}]\n```";
        let cleaned = LlmFactExtractor::<crate::llm::MockChatModel>::clean_json_response(raw);
        assert_eq!(cleaned, "[{\"topic\": \"test\"}]");

        // Test with no wrapper
        let raw = "[{\"topic\": \"test\"}]";
        let cleaned = LlmFactExtractor::<crate::llm::MockChatModel>::clean_json_response(raw);
        assert_eq!(cleaned, "[{\"topic\": \"test\"}]");
    }
}

// Rust guideline compliant 2026-05-26
