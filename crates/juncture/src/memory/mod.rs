//! High-level memory abstractions for Juncture agents.
//!
//! This module provides advanced memory management capabilities on top of the
//! low-level [`Store`](juncture_core::store::Store) trait, including:
//!
//! - **Summarization**: Condense conversation history and documents
//! - **Fact extraction**: Extract structured facts from unstructured text
//! - **Conversation memory**: Manage message history with auto-summarization
//!
//! # Example
//!
//! ```ignore
//! use juncture::memory::{ConversationMemory, LlmSummarizer};
//! use juncture::llm::ChatOpenAI;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let store = Arc::new(juncture_core::store::MemoryStore::new());
//! let model = Arc::new(ChatOpenAI::new("sk-..."));
//! let summarizer = Arc::new(LlmSummarizer::new(model, 500));
//!
//! let memory = ConversationMemory::new(store, "conversations".to_string())
//!     .with_summarizer(summarizer)
//!     .with_max_messages(100);
//! # Ok(())
//! # }
//! ```

mod conversation;
mod fact;
mod summarizer;

pub use conversation::ConversationMemory;
pub use fact::{Fact, FactExtractor, LlmFactExtractor};
pub use summarizer::{LlmSummarizer, MemoryError, Summarizer};

// Rust guideline compliant 2026-05-26
