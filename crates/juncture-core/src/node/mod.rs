//! Node system for graph execution
//!
//! This module provides the [`Node`] trait and conversion utilities for creating
//! nodes from async functions. Nodes are the basic unit of execution in a Juncture graph.

mod into_node;
mod r#trait;

pub use into_node::{
    IntoNode, NodeFnCommand, NodeFnCommandWithConfig, NodeFnUpdate, NodeFnUpdateWithConfig,
};
pub use r#trait::Node;

/// Error information for node execution failures
///
/// Contains details about which node failed, the error that occurred,
/// the state at time of failure, and the attempt count.
#[derive(Debug)]
pub struct NodeError<S: crate::State> {
    /// Name of the node that failed
    pub node: String,

    /// The error that caused the failure
    pub error: crate::JunctureError,

    /// State snapshot at time of failure
    pub state: S,

    /// Current attempt count (1-indexed)
    pub attempt: u32,
}

impl<S: crate::State> std::fmt::Display for NodeError<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Node '{}' failed on attempt {}: {}",
            self.node, self.attempt, self.error
        )
    }
}

impl<S: crate::State> std::error::Error for NodeError<S> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

// Rust guideline compliant 2025-01-18
