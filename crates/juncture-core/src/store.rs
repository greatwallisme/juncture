// Store trait and implementations for cross-thread long-term memory
//
// This module provides the Store abstraction for persistent key-value storage
// that is independent of checkpoint and shared across all threads.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use std::fmt::Write;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
use sqlx::Row;

/// Store error types
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Item not found
    #[error("item not found: {0}")]
    NotFound(String),

    /// Invalid operation
    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// IO error
    #[error("io error: {0}")]
    Io(String),

    /// Database error
    #[error("database error: {0}")]
    Database(String),

    /// Other errors
    #[error("store error: {0}")]
    Other(String),
}

/// Store trait for cross-thread long-term memory
///
/// Provides hierarchical namespace key-value storage independent of
/// checkpoint and shared across all threads and graph executions.
///
/// Note: This trait cannot implement Debug as it's an async trait intended
/// for dynamic dispatch via trait objects.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// Get item from store
    ///
    /// # Arguments
    ///
    /// * `namespace` - Namespace path
    /// * `key` - Item key within namespace
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError>;

    /// Put item into store
    ///
    /// # Arguments
    ///
    /// * `namespace` - Namespace path
    /// * `key` - Item key within namespace
    /// * `value` - Item value
    /// * `index` - Optional index fields for vector search
    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        index: Option<Vec<String>>,
    ) -> Result<(), StoreError>;

    /// Delete item from store
    ///
    /// # Arguments
    ///
    /// * `namespace` - Namespace path
    /// * `key` - Item key within namespace
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError>;

    /// Search items with filtering and optional vector search
    ///
    /// # Arguments
    ///
    /// * `query` - Search query
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError>;

    /// List namespaces
    ///
    /// # Arguments
    ///
    /// * `prefix` - Optional prefix filter
    /// * `suffix` - Optional suffix filter
    /// * `max_depth` - Optional maximum depth
    /// * `limit` - Optional result limit
    /// * `offset` - Optional offset
    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError>;

    /// Execute batch operations
    ///
    /// # Arguments
    ///
    /// * `ops` - List of operations to execute
    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError>;
}

/// Stored item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Namespace path
    pub namespace: String,
    /// Key within namespace
    pub key: String,
    /// Stored value
    pub value: serde_json::Value,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
    /// Optional expiration timestamp for TTL support
    pub expires_at: Option<DateTime<Utc>>,
    /// Optional embedding vector for vector search.
    ///
    /// Pre-computed during `put()` when an [`IndexConfig`] with an
    /// [`EmbeddingFunc`] is configured and index fields are provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

impl Item {
    /// Returns `true` if the item has expired based on `expires_at`.
    ///
    /// Items without an expiration timestamp are never considered expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| Utc::now() > expires_at)
    }
}

/// Search result item with optional similarity score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchItem {
    /// Base item
    #[serde(flatten)]
    pub item: Item,
    /// Similarity score (for vector search)
    pub score: Option<f64>,
}

/// Search query
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Namespace prefix
    pub namespace_prefix: String,
    /// Filter expression
    pub filter: Option<FilterExpr>,
    /// Natural language query (for vector search)
    pub query: Option<String>,
    /// Result limit
    pub limit: usize,
    /// Offset for pagination
    pub offset: usize,
}

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Matching items
    pub items: Vec<SearchItem>,
    /// Total count
    pub total_count: usize,
}

/// Filter expression for search
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum FilterExpr {
    /// Equality
    #[serde(rename = "$eq")]
    Eq {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Inequality
    #[serde(rename = "$ne")]
    Ne {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Greater than
    #[serde(rename = "$gt")]
    Gt {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Greater than or equal
    #[serde(rename = "$gte")]
    Gte {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Less than
    #[serde(rename = "$lt")]
    Lt {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Less than or equal
    #[serde(rename = "$lte")]
    Lte {
        /// Field path
        field: String,
        /// Value to compare
        value: serde_json::Value,
    },
    /// Logical AND
    #[serde(rename = "$and")]
    And {
        /// Sub-expressions
        expressions: Vec<FilterExpr>,
    },
    /// Logical OR
    #[serde(rename = "$or")]
    Or {
        /// Sub-expressions
        expressions: Vec<FilterExpr>,
    },
    /// Logical NOT
    #[serde(rename = "$not")]
    Not {
        /// Negated expression
        expr: Box<FilterExpr>,
    },
}

/// Store operation type
#[derive(Debug, Clone)]
pub enum StoreOp {
    /// Get operation
    Get {
        /// Namespace
        namespace: String,
        /// Key
        key: String,
    },
    /// Put operation
    Put {
        /// Namespace
        namespace: String,
        /// Key
        key: String,
        /// Value
        value: serde_json::Value,
        /// Index fields
        index: Option<Vec<String>>,
    },
    /// Delete operation
    Delete {
        /// Namespace
        namespace: String,
        /// Key
        key: String,
    },
    /// Search operation
    Search(SearchQuery),
    /// List namespaces operation
    ListNamespaces {
        /// Prefix filter
        prefix: Option<String>,
        /// Suffix filter
        suffix: Option<String>,
        /// Maximum depth
        max_depth: Option<usize>,
        /// Result limit
        limit: Option<usize>,
        /// Offset for pagination
        offset: Option<usize>,
    },
}

/// Store operation result
#[derive(Debug, Clone)]
pub enum StoreResult {
    /// Single item result
    Item(Option<Item>),
    /// Multiple items result
    Items(SearchResult),
    /// Namespaces result
    Namespaces(Vec<String>),
    /// Empty result
    None,
}

/// Configuration for time-to-live (TTL) behavior on [`MemoryStore`].
///
/// Controls automatic expiration of items using lazy evaluation on read.
/// Expired items are detected and removed during `get()` and `search()`
/// operations (no background sweep task is used).
///
/// # Examples
///
/// ```
/// use juncture_core::store::{MemoryStore, TTLConfig};
/// use std::time::Duration;
///
/// let store = MemoryStore::new().with_ttl_config(TTLConfig {
///     default_ttl: Some(Duration::from_secs(300)),
///     refresh_on_read: true,
/// });
/// ```
#[derive(Clone, Debug, Default)]
pub struct TTLConfig {
    /// Default TTL duration applied to items inserted via `put()`.
    ///
    /// When `None`, items never expire (the default).
    /// When `Some(duration)`, each `put()` sets `expires_at = now + duration`.
    pub default_ttl: Option<std::time::Duration>,
    /// Whether to extend an item's expiration time when it is read via `get()`.
    ///
    /// When `true` and `default_ttl` is set, a successful `get()` will update
    /// the item's `expires_at` to `now + default_ttl`, effectively resetting
    /// its TTL timer.
    pub refresh_on_read: bool,
}

/// In-memory store implementation
///
/// Thread-safe in-memory store using `RwLock` for concurrent access.
#[derive(Debug)]
pub struct MemoryStore {
    /// Data: namespace -> (key -> Item)
    data: Arc<tokio::sync::RwLock<HashMap<String, HashMap<String, Item>>>>,
    /// Vector index configuration
    index_config: Option<IndexConfig>,
    /// TTL configuration for item expiration.
    ttl_config: TTLConfig,
}

/// Trait for computing embeddings for vector search.
///
/// Implementations provide async embedding generation from text inputs,
/// used by vector-capable stores for similarity search.
///
/// # Errors
///
/// Implementations should return [`StoreError::Other`] or a suitable variant
/// if embedding generation fails (network error, model error, etc.).
#[async_trait::async_trait]
pub trait EmbeddingFunc: Send + Sync + 'static {
    /// Generate embedding vectors for the given texts.
    ///
    /// # Arguments
    ///
    /// * `texts` - Text strings to embed
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if embedding generation fails.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, StoreError>;
}

/// Vector index configuration
///
/// Configure vector similarity search on a store by providing an
/// [`EmbeddingFunc`] implementation and the embedding dimension count.
pub struct IndexConfig {
    /// Embedding dimensions
    pub dims: usize,
    /// Embedding function for computing vectors from text
    pub embed: Option<Box<dyn EmbeddingFunc>>,
    /// Fields to index (None indexes all text fields)
    pub fields: Option<Vec<String>>,
}

impl std::fmt::Debug for IndexConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexConfig")
            .field("dims", &self.dims)
            .field("embed", &self.embed.as_ref().map(|_| "..."))
            .field("fields", &self.fields)
            .finish()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// Create new in-memory store
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            index_config: None,
            ttl_config: TTLConfig::default(),
        }
    }

    /// Create store with vector search enabled
    ///
    /// # Arguments
    ///
    /// * `config` - Index configuration
    #[must_use]
    pub fn with_vector_search(mut self, config: IndexConfig) -> Self {
        self.index_config = Some(config);
        self
    }

    /// Configure TTL behavior for item expiration.
    ///
    /// # Arguments
    ///
    /// * `config` - TTL configuration
    #[must_use]
    pub const fn with_ttl_config(mut self, config: TTLConfig) -> Self {
        self.ttl_config = config;
        self
    }
}

#[async_trait]
impl Store for MemoryStore {
    #[allow(
        clippy::significant_drop_tightening,
        reason = "Read lock is scoped tightly; write lock acquired after release"
    )]
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        // Phase 1: read lock -- check if item exists and whether it is expired
        let is_expired = {
            let data = self.data.read().await;
            let Some(ns) = data.get(namespace) else {
                return Ok(None);
            };
            let Some(item) = ns.get(key) else {
                return Ok(None);
            };
            item.is_expired()
        };

        if is_expired {
            // Phase 2a: write lock -- lazily remove expired item
            let mut data = self.data.write().await;
            if let Some(ns_map) = data.get_mut(namespace) {
                ns_map.remove(key);
            }
            drop(data);
            return Ok(None);
        }

        if self.ttl_config.refresh_on_read && self.ttl_config.default_ttl.is_some() {
            // Phase 2b: write lock -- refresh TTL and return item
            let ttl = self.ttl_config.default_ttl.expect("checked is_some above");
            let now = Utc::now();
            let new_expires =
                now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);

            let mut data = self.data.write().await;
            if let Some(ns_map) = data.get_mut(namespace)
                && let Some(item) = ns_map.get_mut(key)
            {
                item.expires_at = Some(new_expires);
                item.updated_at = now;
                let cloned = item.clone();
                drop(data);
                return Ok(Some(cloned));
            }
            drop(data);
            // Item was removed between read and write phases
            return Ok(None);
        }

        // Phase 2c: read lock -- return item without modification
        let data = self.data.read().await;
        let item = data.get(namespace).and_then(|ns| ns.get(key).cloned());
        Ok(item)
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "Lock must be held for entire put operation after embedding"
    )]
    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        index: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        // Compute embedding outside the write lock (embeddings may be async)
        let embedding = if let Some(ref index_config) = self.index_config {
            if let (Some(embed_fn), Some(index_fields)) = (&index_config.embed, &index) {
                if index_fields.is_empty() {
                    None
                } else {
                    let text = extract_index_text(&value, index_fields);
                    if text.is_empty() {
                        None
                    } else {
                        let mut embeddings = embed_fn.embed(vec![text]).await?;
                        embeddings.pop()
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let now = Utc::now();
        let expires_at = self
            .ttl_config
            .default_ttl
            .map(|ttl| now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX));

        let mut data = self.data.write().await;

        let namespace_map = data
            .entry(namespace.to_string())
            .or_insert_with(HashMap::new);
        let existing = namespace_map.get(key);

        let item = Item {
            namespace: namespace.to_string(),
            key: key.to_string(),
            value,
            created_at: existing.map_or(now, |i| i.created_at),
            updated_at: now,
            expires_at,
            embedding,
        };

        namespace_map.insert(key.to_string(), item);
        Ok(())
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "Lock must be held for entire delete operation"
    )]
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        if let Some(namespace_map) = data.get_mut(namespace) {
            namespace_map.remove(key);
        }
        Ok(())
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "Lock must be held for entire search iteration"
    )]
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError> {
        // Compute query embedding outside the read lock
        let query_embedding: Option<Vec<f32>> = if let Some(ref index_config) = self.index_config {
            if let (Some(embed_fn), Some(query_text)) = (&index_config.embed, &query.query) {
                if query_text.is_empty() {
                    None
                } else {
                    let mut embeddings = embed_fn.embed(vec![query_text.clone()]).await?;
                    embeddings.pop()
                }
            } else {
                None
            }
        } else {
            None
        };

        // Phase 1: Gather items under read lock
        let mut items: Vec<SearchItem> = {
            let data = self.data.read().await;
            let mut results = Vec::new();

            for (namespace, namespace_map) in data.iter() {
                if namespace.starts_with(&query.namespace_prefix) {
                    for item in namespace_map.values() {
                        if item.is_expired() {
                            continue;
                        }

                        if query
                            .filter
                            .as_ref()
                            .is_some_and(|filter| !evaluate_filter(filter, &item.value))
                        {
                            continue;
                        }

                        let score = query_embedding.as_ref().and_then(|q_emb| {
                            item.embedding
                                .as_ref()
                                .map(|i_emb| f64::from(cosine_similarity(q_emb, i_emb)))
                        });

                        results.push(SearchItem {
                            item: item.clone(),
                            score,
                        });
                    }
                }
            }
            results
        };

        let total = items.len();

        // Phase 2: Sort by similarity when vector search is active
        if query_embedding.is_some() {
            items.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Phase 3: Apply pagination
        let start = query.offset.min(items.len());
        let end = (start + query.limit).min(items.len());
        let page = items.drain(start..end).collect();

        Ok(SearchResult {
            items: page,
            total_count: total,
        })
    }

    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        _max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError> {
        let mut namespaces: Vec<String> = {
            let data = self.data.read().await;
            data.keys().cloned().collect()
        };

        // Apply filters
        if let Some(prefix_filter) = prefix {
            namespaces.retain(|ns| ns.starts_with(prefix_filter));
        }
        if let Some(suffix_filter) = suffix {
            namespaces.retain(|ns| ns.ends_with(suffix_filter));
        }

        // Apply offset-based pagination (skip first N results)
        if let Some(offset_value) = offset {
            let skip = offset_value.min(namespaces.len());
            namespaces.drain(..skip);
        }

        // Apply limit
        if let Some(limit_value) = limit {
            namespaces.truncate(limit_value);
        }

        Ok(namespaces)
    }

    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError> {
        let mut results = Vec::with_capacity(ops.len());

        for op in ops {
            let result = match op {
                StoreOp::Get { namespace, key } => {
                    let item = self.get(&namespace, &key).await?;
                    StoreResult::Item(item)
                }
                StoreOp::Put {
                    namespace,
                    key,
                    value,
                    index,
                } => {
                    self.put(&namespace, &key, value, index).await?;
                    StoreResult::None
                }
                StoreOp::Delete { namespace, key } => {
                    self.delete(&namespace, &key).await?;
                    StoreResult::None
                }
                StoreOp::Search(query) => {
                    let result = self.search(query).await?;
                    StoreResult::Items(result)
                }
                StoreOp::ListNamespaces {
                    prefix,
                    suffix,
                    max_depth,
                    limit,
                    offset,
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            offset,
                        )
                        .await?;
                    StoreResult::Namespaces(namespaces)
                }
            };
            results.push(result);
        }

        Ok(results)
    }
}

/// Evaluate filter expression against value
fn evaluate_filter(filter: &FilterExpr, value: &serde_json::Value) -> bool {
    match filter {
        FilterExpr::Eq {
            field,
            value: expected,
        } => get_field(value, field).is_some_and(|v| v == *expected),
        FilterExpr::Ne {
            field,
            value: expected,
        } => get_field(value, field).is_none_or(|v| v != *expected),
        FilterExpr::Gt {
            field,
            value: expected,
        } => compare_numbers(value, field, expected, |a, b| a > b),
        FilterExpr::Gte {
            field,
            value: expected,
        } => compare_numbers(value, field, expected, |a, b| a >= b),
        FilterExpr::Lt {
            field,
            value: expected,
        } => compare_numbers(value, field, expected, |a, b| a < b),
        FilterExpr::Lte {
            field,
            value: expected,
        } => compare_numbers(value, field, expected, |a, b| a <= b),
        FilterExpr::And { expressions } => {
            expressions.iter().all(|expr| evaluate_filter(expr, value))
        }
        FilterExpr::Or { expressions } => {
            expressions.iter().any(|expr| evaluate_filter(expr, value))
        }
        FilterExpr::Not { expr } => !evaluate_filter(expr, value),
    }
}

/// Get nested field from JSON value
fn get_field(value: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(part)?;
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

/// Compare numeric fields
fn compare_numbers(
    value: &serde_json::Value,
    field: &str,
    expected: &serde_json::Value,
    comparator: impl Fn(f64, f64) -> bool,
) -> bool {
    match (get_field(value, field), expected) {
        (Some(serde_json::Value::Number(a)), serde_json::Value::Number(b)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(a_val), Some(b_val)) => comparator(a_val, b_val),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Compute cosine similarity between two vectors.
///
/// Returns a value in `[-1, 1]` where 1 means identical direction,
/// 0 means orthogonal, and -1 means opposite direction.
/// Returns `0.0` if either vector has zero magnitude.
/// When vectors differ in length, only the common prefix is compared.
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let a = &a[..len];
    let b = &b[..len];
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot_product / (norm_a * norm_b)
}

/// Extract indexable text from a JSON value for the given field paths.
///
/// Concatenates string representations of each field's value, separated by
/// spaces, for use as input to an embedding function.
fn extract_index_text(value: &serde_json::Value, fields: &[String]) -> String {
    fields
        .iter()
        .filter_map(|field| {
            get_field(value, field).map(|v| {
                v.as_str()
                    .map_or_else(|| v.to_string(), ToString::to_string)
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// SQLite-based store implementation
///
/// Provides persistent storage using `SQLite` database.
/// Requires the `sqlite` feature flag.
#[cfg(feature = "sqlite")]
#[derive(Debug)]
pub struct SqliteStore {
    /// Database connection pool
    pool: Option<sqlx::SqlitePool>,
    /// Vector index configuration
    pub index_config: Option<IndexConfig>,
}

#[cfg(feature = "sqlite")]
impl SqliteStore {
    /// Create new `SQLite` store
    ///
    /// # Arguments
    ///
    /// * `database_url` - `SQLite` database connection string
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if connection fails or table creation fails.
    pub async fn new(database_url: &str) -> Result<Self, StoreError> {
        let pool = sqlx::SqlitePool::connect(database_url).await.map_err(|e| {
            StoreError::InvalidOperation(format!("Failed to connect to database: {e}"))
        })?;

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS store_items (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (namespace, key)
            )
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to create table: {e}")))?;

        Ok(Self {
            pool: Some(pool),
            index_config: None,
        })
    }

    /// Create new `SQLite` store with index config
    ///
    /// # Arguments
    ///
    /// * `database_url` - `SQLite` database connection string
    /// * `index_config` - Optional vector index configuration
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if connection fails or table creation fails.
    pub async fn with_index_config(
        database_url: &str,
        index_config: IndexConfig,
    ) -> Result<Self, StoreError> {
        let mut store = Self::new(database_url).await?;
        store.index_config = Some(index_config);
        Ok(store)
    }
}

#[cfg(feature = "sqlite")]
#[async_trait]
impl Store for SqliteStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let result = sqlx::query(
            "SELECT value, created_at, updated_at FROM store_items WHERE namespace = ? AND key = ?",
        )
        .bind(namespace)
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to get item: {e}")))?;

        if let Some(row) = result {
            let value_str: String = row
                .try_get("value")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            let value = serde_json::from_str(&value_str).map_err(StoreError::Serialization)?;
            let created_at: String = row
                .try_get("created_at")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            let updated_at: String = row
                .try_get("updated_at")
                .map_err(|e| StoreError::Database(e.to_string()))?;

            Ok(Some(Item {
                namespace: namespace.to_string(),
                key: key.to_string(),
                value,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|e| StoreError::Database(format!("invalid timestamp: {e}")))?
                    .with_timezone(&chrono::Utc),
                updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
                    .map_err(|e| StoreError::Database(format!("invalid timestamp: {e}")))?
                    .with_timezone(&chrono::Utc),
                expires_at: None,
                embedding: None,
            }))
        } else {
            Ok(None)
        }
    }

    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        _index: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let value_str = serde_json::to_string(&value).map_err(StoreError::Serialization)?;
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r"
            INSERT INTO store_items (namespace, key, value, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT (namespace, key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at
            ",
        )
        .bind(namespace)
        .bind(key)
        .bind(&value_str)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to put item: {e}")))?;

        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        sqlx::query("DELETE FROM store_items WHERE namespace = ? AND key = ?")
            .bind(namespace)
            .bind(key)
            .execute(pool)
            .await
            .map_err(|e| StoreError::InvalidOperation(format!("Failed to delete item: {e}")))?;

        Ok(())
    }

    async fn search(&self, _query: SearchQuery) -> Result<SearchResult, StoreError> {
        // For basic SQLite, we don't support advanced search
        // Return empty result
        Ok(SearchResult {
            items: vec![],
            total_count: 0,
        })
    }

    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        _max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let mut query_str = "SELECT DISTINCT namespace FROM store_items WHERE 1=1".to_string();
        let mut params = Vec::new();

        if let Some(prefix_filter) = prefix {
            query_str.push_str(" AND namespace LIKE ?");
            params.push(format!("{prefix_filter}%"));
        }
        if let Some(suffix_filter) = suffix {
            query_str.push_str(" AND namespace LIKE ?");
            params.push(format!("%{suffix_filter}"));
        }
        if let Some(limit_value) = limit {
            let _ = write!(query_str, " LIMIT {limit_value}");
        }
        if let Some(offset_value) = offset {
            let _ = write!(query_str, " OFFSET {offset_value}");
        }

        let mut query = sqlx::query(&query_str);
        for param in params {
            query = query.bind(param);
        }

        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|e| StoreError::InvalidOperation(format!("Failed to list namespaces: {e}")))?;

        let mut namespaces = Vec::new();
        for row in rows {
            let ns: String = row
                .try_get("namespace")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            namespaces.push(ns);
        }

        Ok(namespaces)
    }

    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError> {
        let mut results = Vec::with_capacity(ops.len());

        for op in ops {
            let result = match op {
                StoreOp::Get { namespace, key } => {
                    let item = self.get(&namespace, &key).await?;
                    StoreResult::Item(item)
                }
                StoreOp::Put {
                    namespace,
                    key,
                    value,
                    index,
                } => {
                    self.put(&namespace, &key, value, index).await?;
                    StoreResult::None
                }
                StoreOp::Delete { namespace, key } => {
                    self.delete(&namespace, &key).await?;
                    StoreResult::None
                }
                StoreOp::Search(query) => {
                    let result = self.search(query).await?;
                    StoreResult::Items(result)
                }
                StoreOp::ListNamespaces {
                    prefix,
                    suffix,
                    max_depth,
                    limit,
                    offset,
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            offset,
                        )
                        .await?;
                    StoreResult::Namespaces(namespaces)
                }
            };
            results.push(result);
        }

        Ok(results)
    }
}

/// `PostgreSQL`-based store implementation
///
/// Provides persistent storage using `PostgreSQL` database.
/// Requires the `postgres` feature flag.
#[cfg(feature = "postgres")]
#[derive(Debug)]
pub struct PostgresStore {
    /// Database connection pool
    pool: Option<sqlx::PgPool>,
    /// Vector index configuration
    pub index_config: Option<IndexConfig>,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    /// Create new `PostgreSQL` store
    ///
    /// # Arguments
    ///
    /// * `database_url` - `PostgreSQL` database connection string
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if connection fails or table creation fails.
    pub async fn new(database_url: &str) -> Result<Self, StoreError> {
        let pool = sqlx::PgPool::connect(database_url).await.map_err(|e| {
            StoreError::InvalidOperation(format!("Failed to connect to database: {e}"))
        })?;

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS store_items (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL,
                PRIMARY KEY (namespace, key)
            )
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to create table: {e}")))?;

        Ok(Self {
            pool: Some(pool),
            index_config: None,
        })
    }

    /// Create new `PostgreSQL` store with index config
    ///
    /// # Arguments
    ///
    /// * `database_url` - `PostgreSQL` database connection string
    /// * `index_config` - Optional vector index configuration
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if connection fails or table creation fails.
    pub async fn with_index_config(
        database_url: &str,
        index_config: IndexConfig,
    ) -> Result<Self, StoreError> {
        let mut store = Self::new(database_url).await?;
        store.index_config = Some(index_config);
        Ok(store)
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl Store for PostgresStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let result = sqlx::query(
            "SELECT value, created_at, updated_at FROM store_items WHERE namespace = $1 AND key = $2"
        )
        .bind(namespace)
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to get item: {e}")))?;

        if let Some(row) = result {
            let value: serde_json::Value = row
                .try_get("value")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            let created_at: chrono::DateTime<chrono::Utc> = row
                .try_get("created_at")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            let updated_at: chrono::DateTime<chrono::Utc> = row
                .try_get("updated_at")
                .map_err(|e| StoreError::Database(e.to_string()))?;

            Ok(Some(Item {
                namespace: namespace.to_string(),
                key: key.to_string(),
                value,
                created_at,
                updated_at,
                expires_at: None,
                embedding: None,
            }))
        } else {
            Ok(None)
        }
    }

    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        _index: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let now = Utc::now();

        sqlx::query(
            r"
            INSERT INTO store_items (namespace, key, value, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (namespace, key) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = EXCLUDED.updated_at
            ",
        )
        .bind(namespace)
        .bind(key)
        .bind(&value)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| StoreError::InvalidOperation(format!("Failed to put item: {e}")))?;

        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        sqlx::query("DELETE FROM store_items WHERE namespace = $1 AND key = $2")
            .bind(namespace)
            .bind(key)
            .execute(pool)
            .await
            .map_err(|e| StoreError::InvalidOperation(format!("Failed to delete item: {e}")))?;

        Ok(())
    }

    async fn search(&self, _query: SearchQuery) -> Result<SearchResult, StoreError> {
        // For basic PostgreSQL, we don't support advanced search
        // Return empty result
        Ok(SearchResult {
            items: vec![],
            total_count: 0,
        })
    }

    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        _max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| StoreError::InvalidOperation("Store not initialized".to_string()))?;

        let mut query_str = "SELECT DISTINCT namespace FROM store_items WHERE 1=1".to_string();
        let mut param_idx = 1;
        let mut params = Vec::new();

        if let Some(prefix_filter) = prefix {
            param_idx += 1;
            let _ = write!(query_str, " AND namespace LIKE ${param_idx}");
            params.push(format!("{prefix_filter}%"));
        }
        if let Some(suffix_filter) = suffix {
            param_idx += 1;
            let _ = write!(query_str, " AND namespace LIKE ${param_idx}");
            params.push(format!("%{suffix_filter}"));
        }
        if let Some(limit_value) = limit {
            let _ = write!(query_str, " LIMIT {limit_value}");
        }
        if let Some(offset_value) = offset {
            let _ = write!(query_str, " OFFSET {offset_value}");
        }

        let mut query = sqlx::query(&query_str);
        for param in params {
            query = query.bind(param);
        }

        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|e| StoreError::InvalidOperation(format!("Failed to list namespaces: {e}")))?;

        let mut namespaces = Vec::new();
        for row in rows {
            let ns: String = row
                .try_get("namespace")
                .map_err(|e| StoreError::Database(e.to_string()))?;
            namespaces.push(ns);
        }

        Ok(namespaces)
    }

    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError> {
        let mut results = Vec::with_capacity(ops.len());

        for op in ops {
            let result = match op {
                StoreOp::Get { namespace, key } => {
                    let item = self.get(&namespace, &key).await?;
                    StoreResult::Item(item)
                }
                StoreOp::Put {
                    namespace,
                    key,
                    value,
                    index,
                } => {
                    self.put(&namespace, &key, value, index).await?;
                    StoreResult::None
                }
                StoreOp::Delete { namespace, key } => {
                    self.delete(&namespace, &key).await?;
                    StoreResult::None
                }
                StoreOp::Search(query) => {
                    let result = self.search(query).await?;
                    StoreResult::Items(result)
                }
                StoreOp::ListNamespaces {
                    prefix,
                    suffix,
                    max_depth,
                    limit,
                    offset,
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            offset,
                        )
                        .await?;
                    StoreResult::Namespaces(namespaces)
                }
            };
            results.push(result);
        }

        Ok(results)
    }
}

// Rust guideline compliant 2026-05-22

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn active_value() -> serde_json::Value {
        json!({ "status": "active" })
    }

    fn inactive_value() -> serde_json::Value {
        json!({ "status": "inactive" })
    }

    #[test]
    fn test_filter_not_negates_match() {
        // Not(Eq{status=active}) on {status=inactive} => true
        let filter = FilterExpr::Not {
            expr: Box::new(FilterExpr::Eq {
                field: "status".to_string(),
                value: json!("active"),
            }),
        };
        assert!(evaluate_filter(&filter, &inactive_value()));
    }

    #[test]
    fn test_filter_not_inverts_true_to_false() {
        // Not(Eq{status=active}) on {status=active} => false
        let filter = FilterExpr::Not {
            expr: Box::new(FilterExpr::Eq {
                field: "status".to_string(),
                value: json!("active"),
            }),
        };
        assert!(!evaluate_filter(&filter, &active_value()));
    }

    #[test]
    fn test_filter_not_combined_with_and() {
        // And([Gte{age>=18}, Not(Eq{status=banned})]) on {age:25, status:active} => true
        let value = json!({ "age": 25, "status": "active" });
        let filter = FilterExpr::And {
            expressions: vec![
                FilterExpr::Gte {
                    field: "age".to_string(),
                    value: json!(18),
                },
                FilterExpr::Not {
                    expr: Box::new(FilterExpr::Eq {
                        field: "status".to_string(),
                        value: json!("banned"),
                    }),
                },
            ],
        };
        assert!(evaluate_filter(&filter, &value));

        // And([Gte{age>=18}, Not(Eq{status=banned})]) on {age:25, status:banned} => false
        let banned_value = json!({ "age": 25, "status": "banned" });
        assert!(!evaluate_filter(&filter, &banned_value));

        // And([Gte{age>=18}, Not(Eq{status=banned})]) on {age:17, status:active} => false
        let young_value = json!({ "age": 17, "status": "active" });
        assert!(!evaluate_filter(&filter, &young_value));
    }

    #[test]
    fn test_filter_not_serialization_roundtrip() {
        let filter = FilterExpr::Not {
            expr: Box::new(FilterExpr::Eq {
                field: "status".to_string(),
                value: json!("active"),
            }),
        };

        let serialized = serde_json::to_string(&filter).expect("serialization failed");
        assert!(
            serialized.contains("\"$not\""),
            "serialized form must contain $not tag"
        );

        let deserialized: FilterExpr =
            serde_json::from_str(&serialized).expect("deserialization failed");

        // Verify roundtrip correctness by evaluating both against the same value
        let value = active_value();
        assert_eq!(
            evaluate_filter(&filter, &value),
            evaluate_filter(&deserialized, &value),
            "roundtrip filter must produce the same result"
        );
    }

    #[test]
    fn test_filter_nested_not() {
        // Not(Not(Eq{status=active})) is equivalent to Eq{status=active}
        let filter = FilterExpr::Not {
            expr: Box::new(FilterExpr::Not {
                expr: Box::new(FilterExpr::Eq {
                    field: "status".to_string(),
                    value: json!("active"),
                }),
            }),
        };
        assert!(evaluate_filter(&filter, &active_value()));
        assert!(!evaluate_filter(&filter, &inactive_value()));
    }

    // --- TTL tests ---

    #[tokio::test]
    async fn test_ttl_expiration_on_get() {
        let store = MemoryStore::new().with_ttl_config(TTLConfig {
            default_ttl: Some(std::time::Duration::from_millis(50)),
            refresh_on_read: false,
        });

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");

        // Item should be visible immediately
        let item = store
            .get("ns", "key1")
            .await
            .expect("get failed")
            .expect("item should exist");
        assert_eq!(item.key, "key1");

        // Wait for TTL to expire
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // Item should be expired and lazily removed
        let result = store.get("ns", "key1").await.expect("get failed");
        assert!(result.is_none(), "item should have expired");
    }

    #[tokio::test]
    async fn test_ttl_refresh_on_read() {
        let store = MemoryStore::new().with_ttl_config(TTLConfig {
            default_ttl: Some(std::time::Duration::from_millis(100)),
            refresh_on_read: true,
        });

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");

        // Read the item multiple times, each read should refresh TTL
        for _ in 0..3 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let item = store
                .get("ns", "key1")
                .await
                .expect("get failed")
                .expect("item should still exist after refresh");
            assert_eq!(item.key, "key1");
        }

        // After 3 reads at 50ms each (~150ms total), the item should still be alive
        // because each read reset the TTL (100ms).
        let result = store.get("ns", "key1").await.expect("get failed");
        assert!(
            result.is_some(),
            "item should still exist after TTL refreshes"
        );

        // Now wait longer than the TTL without reading -- it should expire
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let result = store.get("ns", "key1").await.expect("get failed");
        assert!(result.is_none(), "item should have expired after no reads");
    }

    #[tokio::test]
    async fn test_ttl_search_filters_expired() {
        let store = MemoryStore::new().with_ttl_config(TTLConfig {
            default_ttl: Some(std::time::Duration::from_millis(50)),
            refresh_on_read: false,
        });

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");
        store
            .put("ns", "key2", json!({"v": 2}), None)
            .await
            .expect("put failed");

        // Both items should appear in search
        let query = SearchQuery {
            namespace_prefix: "ns".to_string(),
            filter: None,
            query: None,
            limit: 10,
            offset: 0,
        };
        let result = store.search(query).await.expect("search failed");
        assert_eq!(result.total_count, 2);

        // Wait for items to expire
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // Search should return zero items
        let query = SearchQuery {
            namespace_prefix: "ns".to_string(),
            filter: None,
            query: None,
            limit: 10,
            offset: 0,
        };
        let result = store.search(query).await.expect("search failed");
        assert_eq!(
            result.total_count, 0,
            "expired items should be filtered from search"
        );
    }

    #[tokio::test]
    async fn test_no_ttl_items_never_expire() {
        let store = MemoryStore::new();

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");

        // Item should have no expiration -- read internal data to verify
        let has_no_expiry = {
            let data = store.data.read().await;
            data.get("ns")
                .and_then(|ns| ns.get("key1"))
                .is_some_and(|item| item.expires_at.is_none())
        };
        assert!(has_no_expiry, "item should have no expiration set");

        // Even after a delay, item should remain
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let result = store.get("ns", "key1").await.expect("get failed");
        assert!(result.is_some(), "item without TTL should never expire");
    }

    #[tokio::test]
    async fn test_ttl_lazy_cleanup_removes_from_underlying_storage() {
        let store = MemoryStore::new().with_ttl_config(TTLConfig {
            default_ttl: Some(std::time::Duration::from_millis(30)),
            refresh_on_read: false,
        });

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");

        // Verify item is in storage
        let exists_before = {
            let data = store.data.read().await;
            data.get("ns").is_some_and(|ns| ns.contains_key("key1"))
        };
        assert!(exists_before, "item should exist in storage initially");

        // Wait for expiration
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Trigger lazy cleanup via get
        let _ = store.get("ns", "key1").await;

        // Verify item was removed from underlying storage
        let exists_after = {
            let data = store.data.read().await;
            data.get("ns").is_some_and(|ns| ns.contains_key("key1"))
        };
        assert!(!exists_after, "expired item should be removed from storage");
    }

    #[tokio::test]
    async fn test_ttl_refresh_updates_expires_at() {
        let store = MemoryStore::new().with_ttl_config(TTLConfig {
            default_ttl: Some(std::time::Duration::from_millis(200)),
            refresh_on_read: true,
        });

        store
            .put("ns", "key1", json!({"v": 1}), None)
            .await
            .expect("put failed");

        let original_expires = {
            let data = store.data.read().await;
            data.get("ns")
                .and_then(|ns| ns.get("key1"))
                .expect("item")
                .expires_at
                .expect("should have expires_at")
        };

        // Small delay so updated_at differs
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let _ = store.get("ns", "key1").await;

        let refreshed_expires = {
            let data = store.data.read().await;
            data.get("ns")
                .and_then(|ns| ns.get("key1"))
                .expect("item")
                .expires_at
                .expect("should have expires_at")
        };

        assert!(
            refreshed_expires > original_expires,
            "refresh_on_read should advance the expiration time: {refreshed_expires} should be > {original_expires}"
        );
    }

    // --- Vector search tests ---

    /// Test embedding function for deterministic vector search testing.
    ///
    /// Produces an 8-dimensional normalized embedding from text using
    /// a polynomial hash. Same text always produces the same embedding;
    /// different texts produce (with high probability) different embeddings.
    struct TestEmbeddingFunc;

    #[async_trait::async_trait]
    impl EmbeddingFunc for TestEmbeddingFunc {
        async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, StoreError> {
            Ok(texts
                .iter()
                .map(|text| {
                    // FNV-1a-like hash for deterministic embedding
                    let hash: u64 = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
                    });
                    let mut vec: Vec<f32> = (0..8)
                        .map(|i| f32::from(((hash >> (i * 8)) & 0xFF) as u8) / 255.0)
                        .collect();
                    let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                    if norm > 0.0 {
                        for v in &mut vec {
                            *v /= norm;
                        }
                    }
                    vec
                })
                .collect())
        }
    }

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let v = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&v, &v);
        let expected = 1.0;
        assert!(
            (sim - expected).abs() < f32::EPSILON,
            "identical vectors should have similarity 1.0, got {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        let expected = 0.0;
        assert!(
            (sim - expected).abs() < f32::EPSILON,
            "orthogonal vectors should have similarity 0.0, got {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        let expected = -1.0;
        assert!(
            (sim - expected).abs() < f32::EPSILON,
            "opposite vectors should have similarity -1.0, got {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_zero_norm() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        let expected = 0.0;
        assert!(
            (sim - expected).abs() < f32::EPSILON,
            "zero-norm vector should give similarity 0.0, got {sim}"
        );
    }

    #[tokio::test]
    async fn test_search_with_embeddings_returns_scored_results() {
        let index_config = IndexConfig {
            dims: 8,
            embed: Some(Box::new(TestEmbeddingFunc)),
            fields: Some(vec!["text".to_string()]),
        };
        let store = MemoryStore::new().with_vector_search(index_config);

        // Put items with index fields
        store
            .put(
                "docs",
                "item1",
                json!({"text": "hello world"}),
                Some(vec!["text".to_string()]),
            )
            .await
            .expect("put failed");
        store
            .put(
                "docs",
                "item2",
                json!({"text": "quantum physics"}),
                Some(vec!["text".to_string()]),
            )
            .await
            .expect("put failed");

        // Search with a query text
        let query = SearchQuery {
            namespace_prefix: "docs".to_string(),
            filter: None,
            query: Some("hello world".to_string()),
            limit: 10,
            offset: 0,
        };
        let result = store.search(query).await.expect("search failed");

        assert!(
            !result.items.is_empty(),
            "search should return matching items"
        );
        // All returned items should have scores since they have embeddings
        for item in &result.items {
            assert!(
                item.score.is_some(),
                "items with embeddings should have similarity scores"
            );
        }

        // The most relevant result should have a high score (> 0.9)
        if let Some(score) = result.items.first().and_then(|i| i.score) {
            assert!(
                score > 0.9,
                "top result should have high similarity score, got {score}"
            );
        }
    }

    #[tokio::test]
    async fn test_search_ordering_respects_similarity() {
        let index_config = IndexConfig {
            dims: 8,
            embed: Some(Box::new(TestEmbeddingFunc)),
            fields: Some(vec!["text".to_string()]),
        };
        let store = MemoryStore::new().with_vector_search(index_config);

        // Put items with clearly different content
        store
            .put(
                "docs",
                "hello-world",
                json!({"text": "hello world"}),
                Some(vec!["text".to_string()]),
            )
            .await
            .expect("put failed");
        store
            .put(
                "docs",
                "hello-there",
                json!({"text": "hello there"}),
                Some(vec!["text".to_string()]),
            )
            .await
            .expect("put failed");
        store
            .put(
                "docs",
                "quantum-physics",
                json!({"text": "quantum physics"}),
                Some(vec!["text".to_string()]),
            )
            .await
            .expect("put failed");

        // Search with query matching the first item
        let query = SearchQuery {
            namespace_prefix: "docs".to_string(),
            filter: None,
            query: Some("hello world".to_string()),
            limit: 10,
            offset: 0,
        };
        let result = store.search(query).await.expect("search failed");

        assert_eq!(
            result.items.len(),
            3,
            "should return all 3 items in the namespace"
        );

        // The most similar item should be first
        let first = result
            .items
            .first()
            .expect("should have at least one result");
        assert_eq!(
            first.item.key, "hello-world",
            "the most similar item should be ranked first"
        );

        // Scores should be in descending order (best match first)
        for pair in result.items.windows(2) {
            if let (Some(a), Some(b)) = (pair[0].score, pair[1].score) {
                assert!(
                    a >= b,
                    "scores should be in descending order: {a} should be >= {b}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_search_without_index_returns_no_scores() {
        // Store with no vector search configured
        let store = MemoryStore::new();

        store
            .put("docs", "item1", json!({"text": "hello"}), None)
            .await
            .expect("put failed");
        store
            .put("docs", "item2", json!({"text": "world"}), None)
            .await
            .expect("put failed");

        // Search with a query should still work but without scores
        let query = SearchQuery {
            namespace_prefix: "docs".to_string(),
            filter: None,
            query: Some("hello".to_string()),
            limit: 10,
            offset: 0,
        };
        let result = store.search(query).await.expect("search failed");

        assert_eq!(result.items.len(), 2, "should return all items");
        // All scores should be None since no index is configured
        for item in &result.items {
            assert!(
                item.score.is_none(),
                "items without index should have no score"
            );
        }
    }

    #[tokio::test]
    async fn test_list_namespaces_offset_skips_first_n() {
        let store = MemoryStore::new();

        // Insert items across multiple namespaces
        for i in 0..5 {
            store
                .put(&format!("ns-{i}"), "key", json!({"v": i}), None)
                .await
                .expect("put failed");
        }

        // Without offset, all 5 namespaces are returned
        let all_ns = store
            .list_namespaces(None, None, None, None, None)
            .await
            .expect("list_namespaces failed");
        assert_eq!(all_ns.len(), 5, "expected all 5 namespaces");

        // With offset=2, skip first 2 => 3 remaining
        let offset_ns = store
            .list_namespaces(None, None, None, None, Some(2))
            .await
            .expect("list_namespaces with offset failed");
        assert_eq!(
            offset_ns.len(),
            3,
            "offset=2 should skip 2 namespaces, leaving 3"
        );
    }

    #[tokio::test]
    async fn test_list_namespaces_offset_and_limit_together() {
        let store = MemoryStore::new();

        for i in 0..10 {
            store
                .put(&format!("ns-{i:02}"), "key", json!({"v": i}), None)
                .await
                .expect("put failed");
        }

        // Offset=3, limit=4 => skip first 3, take next 4 => 4 results
        let page = store
            .list_namespaces(None, None, None, Some(4), Some(3))
            .await
            .expect("list_namespaces failed");
        assert_eq!(page.len(), 4, "offset=3 + limit=4 should yield 4 results");
    }

    #[tokio::test]
    async fn test_list_namespaces_offset_larger_than_results() {
        let store = MemoryStore::new();

        store
            .put("only-ns", "key", json!({"v": 1}), None)
            .await
            .expect("put failed");

        // offset=100 but only 1 namespace exists => empty result
        let result = store
            .list_namespaces(None, None, None, None, Some(100))
            .await
            .expect("list_namespaces failed");
        assert!(
            result.is_empty(),
            "offset larger than result set should return empty"
        );
    }

    #[tokio::test]
    async fn test_list_namespaces_offset_with_prefix_filter() {
        let store = MemoryStore::new();

        for i in 0..6 {
            let ns = if i < 3 {
                format!("alpha-{i}")
            } else {
                format!("beta-{i}")
            };
            store
                .put(&ns, "key", json!({"v": i}), None)
                .await
                .expect("put failed");
        }

        // Filter to "alpha-" namespaces only, then offset=1 => skip 1 of 3 => 2 remaining
        let result = store
            .list_namespaces(Some("alpha-"), None, None, None, Some(1))
            .await
            .expect("list_namespaces failed");
        assert_eq!(
            result.len(),
            2,
            "prefix filter + offset=1 should leave 2 namespaces"
        );
        assert!(
            result.iter().all(|ns| ns.starts_with("alpha-")),
            "all results must match prefix filter"
        );
    }
}
