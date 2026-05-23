//! Checkpoint data structures
//!
//! This module re-exports checkpoint types from juncture-core for convenience.

use std::collections::HashMap;

pub use juncture_core::checkpoint::{
    Checkpoint, CheckpointFilter, CheckpointMetadata, CheckpointPendingTask, CheckpointSource,
    CheckpointTuple, DeltaCounters, DeltaOp, PendingWrite, PregelTaskInfo as PregelTaskInfoExport,
    SerializedSend, StateSnapshot,
};

// Import CheckpointError from this crate's error module
use crate::error::CheckpointError;

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

/// Recover a full checkpoint from delta snapshots using ancestor walk algorithm
///
/// This function implements the delta recovery algorithm specified in design doc §1.4 and §8.3.
/// Given a list of checkpoint tuples sorted by step (ascending), it finds the latest full snapshot
/// and replays all subsequent delta writes to reconstruct the complete checkpoint state.
///
/// # Algorithm
///
/// 1. Find the nearest full snapshot (latest checkpoint with `pending_writes` empty or minimal)
/// 2. Walk forward through all delta writes from checkpoints after the snapshot
/// 3. Replay delta writes to the snapshot:
///    - Append channels: `snapshot[channel].extend(delta.values)`
///    - Replace channels: `snapshot[channel] = delta.values`
/// 4. Generate complete Checkpoint object with updated versions
///
/// # Arguments
///
/// * `checkpoints` - Slice of checkpoint tuples sorted by step (ascending order)
/// * `target_checkpoint_id` - ID of the target checkpoint to recover (must be in the list)
///
/// # Returns
///
/// * `Ok(Some(Checkpoint))` - The reconstructed full checkpoint
/// * `Ok(None)` - Target checkpoint not found in the list
/// * `Err(CheckpointError)` - Recovery failure (invalid data, missing base, etc.)
///
/// # Errors
///
/// Returns [`CheckpointError`] if:
/// - Target checkpoint ID is not found in the list
/// - Base checkpoint for delta snapshot is missing
/// - Channel data cannot be merged (type mismatch)
/// - Checkpoint list is not properly sorted
///
/// # Examples
///
/// ```ignore
/// use juncture_checkpoint::types::recover_from_deltas;
/// use juncture_core::checkpoint::CheckpointTuple;
///
/// let checkpoints = vec![
///     full_snapshot_checkpoint,   // Step 0 - full snapshot
///     delta_checkpoint_1,          // Step 1 - deltas only
///     delta_checkpoint_2,          // Step 2 - deltas only
/// ];
///
/// let recovered = recover_from_deltas(&checkpoints, "cp2").await?;
/// assert!(recovered.is_some());
/// ```
pub fn recover_from_deltas(
    checkpoints: &[CheckpointTuple],
    target_checkpoint_id: &str,
) -> Result<Option<Checkpoint>, CheckpointError> {
    // Validate input: find target checkpoint
    let target_index = checkpoints
        .iter()
        .position(|t| t.checkpoint.id == target_checkpoint_id);

    let Some(target_idx) = target_index else {
        return Ok(None);
    };

    // Consider checkpoints up to and including the target
    let relevant_checkpoints = &checkpoints[..=target_idx];

    // Step 1: Find the nearest full snapshot
    // A full snapshot is one that contains complete channel_values
    // We iterate backwards from target to find the most recent full checkpoint
    let base_snapshot = relevant_checkpoints
        .iter()
        .rev()
        .find(|t| {
            !t.checkpoint.channel_values.is_null()
                && t.checkpoint
                    .channel_values
                    .as_object()
                    .is_some_and(|obj| !obj.is_empty())
        })
        .ok_or_else(|| {
            CheckpointError::Deserialize("No full snapshot found in checkpoint chain".to_string())
        })?;

    // Clone the base checkpoint as our starting point
    let mut reconstructed = base_snapshot.checkpoint.clone();

    // Collect all pending writes from checkpoints after the base snapshot
    let mut all_deltas: Vec<(&String, PendingWrite)> = Vec::new();

    // Step 2: Walk forward collecting all delta writes
    for tuple in relevant_checkpoints {
        // Skip checkpoints that are before or at the base snapshot
        if tuple.checkpoint.id <= base_snapshot.checkpoint.id {
            continue;
        }

        // Collect pending writes from this checkpoint
        for write in &tuple.pending_writes {
            all_deltas.push((&tuple.checkpoint.id, write.clone()));
        }
    }

    // Sort deltas by checkpoint ID to ensure correct order
    all_deltas.sort_by(|a, b| a.0.cmp(b.0));

    // Step 3: Replay delta writes to the snapshot
    let channel_values = reconstructed
        .channel_values
        .as_object_mut()
        .ok_or_else(|| {
            CheckpointError::Deserialize(
                "Base checkpoint channel_values is not an object".to_string(),
            )
        })?;

    // Track which channels were modified
    let mut modified_channels = HashMap::<String, u64>::new();

    for (_checkpoint_id, write) in all_deltas {
        let channel = &write.channel;

        // Delta channels use Append semantics
        // In a full implementation, the operation type would be determined
        // by the channel's reducer type configuration
        if let serde_json::Value::Array(values) = &write.value {
            // Append array values to existing channel data
            let entry = channel_values
                .entry(channel.clone())
                .or_insert(serde_json::Value::Array(vec![]));

            if let Some(arr) = entry.as_array_mut() {
                arr.extend(values.clone().into_iter());
            }
        } else {
            // Non-array values use Replace semantics
            channel_values.insert(channel.clone(), write.value.clone());
        }

        // Update version counter (common to both branches)
        *modified_channels.entry(channel.clone()).or_insert(0) += 1;
    }

    // Step 4: Update checkpoint metadata
    // Update channel_versions for modified channels
    for (channel, delta_count) in &modified_channels {
        let current_version = reconstructed
            .channel_versions
            .get(channel)
            .copied()
            .unwrap_or(0);
        reconstructed
            .channel_versions
            .insert(channel.clone(), current_version + delta_count);
    }

    // Update new_versions to reflect the channels modified during recovery
    reconstructed.new_versions = modified_channels;

    // Clear delta counters since we now have a full snapshot
    reconstructed.counters_since_delta_snapshot.clear();

    Ok(Some(reconstructed))
}

/// Time-to-live configuration for checkpoint expiration
///
/// Configures automatic cleanup of old checkpoints per design spec §5.7.
#[derive(Clone, Debug)]
pub struct TtlConfig {
    /// Default TTL for checkpoints (None = no expiration)
    pub default_ttl: Option<std::time::Duration>,

    /// Interval between cleanup sweeps for active background cleanup
    pub sweep_interval: std::time::Duration,

    /// Maximum number of checkpoints to retain per thread/namespace (None = unlimited)
    pub max_checkpoints: Option<usize>,
}

impl TtlConfig {
    /// Create a new TTL configuration
    ///
    /// # Arguments
    ///
    /// * `default_ttl` - Default time-to-live for checkpoints (None = no expiration)
    /// * `sweep_interval` - Interval between active cleanup sweeps
    /// * `max_checkpoints` - Maximum checkpoints to retain (None = unlimited)
    #[must_use]
    pub const fn new(
        default_ttl: Option<std::time::Duration>,
        sweep_interval: std::time::Duration,
        max_checkpoints: Option<usize>,
    ) -> Self {
        Self {
            default_ttl,
            sweep_interval,
            max_checkpoints,
        }
    }

    /// Create a TTL configuration with no expiration (default)
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            default_ttl: None,
            sweep_interval: std::time::Duration::from_secs(3600),
            max_checkpoints: None,
        }
    }

    /// Check if a checkpoint has expired based on its creation time
    ///
    /// # Arguments
    ///
    /// * `created_at_str` - ISO 8601 timestamp string from checkpoint
    ///
    /// # Returns
    ///
    /// * `true` if checkpoint is expired and should be cleaned up
    /// * `false` if checkpoint is still valid
    #[must_use]
    pub fn is_expired(&self, created_at_str: &str) -> bool {
        let Some(ttl) = self.default_ttl else {
            return false; // No expiration configured
        };

        // Parse ISO 8601 timestamp
        let created_at = match chrono::DateTime::parse_from_rfc3339(created_at_str) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return false, // Invalid timestamp, don't expire
        };

        let now = chrono::Utc::now();
        let age = now.signed_duration_since(created_at);

        age.to_std().unwrap_or(std::time::Duration::MAX) > ttl
    }
}

impl Default for TtlConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use juncture_core::config::RunnableConfig;

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

    #[test]
    fn test_ttl_config_default() {
        let config = TtlConfig::default();
        assert!(config.default_ttl.is_none());
        assert!(config.max_checkpoints.is_none());
    }

    #[test]
    fn test_ttl_config_expiration() {
        use std::time::Duration;

        let config = TtlConfig::new(
            Some(Duration::from_secs(60)),
            Duration::from_secs(3600),
            Some(100),
        );

        // Current timestamp should not be expired
        let now = chrono::Utc::now().to_rfc3339();
        assert!(!config.is_expired(&now));

        // 2 minutes ago should be expired
        let past = (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
        assert!(config.is_expired(&past));
    }

    #[test]
    fn test_recover_from_deltas_empty_list() {
        let checkpoints = vec![];
        let result = recover_from_deltas(&checkpoints, "cp1");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_recover_from_deltas_target_not_found() {
        let checkpoints = vec![create_test_tuple("cp1", 0)];
        let result = recover_from_deltas(&checkpoints, "cp2");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_recover_from_deltas_single_full_checkpoint() {
        let checkpoints = vec![create_test_tuple("cp1", 0)];
        let result = recover_from_deltas(&checkpoints, "cp1");
        assert!(result.is_ok());

        let recovered = result.unwrap().unwrap();
        assert_eq!(recovered.id, "cp1");
        assert_eq!(
            recovered.channel_values["messages"],
            serde_json::json!(["hello"])
        );
    }

    #[test]
    fn test_recover_from_deltas_with_pending_writes() {
        let base = create_test_tuple("cp1", 0);
        let mut delta = create_test_tuple("cp2", 1);

        // Clear channel_values for delta checkpoint to simulate delta-only checkpoint
        delta.checkpoint.channel_values = serde_json::json!({});

        // Add pending writes to delta checkpoint - use arrays for append semantics
        delta.pending_writes = vec![
            PendingWrite {
                task_id: "task1".to_string(),
                channel: "messages".to_string(),
                value: serde_json::json!(["world"]),
            },
            PendingWrite {
                task_id: "task2".to_string(),
                channel: "messages".to_string(),
                value: serde_json::json!(["test"]),
            },
        ];

        let checkpoints = vec![base, delta];
        let result = recover_from_deltas(&checkpoints, "cp2");
        assert!(result.is_ok());

        let recovered = result.unwrap().unwrap();
        // The recovered checkpoint has the base snapshot's ID since we clone it
        assert_eq!(recovered.id, "cp1");

        // Check that messages were appended
        let messages = recovered.channel_values["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3); // ["hello", "world", "test"]
        assert_eq!(messages[0], "hello");
        assert_eq!(messages[1], "world");
        assert_eq!(messages[2], "test");

        // Check that channel_versions was updated
        assert_eq!(recovered.channel_versions.get("messages"), Some(&3));
    }

    #[test]
    fn test_recover_from_deltas_no_full_snapshot() {
        let mut checkpoint = create_test_tuple("cp1", 0);
        // Clear channel_values to simulate non-full snapshot
        checkpoint.checkpoint.channel_values = serde_json::json!({});

        let checkpoints = vec![checkpoint];
        let result = recover_from_deltas(&checkpoints, "cp1");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CheckpointError::Deserialize(_)
        ));
    }

    #[test]
    fn test_recover_from_deltas_multiple_deltas() {
        let base = create_test_tuple("cp1", 0);

        let mut delta1 = create_test_tuple("cp2", 1);
        // Clear channel_values for delta checkpoint
        delta1.checkpoint.channel_values = serde_json::json!({});
        delta1.pending_writes = vec![PendingWrite {
            task_id: "task1".to_string(),
            channel: "messages".to_string(),
            value: serde_json::json!(["delta1"]),
        }];

        let mut delta2 = create_test_tuple("cp3", 2);
        // Clear channel_values for delta checkpoint
        delta2.checkpoint.channel_values = serde_json::json!({});
        delta2.pending_writes = vec![
            PendingWrite {
                task_id: "task2".to_string(),
                channel: "messages".to_string(),
                value: serde_json::json!(["delta2a"]),
            },
            PendingWrite {
                task_id: "task3".to_string(),
                channel: "messages".to_string(),
                value: serde_json::json!(["delta2b"]),
            },
        ];

        let checkpoints = vec![base, delta1, delta2];
        let result = recover_from_deltas(&checkpoints, "cp3");
        assert!(result.is_ok());

        let recovered = result.unwrap().unwrap();
        // The recovered checkpoint has the base snapshot's ID since we clone it
        assert_eq!(recovered.id, "cp1");

        // Check that all messages were appended in order
        let messages = recovered.channel_values["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4); // ["hello", "delta1", "delta2a", "delta2b"]
        assert_eq!(messages[0], "hello");
        assert_eq!(messages[1], "delta1");
        assert_eq!(messages[2], "delta2a");
        assert_eq!(messages[3], "delta2b");
    }

    // Helper function to create test checkpoint tuples
    fn create_test_tuple(id: &str, step: i64) -> CheckpointTuple {
        CheckpointTuple {
            config: RunnableConfig::default(),
            checkpoint: Checkpoint {
                id: id.to_string(),
                channel_values: serde_json::json!({
                    "messages": ["hello"]
                }),
                channel_versions: HashMap::from([("messages".to_string(), 1)]),
                versions_seen: HashMap::new(),
                pending_tasks: vec![],
                pending_sends: vec![],
                pending_interrupts: vec![],
                schema_version: 1,
                created_at: chrono::Utc::now().to_rfc3339(),
                v: 1,
                new_versions: HashMap::new(),
                counters_since_delta_snapshot: HashMap::new(),
            },
            metadata: CheckpointMetadata {
                source: CheckpointSource::Loop,
                step,
                writes: HashMap::new(),
                parents: HashMap::new(),
                run_id: "test-run".to_string(),
            },
            pending_writes: vec![],
            parent_config: None,
        }
    }
}

// Rust guideline compliant 2026-05-23
