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

/// In-memory store implementation
///
/// Thread-safe in-memory store using `RwLock` for concurrent access.
#[derive(Debug)]
pub struct MemoryStore {
    /// Data: namespace -> (key -> Item)
    data: Arc<tokio::sync::RwLock<HashMap<String, HashMap<String, Item>>>>,
    /// Vector index configuration
    index_config: Option<IndexConfig>,
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
}

#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let data = self.data.read().await;
        Ok(data.get(namespace).and_then(|ns| ns.get(key).cloned()))
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "Lock must be held for entire put operation"
    )]
    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        _index: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let now = Utc::now();
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
            expires_at: None,
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
        reason = "Lock must be held for entire search operation"
    )]
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError> {
        let data = self.data.read().await;
        let mut items = Vec::new();

        for (namespace, namespace_map) in data.iter() {
            if namespace.starts_with(&query.namespace_prefix) {
                for item in namespace_map.values() {
                    if query
                        .filter
                        .as_ref()
                        .is_some_and(|filter| !evaluate_filter(filter, &item.value))
                    {
                        continue;
                    }
                    items.push(SearchItem {
                        item: item.clone(),
                        score: None,
                    });
                }
            }
        }

        // Apply pagination
        let total = items.len();
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
        _offset: Option<usize>,
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
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            None,
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
        _offset: Option<usize>,
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
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            None,
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
        _offset: Option<usize>,
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
                } => {
                    let namespaces = self
                        .list_namespaces(
                            prefix.as_deref(),
                            suffix.as_deref(),
                            max_depth,
                            limit,
                            None,
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

// Rust guideline compliant 2026-05-21

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
}
