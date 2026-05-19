//! Store trait for key-value storage operations.

use async_trait::async_trait;

use crate::error::StoreError;
use crate::types::{Item, SearchQuery, SearchResult, StoreOp, StoreResult};

/// Async key-value store trait for cross-thread persistent storage.
#[async_trait]
pub trait Store: Send + Sync + std::fmt::Debug + 'static {
    /// Gets a single item by namespace and key.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the get operation fails.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError>;

    /// Puts or updates an item in the store.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the put operation fails.
    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        index: Option<Vec<String>>,
    ) -> Result<(), StoreError>;

    /// Deletes an item from the store.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the delete operation fails.
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError>;

    /// Searches for items matching the given query.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the search operation fails.
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError>;

    /// Lists namespaces matching the given criteria.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Optional prefix to filter namespaces (e.g., "checkpoint:" matches "checkpoint:abc").
    /// * `suffix` - Optional suffix to filter namespaces.
    /// * `max_depth` - Maximum depth of namespace hierarchy to return.
    /// * `limit` - Maximum number of namespaces to return.
    /// * `offset` - Number of namespaces to skip for pagination.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the list operation fails.
    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError>;

    /// Executes multiple operations in a batch.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` if the batch operation fails.
    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError>;
}

// Rust guideline compliant 2026-05-19
