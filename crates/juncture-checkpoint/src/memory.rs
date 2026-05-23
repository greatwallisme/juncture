//! In-memory checkpoint storage
//!
//! Thread-safe in-memory implementation of `CheckpointSaver` for development and testing.

use juncture_core::checkpoint::{
    Checkpoint, CheckpointError as CoreCheckpointError, CheckpointFilter, CheckpointMetadata,
    CheckpointSaver, CheckpointTuple, PendingWrite,
};
use juncture_core::config::RunnableConfig;
use juncture_tracing::spans::names;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::Instrument;

use crate::error::CheckpointError;

// Convert crate's CheckpointError to core's CheckpointError
#[allow(dead_code, reason = "conversion trait used internally")]
trait ToCoreCheckpointError<T> {
    fn map_checkpoint(self) -> Result<T, CoreCheckpointError>;
}

impl<T> ToCoreCheckpointError<T> for Result<T, CheckpointError> {
    fn map_checkpoint(self) -> Result<T, CoreCheckpointError> {
        self.map_err(|e| match e {
            CheckpointError::Serialize(msg) | CheckpointError::Serialization(msg) => {
                CoreCheckpointError::Serialize(msg)
            }
            CheckpointError::Deserialize(msg) => CoreCheckpointError::Deserialize(msg),
            CheckpointError::NotFound {
                thread_id,
                checkpoint_id,
            } => CoreCheckpointError::NotFound {
                thread_id,
                checkpoint_id,
            },
            CheckpointError::Storage(msg) | CheckpointError::Database(msg) => {
                CoreCheckpointError::Storage(msg)
            }
            CheckpointError::SchemaMigration { from, to, reason } => {
                CoreCheckpointError::Other(format!("Schema migration: {from} -> {to}: {reason}"))
            }
            CheckpointError::PoolExhausted => {
                CoreCheckpointError::Storage("Connection pool exhausted".to_string())
            }
        })
    }
}

/// Type alias for storage: `thread_id` -> `checkpoint_ns` -> Vec<CheckpointTuple>
type StorageMap = HashMap<String, HashMap<String, Vec<CheckpointTuple>>>;

/// Type alias for writes: (`thread_id`, `checkpoint_id`, `checkpoint_ns`) -> Vec<PendingWrite>
type WritesMap = HashMap<(String, String, String), Vec<PendingWrite>>;

/// In-memory checkpoint storage
///
/// Thread-safe checkpoint storage using in-memory data structures.
/// Data is lost when the process exits. Suitable for development and testing.
#[derive(Clone, Debug)]
pub struct MemorySaver {
    /// `thread_id` -> `checkpoint_ns` -> Vec<CheckpointTuple> (sorted by `created_at` DESC)
    storage: Arc<RwLock<StorageMap>>,

    /// (`thread_id`, `checkpoint_id`, `checkpoint_ns`) -> Vec<PendingWrite>
    writes: Arc<RwLock<WritesMap>>,

    /// TTL configuration for checkpoint expiration (M04-001)
    ttl_config: Arc<std::sync::RwLock<crate::types::TtlConfig>>,
}

impl MemorySaver {
    /// Create a new in-memory saver
    #[must_use]
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            writes: Arc::new(RwLock::new(HashMap::new())),
            ttl_config: Arc::new(std::sync::RwLock::new(crate::types::TtlConfig::default())),
        }
    }

    /// Create a new in-memory saver with TTL configuration (M04-001)
    ///
    /// # Arguments
    ///
    /// * `ttl_config` - TTL configuration for automatic checkpoint expiration
    #[must_use]
    pub fn with_ttl_config(mut self, ttl_config: crate::types::TtlConfig) -> Self {
        self.ttl_config = Arc::new(std::sync::RwLock::new(ttl_config));
        self
    }

    /// Get the current TTL configuration
    ///
    /// Returns a clone of the current TTL configuration.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned (indicating a writer thread
    /// panicked while holding the write lock).
    #[must_use]
    pub fn ttl_config(&self) -> crate::types::TtlConfig {
        self.ttl_config.read().unwrap().clone()
    }

    /// Update the TTL configuration (M04-001)
    ///
    /// # Arguments
    ///
    /// * `ttl_config` - New TTL configuration
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned (indicating a writer thread
    /// panicked while holding the write lock).
    pub fn set_ttl_config(&self, ttl_config: crate::types::TtlConfig) {
        *self.ttl_config.write().unwrap() = ttl_config;
    }

    /// Perform lazy cleanup of expired checkpoints (M04-001)
    ///
    /// This method implements lazy cleanup as specified in design doc §5.7.
    /// It removes expired checkpoints and enforces `max_checkpoints` limit.
    /// Called automatically by `list()` and `get_tuple()` operations.
    #[allow(
        clippy::significant_drop_tightening,
        reason = "lock scope is already optimized for minimal contention"
    )]
    async fn lazy_cleanup(
        &self,
        thread_id: &str,
        checkpoint_ns: &str,
    ) -> Result<(), CheckpointError> {
        let ttl_config = self.ttl_config.read().unwrap().clone();

        // Reduce lock contention by limiting write lock scope
        let (checkpoint_ids, expired_count) = {
            let mut storage = self.storage.write().await;

            let thread_map = storage
                .entry(thread_id.to_string())
                .or_insert_with(HashMap::new);
            let checkpoints = thread_map
                .entry(checkpoint_ns.to_string())
                .or_insert_with(Vec::new);

            // Remove expired checkpoints (lazy cleanup per design §5.7)
            let original_len = checkpoints.len();
            checkpoints.retain(|tuple| !ttl_config.is_expired(&tuple.checkpoint.created_at));
            let expired_count = original_len - checkpoints.len();

            // Enforce max_checkpoints limit (delete oldest)
            let Some(max) = ttl_config.max_checkpoints else {
                return Ok(());
            };

            if checkpoints.len() > max {
                let excess = checkpoints.len() - max;
                checkpoints.truncate(max);
                tracing::debug!("Deleted {excess} oldest checkpoints (max_checkpoints={max})");
            }

            // Collect checkpoint IDs for writes cleanup
            let checkpoint_ids: std::collections::HashSet<String> = checkpoints
                .iter()
                .map(|t| t.checkpoint.id.clone())
                .collect();

            (checkpoint_ids, expired_count)
        };

        // Clean up writes for deleted checkpoints outside storage lock
        if expired_count > 0 {
            let mut writes = self.writes.write().await;

            // Remove writes for checkpoints that no longer exist
            writes.retain(|(thread, ns, id), _| {
                thread == thread_id && ns == checkpoint_ns && checkpoint_ids.contains(id)
            });
        }

        Ok(())
    }

    /// Get checkpoint namespace string from config, defaulting to empty string
    #[must_use]
    fn get_checkpoint_ns(config: &RunnableConfig) -> String {
        config
            .checkpoint_ns
            .as_ref()
            .map_or_else(String::new, juncture_core::CheckpointNamespace::as_str)
    }

    /// Get thread ID from config, returning error if not set
    fn get_thread_id(config: &RunnableConfig) -> Result<String, CheckpointError> {
        config
            .thread_id
            .clone()
            .ok_or_else(|| CheckpointError::Storage("thread_id is required".to_string()))
    }

    /// Sort checkpoints by creation time descending
    fn sort_checkpoints(checkpoints: &mut [CheckpointTuple]) {
        checkpoints.sort_by(|a, b| {
            b.checkpoint
                .created_at
                .cmp(&a.checkpoint.created_at)
                .then_with(|| b.checkpoint.id.cmp(&a.checkpoint.id))
        });
    }
}

impl Default for MemorySaver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CheckpointSaver for MemorySaver {
    async fn get_tuple(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<CheckpointTuple>, CoreCheckpointError> {
        let thread_id = Self::get_thread_id(config).map_checkpoint()?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        // Perform lazy cleanup before retrieving checkpoint (M04-001)
        Self::lazy_cleanup(self, &thread_id, &checkpoint_ns)
            .await
            .map_checkpoint()?;

        // Clone the checkpoint data we need while holding the lock briefly
        let storage = self.storage.read().await;
        let checkpoint_data = storage
            .get(&thread_id)
            .and_then(|ns| ns.get(&checkpoint_ns))
            .cloned();
        drop(storage);

        let tuple_opt = checkpoint_data.and_then(|checkpoints| {
            config.checkpoint_id.as_ref().map_or_else(
                || checkpoints.first().cloned(),
                |checkpoint_id| {
                    checkpoints
                        .iter()
                        .find(|t| t.checkpoint.id == *checkpoint_id)
                        .cloned()
                },
            )
        });

        // Then, get pending writes if we found a checkpoint
        if let Some(mut tuple) = tuple_opt {
            let checkpoint_id = tuple.checkpoint.id.clone();
            let writes = self.writes.read().await;
            let pending_writes = writes
                .get(&(thread_id, checkpoint_id, checkpoint_ns))
                .cloned()
                .unwrap_or_default();
            drop(writes);

            tuple.pending_writes = pending_writes;
            Ok(Some(tuple))
        } else {
            Ok(None)
        }
    }

    async fn list(
        &self,
        config: &RunnableConfig,
        filter: Option<CheckpointFilter>,
    ) -> Result<Vec<CheckpointTuple>, CoreCheckpointError> {
        let thread_id = Self::get_thread_id(config).map_checkpoint()?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        // Perform lazy cleanup before listing checkpoints (M04-001)
        Self::lazy_cleanup(self, &thread_id, &checkpoint_ns)
            .await
            .map_checkpoint()?;

        let namespace = {
            let storage = self.storage.read().await;
            storage
                .get(&thread_id)
                .and_then(|ns| ns.get(&checkpoint_ns))
                .cloned()
        };

        let mut checkpoints = namespace.unwrap_or_default();

        // Apply filters
        if let Some(f) = filter {
            // Filter by source
            if let Some(source) = f.source {
                checkpoints.retain(|t| t.metadata.source == source);
            }

            // Filter by step range
            if let Some(min_step) = f.step_gte {
                checkpoints.retain(|t| t.metadata.step >= min_step);
            }
            if let Some(max_step) = f.step_lte {
                checkpoints.retain(|t| t.metadata.step <= max_step);
            }

            // Filter by checkpoint_id range (before/after)
            if let Some(before_id) = f.before {
                let before_pos = checkpoints
                    .iter()
                    .position(|t| t.checkpoint.id == before_id);
                if let Some(pos) = before_pos {
                    checkpoints = checkpoints.into_iter().take(pos).collect();
                }
            }
            if let Some(after_id) = f.after {
                let after_pos = checkpoints.iter().position(|t| t.checkpoint.id == after_id);
                if let Some(pos) = after_pos {
                    checkpoints = checkpoints.into_iter().skip(pos + 1).collect();
                }
            }

            // Apply limit
            if let Some(limit) = f.limit {
                checkpoints.truncate(limit);
            }
        }

        Ok(checkpoints)
    }

    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
    ) -> Result<RunnableConfig, CoreCheckpointError> {
        // Create tracing span for checkpoint put operation
        let span = tracing::info_span!(
            target: "juncture",
            names::CHECKPOINT_PUT,
            "juncture.checkpoint.id" = %checkpoint.id,
            "juncture.checkpoint.source" = ?metadata.source,
            "juncture.checkpoint.step" = metadata.step,
        );

        async move {
            let thread_id = Self::get_thread_id(config).map_checkpoint()?;
            let checkpoint_ns = Self::get_checkpoint_ns(config);
            let checkpoint_id = checkpoint.id.clone();
            let source = metadata.source.clone();

            // Create checkpoint tuple
            let tuple = CheckpointTuple {
                config: config.clone(),
                checkpoint,
                metadata,
                pending_writes: Vec::new(),
                parent_config: None,
            };

            // Store checkpoint by cloning, modifying, and replacing
            // This approach avoids holding the write lock for too long
            let mut storage = self.storage.write().await;
            let thread_map = storage
                .entry(thread_id.clone())
                .or_insert_with(HashMap::new);
            let namespace = thread_map
                .entry(checkpoint_ns.clone())
                .or_insert_with(Vec::new);

            namespace.push(tuple);

            // Keep sorted by creation time descending
            Self::sort_checkpoints(namespace);
            drop(storage);

            // Emit metrics for checkpoint write
            tracing::debug!(
                name: "juncture.checkpoint.writes",
                source = ?source,
            );

            // Return updated config with checkpoint_id
            let mut result_config = config.clone();
            result_config.checkpoint_id = Some(checkpoint_id);

            Ok(result_config)
        }
        .instrument(span)
        .await
    }

    async fn put_writes(
        &self,
        config: &RunnableConfig,
        writes: Vec<PendingWrite>,
        task_id: &str,
    ) -> Result<(), CoreCheckpointError> {
        let checkpoint_id_for_span = config.checkpoint_id.clone().unwrap_or_default();

        // Create tracing span for checkpoint put_writes operation
        let span = tracing::info_span!(
            target: "juncture",
            "juncture.checkpoint.put_writes",
            "juncture.checkpoint.id" = %checkpoint_id_for_span,
            "juncture.checkpoint.task_id" = %task_id,
            "juncture.checkpoint.writes_count" = writes.len(),
        );

        async move {
            let thread_id = Self::get_thread_id(config).map_checkpoint()?;
            let checkpoint_ns = Self::get_checkpoint_ns(config);
            let checkpoint_id = config.checkpoint_id.clone().ok_or_else(|| {
                CoreCheckpointError::Storage("checkpoint_id is required".to_string())
            })?;

            let key = (thread_id, checkpoint_id, checkpoint_ns);

            // Prepare the writes with task_id set
            let prepared_writes: Vec<PendingWrite> = writes
                .into_iter()
                .map(|mut w| {
                    w.task_id = task_id.to_string();
                    w
                })
                .collect();

            // Insert the prepared writes in a single statement to minimize lock time
            // We chain the operations to avoid storing the lock guard
            self.writes
                .write()
                .await
                .entry(key)
                .or_insert_with(Vec::new)
                .extend(prepared_writes);

            Ok(())
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use juncture_core::checkpoint::CheckpointSource;
    use serde_json::json;

    fn create_test_checkpoint(id: &str, _step: i64) -> Checkpoint {
        Checkpoint {
            id: id.to_string(),
            channel_values: json!({}),
            channel_versions: HashMap::new(),
            versions_seen: HashMap::new(),
            pending_tasks: vec![],
            pending_sends: vec![],
            pending_interrupts: vec![],
            schema_version: 1,
            created_at: Utc::now().to_rfc3339(),
            v: 1,
            new_versions: HashMap::new(),
            counters_since_delta_snapshot: HashMap::new(),
        }
    }

    fn create_test_metadata(source: CheckpointSource, step: i64) -> CheckpointMetadata {
        CheckpointMetadata {
            source,
            step,
            writes: HashMap::new(),
            parents: HashMap::new(),
            run_id: "test-run".to_string(),
        }
    }

    fn create_test_config(thread_id: &str) -> RunnableConfig {
        RunnableConfig::default().with_thread_id(thread_id)
    }

    #[tokio::test]
    async fn test_memory_saver_put_get() {
        let saver = MemorySaver::new();
        let config = create_test_config("thread1");
        let checkpoint = create_test_checkpoint("cp1", 0);
        let metadata = create_test_metadata(CheckpointSource::Input, 0);

        let result_config = saver
            .put(&config, checkpoint.clone(), metadata)
            .await
            .unwrap();

        assert_eq!(result_config.checkpoint_id, Some("cp1".to_string()));

        let retrieved = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(retrieved.checkpoint.id, "cp1");
    }

    #[tokio::test]
    async fn test_memory_saver_get_latest() {
        let saver = MemorySaver::new();
        let config = create_test_config("thread1");

        // Add multiple checkpoints
        for i in 0..3 {
            let checkpoint = create_test_checkpoint(&format!("cp{i}"), i);
            let metadata = create_test_metadata(CheckpointSource::Loop, i);
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // Get latest (without checkpoint_id)
        let latest = saver.get_tuple(&config).await.unwrap().unwrap();
        assert_eq!(latest.checkpoint.id, "cp2"); // Last one added
    }

    #[tokio::test]
    async fn test_memory_saver_list() {
        let saver = MemorySaver::new();
        let config = create_test_config("thread1");

        // Add checkpoints
        for i in 0..5 {
            let checkpoint = create_test_checkpoint(&format!("cp{i}"), i);
            let metadata = create_test_metadata(CheckpointSource::Loop, i);
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // List all
        let all = saver.list(&config, None).await.unwrap();
        assert_eq!(all.len(), 5);

        // List with limit
        let limited = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    limit: Some(3),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(limited.len(), 3);

        // List with step filter
        let filtered = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    step_gte: Some(2),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(filtered.len(), 3); // steps 2, 3, 4
    }

    #[tokio::test]
    async fn test_memory_saver_put_writes() {
        let saver = MemorySaver::new();
        let config = create_test_config("thread1");
        let checkpoint = create_test_checkpoint("cp1", 0);
        let metadata = create_test_metadata(CheckpointSource::Input, 0);

        let result_config = saver.put(&config, checkpoint, metadata).await.unwrap();

        // Add writes
        let writes = vec![PendingWrite {
            task_id: String::new(),
            channel: "messages".to_string(),
            value: json!("hello"),
        }];

        saver
            .put_writes(&result_config, writes, "task1")
            .await
            .unwrap();

        // Retrieve with writes
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.pending_writes.len(), 1);
        assert_eq!(tuple.pending_writes[0].channel, "messages");
        assert_eq!(tuple.pending_writes[0].task_id, "task1");
    }

    #[tokio::test]
    async fn test_memory_saver_namespace_isolation() {
        let saver = MemorySaver::new();

        let config_ns1 = RunnableConfig::default()
            .with_thread_id("thread1")
            .with_checkpoint_ns(juncture_core::checkpoint::CheckpointNamespace::parse("ns1"));
        let config_ns2 = RunnableConfig::default()
            .with_thread_id("thread1")
            .with_checkpoint_ns(juncture_core::checkpoint::CheckpointNamespace::parse("ns2"));

        let checkpoint1 = create_test_checkpoint("cp1", 0);
        let checkpoint2 = create_test_checkpoint("cp2", 0);
        let metadata = create_test_metadata(CheckpointSource::Input, 0);

        saver
            .put(&config_ns1, checkpoint1, metadata.clone())
            .await
            .unwrap();
        saver.put(&config_ns2, checkpoint2, metadata).await.unwrap();

        // Should not find cp1 in ns2
        let result = saver.get_tuple(&config_ns2).await.unwrap().unwrap();
        assert_eq!(result.checkpoint.id, "cp2");
    }

    #[tokio::test]
    async fn test_memory_saver_thread_isolation() {
        let saver = MemorySaver::new();

        let config_t1 = RunnableConfig::default().with_thread_id("thread1");
        let config_t2 = RunnableConfig::default().with_thread_id("thread2");

        let checkpoint1 = create_test_checkpoint("cp1", 0);
        let checkpoint2 = create_test_checkpoint("cp2", 0);
        let metadata = create_test_metadata(CheckpointSource::Input, 0);

        saver
            .put(&config_t1, checkpoint1, metadata.clone())
            .await
            .unwrap();
        saver.put(&config_t2, checkpoint2, metadata).await.unwrap();

        // Each thread should only see its own checkpoints
        let result1 = saver.get_tuple(&config_t1).await.unwrap().unwrap();
        assert_eq!(result1.checkpoint.id, "cp1");

        let result2 = saver.get_tuple(&config_t2).await.unwrap().unwrap();
        assert_eq!(result2.checkpoint.id, "cp2");
    }

    #[tokio::test]
    async fn test_memory_saver_not_found() {
        let saver = MemorySaver::new();
        let config = RunnableConfig::default()
            .with_thread_id("nonexistent")
            .with_checkpoint_id("missing");

        let result = saver.get_tuple(&config).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_memory_saver_filter_by_source() {
        let saver = MemorySaver::new();
        let config = create_test_config("thread1");

        // Add checkpoints with different sources
        let cp_input = create_test_checkpoint("cp1", 0);
        let meta_input = create_test_metadata(CheckpointSource::Input, 0);
        saver.put(&config, cp_input, meta_input).await.unwrap();

        let cp_loop = create_test_checkpoint("cp2", 1);
        let meta_loop = create_test_metadata(CheckpointSource::Loop, 1);
        saver.put(&config, cp_loop, meta_loop).await.unwrap();

        // Filter by Loop source
        let filtered = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    source: Some(CheckpointSource::Loop),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        assert_eq!(filtered.len(), 1);
        assert!(matches!(
            filtered[0].metadata.source,
            CheckpointSource::Loop
        ));
    }

    #[tokio::test]
    async fn test_memory_saver_clone() {
        let saver = MemorySaver::new();
        let cloned = saver.clone();

        let config = create_test_config("thread1");
        let checkpoint = create_test_checkpoint("cp1", 0);
        let metadata = create_test_metadata(CheckpointSource::Input, 0);

        // Use original
        saver
            .put(&config, checkpoint.clone(), metadata.clone())
            .await
            .unwrap();

        // Use cloned - should see same data
        let result = cloned.get_tuple(&config).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().checkpoint.id, "cp1");
    }

    #[tokio::test]
    async fn test_memory_saver_ttl_expiration() {
        use crate::types::TtlConfig;
        use std::time::Duration;

        let saver = MemorySaver::new().with_ttl_config(TtlConfig {
            default_ttl: Some(Duration::from_millis(100)), // Very short TTL for testing
            sweep_interval: Duration::from_secs(3600),
            max_checkpoints: None,
        });

        let config = create_test_config("thread1");

        // Add checkpoints
        for i in 0..3 {
            let checkpoint = create_test_checkpoint(&format!("cp{i}"), i);
            let metadata = create_test_metadata(CheckpointSource::Loop, i);
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // Should have 3 checkpoints initially
        let list = saver.list(&config, None).await.unwrap();
        assert_eq!(list.len(), 3);

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Trigger lazy cleanup via get_tuple (M04-001)
        let result = saver.get_tuple(&config).await.unwrap();

        // All checkpoints should be expired and cleaned up
        assert!(result.is_none());

        // List should also be empty after lazy cleanup
        let list = saver.list(&config, None).await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_memory_saver_max_checkpoints() {
        use crate::types::TtlConfig;

        let saver = MemorySaver::new().with_ttl_config(TtlConfig {
            default_ttl: None,
            sweep_interval: std::time::Duration::from_secs(3600),
            max_checkpoints: Some(2), // Keep only 2 most recent
        });

        let config = create_test_config("thread1");

        // Add 5 checkpoints
        for i in 0..5 {
            let checkpoint = create_test_checkpoint(&format!("cp{i}"), i);
            let metadata = create_test_metadata(CheckpointSource::Loop, i);
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // Trigger lazy cleanup via list (M04-001)
        let list = saver.list(&config, None).await.unwrap();

        // Should only keep 2 most recent (cp3, cp4)
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].checkpoint.id, "cp4"); // Most recent
        assert_eq!(list[1].checkpoint.id, "cp3"); // Second most recent
    }
}

// Rust guideline compliant 2026-05-23
