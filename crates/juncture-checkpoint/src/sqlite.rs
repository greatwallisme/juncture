// SQLite checkpoint saver implementation
//
// This module provides the SqliteSaver for persistent checkpoint storage
// using SQLite database.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "sqlite")]
use sqlx::Row;

use juncture_core::checkpoint::{
    Checkpoint, CheckpointError as CoreCheckpointError, CheckpointFilter, CheckpointMetadata,
    CheckpointPendingTask, CheckpointTuple, PendingWrite, SerializedSend,
};
use juncture_core::config::RunnableConfig;
use juncture_core::interrupt::InterruptSignal;

use crate::error::CheckpointError;
use crate::serde::{SerializerKind, deserialize_auto};

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

/// `SQLite` checkpoint saver
///
/// Stores checkpoints in a `SQLite` database for persistence.
/// Uses `MessagePack` serialization by default for high-performance binary storage.
#[derive(Clone)]
pub struct SqliteSaver {
    /// Database connection pool
    #[cfg(feature = "sqlite")]
    pool: Arc<sqlx::sqlite::SqlitePool>,
    /// Database file path
    #[cfg(feature = "sqlite")]
    #[allow(dead_code, reason = "Path stored for debugging and future use")]
    db_path: PathBuf,
    /// Serializer for checkpoint data fields
    serializer: SerializerKind,
}

#[cfg(feature = "sqlite")]
impl std::fmt::Debug for SqliteSaver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSaver")
            .field("pool", &self.pool)
            .field("db_path", &self.db_path)
            .field("serializer", &self.serializer)
            .finish()
    }
}

#[cfg(feature = "sqlite")]
impl SqliteSaver {
    /// Create new `SQLite` saver
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to `SQLite` database file
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection fails or migrations fail.
    pub async fn new(db_path: impl Into<PathBuf>) -> Result<Self, CheckpointError> {
        let path = db_path.into();
        let pool = sqlx::sqlite::SqlitePool::connect(&format!("sqlite:{}", path.display()))
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS checkpoints (
                thread_id TEXT NOT NULL,
                checkpoint_ns TEXT NOT NULL DEFAULT '',
                checkpoint_id TEXT NOT NULL,
                parent_checkpoint_id TEXT,
                channel_values BLOB NOT NULL,
                channel_versions BLOB NOT NULL,
                versions_seen BLOB NOT NULL,
                pending_tasks BLOB,
                pending_sends BLOB,
                pending_interrupts BLOB,
                schema_version INTEGER NOT NULL DEFAULT 1,
                metadata BLOB NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
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
                value TEXT NOT NULL,
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
            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_time
                ON checkpoints(thread_id, checkpoint_ns, created_at DESC)
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

        // Add pending_interrupts column for databases created before this field existed.
        // SQLite does not support IF NOT EXISTS for ALTER TABLE ADD COLUMN, so we
        // must catch and ignore the "duplicate column name" error.
        let alter_result = sqlx::query("ALTER TABLE checkpoints ADD COLUMN pending_interrupts BLOB")
            .execute(&pool)
            .await;
        match alter_result {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(CheckpointError::Database(e.to_string())),
        }

        Ok(Self {
            pool: Arc::new(pool),
            db_path: path,
            serializer: SerializerKind::default(),
        })
    }

    /// Create new `SQLite` saver from connection string
    ///
    /// # Arguments
    ///
    /// * `connection_string` - `SQLite` connection string
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection fails or migrations fail.
    pub async fn from_connection_string(connection_string: &str) -> Result<Self, CheckpointError> {
        let pool = sqlx::sqlite::SqlitePool::connect(connection_string)
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Run migrations
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS checkpoints (
                thread_id TEXT NOT NULL,
                checkpoint_ns TEXT NOT NULL DEFAULT '',
                checkpoint_id TEXT NOT NULL,
                parent_checkpoint_id TEXT,
                channel_values BLOB NOT NULL,
                channel_versions BLOB NOT NULL,
                versions_seen BLOB NOT NULL,
                pending_tasks BLOB,
                pending_sends BLOB,
                pending_interrupts BLOB,
                schema_version INTEGER NOT NULL DEFAULT 1,
                metadata BLOB NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
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
                value TEXT NOT NULL,
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
            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_time
                ON checkpoints(thread_id, checkpoint_ns, created_at DESC)
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

        // Add pending_interrupts column for databases created before this field existed.
        // SQLite does not support IF NOT EXISTS for ALTER TABLE ADD COLUMN, so we
        // must catch and ignore the "duplicate column name" error.
        let alter_result = sqlx::query("ALTER TABLE checkpoints ADD COLUMN pending_interrupts BLOB")
            .execute(&pool)
            .await;
        match alter_result {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(CheckpointError::Database(e.to_string())),
        }

        Ok(Self {
            pool: Arc::new(pool),
            db_path: PathBuf::from(":memory:"),
            serializer: SerializerKind::default(),
        })
    }

    /// Create a `SqliteSaver` with a custom serializer
    ///
    /// Allows overriding the default `MessagePack` serializer with a custom
    /// format (e.g., `SerializerKind::Json` for debugging).
    #[must_use]
    pub const fn with_serializer(mut self, serializer: SerializerKind) -> Self {
        self.serializer = serializer;
        self
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

    /// Deserialize checkpoint from database row fields
    ///
    /// Helper function to reconstruct a Checkpoint from individual column values
    /// as per design specification (section 4.2).
    #[allow(clippy::too_many_arguments, reason = "required by database schema")]
    fn deserialize_checkpoint(
        channel_values_bytes: &[u8],
        channel_versions_bytes: &[u8],
        versions_seen_bytes: &[u8],
        pending_tasks_bytes: Option<&[u8]>,
        pending_sends_bytes: Option<&[u8]>,
        pending_interrupts_bytes: Option<&[u8]>,
        schema_version: i64,
        checkpoint_id: String,
        created_at: String,
    ) -> Result<Checkpoint, CoreCheckpointError> {
        let channel_values: serde_json::Value = deserialize_auto(channel_values_bytes)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let channel_versions: std::collections::HashMap<String, u64> =
            deserialize_auto(channel_versions_bytes)
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let versions_seen: std::collections::HashMap<
            String,
            std::collections::HashMap<String, u64>,
        > = deserialize_auto(versions_seen_bytes)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_tasks: Vec<CheckpointPendingTask> = pending_tasks_bytes
            .map(|bytes| {
                deserialize_auto::<Vec<CheckpointPendingTask>>(bytes)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))
            })
            .transpose()?
            .unwrap_or_default();
        let pending_sends: Vec<SerializedSend> = pending_sends_bytes
            .map(|bytes| {
                deserialize_auto::<Vec<SerializedSend>>(bytes)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))
            })
            .transpose()?
            .unwrap_or_default();
        let pending_interrupts: Vec<InterruptSignal> = pending_interrupts_bytes
            .map(|bytes| {
                deserialize_auto::<Vec<InterruptSignal>>(bytes)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))
            })
            .transpose()?
            .unwrap_or_default();

        Ok(Checkpoint {
            id: checkpoint_id,
            channel_values,
            channel_versions,
            versions_seen,
            pending_tasks,
            pending_sends,
            pending_interrupts,
            schema_version: u32::try_from(schema_version).expect("schema_version fits in u32"),
            created_at,
            v: 1,
            new_versions: std::collections::HashMap::new(),
            counters_since_delta_snapshot: std::collections::HashMap::new(),
        })
    }

    /// Deserialize a single database row into a `CheckpointTuple`
    ///
    /// Extracts raw bytes from each column, deserializes checkpoint and metadata,
    /// and assembles a complete `CheckpointTuple` without pending writes.
    fn row_to_tuple(
        row: &sqlx::sqlite::SqliteRow,
        config: &RunnableConfig,
    ) -> Result<CheckpointTuple, CoreCheckpointError> {
        let channel_values_bytes: Vec<u8> = row
            .try_get("channel_values")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let channel_versions_bytes: Vec<u8> = row
            .try_get("channel_versions")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let versions_seen_bytes: Vec<u8> = row
            .try_get("versions_seen")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_tasks_bytes: Option<Vec<u8>> = row
            .try_get("pending_tasks")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_sends_bytes: Option<Vec<u8>> = row
            .try_get("pending_sends")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_interrupts_bytes: Option<Vec<u8>> = row
            .try_get("pending_interrupts")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let schema_version: i64 = row
            .try_get("schema_version")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let metadata_bytes: Vec<u8> = row
            .try_get("metadata")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_id: String = row
            .try_get("checkpoint_id")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let created_at: String = row
            .try_get("created_at")
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let checkpoint = Self::deserialize_checkpoint(
            &channel_values_bytes,
            &channel_versions_bytes,
            &versions_seen_bytes,
            pending_tasks_bytes.as_deref(),
            pending_sends_bytes.as_deref(),
            pending_interrupts_bytes.as_deref(),
            schema_version,
            checkpoint_id,
            created_at,
        )
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let metadata: CheckpointMetadata = deserialize_auto(&metadata_bytes)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        Ok(CheckpointTuple {
            config: config.clone(),
            checkpoint,
            metadata,
            pending_writes: vec![],
            parent_config: None,
        })
    }

    /// Apply `CheckpointFilter` to a list of deserialized tuples
    ///
    /// Filters by source, step range, and `checkpoint_id` position (before/after),
    /// then applies the final limit. This runs in Rust because the metadata fields
    /// (source, step) are stored as serialized BLOBs that cannot be filtered at
    /// the SQL level.
    fn apply_list_filter(
        tuples: Vec<CheckpointTuple>,
        filter: &CheckpointFilter,
    ) -> Vec<CheckpointTuple> {
        let mut results = tuples;

        if let Some(source) = &filter.source {
            results.retain(|t| t.metadata.source == *source);
        }
        if let Some(min_step) = filter.step_gte {
            results.retain(|t| t.metadata.step >= min_step);
        }
        if let Some(max_step) = filter.step_lte {
            results.retain(|t| t.metadata.step <= max_step);
        }
        // before: only checkpoints newer than (before) the given id
        if let Some(before_id) = &filter.before {
            let before_pos = results.iter().position(|t| t.checkpoint.id == *before_id);
            if let Some(pos) = before_pos {
                results.truncate(pos);
            }
        }
        // after: only checkpoints older than (after) the given id
        if let Some(after_id) = &filter.after {
            let after_pos = results.iter().position(|t| t.checkpoint.id == *after_id);
            if let Some(pos) = after_pos {
                results = results.into_iter().skip(pos + 1).collect();
            }
        }
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        results
    }

    /// Load pending writes for a checkpoint from the database
    ///
    /// Helper function to load and deserialize pending writes associated
    /// with a specific checkpoint.
    async fn load_pending_writes(
        &self,
        thread_id: &str,
        checkpoint_ns: &str,
        checkpoint_id: &str,
    ) -> Result<Vec<PendingWrite>, CoreCheckpointError> {
        let write_rows = sqlx::query(
            "SELECT task_id, channel, value
             FROM checkpoint_writes
             WHERE thread_id = ? AND checkpoint_ns = ? AND checkpoint_id = ?
             ORDER BY task_id, idx",
        )
        .bind(thread_id)
        .bind(checkpoint_ns)
        .bind(checkpoint_id)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        write_rows
            .into_iter()
            .map(|row| {
                let task_id: String = row
                    .try_get("task_id")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let channel: String = row
                    .try_get("channel")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let value: String = row
                    .try_get("value")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let value_json: serde_json::Value = serde_json::from_str(&value)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                Ok(PendingWrite {
                    task_id,
                    channel,
                    value: value_json,
                })
            })
            .collect::<Result<Vec<_>, CoreCheckpointError>>()
    }
}

#[async_trait]
#[cfg(feature = "sqlite")]
impl juncture_core::checkpoint::CheckpointSaver for SqliteSaver {
    async fn get_tuple(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<CheckpointTuple>, CoreCheckpointError> {
        let thread_id =
            Self::get_thread_id(config).map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let checkpoint_ns = Self::get_checkpoint_ns(config);

        let row = if let Some(checkpoint_id) = &config.checkpoint_id {
            sqlx::query(
                "SELECT channel_values, channel_versions, versions_seen,
                        pending_tasks, pending_sends, pending_interrupts,
                        schema_version, metadata,
                        checkpoint_id, created_at
                 FROM checkpoints
                 WHERE thread_id = ? AND checkpoint_ns = ? AND checkpoint_id = ?",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(checkpoint_id)
            .fetch_optional(&*self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT channel_values, channel_versions, versions_seen,
                        pending_tasks, pending_sends, pending_interrupts,
                        schema_version, metadata,
                        checkpoint_id, created_at
                 FROM checkpoints
                 WHERE thread_id = ? AND checkpoint_ns = ?
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
            Some(ref row) => {
                let mut tuple = Self::row_to_tuple(row, config)?;
                let pending_writes = self
                    .load_pending_writes(&thread_id, &checkpoint_ns, &tuple.checkpoint.id)
                    .await?;
                tuple.pending_writes = pending_writes;
                Ok(Some(tuple))
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

        // When non-limit filters are active, metadata fields (source, step) require
        // deserialization so we must fetch all rows and filter in Rust.
        let has_non_limit_filter = filter.as_ref().is_some_and(|f| {
            f.source.is_some()
                || f.step_gte.is_some()
                || f.step_lte.is_some()
                || f.before.is_some()
                || f.after.is_some()
        });

        let rows = if has_non_limit_filter {
            sqlx::query(
                "SELECT channel_values, channel_versions, versions_seen,
                        pending_tasks, pending_sends, pending_interrupts,
                        schema_version, metadata,
                        checkpoint_id, created_at
                 FROM checkpoints
                 WHERE thread_id = ? AND checkpoint_ns = ?
                 ORDER BY created_at DESC",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?
        } else {
            let limit = i64::try_from(filter.as_ref().and_then(|f| f.limit).unwrap_or(10))
                .expect("limit value fits in i64");
            sqlx::query(
                "SELECT channel_values, channel_versions, versions_seen,
                        pending_tasks, pending_sends, pending_interrupts,
                        schema_version, metadata,
                        checkpoint_id, created_at
                 FROM checkpoints
                 WHERE thread_id = ? AND checkpoint_ns = ?
                 ORDER BY created_at DESC
                 LIMIT ?",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(limit)
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?
        };

        let tuples: Vec<CheckpointTuple> = rows
            .iter()
            .map(|row| Self::row_to_tuple(row, config))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let results = match filter {
            Some(ref f) if has_non_limit_filter => Self::apply_list_filter(tuples, f),
            Some(ref f) => {
                // Only limit was active; SQL already applied it, but for consistency
                // with the filter contract, still truncate (no-op if SQL limit matched).
                let mut out = tuples;
                if let Some(limit) = f.limit {
                    out.truncate(limit);
                }
                out
            }
            None => tuples,
        };

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

        // Serialize each field separately per design spec using the configured serializer
        let channel_values_bytes = self
            .serializer
            .serialize(&checkpoint.channel_values)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let channel_versions_bytes = self
            .serializer
            .serialize(&checkpoint.channel_versions)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let versions_seen_bytes = self
            .serializer
            .serialize(&checkpoint.versions_seen)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_tasks_bytes = self
            .serializer
            .serialize(&checkpoint.pending_tasks)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_sends_bytes = self
            .serializer
            .serialize(&checkpoint.pending_sends)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let pending_interrupts_bytes = self
            .serializer
            .serialize(&checkpoint.pending_interrupts)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
        let metadata_bytes = self
            .serializer
            .serialize(&metadata)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Extract parent_checkpoint_id from metadata.parents using empty namespace key
        let parent_checkpoint_id = metadata.parents.get("").cloned();

        // Begin transaction for checkpoint save and write cleanup
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Insert or update checkpoint with new schema
        sqlx::query(
            r"
            INSERT INTO checkpoints
            (thread_id, checkpoint_ns, checkpoint_id, parent_checkpoint_id,
             channel_values, channel_versions, versions_seen,
             pending_tasks, pending_sends, pending_interrupts,
             schema_version, metadata, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (thread_id, checkpoint_ns, checkpoint_id) DO UPDATE SET
                parent_checkpoint_id = excluded.parent_checkpoint_id,
                channel_values = excluded.channel_values,
                channel_versions = excluded.channel_versions,
                versions_seen = excluded.versions_seen,
                pending_tasks = excluded.pending_tasks,
                pending_sends = excluded.pending_sends,
                pending_interrupts = excluded.pending_interrupts,
                schema_version = excluded.schema_version,
                metadata = excluded.metadata
            ",
        )
        .bind(&thread_id)
        .bind(&checkpoint_ns)
        .bind(&checkpoint.id)
        .bind(&parent_checkpoint_id)
        .bind(&channel_values_bytes)
        .bind(&channel_versions_bytes)
        .bind(&versions_seen_bytes)
        .bind(&pending_tasks_bytes)
        .bind(&pending_sends_bytes)
        .bind(&pending_interrupts_bytes)
        .bind(i64::from(checkpoint.schema_version))
        .bind(&metadata_bytes)
        .bind(&checkpoint.created_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Clean up old writes for this thread/namespace (crash recovery)
        // When a new checkpoint is saved, all previous pending writes are obsolete
        sqlx::query(
            "DELETE FROM checkpoint_writes
             WHERE thread_id = ? AND checkpoint_ns = ?",
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
            let value_str = serde_json::to_string(&write.value)
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

            sqlx::query(
                "INSERT INTO checkpoint_writes
                 (thread_id, checkpoint_ns, checkpoint_id, task_id, channel, value, idx)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
                 DO UPDATE SET
                     channel = excluded.channel,
                     value = excluded.value",
            )
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(&checkpoint_id)
            .bind(task_id)
            .bind(&write.channel)
            .bind(&value_str)
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
            pending_interrupts: vec![],
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
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_put_writes() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
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
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_put_writes_persistence() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread1");
        let checkpoint = create_test_checkpoint("cp1");
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
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_put_cleans_old_writes() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread1");

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
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_msgpack_roundtrip() {
        use crate::SerializationFormat;

        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread-msgpack");

        // Create a checkpoint with non-trivial data to exercise msgpack encoding
        let mut channel_versions = std::collections::HashMap::new();
        channel_versions.insert("messages".to_string(), 3);
        channel_versions.insert("context".to_string(), 1);

        let mut versions_seen = std::collections::HashMap::new();
        let mut inner = std::collections::HashMap::new();
        inner.insert("node_a".to_string(), 2);
        versions_seen.insert("messages".to_string(), inner);

        let checkpoint = Checkpoint {
            id: "cp-msgpack-1".to_string(),
            channel_values: json!({"messages": ["hello", "world"], "count": 42}),
            channel_versions,
            versions_seen,
            pending_tasks: vec![CheckpointPendingTask {
                id: "task-1".to_string(),
                node: "process_node".to_string(),
                triggers: vec!["trigger_a".to_string()],
                state_override: Some(json!({"key": "value"})),
            }],
            pending_sends: vec![SerializedSend {
                node: "outbox_node".to_string(),
                state: serde_json::Value::String("payload".to_string()),
            }],
            pending_interrupts: vec![],
            schema_version: 1,
            created_at: Utc::now().to_rfc3339(),
            v: 1,
            new_versions: std::collections::HashMap::new(),
            counters_since_delta_snapshot: std::collections::HashMap::new(),
        };

        let metadata = create_test_metadata();
        let result_config = saver
            .put(&config, checkpoint.clone(), metadata)
            .await
            .unwrap();

        // Verify the default serializer is MessagePack
        assert_eq!(saver.serializer.format(), SerializationFormat::MessagePack);

        // Retrieve and verify all fields round-tripped correctly
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.checkpoint.id, "cp-msgpack-1");
        assert_eq!(
            tuple.checkpoint.channel_values,
            json!({"messages": ["hello", "world"], "count": 42})
        );
        assert_eq!(tuple.checkpoint.channel_versions.get("messages"), Some(&3));
        assert_eq!(tuple.checkpoint.channel_versions.get("context"), Some(&1));
        assert!(
            tuple
                .checkpoint
                .versions_seen
                .get("messages")
                .is_some_and(|m| m.get("node_a") == Some(&2))
        );
        assert_eq!(tuple.checkpoint.pending_tasks.len(), 1);
        assert_eq!(tuple.checkpoint.pending_tasks[0].id, "task-1");
        assert_eq!(tuple.checkpoint.pending_sends.len(), 1);
        assert_eq!(tuple.checkpoint.pending_sends[0].node, "outbox_node");
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_reads_legacy_json_data() {
        use crate::SerializationFormat;

        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();

        // Manually insert JSON-format data to simulate legacy checkpoints
        // written before the MsgpackSerializer default
        let channel_values_bytes = serde_json::to_vec(&json!({"key": "legacy"})).unwrap();
        let channel_versions_bytes =
            serde_json::to_vec(&std::collections::HashMap::<String, u64>::new()).unwrap();
        let versions_seen_bytes = serde_json::to_vec(&std::collections::HashMap::<
            String,
            std::collections::HashMap<String, u64>,
        >::new())
        .unwrap();
        let pending_tasks_bytes = serde_json::to_vec(&Vec::<CheckpointPendingTask>::new()).unwrap();
        let pending_sends_bytes = serde_json::to_vec(&Vec::<SerializedSend>::new()).unwrap();
        let metadata_bytes = serde_json::to_vec(&create_test_metadata()).unwrap();

        sqlx::query(
            r"
            INSERT INTO checkpoints
            (thread_id, checkpoint_ns, checkpoint_id, parent_checkpoint_id,
             channel_values, channel_versions, versions_seen,
             pending_tasks, pending_sends, schema_version, metadata, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind("thread-legacy")
        .bind("")
        .bind("cp-legacy-1")
        .bind(Option::<String>::None)
        .bind(&channel_values_bytes)
        .bind(&channel_versions_bytes)
        .bind(&versions_seen_bytes)
        .bind(&pending_tasks_bytes)
        .bind(&pending_sends_bytes)
        .bind(1_i64)
        .bind(&metadata_bytes)
        .bind(Utc::now().to_rfc3339())
        .execute(&*saver.pool)
        .await
        .unwrap();

        // Verify the saver can read the legacy JSON data
        let config = RunnableConfig::default().with_thread_id("thread-legacy");
        let tuple = saver.get_tuple(&config).await.unwrap().unwrap();
        assert_eq!(tuple.checkpoint.id, "cp-legacy-1");
        assert_eq!(tuple.checkpoint.channel_values, json!({"key": "legacy"}));

        // Verify that the serializer is indeed MessagePack (default), proving auto-detection works
        assert_eq!(saver.serializer.format(), SerializationFormat::MessagePack);
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_list_filter_by_source() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread-filter-source");

        // Insert checkpoints with different sources
        let metadata_input = CheckpointMetadata {
            source: CheckpointSource::Input,
            step: 0,
            ..create_test_metadata()
        };
        let cp_input = create_test_checkpoint("cp-input");
        saver.put(&config, cp_input, metadata_input).await.unwrap();

        let metadata_loop = CheckpointMetadata {
            source: CheckpointSource::Loop,
            step: 1,
            ..create_test_metadata()
        };
        let cp_loop = create_test_checkpoint("cp-loop");
        saver.put(&config, cp_loop, metadata_loop).await.unwrap();

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
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_list_filter_by_step_range() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread-filter-step");

        // Insert checkpoints at steps 0..5
        for step in 0..5 {
            let metadata = CheckpointMetadata {
                source: CheckpointSource::Loop,
                step,
                ..create_test_metadata()
            };
            let checkpoint = create_test_checkpoint(&format!("cp-step-{step}"));
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // Filter step >= 2
        let result_min_step = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    step_gte: Some(2),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result_min_step.len(), 3);

        // Filter step <= 2
        let result_max_step = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    step_lte: Some(2),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result_max_step.len(), 3);

        // Filter step 1..=3
        let result_step_range = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    step_gte: Some(1),
                    step_lte: Some(3),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result_step_range.len(), 3);
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_list_filter_before_after() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread-filter-before-after");

        // Insert 5 checkpoints; they are sorted by created_at DESC in list()
        for i in 0..5 {
            let metadata = CheckpointMetadata {
                source: CheckpointSource::Loop,
                step: i,
                ..create_test_metadata()
            };
            let checkpoint = create_test_checkpoint(&format!("cp-ba-{i}"));
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // "before" cp-ba-2: items newer than cp-ba-2 (positions before it)
        let before = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    before: Some("cp-ba-2".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert!(before.len() < 5);
        assert!(before.iter().all(|t| t.checkpoint.id != "cp-ba-2"));

        // "after" cp-ba-2: items older than cp-ba-2 (positions after it)
        let after = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    after: Some("cp-ba-2".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert!(after.len() < 5);
        assert!(after.iter().all(|t| t.checkpoint.id != "cp-ba-2"));

        // "before" + "after" combined narrows the range
        let combo = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    before: Some("cp-ba-3".to_string()),
                    after: Some("cp-ba-1".to_string()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        // Should exclude cp-ba-3 (the before pivot) and cp-ba-1 (the after pivot)
        assert!(!combo.iter().any(|t| t.checkpoint.id == "cp-ba-3"));
        assert!(!combo.iter().any(|t| t.checkpoint.id == "cp-ba-1"));
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_list_filter_with_limit() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();
        let config = create_test_config("thread-filter-limit");

        // Insert 10 checkpoints
        for step in 0..10 {
            let metadata = CheckpointMetadata {
                source: CheckpointSource::Loop,
                step,
                ..create_test_metadata()
            };
            let checkpoint = create_test_checkpoint(&format!("cp-limit-{step}"));
            saver.put(&config, checkpoint, metadata).await.unwrap();
        }

        // Filter step >= 3 with limit 2 should return at most 2 items
        let filtered = saver
            .list(
                &config,
                Some(CheckpointFilter {
                    step_gte: Some(3),
                    limit: Some(2),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|t| t.metadata.step >= 3));
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn test_sqlite_saver_pending_interrupts_roundtrip() {
        let saver = SqliteSaver::from_connection_string("sqlite::memory:")
            .await
            .unwrap();

        let config = create_test_config("thread-interrupts-sqlite");

        let checkpoint = Checkpoint {
            id: "cp-int-sqlite-1".to_string(),
            channel_values: json!({"state": "paused"}),
            channel_versions: std::collections::HashMap::new(),
            versions_seen: std::collections::HashMap::new(),
            pending_tasks: vec![],
            pending_sends: vec![],
            pending_interrupts: vec![
                InterruptSignal {
                    index: 0,
                    id: Some("interrupt-approval".to_string()),
                    payload: json!({"reason": "awaiting human review"}),
                },
                InterruptSignal {
                    index: 1,
                    id: None,
                    payload: json!({"type": "confirmation"}),
                },
            ],
            schema_version: 1,
            created_at: Utc::now().to_rfc3339(),
            v: 1,
            new_versions: std::collections::HashMap::new(),
            counters_since_delta_snapshot: std::collections::HashMap::new(),
        };

        let metadata = CheckpointMetadata {
            source: CheckpointSource::Interrupt {
                node: "approval_node".to_string(),
            },
            step: 3,
            ..create_test_metadata()
        };
        let result_config = saver
            .put(&config, checkpoint.clone(), metadata)
            .await
            .unwrap();

        // Retrieve and verify pending_interrupts persisted correctly
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.checkpoint.pending_interrupts.len(), 2);

        let first = &tuple.checkpoint.pending_interrupts[0];
        assert_eq!(first.index, 0);
        assert_eq!(first.id.as_deref(), Some("interrupt-approval"));
        assert_eq!(first.payload, json!({"reason": "awaiting human review"}));

        let second = &tuple.checkpoint.pending_interrupts[1];
        assert_eq!(second.index, 1);
        assert!(second.id.is_none());
        assert_eq!(second.payload, json!({"type": "confirmation"}));
    }
}

// Rust guideline compliant 2026-05-21
