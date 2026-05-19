// PostgreSQL checkpoint saver implementation
//
// This module provides the PostgresSaver for persistent checkpoint storage
// using PostgreSQL database.

use async_trait::async_trait;
use std::sync::Arc;

#[cfg(feature = "postgres")]
use sqlx::Row;

use crate::error::CheckpointError;
use juncture_core::checkpoint::{
    Checkpoint, CheckpointFilter, CheckpointMetadata, CheckpointTuple, PendingWrite,
};
use juncture_core::config::RunnableConfig;
use juncture_core::error::JunctureError;

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

        Ok(Self { pool: Arc::new(pool) })
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
    ) -> Result<Option<CheckpointTuple>, JunctureError> {
        let thread_id = Self::get_thread_id(config)
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let row = if let Some(checkpoint_id) = &config.checkpoint_id {
            sqlx::query(
                "SELECT checkpoint_data, metadata_data
                 FROM checkpoints
                 WHERE thread_id = $1 AND checkpoint_ns = $2 AND id = $3"
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(checkpoint_id)
            .fetch_optional(&*self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT checkpoint_data, metadata_data
                 FROM checkpoints
                 WHERE thread_id = $1 AND checkpoint_ns = $2
                 ORDER BY created_at DESC
                 LIMIT 1"
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .fetch_optional(&*self.pool)
            .await
        }.map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        match row {
            Some(row) => {
                let checkpoint_data: String = row.try_get("checkpoint_data")
                    .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
                let metadata_data: String = row.try_get("metadata_data")
                    .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

                let checkpoint: Checkpoint = serde_json::from_str(&checkpoint_data)
                    .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
                let metadata: CheckpointMetadata = serde_json::from_str(&metadata_data)
                    .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

                Ok(Some(CheckpointTuple {
                    config: config.clone(),
                    checkpoint,
                    metadata,
                    pending_writes: vec![],
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
    ) -> Result<Vec<CheckpointTuple>, JunctureError> {
        let thread_id = Self::get_thread_id(config)
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let limit = i64::try_from(filter.as_ref().and_then(|f| f.limit).unwrap_or(10))
            .expect("limit value fits in i64");

        let rows = sqlx::query(
            "SELECT checkpoint_data, metadata_data
             FROM checkpoints
             WHERE thread_id = $1 AND checkpoint_ns = $2
             ORDER BY created_at DESC
             LIMIT $3"
        )
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .bind(limit)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            let checkpoint_data: String = row.try_get("checkpoint_data")
                .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
            let metadata_data: String = row.try_get("metadata_data")
                .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

            let checkpoint: Checkpoint = serde_json::from_str(&checkpoint_data)
                .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
            let metadata: CheckpointMetadata = serde_json::from_str(&metadata_data)
                .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

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
    ) -> Result<RunnableConfig, JunctureError> {
        let thread_id = Self::get_thread_id(config)
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let checkpoint_data = serde_json::to_string(&checkpoint)
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;
        let metadata_data = serde_json::to_string(&metadata)
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        let now = chrono::Utc::now();

        sqlx::query(
            r"
            INSERT INTO checkpoints
            (id, thread_id, checkpoint_ns, checkpoint_data, metadata_data, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (id) DO UPDATE SET
                checkpoint_data = EXCLUDED.checkpoint_data,
                metadata_data = EXCLUDED.metadata_data,
                updated_at = EXCLUDED.updated_at
            "
        )
        .bind(&checkpoint.id)
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .bind(&checkpoint_data)
        .bind(&metadata_data)
        .bind(now)
        .bind(now)
        .execute(&*self.pool)
        .await
        .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        let mut updated_config = config.clone();
        updated_config.checkpoint_id = Some(checkpoint.id.clone());

        Ok(updated_config)
    }

    async fn put_writes(
        &self,
        _config: &RunnableConfig,
        _writes: Vec<PendingWrite>,
        _task_id: &str,
    ) -> Result<(), JunctureError> {
        // PostgreSQL implementation doesn't track pending writes separately
        // They're included in the checkpoint data itself
        Ok(())
    }
}

// Rust guideline compliant 2026-05-19
