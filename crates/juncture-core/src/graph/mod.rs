//! Graph building, compilation, and topology validation
//!
//! This module provides the core graph construction API for Juncture.
//! It includes:
//! - [`StateGraph`]: Builder for constructing executable graphs
//! - [`CompiledGraph`]: Optimized, validated graph for execution
//! - [`TopologyValidator`]: Ensures graph structure is valid
//! - [`TopologyError`]: Validation failure details
//!
//! # Examples
//!
//! ```ignore
//! use juncture_core::{StateGraph, State, Node, IntoNode};
//!
//! struct MyState;
//! impl State for MyState { type Update = MyStateUpdate; }
//! struct MyStateUpdate;
//!
//! // Build a simple graph
//! let mut graph = StateGraph::<MyState>::new();
//! graph.add_node_simple("process", |state: MyState| async move {
//!     Ok(MyStateUpdate)
//! });
//! graph.set_entry_point("process");
//! graph.set_finish_point("process");
//!
//! // Compile and validate
//! let compiled = graph.compile()?;
//! # Ok::<(), juncture_core::graph::TopologyError>(())
//! ```

mod builder;
mod compiled;
mod remote;
mod topology;

pub use builder::{
    CompileConfig, ErrorHandlerNode, NodeMetadata, RetryPolicy, RetryingNode, StateGraph,
    TimeoutNode, execute_with_retry, execute_with_timeout,
};
pub use compiled::{
    CompiledGraph, DrawableEdge, DrawableGraph, DrawableNode, GraphOutput, GraphOutputMetadata,
    InterruptInfo, StateFilter, StateUpdate, StreamHandle, SubgraphInfo,
};
pub use remote::RemoteGraph;
pub use topology::TopologyError;

// Rust guideline compliant 2026-05-19
