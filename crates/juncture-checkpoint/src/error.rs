//! Checkpoint error types

use std::io;

/// Checkpoint operation errors
///
/// Represents all possible errors that can occur during checkpoint operations,
/// including serialization, storage, and schema migration failures.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// Serialization failed
    ///
    /// Occurs when converting checkpoint data to a serialized format (JSON/MessagePack).
    #[error("Serialization failed: {0}")]
    Serialize(String),

    /// Deserialization failed
    ///
    /// Occurs when parsing serialized data back into checkpoint structures.
    #[error("Deserialization failed: {0}")]
    Deserialize(String),

    /// Schema migration failed
    ///
    /// Occurs when migrating checkpoint data between incompatible schema versions.
    #[error("Schema migration failed: from version {from} to {to}: {reason}")]
    SchemaMigration {
        /// Source schema version
        from: u32,
        /// Target schema version
        to: u32,
        /// Human-readable reason for migration failure
        reason: String,
    },

    /// Storage operation error
    ///
    /// Occurs when underlying storage backend fails (database, filesystem, etc.).
    #[error("Storage error: {0}")]
    Storage(String),

    /// Database operation error
    ///
    /// Occurs when database operations fail (query, connection, transaction, etc.).
    #[error("Database error: {0}")]
    Database(String),

    /// Serialization error (alias for Serialize)
    ///
    /// This variant is an alias for [`Serialize`] and exists for compatibility
    /// with external code that expects this name.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Checkpoint not found
    ///
    /// Occurs when attempting to retrieve a non-existent checkpoint.
    #[error("Checkpoint not found: thread={thread_id}, id={checkpoint_id}")]
    NotFound {
        /// Thread identifier
        thread_id: String,
        /// Checkpoint identifier
        checkpoint_id: String,
    },

    /// Connection pool exhausted
    ///
    /// Occurs when no database connections are available in the pool.
    #[error("Connection pool exhausted")]
    PoolExhausted,
}

impl From<serde_json::Error> for CheckpointError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialize(err.to_string())
    }
}

impl From<io::Error> for CheckpointError {
    fn from(err: io::Error) -> Self {
        Self::Storage(err.to_string())
    }
}

// Rust guideline compliant 2026-05-19
