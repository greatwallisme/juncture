// PostgreSQL checkpoint saver implementation
//
// This module provides the PostgresSaver for persistent checkpoint storage
// using PostgreSQL database.

use async_trait::async_trait;
use serde::Serialize;
use std::sync::Arc;

#[cfg(feature = "postgres")]
use sqlx::Row;

use juncture_core::checkpoint::{
    Checkpoint, CheckpointError as CoreCheckpointError, CheckpointFilter, CheckpointMetadata,
    CheckpointPendingTask, CheckpointTuple, PendingWrite, SerializedSend,
};
use juncture_core::config::RunnableConfig;
use juncture_core::interrupt::InterruptSignal;

use crate::error::CheckpointError;
use crate::serde::{SerializerKind, deserialize_auto};

/// Schema migration function type.
///
/// Called for each step in the chain migration from `from_version` to `to_version`.
/// Receives the raw `serde_json::Value` and must return the migrated value.
/// On failure, return `Err(String)` with a human-readable reason.
pub type SchemaMigratorFn =
    Box<dyn Fn(u32, u32, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

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
/// Uses `MessagePack` serialization by default for high-performance binary storage.
#[derive(Clone)]
pub struct PostgresSaver {
    /// Database connection pool
    #[cfg(feature = "postgres")]
    pool: Arc<sqlx::PgPool>,
    /// Serializer for checkpoint data fields
    serializer: SerializerKind,
    /// Optional schema migration function for chain migration (design doc §5.4)
    schema_migrator: Option<Arc<SchemaMigratorFn>>,
}

#[cfg(feature = "postgres")]
impl std::fmt::Debug for PostgresSaver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresSaver")
            .field("pool", &self.pool)
            .field("serializer", &self.serializer)
            .field("has_schema_migrator", &self.schema_migrator.is_some())
            .finish()
    }
}

/// SQL for creating the `checkpoints` table, including the `pending_interrupts` column.
#[cfg(feature = "postgres")]
const CHECKPOINTS_CREATE_TABLE_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS checkpoints (
        thread_id TEXT NOT NULL,
        checkpoint_ns TEXT NOT NULL DEFAULT '',
        checkpoint_id TEXT NOT NULL,
        parent_checkpoint_id TEXT,
        channel_values BYTEA NOT NULL,
        channel_versions BYTEA NOT NULL,
        versions_seen BYTEA NOT NULL,
        pending_tasks BYTEA,
        pending_sends BYTEA,
        pending_interrupts BYTEA,
        schema_version INTEGER NOT NULL DEFAULT 1,
        metadata BYTEA NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id)
    )
";

/// SQL for creating the `checkpoint_writes` table.
#[cfg(feature = "postgres")]
const CHECKPOINT_WRITES_CREATE_TABLE_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS checkpoint_writes (
        thread_id TEXT NOT NULL,
        checkpoint_ns TEXT NOT NULL DEFAULT '',
        checkpoint_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        channel TEXT NOT NULL,
        value BYTEA NOT NULL,
        idx INTEGER NOT NULL,
        PRIMARY KEY (thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
    )
";

/// Columns selected from the checkpoints table for deserialization.
#[cfg(feature = "postgres")]
const CHECKPOINT_SELECT_COLUMNS: &str = "channel_values, channel_versions, versions_seen, pending_tasks, pending_sends, \
     pending_interrupts, schema_version, metadata, checkpoint_id, created_at";

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

        Self::run_schema_migrations(&pool).await?;

        Ok(Self {
            pool: Arc::new(pool),
            serializer: SerializerKind::default(),
            schema_migrator: None,
        })
    }

    /// Run all schema migrations on a newly connected pool.
    ///
    /// Creates tables and indexes if they do not exist, then applies
    /// additive column migrations for databases created before schema changes.
    async fn run_schema_migrations(pool: &sqlx::PgPool) -> Result<(), CheckpointError> {
        sqlx::query(CHECKPOINTS_CREATE_TABLE_SQL)
            .execute(pool)
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        sqlx::query(CHECKPOINT_WRITES_CREATE_TABLE_SQL)
            .execute(pool)
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Create indexes
        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_time
                ON checkpoints(thread_id, checkpoint_ns, created_at DESC)
            ",
        )
        .execute(pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_checkpoint_writes_lookup
                ON checkpoint_writes (thread_id, checkpoint_ns, checkpoint_id)
            ",
        )
        .execute(pool)
        .await
        .map_err(|e| CheckpointError::Database(e.to_string()))?;

        // Add pending_interrupts column for databases created before this field existed.
        // PostgreSQL supports IF NOT EXISTS for ALTER TABLE ADD COLUMN.
        sqlx::query("ALTER TABLE checkpoints ADD COLUMN IF NOT EXISTS pending_interrupts BYTEA")
            .execute(pool)
            .await
            .map_err(|e| CheckpointError::Database(e.to_string()))?;

        Ok(())
    }

    /// Create a `PostgresSaver` with a custom serializer
    ///
    /// Allows overriding the default `MessagePack` serializer with a custom
    /// format (e.g., `SerializerKind::Json` for debugging).
    #[must_use]
    pub const fn with_serializer(mut self, serializer: SerializerKind) -> Self {
        self.serializer = serializer;
        self
    }

    /// Register a custom schema migrator for chain migration (design doc §5.4)
    ///
    /// The migrator is called for each step in the chain migration from
    /// `from_version` to `to_version`. It receives the raw `serde_json::Value`
    /// and must return the migrated value, or an error with a human-readable reason.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let saver = PostgresSaver::new("postgresql://localhost/db").await?
    ///     .with_schema_migrator(Box::new(|from, to, mut values| {
    ///         match (from, to) {
    ///             (1, 2) => {
    ///                 // Add new field with default
    ///                 values["new_field"] = serde_json::json!("default");
    ///                 Ok(values)
    ///             }
    ///             _ => Err(format!("unknown migration: {from} -> {to}")),
    ///         }
    ///     }));
    /// ```
    #[must_use]
    pub fn with_schema_migrator(mut self, migrator: SchemaMigratorFn) -> Self {
        self.schema_migrator = Some(Arc::new(migrator));
        self
    }

    /// Get thread ID from config, returning error if not set
    fn get_thread_id(config: &RunnableConfig) -> Result<String, CheckpointError> {
        config
            .thread_id
            .clone()
            .ok_or_else(|| CheckpointError::Storage("thread_id is required".to_string()))
    }

    /// Get checkpoint namespace string from config, defaulting to empty string
    fn get_checkpoint_ns(config: &RunnableConfig) -> String {
        config
            .checkpoint_ns
            .as_ref()
            .map_or_else(String::new, juncture_core::CheckpointNamespace::as_str)
    }

    /// Migrate checkpoint data from older schema version to current version
    ///
    /// Implements the chain migration strategy from design doc §5.4:
    /// 1. Compare stored `schema_version` with current version
    /// 2. If versions match, return as-is
    /// 3. If stored < current, apply step-by-step chain migration
    /// 4. If stored > current, error (downgrade not supported)
    ///
    /// Migration operates on `serde_json::Value` to avoid dependencies on old
    /// struct definitions. Custom migration steps can be registered via
    /// [`PostgresSaver::with_schema_migrator`].
    fn migrate_checkpoint_schema(
        channel_values: serde_json::Value,
        stored_schema_version: u32,
        checkpoint_id: &str,
        migrator: Option<&Arc<SchemaMigratorFn>>,
    ) -> Result<serde_json::Value, CoreCheckpointError> {
        let current_schema_version = 1u32;

        if stored_schema_version == current_schema_version {
            return Ok(channel_values);
        }

        if stored_schema_version > current_schema_version {
            return Err(CoreCheckpointError::Other(format!(
                "Checkpoint {checkpoint_id} has schema version {stored_schema_version}, \
                 but current version is {current_schema_version}. \
                 Downgrade is not supported."
            )));
        }

        // Chain migration: apply step-by-step from stored_version to current_version
        let mut values = channel_values;
        for step_from in stored_schema_version..current_schema_version {
            let step_to = step_from + 1;

            // Try registered migrator first
            if let Some(migrate_fn) = migrator {
                values = migrate_fn(step_from, step_to, values).map_err(|reason| {
                    CoreCheckpointError::Other(format!(
                        "Checkpoint {checkpoint_id}: schema migration \
                         v{step_from} -> v{step_to} failed: {reason}"
                    ))
                })?;
            } else {
                // No migrator registered and no built-in migration step available.
                // Future built-in steps should be added as if/else branches here:
                // if (step_from, step_to) == (1, 2) { ... }
                return Err(CoreCheckpointError::Other(format!(
                    "Checkpoint {checkpoint_id}: no migration path from \
                     schema v{step_from} to v{step_to}. Register a schema \
                     migrator via PostgresSaver::with_schema_migrator()."
                )));
            }
        }

        Ok(values)
    }

    /// Deserialize checkpoint from database row fields
    ///
    /// Helper function to reconstruct a Checkpoint from individual column values
    /// as per design specification (section 4.2).
    ///
    /// This function implements M04-002: Schema migration logic by calling
    /// `migrate_checkpoint_schema()` to handle version mismatches.
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
        schema_migrator: Option<&Arc<SchemaMigratorFn>>,
    ) -> Result<Checkpoint, CoreCheckpointError> {
        let raw_channel_values: serde_json::Value = deserialize_auto(channel_values_bytes)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        // Apply schema migration (M04-002)
        let schema_version_u32 = u32::try_from(schema_version).expect("schema_version fits in u32");
        let channel_values = Self::migrate_checkpoint_schema(
            raw_channel_values,
            schema_version_u32,
            &checkpoint_id,
            schema_migrator,
        )?;

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
            schema_version: schema_version_u32,
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
        row: &sqlx::postgres::PgRow,
        config: &RunnableConfig,
        schema_migrator: Option<&Arc<SchemaMigratorFn>>,
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
            schema_migrator,
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
    /// (source, step) are stored as serialized BYTEAs that cannot be filtered at
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
             WHERE thread_id = $1 AND checkpoint_ns = $2 AND checkpoint_id = $3
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
                let value_bytes: Vec<u8> = row
                    .try_get("value")
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;
                let value_json: serde_json::Value = deserialize_auto(&value_bytes)
                    .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

                Ok(PendingWrite {
                    task_id,
                    channel,
                    value: value_json,
                })
            })
            .collect::<Result<Vec<_>, CoreCheckpointError>>()
    }

    /// Serialize a value only when it is non-empty, returning `None` for empty slices.
    ///
    /// Avoids storing trivially empty blobs for optional columns.
    fn serialize_optional<T: Serialize>(
        serializer: &SerializerKind,
        value: &Vec<T>,
    ) -> Result<Option<Vec<u8>>, CoreCheckpointError> {
        if value.is_empty() {
            return Ok(None);
        }
        serializer
            .serialize(value)
            .map(Some)
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))
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

        let select_sql = format!("SELECT {CHECKPOINT_SELECT_COLUMNS} FROM checkpoints");

        let row = if let Some(checkpoint_id) = &config.checkpoint_id {
            sqlx::query(&format!(
                "{select_sql} WHERE thread_id = $1 AND checkpoint_ns = $2 AND checkpoint_id = $3"
            ))
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(checkpoint_id)
            .fetch_optional(&*self.pool)
            .await
        } else {
            sqlx::query(&format!(
                "{select_sql} WHERE thread_id = $1 AND checkpoint_ns = $2 \
                 ORDER BY created_at DESC LIMIT 1"
            ))
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .fetch_optional(&*self.pool)
            .await
        }
        .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        match row {
            Some(ref row) => {
                let mut tuple = Self::row_to_tuple(row, config, self.schema_migrator.as_ref())?;
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

        let select_sql = format!("SELECT {CHECKPOINT_SELECT_COLUMNS} FROM checkpoints");

        let rows = if has_non_limit_filter {
            sqlx::query(&format!(
                "{select_sql} WHERE thread_id = $1 AND checkpoint_ns = $2 \
                 ORDER BY created_at DESC"
            ))
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?
        } else {
            let limit = i64::try_from(filter.as_ref().and_then(|f| f.limit).unwrap_or(10))
                .expect("limit value fits in i64");
            sqlx::query(&format!(
                "{select_sql} WHERE thread_id = $1 AND checkpoint_ns = $2 \
                 ORDER BY created_at DESC LIMIT $3"
            ))
            .bind(&thread_id)
            .bind(&checkpoint_ns)
            .bind(limit)
            .fetch_all(&*self.pool)
            .await
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?
        };

        let tuples: Vec<CheckpointTuple> = rows
            .iter()
            .map(|row| Self::row_to_tuple(row, config, self.schema_migrator.as_ref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

        let results = match filter {
            Some(ref f) if has_non_limit_filter => Self::apply_list_filter(tuples, f),
            Some(ref f) => {
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
        let pending_tasks_bytes =
            Self::serialize_optional(&self.serializer, &checkpoint.pending_tasks)?;
        let pending_sends_bytes =
            Self::serialize_optional(&self.serializer, &checkpoint.pending_sends)?;
        let pending_interrupts_bytes =
            Self::serialize_optional(&self.serializer, &checkpoint.pending_interrupts)?;
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (thread_id, checkpoint_ns, checkpoint_id) DO UPDATE SET
                parent_checkpoint_id = EXCLUDED.parent_checkpoint_id,
                channel_values = EXCLUDED.channel_values,
                channel_versions = EXCLUDED.channel_versions,
                versions_seen = EXCLUDED.versions_seen,
                pending_tasks = EXCLUDED.pending_tasks,
                pending_sends = EXCLUDED.pending_sends,
                pending_interrupts = EXCLUDED.pending_interrupts,
                schema_version = EXCLUDED.schema_version,
                metadata = EXCLUDED.metadata
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
            let value_bytes = self
                .serializer
                .serialize(&write.value)
                .map_err(|e| CoreCheckpointError::Storage(e.to_string()))?;

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
            .bind(&value_bytes)
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

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn test_postgres_saver_msgpack_roundtrip() {
        use crate::SerializationFormat;

        let conn_str = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5432/test".to_string());

        let Ok(saver) = PostgresSaver::new(&conn_str).await else {
            return;
        };

        let config = create_test_config("thread-msgpack-pg");

        // Create a checkpoint with non-trivial data to exercise msgpack encoding
        let mut channel_versions = std::collections::HashMap::new();
        channel_versions.insert("messages".to_string(), 3);
        channel_versions.insert("context".to_string(), 1);

        let mut versions_seen = std::collections::HashMap::new();
        let mut inner = std::collections::HashMap::new();
        inner.insert("node_a".to_string(), 2);
        versions_seen.insert("messages".to_string(), inner);

        let checkpoint = Checkpoint {
            id: "cp-msgpack-pg-1".to_string(),
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
        let result_config = saver.put(&config, checkpoint, metadata).await.unwrap();

        // Verify the default serializer is MessagePack
        assert_eq!(saver.serializer.format(), SerializationFormat::MessagePack);

        // Retrieve and verify all fields round-tripped correctly
        let tuple = saver.get_tuple(&result_config).await.unwrap().unwrap();
        assert_eq!(tuple.checkpoint.id, "cp-msgpack-pg-1");
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

        // Clean up
        let _ = sqlx::query("DELETE FROM checkpoints WHERE thread_id = $1")
            .bind(&config.thread_id)
            .execute(&*saver.pool)
            .await;
        let _ = sqlx::query("DELETE FROM checkpoint_writes WHERE thread_id = $1")
            .bind(&config.thread_id)
            .execute(&*saver.pool)
            .await;
    }

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn test_postgres_saver_pending_interrupts_roundtrip() {
        let conn_str = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost:5432/test".to_string());

        let Ok(saver) = PostgresSaver::new(&conn_str).await else {
            return;
        };

        let config = create_test_config("thread-interrupts-pg");

        let checkpoint = Checkpoint {
            id: "cp-int-pg-1".to_string(),
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
                    timestamp: Utc::now(),
                },
                InterruptSignal {
                    index: 1,
                    id: None,
                    payload: json!({"type": "confirmation"}),
                    timestamp: Utc::now(),
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

        // Clean up
        let _ = sqlx::query("DELETE FROM checkpoints WHERE thread_id = $1")
            .bind(&config.thread_id)
            .execute(&*saver.pool)
            .await;
        let _ = sqlx::query("DELETE FROM checkpoint_writes WHERE thread_id = $1")
            .bind(&config.thread_id)
            .execute(&*saver.pool)
            .await;
    }
}

// Rust guideline compliant 2026-05-23
