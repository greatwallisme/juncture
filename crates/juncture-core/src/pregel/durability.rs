//! Durability mode for checkpoint persistence
//!
//! Controls how checkpoint writes are synchronized to durable storage
//! during graph execution.

/// Checkpoint durability mode
///
/// Determines how checkpoint writes are synchronized to storage backends.
/// Higher durability guarantees come at the cost of increased latency.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Durability {
    /// Synchronous checkpoint writes (default)
    ///
    /// Each checkpoint is fully written to storage before the next
    /// superstep begins. Provides the strongest durability guarantee.
    #[default]
    Sync,

    /// Asynchronous checkpoint writes
    ///
    /// Checkpoint writes are submitted to storage but execution
    /// continues without waiting for confirmation. Durability
    /// depends on the storage backend's write-behind behavior.
    Async,

    /// No checkpoint persistence
    ///
    /// Checkpoints are not persisted to external storage. Used
    /// for ephemeral or development runs where durability is
    /// not required.
    Exit,
}

// Rust guideline compliant 2026-05-19
