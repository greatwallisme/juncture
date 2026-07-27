//! Node system for graph execution
//!
//! This module provides the [`Node`] trait and conversion utilities for creating
//! nodes from async functions. Nodes are the basic unit of execution in a Juncture graph.

mod into_node;
mod r#trait;

pub use into_node::{
    IntoNode, NodeFnCommand, NodeFnCommandWithConfig, NodeFnCommandWithConfigAndRuntime,
    NodeFnCommandWithRuntime, NodeFnUpdate, NodeFnUpdateWithConfig,
    NodeFnUpdateWithConfigAndRuntime, NodeFnUpdateWithRuntime,
};
pub use r#trait::Node;

// Rust guideline compliant 2025-01-18
