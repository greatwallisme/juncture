use crate::{State, edge::Router};
use std::{collections::HashMap, sync::Arc};

/// Trigger table for compiled graph execution
///
/// Maps nodes to their outgoing edges (which nodes they trigger) and
/// incoming edges (what triggers them).
///
/// # Examples
///
/// ```ignore
/// use juncture_core::edge::{TriggerTable, CompiledEdge, TriggerSource};
/// use std::collections::HashMap;
///
/// let mut trigger_table = TriggerTable::<MyState>::new();
/// trigger_table.outgoing.insert(
///     "node_a".to_string(),
///     vec![CompiledEdge::Fixed { target: "node_b".to_string() }],
/// );
/// ```
///
/// [`MyState`]: crate::State
#[derive(Clone, Debug)]
pub struct TriggerTable<S: State> {
    /// Map of node name to its outgoing edges
    pub outgoing: HashMap<String, Vec<CompiledEdge<S>>>,

    /// Map of node name to sources that trigger it
    pub incoming: HashMap<String, Vec<TriggerSource>>,
}

impl<S: State> Default for TriggerTable<S> {
    fn default() -> Self {
        Self {
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
        }
    }
}

impl<S: State> TriggerTable<S> {
    /// Create a new empty trigger table
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an outgoing edge for a node
    pub fn add_outgoing(&mut self, from: String, edge: CompiledEdge<S>) {
        self.outgoing.entry(from).or_default().push(edge);
    }

    /// Add an incoming trigger for a node
    pub fn add_incoming(&mut self, to: String, source: TriggerSource) {
        self.incoming.entry(to).or_default().push(source);
    }

    /// Get all nodes that have outgoing edges
    #[must_use = "iterators are lazy and do nothing unless consumed"]
    pub fn sources(&self) -> impl Iterator<Item = &String> {
        self.outgoing.keys()
    }

    /// Get all nodes that have incoming edges
    pub fn targets(&self) -> impl Iterator<Item = &String> {
        self.incoming.keys()
    }
}

/// Compiled edge for efficient execution
///
/// Represents a fixed or conditional edge after graph compilation.
#[derive(Clone)]
pub enum CompiledEdge<S: State> {
    /// Fixed edge to a single target
    Fixed {
        /// Target node name
        target: String,
    },

    /// Conditional edge with router
    Conditional {
        /// Router function
        router: Arc<dyn Router<S>>,
        /// Path mapping for validation
        path_map: super::PathMap,
    },
}

impl<S: State> std::fmt::Debug for CompiledEdge<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed { target } => f.debug_tuple("Fixed").field(target).finish(),
            Self::Conditional { path_map, .. } => {
                f.debug_tuple("Conditional").field(path_map).finish()
            }
        }
    }
}

/// Source of a node trigger
///
/// Indicates what causes a node to be scheduled for execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerSource {
    /// Triggered by an edge from another node
    Edge {
        /// Source node name
        from: String,
    },

    /// Triggered by a Send operation
    Send {
        /// Source node name
        from: String,
    },
}

// Rust guideline compliant 2025-01-18
