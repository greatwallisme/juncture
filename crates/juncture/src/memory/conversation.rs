//! Conversation memory management with automatic summarization.
//!
//! This module provides [`ConversationMemory`] for managing message history
//! with automatic summarization when the message count exceeds a threshold.

use crate::memory::MemoryError;
use crate::memory::Summarizer;
use juncture_core::state::messages::Message;
use juncture_core::store::{Store, StoreError};
use serde::Serialize;
use std::fmt;
use std::sync::Arc;

/// Conversation memory manager with automatic summarization.
///
/// Manages message history and automatically summarizes older messages
/// when the count exceeds `max_messages`. Summaries are stored in the
/// configured [`Store`] for persistence.
///
/// # Type Parameters
///
/// * `S` - The store type for persisting summaries
///
/// # Example
///
/// ```ignore
/// use juncture::memory::{ConversationMemory, LlmSummarizer};
/// use juncture::llm::ChatOpenAI;
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let store = Arc::new(juncture_core::store::MemoryStore::new());
/// let model = Arc::new(ChatOpenAI::new("sk-..."));
/// let summarizer = Arc::new(LlmSummarizer::new(model, 500));
///
/// let memory = ConversationMemory::new(store, "conversations".to_string())
///     .with_summarizer(summarizer)
///     .with_max_messages(100);
/// # Ok(())
/// # }
/// ```
pub struct ConversationMemory<S: Store> {
    /// The underlying store for persisting summaries.
    store: Arc<S>,

    /// Optional summarizer for condensing message history.
    summarizer: Option<Arc<dyn Summarizer>>,

    /// Namespace for storing summaries in the store.
    namespace: String,

    /// Maximum messages before triggering summarization.
    max_messages: usize,
}

/// Helper struct for storing summaries.
#[derive(Serialize)]
struct SummaryData<'a> {
    summary: &'a str,
    stored_at: String,
}

impl<S: Store> ConversationMemory<S> {
    /// Create a new conversation memory manager.
    ///
    /// # Arguments
    ///
    /// * `store` - The store for persisting summaries
    /// * `namespace` - Namespace prefix for storing summaries
    ///
    /// # Panics
    ///
    /// Panics if `max_messages` is 0 (from `with_max_messages`).
    #[must_use]
    pub fn new(store: Arc<S>, namespace: String) -> Self {
        Self {
            store,
            summarizer: None,
            namespace,
            max_messages: 100,
        }
    }

    /// Set the summarizer for automatic condensation of message history.
    ///
    /// # Arguments
    ///
    /// * `summarizer` - The summarizer to use
    #[must_use]
    pub fn with_summarizer(mut self, summarizer: Arc<dyn Summarizer>) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Set the maximum message count before triggering summarization.
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum messages (must be > 0)
    ///
    /// # Panics
    ///
    /// Panics if `max` is 0.
    #[must_use]
    pub fn with_max_messages(mut self, max: usize) -> Self {
        assert!(max > 0, "max_messages must be greater than 0");
        self.max_messages = max;
        self
    }

    /// Check if the message count exceeds the threshold for summarization.
    ///
    /// # Arguments
    ///
    /// * `messages` - The current message list
    ///
    /// # Returns
    ///
    /// `true` if the message count exceeds `max_messages`.
    #[must_use]
    pub const fn should_summarize(&self, messages: &[Message]) -> bool {
        messages.len() > self.max_messages
    }

    /// Summarize older messages if the count exceeds the threshold.
    ///
    /// When summarization is triggered, takes the first N messages (where
    /// N = `len - max_messages / 2`), concatenates their text content,
    /// generates a summary, and stores it in the configured store.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Unique identifier for this conversation thread
    /// * `messages` - The current message list
    ///
    /// # Returns
    ///
    /// - `Ok(Some(summary))` - Summarization was performed, returns the summary text
    /// - `Ok(None)` - No summarization needed (below threshold) or no summarizer configured
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError`] if summarization or storage fails.
    pub async fn summarize_messages(
        &self,
        thread_id: &str,
        messages: &[Message],
    ) -> Result<Option<String>, MemoryError> {
        // Check if summarization is needed
        if !self.should_summarize(messages) {
            return Ok(None);
        }

        // Check if summarizer is configured
        let Some(summarizer) = &self.summarizer else {
            return Ok(None);
        };

        // Determine how many messages to summarize
        // Keep the most recent half, summarize the older half
        let num_to_summarize = messages.len() - self.max_messages / 2;

        // Extract text from older messages
        let text_to_summarize = messages
            .iter()
            .take(num_to_summarize)
            .map(|msg| format!("[{:?}]: {}", msg.role, msg.content_text()))
            .collect::<Vec<_>>()
            .join("\n\n");

        // Generate summary
        let summary = summarizer.summarize(&text_to_summarize).await?;

        // Store the summary
        self.store_summary(thread_id, &summary).await?;

        Ok(Some(summary))
    }

    /// Store a summary in the configured store.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Unique identifier for this conversation thread
    /// * `summary` - The summary text to store
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::StoreError`] if storage fails.
    pub async fn store_summary(&self, thread_id: &str, summary: &str) -> Result<(), MemoryError> {
        let key = format!("summary:{thread_id}");

        // Create JSON value for storage
        let data = SummaryData {
            summary,
            stored_at: chrono::Utc::now().to_rfc3339(),
        };

        let value = serde_json::to_value(data).map_err(|e| {
            MemoryError::StoreError(format!("failed to serialize summary data: {e}"))
        })?;

        self.store
            .put(&self.namespace, &key, value, None)
            .await
            .map_err(|e| match e {
                StoreError::Serialize(e) => {
                    MemoryError::StoreError(format!("serialization error: {e}"))
                }
                StoreError::Storage(e) => MemoryError::StoreError(format!("storage error: {e}")),
                _ => MemoryError::StoreError(format!("store error: {e}")),
            })?;

        Ok(())
    }

    /// Retrieve a stored summary for a conversation thread.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Unique identifier for this conversation thread
    ///
    /// # Returns
    ///
    /// - `Ok(Some(summary))` - Summary found and returned
    /// - `Ok(None)` - No summary exists for this thread
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError`] if retrieval fails.
    pub async fn get_summary(&self, thread_id: &str) -> Result<Option<String>, MemoryError> {
        let key = format!("summary:{thread_id}");

        let item = self
            .store
            .get(&self.namespace, &key)
            .await
            .map_err(|e| match e {
                StoreError::Serialize(e) => {
                    MemoryError::StoreError(format!("serialization error: {e}"))
                }
                StoreError::Storage(e) => MemoryError::StoreError(format!("storage error: {e}")),
                _ => MemoryError::StoreError(format!("store error: {e}")),
            })?;

        match item {
            Some(item) => {
                // Extract summary from stored JSON
                let summary = item
                    .value
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        MemoryError::StoreError(
                            "invalid summary data: missing 'summary' field".to_string(),
                        )
                    })?;

                Ok(Some(summary.to_string()))
            }
            None => Ok(None),
        }
    }
}

// Custom Debug impl since Store may not implement Debug
impl<S: Store> fmt::Debug for ConversationMemory<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationMemory")
            .field("store", &"<Store>")
            .field("summarizer", &self.summarizer.is_some())
            .field("namespace", &self.namespace)
            .field("max_messages", &self.max_messages)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use juncture_core::store::MemoryStore;

    #[test]
    fn test_conversation_memory_new() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string());

        assert_eq!(memory.namespace, "test");
        assert_eq!(memory.max_messages, 100);
        assert!(memory.summarizer.is_none());
    }

    #[test]
    fn test_conversation_memory_builder() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string()).with_max_messages(50);

        assert_eq!(memory.max_messages, 50);
    }

    #[test]
    #[should_panic(expected = "max_messages must be greater than 0")]
    fn test_conversation_memory_invalid_max() {
        let store = Arc::new(MemoryStore::new());
        let _ = ConversationMemory::new(store, "test".to_string()).with_max_messages(0);
    }

    #[tokio::test]
    async fn test_should_summarize_below_threshold() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string()).with_max_messages(100);

        let messages = vec![Message::human("Hello"); 50];
        assert!(!memory.should_summarize(&messages));
    }

    #[tokio::test]
    async fn test_should_summarize_above_threshold() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string()).with_max_messages(100);

        let messages = vec![Message::human("Hello"); 101];
        assert!(memory.should_summarize(&messages));
    }

    #[tokio::test]
    async fn test_store_and_get_summary() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string());

        // Store a summary
        memory
            .store_summary("thread_1", "This is a summary")
            .await
            .expect("store_summary failed");

        // Retrieve it
        let retrieved = memory
            .get_summary("thread_1")
            .await
            .expect("get_summary failed");

        assert_eq!(retrieved, Some("This is a summary".to_string()));
    }

    #[tokio::test]
    async fn test_get_nonexistent_summary() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string());

        let result = memory
            .get_summary("nonexistent")
            .await
            .expect("get_summary failed");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_summarize_below_threshold() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string()).with_max_messages(100);

        let messages = vec![Message::human("Hello"); 50];
        let result = memory
            .summarize_messages("thread_1", &messages)
            .await
            .expect("summarize_messages failed");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_summarize_no_summarizer_configured() {
        let store = Arc::new(MemoryStore::new());
        let memory = ConversationMemory::new(store, "test".to_string()).with_max_messages(100);

        let messages = vec![Message::human("Hello"); 101];
        let result = memory
            .summarize_messages("thread_1", &messages)
            .await
            .expect("summarize_messages failed");

        assert_eq!(result, None);
    }
}

// Rust guideline compliant 2026-05-26
