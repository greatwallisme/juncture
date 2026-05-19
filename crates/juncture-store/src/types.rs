//! Core data types for the store.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::StoreError;
use crate::filter::FilterExpr;
use async_trait::async_trait;

/// A stored item with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Namespace for the item (e.g., "checkpoint", "state", "cache").
    pub namespace: String,
    /// Unique key within the namespace.
    pub key: String,
    /// The stored value as JSON.
    pub value: serde_json::Value,
    /// When the item was created.
    pub created_at: DateTime<Utc>,
    /// When the item was last updated.
    pub updated_at: DateTime<Utc>,
    /// Optional expiration time.
    pub expires_at: Option<DateTime<Utc>>,
}

impl Item {
    /// Creates a new item with the given namespace, key, and value.
    #[must_use]
    pub fn new(namespace: String, key: String, value: serde_json::Value) -> Self {
        let now = Utc::now();
        Self {
            namespace,
            key,
            value,
            created_at: now,
            updated_at: now,
            expires_at: None,
        }
    }

    /// Sets the expiration time for this item.
    #[must_use]
    pub const fn with_expiration(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Returns true if the item has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| Utc::now() > expires_at)
    }
}

/// A search result item with optional relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchItem {
    /// The matching item (flattened for JSON compatibility).
    #[serde(flatten)]
    pub item: Item,
    /// Relevance score (0.0 to 1.0) for vector search results.
    pub score: Option<f64>,
}

/// A query for searching items in the store.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchQuery {
    /// Namespace prefix to filter by (e.g., "checkpoint:" matches "checkpoint:abc").
    #[serde(default)]
    pub namespace_prefix: String,
    /// Optional filter expression for value-based filtering.
    #[serde(default)]
    pub filter: Option<FilterExpr>,
    /// Optional query text for semantic search (requires vector feature).
    #[serde(default)]
    pub query: Option<String>,
    /// Maximum number of results to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Number of results to skip (for pagination).
    #[serde(default)]
    pub offset: usize,
}

const fn default_limit() -> usize {
    100
}

/// Results from a search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The matching items with optional scores.
    pub items: Vec<SearchItem>,
    /// Total number of matching items (before pagination).
    pub total_count: usize,
}

/// Operations that can be performed on the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StoreOp {
    /// Get a single item by namespace and key.
    Get {
        /// The namespace to search in.
        namespace: String,
        /// The key to look up.
        key: String,
    },
    /// Put or update an item.
    Put {
        /// The namespace for the item.
        namespace: String,
        /// The key for the item.
        key: String,
        /// The value to store.
        value: serde_json::Value,
        /// Optional list of fields to index for vector search.
        index: Option<Vec<String>>,
    },
    /// Delete an item.
    Delete {
        /// The namespace containing the item.
        namespace: String,
        /// The key of the item to delete.
        key: String,
    },
    /// Search for items matching criteria.
    Search(SearchQuery),
    /// List namespaces matching criteria.
    ListNamespaces {
        /// Optional prefix to filter namespaces.
        prefix: Option<String>,
        /// Optional suffix to filter namespaces.
        suffix: Option<String>,
        /// Maximum depth of namespace hierarchy to return.
        max_depth: Option<usize>,
        /// Maximum number of namespaces to return.
        limit: Option<usize>,
    },
}

/// Results from store operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StoreResult {
    /// Result of a get operation.
    Item(Option<Item>),
    /// Result of a search operation.
    Items(SearchResult),
    /// Result of a list namespaces operation.
    Namespaces(Vec<String>),
    /// No result (for put/delete operations).
    None,
}

/// Configuration for vector indexing.
#[derive(Debug)]
pub struct IndexConfig {
    /// Number of dimensions for embedding vectors.
    pub dims: usize,
    /// Function to generate embeddings from text.
    pub embed: Box<dyn EmbeddingFunc>,
    /// Optional list of fields to include in embedding.
    pub fields: Option<Vec<String>>,
}

/// Trait for embedding generation functions.
#[async_trait]
pub trait EmbeddingFunc: Send + Sync + fmt::Debug {
    /// Generates embeddings for the given texts.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError::Embedding` if embedding generation fails.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, StoreError>;
}

/// Configuration for time-to-live (TTL) behavior.
#[derive(Clone, Debug)]
pub struct TTLConfig {
    /// Default TTL for new items.
    pub default_ttl: Option<Duration>,
    /// Whether to refresh expiration time on read.
    pub refresh_on_read: bool,
    /// Interval between background cleanup sweeps.
    pub sweep_interval: Duration,
    /// Maximum number of items to check per sweep.
    pub sweep_max_items: usize,
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            default_ttl: None,
            refresh_on_read: false,
            sweep_interval: Duration::seconds(300), // 5 minutes
            sweep_max_items: 1000,
        }
    }
}

// Rust guideline compliant 2026-05-19
