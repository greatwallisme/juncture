//! Graph lifecycle callback trait and events
//!
//! This module provides the `GraphCallbackHandler` trait which allows users to
//! hook into key lifecycle events during graph execution. Callbacks are useful
//! for custom logging, metrics collection, and integrating with external systems.

use juncture_core::JunctureError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Graph lifecycle callback trait
///
/// Implement this trait to receive notifications of important events during
/// graph execution. All methods have default no-op implementations, so you only
/// need to implement the events you care about.
///
/// # Examples
///
/// ```
/// use juncture_tracing::callback::{GraphCallbackHandler, GraphInterruptEvent};
/// use juncture_core::JunctureError;
/// use std::sync::Arc;
///
/// struct MyCallbackHandler;
///
/// impl GraphCallbackHandler for MyCallbackHandler {
///     fn on_interrupt(&self, event: &GraphInterruptEvent) {
///         // Handle interrupt - e.g., log to file or send metrics
///         let _ = event;
///     }
///
///     fn on_graph_end(&self, result: &Result<(), JunctureError>) {
///         // Handle completion - e.g., record final status
///         let _ = result;
///     }
/// }
/// ```
pub trait GraphCallbackHandler: Send + Sync + 'static {
    /// Called when the graph is interrupted
    ///
    /// This method is invoked when a node triggers an interrupt during execution.
    ///
    /// # Parameters
    ///
    /// * `event` - Details about the interrupt event
    fn on_interrupt(&self, event: &GraphInterruptEvent) {
        let _ = event;
    }

    /// Called when the graph resumes from an interrupt
    ///
    /// This method is invoked when the graph continues execution after being
    /// interrupted.
    ///
    /// # Parameters
    ///
    /// * `event` - Details about the resume event
    fn on_resume(&self, event: &GraphResumeEvent) {
        let _ = event;
    }

    /// Called when a checkpoint is saved
    ///
    /// This method is invoked after a checkpoint is successfully persisted.
    ///
    /// # Parameters
    ///
    /// * `checkpoint_id` - Unique identifier for the checkpoint
    /// * `step` - The step number at which this checkpoint was created
    fn on_checkpoint_saved(&self, checkpoint_id: &str, step: usize) {
        let _ = (checkpoint_id, step);
    }

    /// Called when a node starts execution
    ///
    /// This method is invoked when a node begins processing.
    ///
    /// # Parameters
    ///
    /// * `node` - Name of the node starting execution
    /// * `task_id` - Unique identifier for this task instance
    fn on_node_start(&self, node: &str, task_id: &str) {
        let _ = (node, task_id);
    }

    /// Called when a node completes execution
    ///
    /// This method is invoked when a node finishes processing successfully.
    ///
    /// # Parameters
    ///
    /// * `node` - Name of the node that completed
    /// * `task_id` - Unique identifier for this task instance
    /// * `duration_ms` - Execution duration in milliseconds
    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        let _ = (node, task_id, duration_ms);
    }

    /// Called when a node encounters an error
    ///
    /// This method is invoked when a node fails during execution.
    ///
    /// # Parameters
    ///
    /// * `node` - Name of the node that failed
    /// * `error` - The error that occurred
    fn on_node_error(&self, node: &str, error: &JunctureError) {
        let _ = (node, error);
    }

    /// Called when the graph execution completes
    ///
    /// This method is invoked when the entire graph execution finishes,
    /// either successfully or with an error.
    ///
    /// # Parameters
    ///
    /// * `result` - The final result of the graph execution
    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        let _ = result;
    }
}

/// Blanket implementation for `Arc<dyn GraphCallbackHandler>`
///
/// This allows `Arc<dyn GraphCallbackHandler>` to be used directly as a callback handler.
impl<T: GraphCallbackHandler + ?Sized> GraphCallbackHandler for Arc<T> {
    fn on_interrupt(&self, event: &GraphInterruptEvent) {
        self.as_ref().on_interrupt(event);
    }

    fn on_resume(&self, event: &GraphResumeEvent) {
        self.as_ref().on_resume(event);
    }

    fn on_checkpoint_saved(&self, checkpoint_id: &str, step: usize) {
        self.as_ref().on_checkpoint_saved(checkpoint_id, step);
    }

    fn on_node_start(&self, node: &str, task_id: &str) {
        self.as_ref().on_node_start(node, task_id);
    }

    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        self.as_ref().on_node_end(node, task_id, duration_ms);
    }

    fn on_node_error(&self, node: &str, error: &JunctureError) {
        self.as_ref().on_node_error(node, error);
    }

    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        self.as_ref().on_graph_end(result);
    }
}

/// Adapter that wraps any [`GraphCallbackHandler`] and implements
/// [`juncture_core::observability::GraphLifecycleCallback`].
///
/// Use [`CallbackHandlerAdapter::new`] to create an instance, then pass the
/// resulting `Arc<CallbackHandlerAdapter>` to
/// [`RunnableConfig::with_callback_handler`].
///
/// [`RunnableConfig::with_callback_handler`]: juncture_core::config::RunnableConfig::with_callback_handler
///
/// # Examples
///
/// ```ignore
/// use std::sync::Arc;
/// use juncture_tracing::callback::{CallbackHandlerAdapter, GraphCallbackHandler};
/// use juncture_core::config::RunnableConfig;
///
/// struct MyHandler;
/// impl GraphCallbackHandler for MyHandler {}
///
/// let handler = Arc::new(MyHandler);
/// let adapter = CallbackHandlerAdapter::new(handler);
/// let config = RunnableConfig::new()
///     .with_callback_handler(adapter);
/// ```
pub struct CallbackHandlerAdapter {
    inner: Arc<dyn GraphCallbackHandler>,
}

impl CallbackHandlerAdapter {
    /// Create a new adapter wrapping the given [`GraphCallbackHandler`].
    #[must_use]
    pub fn new(handler: Arc<dyn GraphCallbackHandler>) -> Self {
        Self { inner: handler }
    }
}

impl std::fmt::Debug for CallbackHandlerAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackHandlerAdapter")
            .field("inner", &"<GraphCallbackHandler>")
            .finish()
    }
}

impl juncture_core::observability::GraphLifecycleCallback for CallbackHandlerAdapter {
    fn on_node_start(&self, node: &str, task_id: &str) {
        self.inner.on_node_start(node, task_id);
    }

    fn on_node_end(&self, node: &str, task_id: &str, duration_ms: u64) {
        self.inner.on_node_end(node, task_id, duration_ms);
    }

    fn on_node_error(&self, node: &str, error: &JunctureError) {
        self.inner.on_node_error(node, error);
    }

    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        self.inner.on_graph_end(result);
    }

    fn on_checkpoint_saved(&self, checkpoint_id: &str, step: usize) {
        self.inner.on_checkpoint_saved(checkpoint_id, step);
    }
}

/// Event payload for graph interruptions
///
/// Contains detailed information about an interruption event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphInterruptEvent {
    /// Name of the node that triggered the interrupt
    pub node: String,

    /// Interrupt payload
    pub payload: Value,

    /// Optional interrupt ID for named interrupts
    pub interrupt_id: Option<String>,

    /// Subgraph namespace (empty for top-level graphs)
    pub namespace: Vec<String>,

    /// Whether this interrupt is resumable
    pub resumable: bool,
}

/// Event payload for graph resume operations
///
/// Contains detailed information about a resume event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphResumeEvent {
    /// Name of the node being resumed
    pub node: String,

    /// Resume value passed to the node
    pub resume_value: Value,

    /// Subgraph namespace (empty for top-level graphs)
    pub namespace: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCallback {
        node_starts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl GraphCallbackHandler for TestCallback {
        fn on_node_start(&self, node: &str, _task_id: &str) {
            self.node_starts.lock().unwrap().push(node.to_string());
        }
    }

    #[test]
    fn test_callback_handler_default_impl() {
        struct NoOpHandler;
        impl GraphCallbackHandler for NoOpHandler {}

        let handler = NoOpHandler;
        let event = GraphInterruptEvent {
            node: "test".to_string(),
            payload: Value::Null,
            interrupt_id: None,
            namespace: vec![],
            resumable: true,
        };

        // Should not panic
        handler.on_interrupt(&event);
        handler.on_checkpoint_saved("test-id", 0);
        handler.on_node_start("test", "task-1");
        handler.on_node_end("test", "task-1", 100);
        handler.on_graph_end(&Ok(()));
    }

    #[test]
    fn test_callback_handler_custom_impl() {
        let node_starts = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let handler = TestCallback {
            node_starts: Arc::clone(&node_starts),
        };

        handler.on_node_start("node1", "task-1");
        handler.on_node_start("node2", "task-2");

        let starts = node_starts.lock().unwrap();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0], "node1");
        assert_eq!(starts[1], "node2");
        drop(starts);
    }

    #[test]
    fn test_arc_callback_handler() {
        let node_starts = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let handler = std::sync::Arc::new(TestCallback {
            node_starts: Arc::clone(&node_starts),
        });

        handler.on_node_start("node1", "task-1");

        let starts = node_starts.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0], "node1");
        drop(starts);
    }

    #[test]
    fn test_interrupt_event_serialization() {
        let event = GraphInterruptEvent {
            node: "agent".to_string(),
            payload: Value::String("test_payload".to_string()),
            interrupt_id: Some("interrupt-1".to_string()),
            namespace: vec![],
            resumable: true,
        };

        let json_str = serde_json::to_string(&event).unwrap();
        let deserialized: GraphInterruptEvent = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.node, "agent");
        assert_eq!(deserialized.interrupt_id, Some("interrupt-1".to_string()));
        assert!(deserialized.resumable);
    }

    #[test]
    fn test_resume_event_serialization() {
        let event = GraphResumeEvent {
            node: "agent".to_string(),
            resume_value: Value::String("resume_value".to_string()),
            namespace: vec!["subgraph".to_string()],
        };

        let json_str = serde_json::to_string(&event).unwrap();
        let deserialized: GraphResumeEvent = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.node, "agent");
        assert_eq!(deserialized.namespace.len(), 1);
        assert_eq!(deserialized.namespace[0], "subgraph");
    }
}

// Rust guideline compliant 2026-05-19
