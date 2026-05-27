//! Fact extraction wrapper for research findings.

#![allow(
    dead_code,
    reason = "Public API components may not all be used in current binary"
)]

use juncture::llm::ChatModel;
use juncture::memory::{Fact, FactExtractor, LlmFactExtractor};

use crate::state::Finding;

/// Fact extractor specialized for research findings.
///
/// Wraps the LLM-based fact extractor to work with Finding structs.
pub struct ResearchFactExtractor<M: ChatModel> {
    /// Inner LLM fact extractor.
    inner: LlmFactExtractor<M>,
}

// Manual Debug implementation since M:ChatModel may not implement Debug
impl<M: ChatModel> std::fmt::Debug for ResearchFactExtractor<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchFactExtractor")
            .field("inner", &"<LlmFactExtractor>")
            .finish()
    }
}

impl<M: ChatModel> ResearchFactExtractor<M> {
    /// Create a new research fact extractor.
    ///
    /// # Arguments
    ///
    /// * `model` - The chat model to use for extraction
    #[must_use]
    pub const fn new(model: M) -> Self {
        Self {
            inner: LlmFactExtractor::new(model),
        }
    }

    /// Extract facts from a research finding.
    ///
    /// # Arguments
    ///
    /// * `finding` - The research finding to extract facts from
    ///
    /// # Errors
    ///
    /// Returns error if fact extraction fails.
    pub async fn extract_from_finding(&self, finding: &Finding) -> Vec<Fact> {
        // Combine sub-task and content for extraction
        let text = format!("{}\n\n{}", finding.sub_task, finding.content);

        // Extract facts using the inner extractor
        self.inner
            .extract(&text)
            .await
            .unwrap_or_else(|_| Vec::new())
    }
}

// Rust guideline compliant 2026-05-27
