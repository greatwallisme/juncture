// PostgreSQL checkpoint saver implementation
//
// This module provides the PostgresSaver for persistent checkpoint storage
// using PostgreSQL database.

use async_trait::async_trait;
use std::sync::Arc;

#[cfg(feature = "postgres")]
use sqlx::Row;

use juncture_core::checkpoint::{
    Checkpoint, CheckpointError as CoreCheckpointError, CheckpointFilter, CheckpointMetadata,
    CheckpointTuple, PendingWrite,
};
use juncture_core::config::RunnableConfig;

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

/// `PostgreSQL` checkpoint saver
///
/// Stores checkpoints in a `PostgreSQL` database for persistence.
#[derive(Clone)]
pub struct PostgresSaver {
    /// Database connection pool
    #[cfg(feature = "postgres")]
    pool: Arc<sqlx::PgPool>,
}

#[cfg(feature = "postgres")]
impl std::fmt::Debug for PostgresSaver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresSaver")
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(feature = "postgres")]
impl PostgresSaver {
    /// Create new `PostgreSQL` saver
    ///
    /// # Arguments
    ///
    /// * `connection_string` - `PostgreSQL` connection string
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection fails or migrations fail.
    pub async fn new(connection_string: &str) -> Result<Self, CheckpointError> {
        let pool = sqlx::PgPool::connect(connection_string)
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                checkpoint_ns TEXT NOT NULL,
                checkpoint_data TEXT NOT NULL,
                metadata_data TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL
            )
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Create checkpoint_writes table for pending writes
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS checkpoint_writes (
                thread_id TEXT NOT NULL,
                checkpoint_ns TEXT NOT NULL,
                checkpoint_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                channel TEXT NOT NULL,
                value JSONB NOT NULL,
                idx INTEGER NOT NULL,
                PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
            )
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Create indexes
        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_thread_ns
            ON checkpoints (thread_id, checkpoint_ns, created_at DESC)
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_checkpoint_writes_lookup
            ON checkpoint_writes (thread_id, checkpoint_ns, checkpoint_id)
            ",
        )
        .execute(&pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    /// Get thread ID from config, returning error if not set
    fn get_thread_id(config: &RunnableConfig) -> Result<String, CheckpointError> {
        config
            .thread_id
            .clone()
            .ok_or_else(|| CheckpointError::Storage("thread_id is required".to_string()))
    }

    /// Get checkpoint namespace from config, defaulting to empty string
    fn get_checkpoint_ns(config: &RunnableConfig) -> String {
        config.checkpoint_ns.as_deref().unwrap_or("").to_string()
    }
}

#[async_trait]
#[cfg(feature = "postgres")]
impl juncture_core::checkpoint::CheckpointSaver for PostgresSaver {
    async fn get_tuple(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<CheckpointTuple>, CoreCheckpointError> {
        let thread_id =
            Self::get_thread_id(config).map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let row = if let Some(checkpoint_id) = &config.checkpoint_id {
            sqlx::query(
                "SELECT checkpoint_data, metadata_data, id as checkpoint_id
                 FROM checkpoints
                 WHERE thread_id = $1 AND checkpoint_ns = $2 AND id = $3",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(checkpoint_id)
            .fetch_optional(&*self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT checkpoint_data, metadata_data, id as checkpoint_id
                 FROM checkpoints
                 WHERE thread_id = $1 AND checkpoint_ns = $2
                 ORDER BY created_at DESC
                 LIMIT 1",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .fetch_optional(&*self.pool)
            .await
        }
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        match row {
            Some(row) => {
                let checkpoint_data: String = row
                    .try_get("checkpoint_data")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let metadata_data: String = row
                    .try_get("metadata_data")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let checkpoint_id: String = row
                    .try_get("checkpoint_id")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                let checkpoint: Checkpoint = serde_json::from_str(&checkpoint_data)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let metadata: CheckpointMetadata = serde_json::from_str(&metadata_data)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                // Load pending writes for this checkpoint
                let write_rows = sqlx::query(
                    "SELECT task_id, channel, value
                     FROM checkpoint_writes
                     WHERE thread_id = $1 AND checkpoint_ns = $2 AND checkpoint_id = $3
                     ORDER BY task_id, idx",
                )
                .bind(&thread_id)
                .bind(&checkpoint_ns)
                .bind(&checkpoint_id)
                .fetch_all(&*self.pool)
                .await
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                let pending_writes: Vec<PendingWrite> = write_rows
                    .into_iter()
                    .map(|row| {
                        let task_id: String = row
                            .try_get("task_id")
                            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                        let channel: String = row
                            .try_get("channel")
                            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                        let value: serde_json::Value = row
                            .try_get("value")
                            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                        Ok(PendingWrite {
                            task_id,
                            channel,
                            value,
                        })
                    })
                    .collect::<Result<Vec<_>, CoreCheckpointError>>()?;

                Ok(Some(CheckpointTuple {
                    config: config.clone(),
                    checkpoint,
                    metadata,
                    pending_writes,
                    parent_config: None,
                }))
            }
            None => Ok(None),
        }
    }

    async fn list(
        &self,
        config: &RunnableConfig,
        filter: Option<CheckpointFilter>,
    ) -> Result<Vec<CheckpointTuple>, CoreCheckpointError> {
        let thread_id =
            Self::get_thread_id(config).map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let limit = i64::try_from(filter.as_ref().and_then(|f| f.limit).unwrap_or(10))
            .expect("limit value fits in i64");

        let rows = sqlx::query(
            "SELECT checkpoint_data, metadata_data
             FROM checkpoints
             WHERE thread_id = $1 AND checkpoint_ns = $2
             ORDER BY created_at DESC
             LIMIT $3",
        )
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .bind(limit)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            let checkpoint_data: String = row
                .try_get("checkpoint_data")
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
            let metadata_data: String = row
                .try_get("metadata_data")
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

            let checkpoint: Checkpoint = serde_json::from_str(&checkpoint_data)
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
            let metadata: CheckpointMetadata = serde_json::from_str(&metadata_data)
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

            results.push(CheckpointTuple {
                config: config.clone(),
                checkpoint,
                metadata,
                pending_writes: vec![],
                parent_config: None,
            });
        }

        Ok(results)
    }

    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
    ) -> Result<RunnableConfig, CoreCheckpointError> {
        let thread_id =
            Self::get_thread_id(config).map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let checkpoint_data = serde_json::to_string(&checkpoint)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let metadata_data = serde_json::to_string(&metadata)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let now = chrono::Utc::now().to_rfc3339();

        // Begin transaction for checkpoint save and write cleanup
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Insert or update checkpoint
        sqlx::query(
            r"
            INSERT INTO checkpoints
            (id, thread_id, checkpoint_ns, checkpoint_data, metadata_data, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (id) DO UPDATE SET
                checkpoint_data = EXCLUDED.checkpoint_data,
                metadata_data = EXCLUDED.metadata_data,
                updated_at = EXCLUDED.updated_at
            ",
        )
        .bind(&checkpoint.id)
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .bind(&checkpoint_data)
        .bind(&metadata_data)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Clean up old writes for this thread/namespace (crash recovery)
        // When a new checkpoint is saved, all previous pending writes are obsolete
        sqlx::query(
            "DELETE FROM checkpoint_writes
             WHERE thread_id = $1 AND checkpoint_ns = $2",
        )
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .execute(&mut *tx)
        .await
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let mut updated_config = config.clone();
        updated_config.checkpoint_id = Some(checkpoint.id.clone());

        Ok(updated_config)
    }

    async fn put_writes(
        &self,
        config: &RunnableConfig,
        writes: Vec<PendingWrite>,
        task_id: &str,
    ) -> Result<(), CoreCheckpointError> {
        let thread_id =
            Self::get_thread_id(config).map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);
        let checkpoint_id = config
            .checkpoint_id
            .clone()
            .ok_or_else(|| CoreCheckpointError::Storage("checkpoint_id is required".to_string()))?;

        // Begin transaction for atomic write insertion
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Insert each write with its index
        for (idx, write) in writes.into_iter().enumerate() {
            sqlx::query(
                "INSERT INTO checkpoint_writes
                 (thread_id, checkpoint_ns, checkpoint_id, task_id, channel, value, idx)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
                 DO UPDATE SET
                     channel = EXCLUDED.channel,
                     value = EXCLUDED.value",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(&checkpoint_id)
            .bind(task_id)
            .bind(&write.channel)
            .bind(&write.value)
            .bind(i64::try_from(idx).expect("idx fits in i64"))
            .execute(&mut *tx)
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use juncture_core::checkpoint::{CheckpointSaver, CheckpointSource};
    use serde_json::json;

    fn create_test_checkpoint(id: &str) -> Checkpoint {
        Checkpoint {
            id: id.to_string(),
            channel_values: json!({}),
            channel_versions: std::collections::HashMap::new(),
            versions_seen: std::collections::HashMap::new(),
            pending_tasks: vec![],
            pending_sends: vec![],
            schema_version: 1,
            created_at: Utc::now().to_rfc3339(),
            v: 1,
            new_versions: std::collections::HashMap::new(),
            counters_since_delta_snapshot: std::collections::HashMap::new(),
        }
    }

    fn create_test_metadata() -> CheckpointMetadata {
        CheckpointMetadata {
            source: CheckpointSource::Input,
            step: 0,
            writes: std::collections::HashMap::new(),
            parents: std::collections::HashMap::new(),
            run_id: "test-run".to_string(),
        }
    }

    fn create_test_config(thread_id: &str) -> RunnableConfig {
        RunnableConfig::default().with_thread_id(thread_id)
    }

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn test_postgres_saver_put_writes() {
        // Skip test if PostgreSQL is not available
        let conn_str = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5432/test".to_string());

        let Ok(saver) = PostgresSaver::new(&conn_str).await else {
            return;
        };

        let config = create_test_config("thread1");
        let checkpoint = create_test_checkpoint("cp1");
        let metadata = create_test_metadata();

        let result_config = saver.put(&config, checkpoint, metadata).await.unwrap();

        // Add writes
        let writes = vec![
            PendingWrite {
                task_id: String::new(),
                channel: "messages".to_string(),
                value: json!("hello"),
            },
            PendingWrite {
                task_id: String::new(),
                channel: "messages".to_string(),
                value: json!("world"),
            },
        ];

        saver
            .put_writes(&result_config, writes, "task1")
            .await
            .unwrap();

        // Retrieve with writes
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.pending_writes.len(), 2);
        assert_eq!(tuple.pending_writes[0].channel, "messages");
        assert_eq!(tuple.pending_writes[0].task_id, "task1");
        assert_eq!(tuple.pending_writes[0].value, json!("hello"));
        assert_eq!(tuple.pending_writes[1].value, json!("world"));

        // Clean up
        let _ = sqlx::query("DELETE FROM checkpoints WHERE thread_id = $1")
            .bind(&result_config.thread_id)
            .execute(&*saver.pool)
            .await;
        let _ = sqlx::query("DELETE FROM checkpoint_writes WHERE thread_id = $1")
            .bind(&result_config.thread_id)
            .execute(&*saver.pool)
            .await;
    }

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn test_postgres_saver_put_writes_persistence() {
        let conn_str = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5432/test".to_string());

        let Ok(saver) = PostgresSaver::new(&conn_str).await else {
            return;
        };

        let config = create_test_config("thread2");
        let checkpoint = create_test_checkpoint("cp2");
        let metadata = create_test_metadata();

        let result_config = saver.put(&config, checkpoint, metadata).await.unwrap();

        // Add writes
        let writes = vec![PendingWrite {
            task_id: String::new(),
            channel: "messages".to_string(),
            value: json!("persistent"),
        }];

        saver
            .put_writes(&result_config, writes, "task1")
            .await
            .unwrap();

        // Retrieve in a new operation
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.pending_writes.len(), 1);
        assert_eq!(tuple.pending_writes[0].value, json!("persistent"));

        // Clean up
        let _ = sqlx::query("DELETE FROM checkpoints WHERE thread_id = $1")
            .bind(&result_config.thread_id)
            .execute(&*saver.pool)
            .await;
        let _ = sqlx::query("DELETE FROM checkpoint_writes WHERE thread_id = $1")
            .bind(&result_config.thread_id)
            .execute(&*saver.pool)
            .await;
    }

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn test_postgres_saver_put_cleans_old_writes() {
        let conn_str = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5432/test".to_string());

        let Ok(saver) = PostgresSaver::new(&conn_str).await else {
            return;
        };

        let config = create_test_config("thread3");

        // Create first checkpoint with writes
        let checkpoint1 = create_test_checkpoint("cp1");
        let metadata = create_test_metadata();
        let result_config1 = saver.put(&config, checkpoint1, metadata).await.unwrap();

        let writes1 = vec![PendingWrite {
            task_id: String::new(),
            channel: "messages".to_string(),
            value: json!("old"),
        }];

        saver
            .put_writes(&result_config1, writes1, "task1")
            .await
            .unwrap();

        // Verify writes exist
        let tuple1 = saver.get_tuple(&result_config1).await.unwrap().unwrap();
        assert_eq!(tuple1.pending_writes.len(), 1);

        // Create new checkpoint (should clean up old writes)
        let checkpoint2 = create_test_checkpoint("cp2");
        let metadata2 = create_test_metadata();
        saver.put(&config, checkpoint2, metadata2).await.unwrap();

        // Old checkpoint should no longer have pending writes
        let tuple_after = saver.get_tuple(&result_config1).await.unwrap().unwrap();
        assert_eq!(tuple_after.pending_writes.len(), 0);

        // Clean up
        let _ = sqlx::query("DELETE FROM checkpoints WHERE thread_id = $1")
            .bind(&result_config1.thread_id)
            .execute(&*saver.pool)
            .await;
        let _ = sqlx::query("DELETE FROM checkpoint_writes WHERE thread_id = $1")
            .bind(&result_config1.thread_id)
            .execute(&*saver.pool)
            .await;
    }
}

// Rust guideline compliant 2026-05-19
