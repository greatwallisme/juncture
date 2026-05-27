//! Conversation tracker with auto-summarization.

use anyhow::Result;
use juncture::memory::{ConversationMemory, LlmSummarizer};
use juncture_core::state::messages::Message;
use juncture_core::store::MemoryStore;

/// Conversation tracker with automatic summarization.
///
/// Note: This feature is intentionally deferred. Conversation memory with
/// auto-summarization requires integration with the orchestrator to track
/// message counts and trigger summarization when context window fills.
/// Current implementation stores messages in `ResearchState` without
/// automatic compression.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "Intentionally deferred - requires orchestrator integration for message count tracking"
)]
pub struct ConversationTracker {
    /// Inner conversation memory.
    inner: ConversationMemory<MemoryStore>,
}

#[allow(
    dead_code,
    reason = "Intentionally deferred - requires orchestrator integration for message count tracking"
)]
impl ConversationTracker {
    /// Create a new conversation tracker.
    ///
    /// # Arguments
    ///
    /// * `store` - Underlying storage
    /// * `session_id` - Unique session identifier
    #[must_use]
    pub fn new(store: MemoryStore, session_id: String) -> Self {
        Self {
            inner: ConversationMemory::new(std::sync::Arc::new(store), session_id),
        }
    }

    /// Set the summarizer for auto-summarization.
    ///
    /// # Arguments
    ///
    /// * `summarizer` - The LLM-based summarizer to use
    #[must_use]
    pub fn with_summarizer<M: juncture::llm::ChatModel>(
        mut self,
        summarizer: LlmSummarizer<M>,
    ) -> Self {
        self.inner = self.inner.with_summarizer(std::sync::Arc::new(summarizer));
        self
    }

    /// Set the maximum messages before summarization.
    ///
    /// # Arguments
    ///
    /// * `max_messages` - Maximum message count before auto-summarize
    #[must_use]
    pub fn with_max_messages(mut self, max_messages: usize) -> Self {
        self.inner = self.inner.with_max_messages(max_messages);
        self
    }

    /// Check if summarization is needed for the given messages.
    ///
    /// # Arguments
    ///
    /// * `messages` - Current message list
    #[must_use]
    pub const fn should_summarize(&self, messages: &[Message]) -> bool {
        self.inner.should_summarize(messages)
    }

    /// Summarize messages if the count exceeds the threshold.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Thread identifier for the conversation
    /// * `messages` - Current message list
    ///
    /// # Errors
    ///
    /// Returns error if summarization fails.
    pub async fn summarize_if_needed(
        &self,
        thread_id: &str,
        messages: &[Message],
    ) -> Result<Option<String>> {
        self.inner
            .summarize_messages(thread_id, messages)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to summarize: {e}"))
    }
}

// Rust guideline compliant 2026-05-27
