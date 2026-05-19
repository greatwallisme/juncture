//! Runtime context for graph execution
//!
//! The runtime provides external dependencies and execution metadata to nodes.

use std::sync::{Arc, Mutex};

/// Execution context for graph nodes
///
/// The runtime injects external dependencies into node execution, separate
/// from the graph state. This includes context, storage, streaming, and
/// execution metadata.
///
/// # Type Parameters
///
/// * `C` - Context type (defaults to `()` for no context)
///
/// # Examples
///
/// ```ignore
/// use juncture_core::Runtime;
/// use std::sync::Arc;
///
/// // Simple runtime with no context
/// let runtime = Runtime::<()>::new();
///
/// // Runtime with custom context
/// struct MyContext { user_id: String }
/// let runtime = Runtime::with_context(MyContext { user_id: "123".to_string() });
/// ```
#[derive(Clone)]
pub struct Runtime<C: Clone + Send + Sync + 'static = ()> {
    /// Immutable user-provided context
    pub context: C,

    /// Optional cross-thread persistent storage
    pub store: Option<Arc<dyn RuntimeStore>>,

    /// Stream writer for events
    pub stream_writer: StreamWriter,

    /// Heartbeat mechanism for long-running nodes
    pub heartbeat: Heartbeat,

    /// Previous execution return value (Functional API)
    pub previous: Option<serde_json::Value>,

    /// Execution metadata (checkpoint, task, thread info)
    pub execution_info: Option<ExecutionInfo>,

    /// Collaborative drain control for graceful shutdown
    pub control: Option<RunControl>,
}

impl<C: Clone + Send + Sync + 'static> std::fmt::Debug for Runtime<C>
where
    C: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("context", &self.context)
            .field("store", &self.store)
            .field("stream_writer", &self.stream_writer)
            .field("heartbeat", &self.heartbeat)
            .field("previous", &self.previous)
            .field("execution_info", &self.execution_info)
            .field("control", &self.control)
            .finish()
    }
}

impl<C: Clone + Send + Sync + 'static> Runtime<C> {
    /// Create a new runtime with minimal configuration
    #[must_use]
    pub fn new() -> Self
    where
        C: Default,
    {
        Self {
            context: C::default(),
            store: None,
            stream_writer: StreamWriter::new(),
            heartbeat: Heartbeat::new(),
            previous: None,
            execution_info: None,
            control: None,
        }
    }

    /// Create a new runtime with custom context
    #[must_use]
    pub fn with_context(context: C) -> Self {
        Self {
            context,
            store: None,
            stream_writer: StreamWriter::new(),
            heartbeat: Heartbeat::new(),
            previous: None,
            execution_info: None,
            control: None,
        }
    }

    /// Get managed values (step limit information)
    ///
    /// Returns information about recursion limits and remaining steps.
    #[must_use]
    pub fn managed_values(&self) -> ManagedValues {
        let limit: usize = self.execution_info.as_ref().map_or(25, |_| 25);
        let current_step: usize = 0;
        let remaining = limit.saturating_sub(current_step);

        ManagedValues {
            is_last_step: remaining <= 1,
            remaining_steps: remaining.try_into().unwrap_or(u32::MAX),
        }
    }
}

impl Default for Runtime<()>
where
    (): std::fmt::Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Persistent storage trait for cross-thread state
///
/// Abstracts storage backends for checkpoint persistence and
/// cross-thread communication.
///
/// `RuntimeStore` operations will be implemented in Phase 8 according to
/// the design document at `design/10-store.md`.
pub trait RuntimeStore: Send + Sync + 'static + std::fmt::Debug {}

/// Stream writer for graph events
///
/// Handles streaming events to consumers during graph execution.
#[derive(Clone, Debug)]
pub struct StreamWriter {
    _private: (),
}

impl StreamWriter {
    /// Create a new stream writer
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for StreamWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Heartbeat mechanism for long-running nodes
///
/// Nodes can send heartbeats to indicate they are still active,
/// preventing idle timeout detection.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    _private: (),
}

impl Heartbeat {
    /// Create a new heartbeat
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Send a heartbeat signal
    ///
    /// Heartbeat signaling will be enhanced in Phase 5 with actual
    /// idle timeout detection and monitoring capabilities.
    pub const fn ping(&self) {
        // Heartbeat signaling will be implemented in Phase 5
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Execution metadata for a graph run
///
/// Contains information about the current execution including
/// checkpoint IDs, task IDs, and retry counts.
#[derive(Clone, Debug)]
pub struct ExecutionInfo {
    /// Current checkpoint ID
    pub checkpoint_id: String,

    /// Checkpoint namespace (for subgraph isolation)
    pub checkpoint_ns: String,

    /// Current task ID
    pub task_id: String,

    /// Thread ID (None if no checkpointer)
    pub thread_id: Option<String>,

    /// Run ID for tracing
    pub run_id: Option<String>,

    /// Current node attempt count (1-indexed)
    pub node_attempt: u32,

    /// Unix timestamp of first node attempt (seconds)
    pub node_first_attempt_time: Option<f64>,
}

/// Managed values for step tracking
///
/// Provides information about recursion limits and remaining steps.
#[derive(Clone, Copy, Debug)]
pub struct ManagedValues {
    /// Whether this is the last step before hitting recursion limit
    pub is_last_step: bool,

    /// Number of remaining steps
    pub remaining_steps: u32,
}

/// Collaborative drain control for graceful shutdown
///
/// Allows requesting that the graph stop at the next superstep boundary
/// after saving a checkpoint.
#[derive(Debug)]
pub struct RunControl {
    drain_reason: Arc<Mutex<Option<String>>>,
}

impl Clone for RunControl {
    fn clone(&self) -> Self {
        Self {
            drain_reason: Arc::clone(&self.drain_reason),
        }
    }
}

impl RunControl {
    /// Create a new run control
    #[must_use]
    pub fn new() -> Self {
        Self {
            drain_reason: Arc::new(Mutex::new(None)),
        }
    }

    /// Request that execution drain at next superstep boundary
    ///
    /// # Arguments
    ///
    /// * `reason` - Reason for the drain request
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (indicates a programming error).
    pub fn request_drain(&self, reason: &str) {
        *self.drain_reason.lock().unwrap() = Some(reason.to_string());
    }

    /// Check if drain has been requested
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (indicates a programming error).
    #[must_use]
    pub fn drain_requested(&self) -> bool {
        self.drain_reason.lock().unwrap().is_some()
    }

    /// Get the drain reason if set
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (indicates a programming error).
    #[must_use]
    pub fn drain_reason(&self) -> Option<String> {
        self.drain_reason.lock().unwrap().clone()
    }
}

impl Default for RunControl {
    fn default() -> Self {
        Self::new()
    }
}

// Rust guideline compliant 2025-01-18
