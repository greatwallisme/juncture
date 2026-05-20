//! Checkpoint persistence types and traits
//!
//! Defines the checkpoint saver trait and all checkpoint-related types.
//!
//! Storage implementations (`MemorySaver`, `SqliteSaver`, etc.) are provided
//! by the `juncture-checkpoint` crate, which implements this trait.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::RunnableConfig;

/// Separator used between namespace segments in checkpoint namespace strings.
///
/// The pipe character `|` is used instead of colon `:` to avoid ambiguity
/// with UUID v6 string representation which already contains colons.
/// See design doc 04-checkpoint.md, Implementation Note C-04-5.
pub const CHECKPOINT_NS_SEPARATOR: &str = "|";

/// Checkpoint operation errors
///
/// Represents all possible errors that can occur during checkpoint operations.
/// This type is defined in `juncture-core` for use in the `CheckpointSaver` trait.
/// The juncture-checkpoint crate provides a compatible implementation with
/// additional storage-specific errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CheckpointError {
    /// Serialization failed
    #[error("Serialization failed: {0}")]
    Serialize(String),

    /// Deserialization failed
    #[error("Deserialization failed: {0}")]
    Deserialize(String),

    /// Checkpoint not found
    #[error("Checkpoint not found: thread={thread_id}, id={checkpoint_id}")]
    NotFound {
        /// Thread identifier
        thread_id: String,
        /// Checkpoint identifier
        checkpoint_id: String,
    },

    /// Storage operation error
    #[error("Storage error: {0}")]
    Storage(String),

    /// Other checkpoint errors
    #[error("Checkpoint error: {0}")]
    Other(String),
}

impl From<serde_json::Error> for CheckpointError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialize(err.to_string())
    }
}

/// Namespace for checkpoint isolation in subgraph execution
///
/// Provides hierarchical namespace isolation to prevent checkpoint
/// collisions when executing nested subgraphs.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::checkpoint::CheckpointNamespace;
///
/// let root_ns = CheckpointNamespace::root();
/// let child_ns = root_ns.child("agent_a");
/// let grandchild_ns = child_ns.child("step_1");
///
/// assert_eq!(root_ns.as_str(), "");
/// assert_eq!(child_ns.as_str(), "agent_a");
/// assert_eq!(grandchild_ns.as_str(), "agent_a|step_1");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CheckpointNamespace {
    /// Namespace segments forming a hierarchical path
    pub segments: Vec<String>,
}

impl CheckpointNamespace {
    /// Create a new root namespace (empty path)
    #[must_use]
    pub const fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Create a namespace from segments
    #[must_use]
    pub const fn new(segments: Vec<String>) -> Self {
        Self { segments }
    }

    /// Create a child namespace
    #[must_use]
    pub fn child(&self, name: &str) -> Self {
        let mut segments = self.segments.clone();
        segments.push(name.to_string());
        Self { segments }
    }

    /// Get the parent namespace
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.is_empty() {
            None
        } else {
            let segments = self.segments[..self.segments.len() - 1].to_vec();
            Some(Self { segments })
        }
    }

    /// Check if this is a root namespace
    #[must_use]
    pub const fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Convert to string representation
    ///
    /// Uses [`CHECKPOINT_NS_SEPARATOR`] as the separator between namespace segments
    /// as per design doc 04-checkpoint.md, Implementation Note C-04-5.
    #[must_use]
    pub fn as_str(&self) -> String {
        self.segments.join(CHECKPOINT_NS_SEPARATOR)
    }

    /// Convert to string representation (alias for `as_str`)
    ///
    /// Note: This method shadows the `Display::to_string` implementation.
    /// Use `as_str()` or the `Display` trait instead.
    #[allow(
        clippy::should_implement_trait,
        clippy::inherent_to_string_shadow_display,
        reason = "required by design spec 04-027"
    )]
    #[must_use]
    pub fn to_string(&self) -> String {
        self.as_str()
    }

    /// Parse from string representation
    ///
    /// Splits on [`CHECKPOINT_NS_SEPARATOR`] to reconstruct namespace segments.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        if s.is_empty() {
            Self::root()
        } else {
            Self {
                segments: s
                    .split(CHECKPOINT_NS_SEPARATOR)
                    .map(String::from)
                    .collect(),
            }
        }
    }
}

impl std::fmt::Display for CheckpointNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<Vec<String>> for CheckpointNamespace {
    fn from(segments: Vec<String>) -> Self {
        Self::new(segments)
    }
}

impl From<&str> for CheckpointNamespace {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

/// Single namespace segment with node name and invocation UUID
///
/// Represents one level in a hierarchical checkpoint namespace,
/// combining a node name with a unique invocation identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NamespaceSegment {
    /// Node name for this segment
    pub node_name: String,

    /// Unique invocation identifier (UUID v4)
    pub invocation_id: String,
}

impl NamespaceSegment {
    /// Create a new namespace segment
    ///
    /// # Arguments
    ///
    /// * `node_name` - The node name
    /// * `invocation_id` - The unique invocation ID
    #[must_use]
    pub const fn new(node_name: String, invocation_id: String) -> Self {
        Self {
            node_name,
            invocation_id,
        }
    }

    /// Get the segment as a string
    ///
    /// Returns the segment in the format `node_name:invocation_id`.
    #[must_use]
    pub fn as_str(&self) -> String {
        format!("{}:{}", self.node_name, self.invocation_id)
    }
}

impl std::fmt::Display for NamespaceSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Checkpoint persistence interface
///
/// Trait defining operations for saving, loading, and listing checkpoints.
/// This trait is implemented by storage backends in the `juncture-checkpoint` crate.
///
/// # Example
///
/// ```ignore
/// use juncture_core::CheckpointSaver;
/// use juncture_checkpoint::MemorySaver;
///
/// let saver = MemorySaver::new();
/// // Use saver as a CheckpointSaver trait object...
/// ```
#[async_trait]
pub trait CheckpointSaver: Send + Sync + 'static {
    /// Get checkpoint tuple by configuration
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] if retrieval fails.
    async fn get_tuple(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<CheckpointTuple>, CheckpointError>;

    /// List checkpoints with optional filtering
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] if listing fails.
    async fn list(
        &self,
        config: &RunnableConfig,
        filter: Option<CheckpointFilter>,
    ) -> Result<Vec<CheckpointTuple>, CheckpointError>;

    /// Save a checkpoint
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] if saving fails.
    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: Checkpoint,
        metadata: CheckpointMetadata,
    ) -> Result<RunnableConfig, CheckpointError>;

    /// Save incremental writes from a completed task
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] if saving fails.
    async fn put_writes(
        &self,
        config: &RunnableConfig,
        writes: Vec<PendingWrite>,
        task_id: &str,
    ) -> Result<(), CheckpointError>;
}

/// Complete checkpoint state
///
/// Captures the entire state of a graph execution at a specific point in time,
/// including channel values, versions, pending tasks, and metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique checkpoint identifier (UUID v6, time-ordered)
    pub id: String,

    /// Serialized channel values (JSON or `MessagePack`)
    pub channel_values: serde_json::Value,

    /// Version number for each channel
    ///
    /// Keys are channel names, values are monotonically increasing version numbers.
    pub channel_versions: HashMap<String, u64>,

    /// Versions of channels each node has consumed
    ///
    /// Outer key is node name, inner key is channel name, value is version consumed.
    pub versions_seen: HashMap<String, HashMap<String, u64>>,

    /// Tasks pending execution in the next superstep
    pub pending_tasks: Vec<CheckpointPendingTask>,

    /// Pending Send operations awaiting delivery
    pub pending_sends: Vec<SerializedSend>,

    /// Interrupt signals captured when execution was interrupted
    ///
    /// Populated when checkpoint source is `CheckpointSource::Interrupt`.
    /// Used for ID-based resume to match incoming resume values.
    #[serde(default)]
    pub pending_interrupts: Vec<crate::interrupt::InterruptSignal>,

    /// State schema version for migration support
    pub schema_version: u32,

    /// ISO 8601 timestamp of checkpoint creation
    pub created_at: String,

    /// Checkpoint format version
    ///
    /// Used for forward compatibility when Checkpoint structure changes.
    pub v: u32,

    /// Channels updated in this checkpoint
    ///
    /// Keys are channel names, values are the new version numbers.
    pub new_versions: HashMap<String, u64>,

    /// Delta counters since last full snapshot
    ///
    /// Keys are channel names, values track changes since last complete snapshot.
    pub counters_since_delta_snapshot: HashMap<String, DeltaCounters>,
}

/// Delta tracking counters for a channel
///
/// Tracks incremental changes since the last complete snapshot,
/// enabling efficient `DeltaChannel` optimization.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeltaCounters {
    /// Number of updates since last snapshot
    pub updates: u64,

    /// Number of supersteps since last snapshot
    pub supersteps: u64,
}

/// Checkpoint metadata
///
/// Provides context about how and when a checkpoint was created.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Source of the checkpoint creation
    pub source: CheckpointSource,

    /// Superstep sequence number
    pub step: i64,

    /// Summary of writes from each node in this superstep
    pub writes: HashMap<String, serde_json::Value>,

    /// Parent checkpoint relationships
    ///
    /// Maps namespace to parent `checkpoint_id`.
    pub parents: HashMap<String, String>,

    /// Unique identifier for this execution run
    pub run_id: String,
}

/// Source of checkpoint creation
///
/// Indicates what triggered the checkpoint to be created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CheckpointSource {
    /// Initial state when graph execution begins
    Input,

    /// End of each superstep loop iteration
    Loop,

    /// External state update via `update_state()`
    Update,

    /// Fork from a historical checkpoint
    Fork,

    /// Interrupt triggered by human-in-the-loop interaction
    Interrupt { node: String },
}

/// Complete checkpoint tuple with all context
///
/// Combines checkpoint data with its metadata, configuration, and pending writes.
/// This is the primary structure returned from checkpoint storage for recovery.
#[derive(Clone, Debug)]
pub struct CheckpointTuple {
    /// Configuration containing `thread_id`, `checkpoint_id`, and `checkpoint_ns`
    pub config: RunnableConfig,

    /// The checkpoint itself
    pub checkpoint: Checkpoint,

    /// Checkpoint metadata
    pub metadata: CheckpointMetadata,

    /// Incremental writes since this checkpoint
    ///
    /// Used for crash recovery: these writes completed after the checkpoint
    /// and before the next checkpoint, so they don't need to be re-executed.
    pub pending_writes: Vec<PendingWrite>,

    /// Parent checkpoint configuration
    ///
    /// Used for time-travel navigation.
    pub parent_config: Option<RunnableConfig>,
}

/// Pending write from a completed task
///
/// Represents a channel write that completed after checkpoint creation
/// but before the next checkpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingWrite {
    /// ID of the task that produced this write
    pub task_id: String,

    /// Target channel name
    pub channel: String,

    /// Serialized value to write
    pub value: serde_json::Value,
}

/// Pending task in checkpoint
///
/// Represents a task scheduled for execution in the next superstep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointPendingTask {
    /// Unique task identifier (UUID)
    pub id: String,

    /// Target node name to execute
    pub node: String,

    /// Channels that triggered this task
    pub triggers: Vec<String>,

    /// Optional state override (used in Send API scenarios)
    pub state_override: Option<serde_json::Value>,
}

/// Serialized Send operation
///
/// Represents a Send object flowing through the `__pregel_tasks` channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerializedSend {
    /// Destination node name
    pub node: String,

    /// Serialized state override
    pub state: serde_json::Value,
}

/// Delta operation type
///
/// Defines how to apply delta values to a channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeltaOp {
    /// Append values to existing channel data
    Append,

    /// Replace entire channel data
    Replace,
}

/// Checkpoint listing filter
///
/// Used to query checkpoint history with specific criteria.
#[derive(Clone, Debug, Default)]
pub struct CheckpointFilter {
    /// Filter by checkpoint source
    pub source: Option<CheckpointSource>,

    /// Minimum step number (inclusive)
    pub step_gte: Option<i64>,

    /// Maximum step number (inclusive)
    pub step_lte: Option<i64>,

    /// Only checkpoints before this `checkpoint_id`
    pub before: Option<String>,

    /// Only checkpoints after this `checkpoint_id`
    pub after: Option<String>,

    /// Maximum number of checkpoints to return
    pub limit: Option<usize>,
}

/// State snapshot at a specific checkpoint
///
/// Represents the deserialized, fully-hydrated execution state at a checkpoint.
///
/// # Type Parameters
///
/// * `S` - State type implementing the [`crate::State`] trait
#[derive(Clone, Debug)]
pub struct StateSnapshot<S: crate::State> {
    /// The complete state values
    pub values: S,

    /// Next nodes to execute
    pub next: Vec<String>,

    /// Configuration with `checkpoint_id` for time-travel
    pub config: RunnableConfig,

    /// Checkpoint metadata
    pub metadata: CheckpointMetadata,

    /// ISO 8601 creation timestamp
    pub created_at: String,

    /// Parent checkpoint configuration
    pub parent_config: Option<RunnableConfig>,

    /// Task information for current superstep
    pub tasks: Vec<PregelTaskInfo>,
}

/// Pregel task information
///
/// Provides execution status for tasks in a superstep.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PregelTaskInfo {
    /// Task identifier
    pub id: String,

    /// Node name being executed
    pub node_name: String,

    /// Error if task failed
    pub error: Option<String>,

    /// Interrupt values if task was interrupted
    pub interrupts: Vec<serde_json::Value>,
}

/// Generate a new time-ordered checkpoint ID using UUID v6.
///
/// UUID v6 reorders the timestamp bits from UUID v1 for lexicographic
/// sortability, making checkpoint IDs suitable for range queries and
/// time-ordered iteration without a separate timestamp column.
///
/// The node ID is derived from random bytes to ensure uniqueness across
/// processes without requiring a persistent MAC address.
///
/// # Panics
///
/// Will not panic under normal circumstances. The uuid crate handles
/// timestamp generation internally using a shared atomic context.
#[must_use]
pub fn generate_checkpoint_id() -> String {
    // Random 6-byte node ID avoids the need for a persistent IEEE 802
    // MAC address while still guaranteeing global uniqueness when
    // combined with the timestamp and monotonic counter.
    let node_id: [u8; 6] = rand::random();
    uuid::Uuid::now_v6(&node_id).to_string()
}

// Rust guideline compliant 2026-05-21
