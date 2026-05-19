//! Error types for the store crate.

/// Errors that can occur during store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Item not found in the store.
    #[error("item not found: {namespace}/{key}")]
    NotFound { namespace: String, key: String },

    /// Invalid namespace provided.
    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),

    /// Serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    /// Generic storage error.
    #[error("storage error: {0}")]
    Storage(String),

    /// Vector search error (feature-gated).
    #[error("vector search error: {0}")]
    VectorSearch(String),

    /// Embedding generation error.
    #[error("embedding error: {0}")]
    Embedding(String),
}

// Rust guideline compliant 2026-05-19
