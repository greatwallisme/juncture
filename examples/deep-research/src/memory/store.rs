//! Persistent fact store using `juncture_core` Store trait.

use anyhow::Result;
use juncture::memory::Fact;
use juncture_core::store::{MemoryStore, SearchQuery, Store};
use std::sync::Arc;

/// Fact store for persisting research facts across sessions.
#[derive(Clone, Debug)]
pub struct FactStore {
    /// Underlying storage implementation.
    store: Arc<MemoryStore>,

    /// Namespace for isolation.
    namespace: String,
}

impl FactStore {
    /// Create a new fact store with the given namespace.
    ///
    /// # Arguments
    ///
    /// * `namespace` - Namespace for isolating facts
    #[must_use]
    pub fn new(namespace: String) -> Self {
        Self {
            store: Arc::new(MemoryStore::new()),
            namespace,
        }
    }

    /// Save a fact to the store.
    ///
    /// # Arguments
    ///
    /// * `fact` - The fact to save
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails.
    pub async fn save_fact(&self, fact: &Fact) -> Result<()> {
        let key = format!("fact:{}:{}", fact.topic, fact.timestamp.timestamp());
        let value = serde_json::to_value(fact)?;
        self.store
            .put(&self.namespace, &key, value, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to save fact: {e}"))
    }

    /// Search for facts by topic query.
    ///
    /// # Arguments
    ///
    /// * `query` - Search query for topic matching
    /// * `limit` - Maximum number of results to return
    ///
    /// # Errors
    ///
    /// Returns error if search operation fails.
    #[allow(dead_code, reason = "Public API reserved for future orchestrator integration")]
    pub async fn search_facts(&self, query: &str, limit: usize) -> Result<Vec<Fact>> {
        let search_query = SearchQuery {
            namespace_prefix: self.namespace.clone(),
            filter: None,
            query: Some(query.to_string()),
            limit,
            offset: 0,
        };

        let result = self
            .store
            .search(search_query)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to search facts: {e}"))?;

        let mut facts = Vec::new();
        for search_item in result.items {
            if let Ok(fact) = serde_json::from_value::<Fact>(search_item.item.value) {
                facts.push(fact);
            }
        }

        Ok(facts)
    }

    /// Get the underlying store reference.
    #[must_use]
    #[allow(dead_code, reason = "Public API reserved for future orchestrator integration")]
    pub const fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }
}

// Rust guideline compliant 2026-05-27
