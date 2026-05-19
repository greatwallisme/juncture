//! Checkpoint data structures
//!
//! This module re-exports checkpoint types from juncture-core for convenience.

pub use juncture_core::checkpoint::{
    Checkpoint, CheckpointFilter, CheckpointMetadata, CheckpointPendingTask, CheckpointSource,
    CheckpointTuple, DeltaCounters, DeltaOp, PendingWrite, PregelTaskInfo as PregelTaskInfoExport,
    SerializedSend, StateSnapshot,
};

/// Pregel task information (re-exported from juncture-core)
pub type PregelTaskInfo = PregelTaskInfoExport;

/// Delta snapshot for incremental checkpointing
///
/// Stores only the changes from a base checkpoint, enabling efficient storage.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeltaSnapshot {
    /// Base checkpoint ID (full snapshot)
    pub base_checkpoint_id: String,

    /// Ordered list of channel deltas
    pub deltas: Vec<ChannelDelta>,
}

/// Delta for a single channel
///
/// Represents incremental changes to a specific channel.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChannelDelta {
    /// Channel name
    pub channel: String,

    /// Operation type
    pub op: DeltaOp,

    /// Values to apply
    pub values: Vec<serde_json::Value>,
}

/// Time-to-live configuration for checkpoint expiration
///
/// Configures automatic cleanup of old checkpoints.
#[derive(Clone, Debug)]
pub struct TtlConfig {
    /// Default TTL for checkpoints
    pub default_ttl: Option<std::time::Duration>,

    /// Interval between cleanup sweeps
    pub sweep_interval: std::time::Duration,

    /// Maximum number of checkpoints to retain
    pub max_checkpoints: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_metadata_serialization() {
        let metadata = CheckpointMetadata {
            source: CheckpointSource::Loop,
            step: 5,
            writes: std::collections::HashMap::new(),
            parents: std::collections::HashMap::new(),
            run_id: "run-123".to_string(),
        };

        let serialized = serde_json::to_value(&metadata).unwrap();
        let deserialized: CheckpointMetadata = serde_json::from_value(serialized).unwrap();

        assert!(matches!(deserialized.source, CheckpointSource::Loop));
        assert_eq!(deserialized.step, 5);
        assert_eq!(deserialized.run_id, "run-123");
    }

    #[test]
    fn test_delta_counters_default() {
        let counters = DeltaCounters::default();
        assert_eq!(counters.updates, 0);
        assert_eq!(counters.supersteps, 0);
    }

    #[test]
    fn test_checkpoint_filter_default() {
        let filter = CheckpointFilter::default();
        assert!(filter.source.is_none());
        assert!(filter.step_gte.is_none());
        assert!(filter.step_lte.is_none());
        assert!(filter.before.is_none());
        assert!(filter.after.is_none());
        assert!(filter.limit.is_none());
    }
}

// Rust guideline compliant 2026-05-19
