//! Debug event types for Juncture graph execution
//!
//! This module defines the `DebugEvent` enum which represents various events
//! that occur during graph execution. These events can be streamed to consumers
//! for debugging and observability purposes.

use serde::Serialize;

/// Debug events emitted during graph execution
///
/// These events provide detailed visibility into the internal execution flow
/// of a Juncture graph, including node execution, channel operations, and
/// state transitions.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event_type")]
pub enum DebugEvent {
    /// Graph execution has started
    GraphStart {
        /// Thread ID for this execution
        thread_id: String,
        /// Input state serialized as JSON
        input: serde_json::Value,
    },

    /// A superstep has started
    SuperstepStart {
        /// Step number
        step: usize,
        /// Nodes pending execution in this step
        pending_nodes: Vec<String>,
    },

    /// A node has started execution
    NodeStart {
        /// Node name
        node: String,
        /// Current step number
        step: usize,
    },

    /// A node has completed execution
    NodeEnd {
        /// Node name
        node: String,
        /// Current step number
        step: usize,
        /// Execution duration in milliseconds
        duration_ms: u64,
        /// Output type (update/command/interrupt)
        output_type: String,
    },

    /// A node has encountered an error
    NodeError {
        /// Node name
        node: String,
        /// Current step number
        step: usize,
        /// Error message
        error: String,
    },

    /// A value has been written to a channel
    ChannelWrite {
        /// Channel name
        channel: String,
        /// Node that performed the write
        node: String,
        /// Summary of the value written
        value_summary: String,
    },

    /// A channel has been updated with a new version
    ChannelUpdate {
        /// Channel name
        channel: String,
        /// New version number
        new_version: u64,
    },

    /// State merge operation has completed
    Merge {
        /// Step number
        step: usize,
        /// Channels that were updated
        channels_updated: Vec<String>,
    },

    /// An edge has been traversed during execution
    EdgeTraversed {
        /// Source node
        from: String,
        /// Target node
        to: String,
        /// Edge type (conditional/normal)
        edge_type: String,
    },

    /// A checkpoint has been saved
    CheckpointSaved {
        /// Checkpoint ID
        checkpoint_id: String,
        /// Step number
        step: usize,
        /// Checkpoint source (input/loop/interrupt)
        source: String,
    },

    /// Budget check has been performed
    BudgetCheck {
        /// Total tokens used
        tokens_used: u64,
        /// Total cost in USD
        cost_usd: f64,
        /// Remaining budget percentage
        budget_remaining_pct: f32,
    },

    /// Graph execution has completed
    GraphEnd {
        /// Total number of steps executed
        total_steps: usize,
        /// Total execution duration in milliseconds
        total_duration_ms: u64,
    },
}

impl DebugEvent {
    /// Check if this event is a graph start event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::GraphStart {
    ///     thread_id: "test".to_string(),
    ///     input: serde_json::json!({}),
    /// };
    /// assert!(event.is_graph_start());
    /// ```
    #[must_use]
    pub const fn is_graph_start(&self) -> bool {
        matches!(self, Self::GraphStart { .. })
    }

    /// Check if this event is a graph end event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::GraphEnd {
    ///     total_steps: 5,
    ///     total_duration_ms: 1000,
    /// };
    /// assert!(event.is_graph_end());
    /// ```
    #[must_use]
    pub const fn is_graph_end(&self) -> bool {
        matches!(self, Self::GraphEnd { .. })
    }

    /// Check if this event is a node start event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::NodeStart {
    ///     node: "agent".to_string(),
    ///     step: 0,
    /// };
    /// assert!(event.is_node_start());
    /// ```
    #[must_use]
    pub const fn is_node_start(&self) -> bool {
        matches!(self, Self::NodeStart { .. })
    }

    /// Check if this event is a node end event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::NodeEnd {
    ///     node: "agent".to_string(),
    ///     step: 0,
    ///     duration_ms: 100,
    ///     output_type: "update".to_string(),
    /// };
    /// assert!(event.is_node_end());
    /// ```
    #[must_use]
    pub const fn is_node_end(&self) -> bool {
        matches!(self, Self::NodeEnd { .. })
    }

    /// Check if this event is a node error event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::NodeError {
    ///     node: "agent".to_string(),
    ///     step: 0,
    ///     error: "Failed".to_string(),
    /// };
    /// assert!(event.is_node_error());
    /// ```
    #[must_use]
    pub const fn is_node_error(&self) -> bool {
        matches!(self, Self::NodeError { .. })
    }

    /// Check if this event is a superstep start event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::SuperstepStart {
    ///     step: 0,
    ///     pending_nodes: vec!["agent".to_string()],
    /// };
    /// assert!(event.is_superstep_start());
    /// ```
    #[must_use]
    pub const fn is_superstep_start(&self) -> bool {
        matches!(self, Self::SuperstepStart { .. })
    }

    /// Check if this event is a checkpoint saved event
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let event = DebugEvent::CheckpointSaved {
    ///     checkpoint_id: "abc123".to_string(),
    ///     step: 2,
    ///     source: "loop".to_string(),
    /// };
    /// assert!(event.is_checkpoint_saved());
    /// ```
    #[must_use]
    pub const fn is_checkpoint_saved(&self) -> bool {
        matches!(self, Self::CheckpointSaved { .. })
    }

    /// Check if this event is an error event
    ///
    /// Returns true for node errors and any other error-type events.
    ///
    /// # Examples
    ///
    /// ```
    /// use juncture_tracing::debug::DebugEvent;
    ///
    /// let error_event = DebugEvent::NodeError {
    ///     node: "agent".to_string(),
    ///     step: 0,
    ///     error: "Failed".to_string(),
    /// };
    /// assert!(error_event.is_error());
    ///
    /// let normal_event = DebugEvent::NodeStart {
    ///     node: "agent".to_string(),
    ///     step: 0,
    /// };
    /// assert!(!normal_event.is_error());
    /// ```
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::NodeError { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_event_graph_start() {
        let event = DebugEvent::GraphStart {
            thread_id: "thread-1".to_string(),
            input: serde_json::json!({"key": "value"}),
        };
        assert!(event.is_graph_start());
        assert!(!event.is_graph_end());
        assert!(!event.is_error());
    }

    #[test]
    fn test_debug_event_graph_end() {
        let event = DebugEvent::GraphEnd {
            total_steps: 10,
            total_duration_ms: 5000,
        };
        assert!(event.is_graph_end());
        assert!(!event.is_graph_start());
    }

    #[test]
    fn test_debug_event_node_lifecycle() {
        let start = DebugEvent::NodeStart {
            node: "agent".to_string(),
            step: 0,
        };
        assert!(start.is_node_start());

        let end = DebugEvent::NodeEnd {
            node: "agent".to_string(),
            step: 0,
            duration_ms: 100,
            output_type: "update".to_string(),
        };
        assert!(end.is_node_end());

        let error = DebugEvent::NodeError {
            node: "agent".to_string(),
            step: 0,
            error: "Something went wrong".to_string(),
        };
        assert!(error.is_node_error());
        assert!(error.is_error());
    }

    #[test]
    fn test_debug_event_serialization() {
        let event = DebugEvent::NodeStart {
            node: "agent".to_string(),
            step: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("NodeStart"));
        assert!(json.contains("agent"));
    }

    #[test]
    fn test_debug_event_superstep() {
        let event = DebugEvent::SuperstepStart {
            step: 1,
            pending_nodes: vec!["agent".to_string(), "tools".to_string()],
        };
        assert!(event.is_superstep_start());
    }

    #[test]
    fn test_debug_event_checkpoint() {
        let event = DebugEvent::CheckpointSaved {
            checkpoint_id: "ckpt-123".to_string(),
            step: 2,
            source: "interrupt".to_string(),
        };
        assert!(event.is_checkpoint_saved());
    }
}

// Rust guideline compliant 2026-05-19
