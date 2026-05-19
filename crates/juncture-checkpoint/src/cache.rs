//! Caching layer for checkpoint storage
//!
//! Provides in-memory caching with LRU eviction and TTL support.

use crate::error::CheckpointError;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Cache entry with optional expiration
struct CacheEntry {
    /// Cached data
    data: Vec<u8>,

    /// Expiration timestamp (None = no expiration)
    expires_at: Option<std::time::Instant>,
}

impl CacheEntry {
    /// Check if the entry has expired
    #[must_use]
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| std::time::Instant::now() >= expires_at)
    }
}

/// Base cache trait for checkpoint caching
///
/// Defines the interface for caching checkpoint data to reduce storage load.
#[async_trait::async_trait]
pub trait BaseCache: Send + Sync + 'static {
    /// Get cached data
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Storage`] if retrieval fails.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>, CheckpointError>;

    /// Set cached data with optional TTL
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Storage`] if storage fails.
    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CheckpointError>;

    /// Delete cached data
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Storage`] if deletion fails.
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CheckpointError>;

    /// Clear cache (optionally by namespace)
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Storage`] if clearing fails.
    async fn clear(&self, namespace: Option<&str>) -> Result<(), CheckpointError>;
}

/// In-memory LRU cache with TTL support
///
/// Thread-safe in-memory cache using LRU eviction policy.
/// Suitable for single-process deployments and development environments.
#[derive(Clone, Debug)]
pub struct MemoryCache {
    /// LRU cache storage (namespace:key -> entry)
    entries: Arc<RwLock<LruCache<String, CacheEntry>>>,

    /// Default TTL for new entries
    default_ttl: Option<Duration>,
}

impl MemoryCache {
    /// Create a new in-memory cache
    ///
    /// # Panics
    ///
    /// Panics if capacity is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(capacity).expect("capacity must be non-zero"),
            ))),
            default_ttl: None,
        }
    }

    /// Create a new cache with default TTL
    ///
    /// # Panics
    ///
    /// Panics if capacity is zero.
    #[must_use]
    pub fn with_ttl(capacity: usize, default_ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(capacity).expect("capacity must be non-zero"),
            ))),
            default_ttl: Some(default_ttl),
        }
    }

    /// Build a cache key from namespace and key
    #[must_use]
    fn build_key(namespace: &str, key: &str) -> String {
        format!("{namespace}:{key}")
    }

    /// Remove expired entries
    ///
    /// This is called automatically during get/set operations,
    /// but can be invoked manually for cleanup.
    async fn purge_expired(&self) {
        let mut cache = self.entries.write().await;
        let expired_keys: Vec<String> = cache
            .iter()
            .filter(|(_, entry)| entry.is_expired())
            .map(|(key, _)| key.clone())
            .collect();

        for key in expired_keys {
            cache.pop(&key);
        }
    }

    /// Get cache statistics
    ///
    /// Returns (`current_size`, capacity).
    pub async fn stats(&self) -> (usize, usize) {
        let cache = self.entries.read().await;
        (cache.len(), cache.cap().get())
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[async_trait::async_trait]
impl BaseCache for MemoryCache {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>, CheckpointError> {
        // Periodic cleanup of expired entries
        self.purge_expired().await;

        let cache_key = Self::build_key(namespace, key);
        {
            let mut cache = self.entries.write().await;

            if let Some(entry) = cache.get_mut(&cache_key) {
                if entry.is_expired() {
                    cache.pop(&cache_key);
                    drop(cache);
                    return Ok(None);
                }
                let result = Ok(Some(entry.data.clone()));
                drop(cache);
                return result;
            }
        }

        Ok(None)
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> Result<(), CheckpointError> {
        let cache_key = Self::build_key(namespace, key);
        let ttl = ttl.or(self.default_ttl);

        let entry = CacheEntry {
            data: value,
            expires_at: ttl.map(|duration| std::time::Instant::now() + duration),
        };

        self.entries.write().await.put(cache_key, entry);

        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CheckpointError> {
        let cache_key = Self::build_key(namespace, key);
        self.entries.write().await.pop(&cache_key);
        Ok(())
    }

    async fn clear(&self, namespace: Option<&str>) -> Result<(), CheckpointError> {
        if let Some(ns) = namespace {
            // Clear all keys in the namespace
            let prefix = format!("{ns}:");
            let mut cache = self.entries.write().await;
            let keys_to_remove: Vec<String> = cache
                .iter()
                .filter(|(key, _)| key.starts_with(&prefix))
                .map(|(key, _)| key.clone())
                .collect();

            for key in keys_to_remove {
                cache.pop(&key);
            }
        } else {
            // Clear all entries
            self.entries.write().await.clear();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_cache_set_get() {
        let cache = MemoryCache::new(10);

        cache
            .set("ns1", "key1", b"hello".to_vec(), None)
            .await
            .unwrap();

        let value = cache.get("ns1", "key1").await.unwrap();
        assert_eq!(value, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn test_memory_cache_miss() {
        let cache = MemoryCache::new(10);

        let value = cache.get("ns1", "nonexistent").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_delete() {
        let cache = MemoryCache::new(10);

        cache
            .set("ns1", "key1", b"hello".to_vec(), None)
            .await
            .unwrap();

        cache.delete("ns1", "key1").await.unwrap();

        let value = cache.get("ns1", "key1").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_ttl() {
        let cache = MemoryCache::with_ttl(10, Duration::from_millis(100));

        cache
            .set("ns1", "key1", b"hello".to_vec(), None)
            .await
            .unwrap();

        // Should be present immediately
        let value = cache.get("ns1", "key1").await.unwrap();
        assert_eq!(value, Some(b"hello".to_vec()));

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be expired
        let value = cache.get("ns1", "key1").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_clear_namespace() {
        let cache = MemoryCache::new(10);

        cache
            .set("ns1", "key1", b"data1".to_vec(), None)
            .await
            .unwrap();
        cache
            .set("ns2", "key2", b"data2".to_vec(), None)
            .await
            .unwrap();

        cache.clear(Some("ns1")).await.unwrap();

        assert!(cache.get("ns1", "key1").await.unwrap().is_none());
        assert_eq!(
            cache.get("ns2", "key2").await.unwrap(),
            Some(b"data2".to_vec())
        );
    }

    #[tokio::test]
    async fn test_memory_cache_clear_all() {
        let cache = MemoryCache::new(10);

        cache
            .set("ns1", "key1", b"data1".to_vec(), None)
            .await
            .unwrap();
        cache
            .set("ns2", "key2", b"data2".to_vec(), None)
            .await
            .unwrap();

        cache.clear(None).await.unwrap();

        assert!(cache.get("ns1", "key1").await.unwrap().is_none());
        assert!(cache.get("ns2", "key2").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_lru_eviction() {
        let cache = MemoryCache::new(2);

        cache
            .set("ns1", "key1", b"data1".to_vec(), None)
            .await
            .unwrap();
        cache
            .set("ns1", "key2", b"data2".to_vec(), None)
            .await
            .unwrap();

        // Access key1 to make it more recently used
        cache.get("ns1", "key1").await.unwrap();

        // Add key3, should evict key2 (least recently used)
        cache
            .set("ns1", "key3", b"data3".to_vec(), None)
            .await
            .unwrap();

        assert_eq!(
            cache.get("ns1", "key1").await.unwrap(),
            Some(b"data1".to_vec())
        );
        assert!(cache.get("ns1", "key2").await.unwrap().is_none());
        assert_eq!(
            cache.get("ns1", "key3").await.unwrap(),
            Some(b"data3".to_vec())
        );
    }

    #[tokio::test]
    async fn test_memory_cache_stats() {
        let cache = MemoryCache::new(100);

        cache
            .set("ns1", "key1", b"data1".to_vec(), None)
            .await
            .unwrap();
        cache
            .set("ns1", "key2", b"data2".to_vec(), None)
            .await
            .unwrap();

        let (size, capacity) = cache.stats().await;
        assert_eq!(size, 2);
        assert_eq!(capacity, 100);
    }
}

// Rust guideline compliant 2026-05-19
