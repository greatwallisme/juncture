//! Edge system for graph routing
//!
//! This module provides types for defining edges between nodes in a Juncture graph.
//! Edges can be fixed (static) or conditional (dynamic routing based on state).

mod compiled;
mod r#types;

pub use compiled::{CompiledEdge, TriggerSource, TriggerTable};
pub use r#types::{Edge, PathMap, RouteResult, Router};

/// Sentinel constant for graph entry point
///
/// Used as the virtual start node when setting entry points via [`super::StateGraph::set_entry_point`].
pub const START: &str = "__start__";

/// Sentinel constant for graph termination
///
/// Routing to `END` indicates that execution path has completed.
pub const END: &str = "__end__";

// Rust guideline compliant 2025-01-18
