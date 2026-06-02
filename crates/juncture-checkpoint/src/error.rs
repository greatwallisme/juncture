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
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Deserialization failed
    ///
    /// Occurs when parsing serialized data back into checkpoint structures.
    #[error("Deserialization failed: {0}")]
    Deserialize(#[source] Box<dyn std::error::Error + Send + Sync>),

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
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Database operation error
    ///
    /// Occurs when database operations fail (query, connection, transaction, etc.).
    #[error("Database error: {0}")]
    Database(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Serialization error (alias for Serialize)
    ///
    /// This variant is an alias for `Serialize` and exists for compatibility
    /// with external code that expects this name.
    #[error("Serialization error: {0}")]
    Serialization(#[source] Box<dyn std::error::Error + Send + Sync>),

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
        Self::Serialize(Box::new(err))
    }
}

impl From<io::Error> for CheckpointError {
    fn from(err: io::Error) -> Self {
        Self::Storage(Box::new(err))
    }
}

impl CheckpointError {
    /// Create a serialization error from a string message
    #[must_use]
    pub fn serialize_msg(msg: String) -> Self {
        Self::Serialize(Box::new(StringError(msg)))
    }

    /// Create a deserialization error from a string message
    #[must_use]
    pub fn deserialize_msg(msg: String) -> Self {
        Self::Deserialize(Box::new(StringError(msg)))
    }

    /// Create a storage error from a string message
    #[must_use]
    pub fn storage_msg(msg: String) -> Self {
        Self::Storage(Box::new(StringError(msg)))
    }

    /// Create a database error from a string message
    #[must_use]
    pub fn database_msg(msg: String) -> Self {
        Self::Database(Box::new(StringError(msg)))
    }
}

/// Wrapper for String errors to convert them into Box<dyn Error>
struct StringError(String);

impl std::fmt::Debug for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StringError {}

// Rust guideline compliant 2026-05-24
