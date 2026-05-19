//! In-memory implementation of the Store trait.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::StoreError;
use crate::trait_::Store;
use crate::types::{Item, SearchItem, SearchQuery, SearchResult, StoreOp, StoreResult, TTLConfig};

#[cfg(feature = "vector")]
use crate::types::IndexConfig;

/// In-memory store implementation with thread-safe access.
#[derive(Debug)]
pub struct MemoryStore {
    /// Nested map: namespace -> (key -> Item).
    data: Arc<RwLock<HashMap<String, HashMap<String, Item>>>>,
    /// Vector index configuration (feature-gated).
    #[cfg(feature = "vector")]
    index_config: Option<IndexConfig>,
    /// TTL configuration.
    ttl_config: TTLConfig,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// Creates a new in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "vector")]
            index_config: None,
            ttl_config: TTLConfig::default(),
        }
    }

    /// Sets the index configuration for vector search.
    #[cfg(feature = "vector")]
    #[must_use]
    pub fn with_index_config(mut self, config: IndexConfig) -> Self {
        self.index_config = Some(config);
        self
    }

    /// Sets the TTL configuration.
    #[must_use]
    pub const fn with_ttl_config(mut self, config: TTLConfig) -> Self {
        self.ttl_config = config;
        self
    }

    /// Starts the background sweep task for expired item cleanup.
    ///
    /// # Panics
    ///
    /// Panics if the `sweep_interval` duration cannot be converted to `std::time::Duration`.
    #[must_use]
    pub fn start_sweep_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = self
            .ttl_config
            .sweep_interval
            .to_std()
            .expect("Invalid duration");
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            loop {
                timer.tick().await;
                let _ = self.sweep_expired_items().await;
            }
        })
    }

    /// Removes expired items from the store.
    async fn sweep_expired_items(&self) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let max_items = self.ttl_config.sweep_max_items;
        let mut checked = 0;
        let mut expired_keys = Vec::new();

        for (namespace, items) in data.iter_mut() {
            for (key, item) in items.iter() {
                if checked >= max_items {
                    break;
                }
                if item.is_expired() {
                    expired_keys.push((namespace.clone(), key.clone()));
                }
                checked += 1;
            }
            if checked >= max_items {
                break;
            }
        }

        for (namespace, key) in expired_keys {
            if let Some(items) = data.get_mut(&namespace) {
                items.remove(&key);
            }
        }

        Ok(())
    }

    /// Checks if a namespace matches the given prefix, suffix, and depth criteria.
    fn namespace_matches(
        namespace: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
    ) -> bool {
        if let Some(p) = prefix
            && !namespace.starts_with(p)
        {
            return false;
        }
        if let Some(s) = suffix
            && !namespace.ends_with(s)
        {
            return false;
        }
        if let Some(depth) = max_depth {
            let actual_depth = namespace.split(':').count();
            if actual_depth > depth {
                return false;
            }
        }
        true
    }

    /// Computes cosine similarity between two vectors.
    #[cfg(feature = "vector")]
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            f64::from(dot_product / (norm_a * norm_b))
        }
    }
}

#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let data = self.data.read().await;
        Ok(data
            .get(namespace)
            .and_then(|items| items.get(key).cloned())
            .filter(|item| !item.is_expired()))
    }

    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        _index: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let now = chrono::Utc::now();
        let expires_at = self.ttl_config.default_ttl.map(|ttl| now + ttl);

        {
            let mut data = self.data.write().await;
            let entry = data.entry(namespace.to_string()).or_default();

            // Check if item exists to preserve created_at
            let created_at = entry.get(key).map_or(now, |existing| existing.created_at);

            let item = Item {
                namespace: namespace.to_string(),
                key: key.to_string(),
                value,
                created_at,
                updated_at: now,
                expires_at,
            };

            entry.insert(key.to_string(), item);
            drop(data);
        };

        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let _ = data.get_mut(namespace).and_then(|items| items.remove(key));
        drop(data);
        Ok(())
    }

    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError> {
        let mut items = Vec::new();

        {
            let data = self.data.read().await;

            for (namespace, items_map) in data.iter() {
                if !namespace.starts_with(&query.namespace_prefix) {
                    continue;
                }

                for item in items_map.values() {
                    if item.is_expired() {
                        continue;
                    }

                    if let Some(ref filter) = query.filter
                        && !filter.matches(&item.value)
                    {
                        continue;
                    }

                    items.push(SearchItem {
                        item: item.clone(),
                        score: None,
                    });
                }
            }
            drop(data);
        };

        let total_count = items.len();

        #[cfg(feature = "vector")]
        if let Some(ref query_text) = query.query
            && let Some(ref config) = self.index_config
        {
            let query_embedding = config.embed.embed(vec![query_text.clone()]).await;
            if let Ok(query_vec) = query_embedding
                && let Some(vec) = query_vec.first()
            {
                for search_item in &mut items {
                    // Simulate embedding for stored items (in real implementation, cache embeddings)
                    let text = serde_json::to_string(&search_item.item.value).unwrap_or_default();
                    if let Ok(item_embedding) = config.embed.embed(vec![text]).await
                        && let Some(item_vec) = item_embedding.first()
                    {
                        search_item.score = Some(Self::cosine_similarity(vec, item_vec));
                    }
                }
                items.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        let start = query.offset.min(items.len());
        let end = (start + query.limit).min(items.len());
        let paginated_items = items[start..end].to_vec();

        Ok(SearchResult {
            items: paginated_items,
            total_count,
        })
    }

    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError> {
        let mut namespaces: Vec<String> = self
            .data
            .read()
            .await
            .keys()
            .filter(|ns| Self::namespace_matches(ns, prefix, suffix, max_depth))
            .cloned()
            .collect();

        namespaces.sort();

        let start = offset.unwrap_or(0).min(namespaces.len());
        let end = limit.map_or(namespaces.len(), |limit| {
            (start + limit).min(namespaces.len())
        });

        Ok(namespaces[start..end].to_vec())
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
                    let search_result = self.search(query).await?;
                    StoreResult::Items(search_result)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::FilterExpr;
    use chrono::Duration;
    use serde_json::json;

    fn create_test_store() -> MemoryStore {
        MemoryStore::new()
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let store = create_test_store();
        let namespace = "test";
        let key = "key1";
        let value = json!({"data": "test_value"});

        store
            .put(namespace, key, value.clone(), None)
            .await
            .expect("Put failed");

        let item = store
            .get(namespace, key)
            .await
            .expect("Get failed")
            .expect("Item not found");

        assert_eq!(item.namespace, namespace);
        assert_eq!(item.key, key);
        assert_eq!(item.value, value);
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let store = create_test_store();
        let result = store.get("nonexistent", "key").await.expect("Get failed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let store = create_test_store();
        let namespace = "test";
        let key = "key1";
        let value = json!({"data": "test"});

        store
            .put(namespace, key, value, None)
            .await
            .expect("Put failed");

        store.delete(namespace, key).await.expect("Delete failed");

        let result = store.get(namespace, key).await.expect("Get failed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_search_with_filter() {
        let store = create_test_store();

        store
            .put("test", "key1", json!({"name": "Alice", "age": 30}), None)
            .await
            .expect("Put failed");
        store
            .put("test", "key2", json!({"name": "Bob", "age": 25}), None)
            .await
            .expect("Put failed");

        let query = SearchQuery {
            namespace_prefix: "test".to_string(),
            filter: Some(FilterExpr::Gt {
                field: "age".to_string(),
                value: json!(25),
            }),
            query: None,
            limit: 10,
            offset: 0,
        };

        let result = store.search(query).await.expect("Search failed");

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].item.key, "key1");
    }

    #[tokio::test]
    async fn test_list_namespaces() {
        let store = create_test_store();

        store
            .put("checkpoint:abc", "key1", json!({"data": "test1"}), None)
            .await
            .expect("Put failed");
        store
            .put("checkpoint:def", "key2", json!({"data": "test2"}), None)
            .await
            .expect("Put failed");
        store
            .put("state:xyz", "key3", json!({"data": "test3"}), None)
            .await
            .expect("Put failed");

        let namespaces = store
            .list_namespaces(Some("checkpoint:"), None, None, None, None)
            .await
            .expect("List namespaces failed");

        assert_eq!(namespaces.len(), 2);
        assert!(namespaces.contains(&"checkpoint:abc".to_string()));
        assert!(namespaces.contains(&"checkpoint:def".to_string()));
    }

    #[tokio::test]
    async fn test_batch_operations() {
        let store = create_test_store();

        let ops = vec![
            StoreOp::Put {
                namespace: "test".to_string(),
                key: "key1".to_string(),
                value: json!({"data": "test1"}),
                index: None,
            },
            StoreOp::Put {
                namespace: "test".to_string(),
                key: "key2".to_string(),
                value: json!({"data": "test2"}),
                index: None,
            },
            StoreOp::Get {
                namespace: "test".to_string(),
                key: "key1".to_string(),
            },
        ];

        let results = store.batch(ops).await.expect("Batch failed");

        assert_eq!(results.len(), 3);
        assert!(matches!(results[0], StoreResult::None));
        assert!(matches!(results[1], StoreResult::None));
        assert!(matches!(&results[2], StoreResult::Item(Some(_))));
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let ttl_config = TTLConfig {
            default_ttl: Some(Duration::milliseconds(100)),
            ..Default::default()
        };
        let store = MemoryStore::new().with_ttl_config(ttl_config);

        store
            .put("test", "key1", json!({"data": "test"}), None)
            .await
            .expect("Put failed");

        // Wait for expiration
        tokio::time::sleep(Duration::milliseconds(150).to_std().unwrap()).await;

        let result = store.get("test", "key1").await.expect("Get failed");
        assert!(result.is_none(), "Item should have expired");
    }

    #[tokio::test]
    async fn test_namespace_prefix_filtering() {
        let store = create_test_store();

        store
            .put("checkpoint:abc:123", "key1", json!({}), None)
            .await
            .expect("Put failed");
        store
            .put("checkpoint:def:456", "key2", json!({}), None)
            .await
            .expect("Put failed");
        store
            .put("state:xyz", "key3", json!({}), None)
            .await
            .expect("Put failed");

        let query = SearchQuery {
            namespace_prefix: "checkpoint:".to_string(),
            filter: None,
            query: None,
            limit: 10,
            offset: 0,
        };

        let result = store.search(query).await.expect("Search failed");
        assert_eq!(result.total_count, 2);
    }

    #[tokio::test]
    async fn test_pagination() {
        let store = create_test_store();

        for i in 0..10 {
            store
                .put("test", &format!("key{i}"), json!({"index": i}), None)
                .await
                .expect("Put failed");
        }

        let query = SearchQuery {
            namespace_prefix: "test".to_string(),
            filter: None,
            query: None,
            limit: 5,
            offset: 3,
        };

        let result = store.search(query).await.expect("Search failed");
        assert_eq!(result.items.len(), 5);
        assert_eq!(result.total_count, 10);
    }

    #[tokio::test]
    async fn test_update_existing_item() {
        let store = create_test_store();
        let namespace = "test";
        let key = "key1";
        let value1 = json!({"version": 1});
        let value2 = json!({"version": 2});

        store
            .put(namespace, key, value1, None)
            .await
            .expect("Put failed");

        let item1 = store
            .get(namespace, key)
            .await
            .expect("Get failed")
            .expect("Item not found");

        store
            .put(namespace, key, value2.clone(), None)
            .await
            .expect("Put failed");

        let item2 = store
            .get(namespace, key)
            .await
            .expect("Get failed")
            .expect("Item not found");

        assert_eq!(item2.value, value2);
        assert_eq!(item2.created_at, item1.created_at);
        assert!(item2.updated_at > item1.updated_at);
    }
}

// Rust guideline compliant 2026-05-19
