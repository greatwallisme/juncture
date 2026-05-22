//! Runtime context for graph execution
//!
//! The runtime provides external dependencies and execution metadata to nodes.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Non-generic stream writer trait for [`Runtime`] integration.
///
/// [`StreamWriter<S>`](crate::stream::StreamWriter) is parameterized over the
/// state type `S` which prevents it from being stored directly in
/// `Runtime<C>`. This trait provides type-erased access so nodes can emit
/// custom stream events through the runtime regardless of the state type.
pub trait StreamWriterTrait: Send + Sync + 'static {
    /// Emit a custom stream data payload.
    fn emit_custom(&self, node: &str, data: serde_json::Value);
}

impl StreamWriterTrait for mpsc::UnboundedSender<(String, serde_json::Value)> {
    fn emit_custom(&self, node: &str, data: serde_json::Value) {
        let _ = self.send((node.to_string(), data));
    }
}

impl std::fmt::Debug for dyn StreamWriterTrait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamWriterTrait").finish_non_exhaustive()
    }
}

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

    /// Heartbeat mechanism for long-running nodes
    pub heartbeat: Heartbeat,

    /// Previous execution return value (Functional API)
    pub previous: Option<serde_json::Value>,

    /// Execution metadata (checkpoint, task, thread info)
    pub execution_info: Option<ExecutionInfo>,

    /// Collaborative drain control for graceful shutdown
    pub control: Option<RunControl>,

    /// Custom stream event emitter.
    ///
    /// When set, nodes can emit custom stream data through this writer
    /// regardless of the state type. The writer is type-erased via
    /// [`StreamWriterTrait`] because `Runtime<C>` cannot directly hold
    /// the state-parameterized [`StreamWriter<S>`](crate::stream::StreamWriter).
    pub stream_writer: Option<Arc<dyn StreamWriterTrait>>,
}

impl<C: Clone + Send + Sync + 'static> std::fmt::Debug for Runtime<C>
where
    C: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("context", &self.context)
            .field("store", &self.store)
            .field("heartbeat", &self.heartbeat)
            .field("previous", &self.previous)
            .field("execution_info", &self.execution_info)
            .field("control", &self.control)
            .field("stream_writer", &self.stream_writer)
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
            heartbeat: Heartbeat::default(),
            previous: None,
            execution_info: None,
            control: None,
            stream_writer: None,
        }
    }

    /// Create a new runtime with custom context
    #[must_use]
    pub fn with_context(context: C) -> Self {
        Self {
            context,
            store: None,
            heartbeat: Heartbeat::default(),
            previous: None,
            execution_info: None,
            control: None,
            stream_writer: None,
        }
    }

    /// Set the execution info for this runtime
    ///
    /// Provides the runtime with execution metadata including step tracking
    /// and recursion limit, enabling nodes to query managed values.
    pub fn set_execution_info(&mut self, info: ExecutionInfo) {
        self.execution_info = Some(info);
    }

    /// Get the managed values for this runtime
    ///
    /// Returns information about recursion limits and remaining steps.
    /// Nodes can use this to adapt behavior based on remaining step budget,
    /// e.g., generating summaries instead of continuing when steps are low.
    #[must_use]
    pub fn managed_values(&self) -> ManagedValues {
        let Some(info) = self.execution_info.as_ref() else {
            return ManagedValues {
                is_last_step: false,
                remaining_steps: 25,
            };
        };

        let remaining = info.recursion_limit.saturating_sub(info.step);

        ManagedValues {
            is_last_step: remaining <= 1,
            remaining_steps: u32::try_from(remaining).unwrap_or(u32::MAX),
        }
    }

    /// Access the heartbeat for sending periodic alive signals
    ///
    /// Long-running nodes should call `heartbeat.ping()` periodically
    /// to prevent false idle timeout detection.
    #[must_use]
    pub const fn heartbeat(&self) -> &Heartbeat {
        &self.heartbeat
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

/// Heartbeat mechanism for long-running nodes
///
/// Nodes can send heartbeats to indicate they are still active,
/// preventing idle timeout detection. The heartbeat carries an
/// unbounded channel sender that signals the engine's idle-timeout
/// watchdog each time `ping()` is called.
///
/// Create paired heartbeat and watcher with [`Heartbeat::new_pair`]:
///
/// ```ignore
/// use juncture_core::Heartbeat;
/// use std::time::Duration;
///
/// let (heartbeat, mut watcher) = Heartbeat::new_pair();
/// heartbeat.ping().unwrap();
/// assert!(watcher.is_alive(Duration::from_secs(10)));
/// ```
pub struct Heartbeat {
    tx: tokio::sync::mpsc::UnboundedSender<()>,
    // Keeps the channel alive when no watcher is attached.
    // The receiver is stored only by the original (non-cloned) Heartbeat.
    // When dropped, all cloned senders will also fail on ping.
    _rx: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,
}

impl Clone for Heartbeat {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            // Only the original Heartbeat keeps the receiver alive.
            // Cloned senders still work because the original's receiver
            // keeps the channel open.
            _rx: None,
        }
    }
}

impl std::fmt::Debug for Heartbeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Heartbeat")
            .field("tx", &"<UnboundedSender>")
            .finish()
    }
}

impl Heartbeat {
    /// Create a new heartbeat from an unbounded sender
    #[must_use]
    pub const fn new(tx: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
        Self { tx, _rx: None }
    }

    /// Create a paired heartbeat sender and watcher
    ///
    /// Returns a `(Heartbeat, HeartbeatWatcher)` pair connected
    /// by an unbounded channel. The watcher can detect staleness
    /// by checking whether heartbeats arrived within the idle timeout.
    #[must_use]
    pub fn new_pair() -> (Self, HeartbeatWatcher) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = HeartbeatWatcher::new(rx);
        (Self { tx, _rx: None }, watcher)
    }

    /// Send a heartbeat signal
    ///
    /// # Errors
    ///
    /// Returns `Err` if the receiver has been dropped (engine shutdown).
    pub fn ping(&self) -> Result<(), tokio::sync::mpsc::error::SendError<()>> {
        self.tx.send(())
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self { tx, _rx: Some(rx) }
    }
}

/// Watches heartbeats and detects staleness for idle timeout detection
///
/// The watcher receives heartbeat signals from a paired [`Heartbeat`]
/// sender. Call [`is_alive`](Self::is_alive) to check whether a
/// heartbeat was received within the specified idle timeout duration.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::Heartbeat;
/// use std::time::Duration;
///
/// let (heartbeat, mut watcher) = Heartbeat::new_pair();
///
/// // Immediately after creation, the watcher considers the source alive
/// assert!(watcher.is_alive(Duration::from_secs(60)));
///
/// // After sending a heartbeat and checking with a short timeout
/// heartbeat.ping().unwrap();
/// assert!(watcher.is_alive(Duration::from_secs(10)));
/// ```
pub struct HeartbeatWatcher {
    rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    last_beat: std::time::Instant,
}

impl std::fmt::Debug for HeartbeatWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeartbeatWatcher")
            .field("last_beat", &self.last_beat)
            .finish_non_exhaustive()
    }
}

impl HeartbeatWatcher {
    /// Create a new heartbeat watcher from an unbounded receiver
    #[must_use]
    pub fn new(rx: tokio::sync::mpsc::UnboundedReceiver<()>) -> Self {
        Self {
            rx,
            last_beat: std::time::Instant::now(),
        }
    }

    /// Check if the watched heartbeat source is still alive
    ///
    /// Drains any pending heartbeat signals and returns `true` if
    /// at least one heartbeat was received within `idle_timeout`.
    /// Returns `false` if no heartbeat was received within the
    /// idle timeout duration.
    ///
    /// This is a non-blocking check.
    #[must_use]
    pub fn is_alive(&mut self, idle_timeout: Duration) -> bool {
        // Drain all pending heartbeats and update the last beat timestamp
        while self.rx.try_recv().is_ok() {
            self.last_beat = std::time::Instant::now();
        }
        self.last_beat.elapsed() < idle_timeout
    }
}

/// Execution metadata for a graph run
///
/// Contains information about the current execution including
/// checkpoint IDs, task IDs, retry counts, and step tracking.
#[derive(Clone, Debug)]
pub struct ExecutionInfo {
    /// Current checkpoint ID
    pub checkpoint_id: String,

    /// Checkpoint namespace (for subgraph isolation)
    pub checkpoint_ns: String,

    /// Current task ID
    pub task_id: String,

    /// Current superstep number (0-indexed)
    pub step: usize,

    /// Maximum allowed superstep count
    pub recursion_limit: usize,

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_managed_values_no_execution_info() {
        // When no execution_info is set, returns default values
        let runtime = Runtime::<()>::new();
        let values = runtime.managed_values();
        assert!(!values.is_last_step, "default should not be last step");
        assert_eq!(
            values.remaining_steps, 25,
            "default remaining steps should be 25"
        );
    }

    #[test]
    fn test_managed_values_early_step() {
        // Step 3 of 25: not last step, 22 remaining
        let mut runtime = Runtime::<()>::new();
        runtime.set_execution_info(ExecutionInfo {
            checkpoint_id: "cp-1".to_string(),
            checkpoint_ns: "default".to_string(),
            task_id: "task-1".to_string(),
            step: 3,
            recursion_limit: 25,
            thread_id: None,
            run_id: None,
            node_attempt: 1,
            node_first_attempt_time: None,
        });
        let values = runtime.managed_values();
        assert!(!values.is_last_step, "early step should not be last step");
        assert_eq!(values.remaining_steps, 22, "remaining: 25 - 3 = 22");
    }

    #[test]
    fn test_managed_values_last_step() {
        // Step 24 of 25: this is the last step, 1 remaining
        let mut runtime = Runtime::<()>::new();
        runtime.set_execution_info(ExecutionInfo {
            checkpoint_id: "cp-1".to_string(),
            checkpoint_ns: "default".to_string(),
            task_id: "task-1".to_string(),
            step: 24,
            recursion_limit: 25,
            thread_id: None,
            run_id: None,
            node_attempt: 1,
            node_first_attempt_time: None,
        });
        let values = runtime.managed_values();
        assert!(values.is_last_step, "step 24 of 25 should be last step");
        assert_eq!(values.remaining_steps, 1, "remaining: 25 - 24 = 1");
    }

    #[test]
    fn test_managed_values_past_recursion_limit() {
        // Step >= recursion_limit: remaining should be 0, is_last_step = true
        let mut runtime = Runtime::<()>::new();
        runtime.set_execution_info(ExecutionInfo {
            checkpoint_id: "cp-1".to_string(),
            checkpoint_ns: "default".to_string(),
            task_id: "task-1".to_string(),
            step: 25,
            recursion_limit: 25,
            thread_id: None,
            run_id: None,
            node_attempt: 1,
            node_first_attempt_time: None,
        });
        let values = runtime.managed_values();
        assert!(
            values.is_last_step,
            "step >= recursion_limit should be last step"
        );
        assert_eq!(
            values.remaining_steps, 0,
            "no remaining steps when at limit"
        );
    }

    #[test]
    fn test_managed_values_custom_recursion_limit() {
        // Custom recursion limit of 10, step 8: 2 remaining, not last step
        let mut runtime = Runtime::<()>::new();
        runtime.set_execution_info(ExecutionInfo {
            checkpoint_id: "cp-1".to_string(),
            checkpoint_ns: "default".to_string(),
            task_id: "task-1".to_string(),
            step: 8,
            recursion_limit: 10,
            thread_id: None,
            run_id: None,
            node_attempt: 1,
            node_first_attempt_time: None,
        });
        let values = runtime.managed_values();
        assert!(!values.is_last_step, "step 8 of 10 should not be last step");
        assert_eq!(values.remaining_steps, 2, "remaining: 10 - 8 = 2");
    }

    #[test]
    fn test_managed_values_exact_countdown() {
        // Step 9 of 10: last step, 1 remaining
        let mut runtime = Runtime::<()>::new();
        runtime.set_execution_info(ExecutionInfo {
            checkpoint_id: "cp-1".to_string(),
            checkpoint_ns: "default".to_string(),
            task_id: "task-1".to_string(),
            step: 9,
            recursion_limit: 10,
            thread_id: None,
            run_id: None,
            node_attempt: 1,
            node_first_attempt_time: None,
        });
        let values = runtime.managed_values();
        assert!(values.is_last_step, "step 9 of 10 should be last step");
        assert_eq!(values.remaining_steps, 1, "remaining: 10 - 9 = 1");
    }
}

// Rust guideline compliant 2026-05-22
