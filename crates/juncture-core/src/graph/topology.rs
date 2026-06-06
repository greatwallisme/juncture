//! Topology validation for `StateGraph`
//!
//! Provides comprehensive validation of graph structure including:
//! - Entry point verification
//! - Node existence checks for all edges
//! - Conditional edge path map validation
//! - Reachability analysis via BFS
//! - Isolated node detection
//! - Unreachable node detection
//! - SCC-based infinite loop detection (Tarjan's algorithm)

use crate::{State, edge::Edge};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Topology validation errors
///
/// These errors indicate structural problems with a graph that would
/// prevent correct execution.
#[derive(Debug, thiserror::Error)]
pub enum TopologyError {
    #[error("node '{name}' already exists")]
    DuplicateNode { name: String },

    #[error("node '{name}' is invalid: {reason}")]
    InvalidNodeName { name: String, reason: String },

    #[error("no entry point set")]
    NoEntryPoint,

    #[error("edge references non-existent node '{name}'")]
    NodeNotFound { name: String },

    #[error(
        "conditional edge from '{from}' branch '{branch}' targets non-existent node '{target}'"
    )]
    EdgeTargetNotFound {
        from: String,
        branch: String,
        target: String,
    },

    #[error("node '{name}' has no incoming or outgoing edges (isolated)")]
    IsolatedNode { name: String },

    #[error("node '{name}' is unreachable from entry point")]
    UnreachableNode { name: String },

    #[error("potential infinite loop detected, path: {cycle:?}")]
    PotentialInfiniteLoop { cycle: Vec<String> },

    #[error(
        "field index {index} in {context} is out of range (state has {field_count} fields: {field_names:?})"
    )]
    InvalidFieldReference {
        index: usize,
        field_count: usize,
        field_names: &'static [&'static str],
        context: String,
    },
}

/// Strongly connected components finder using `Tarjan`'s algorithm
///
/// Used to detect cycles in the graph that could cause infinite loops.
struct TarjanSCC {
    index: usize,
    stack: Vec<String>,
    indices: HashMap<String, usize>,
    lowlink: HashMap<String, usize>,
    onstack: HashSet<String>,
    sccs: Vec<Vec<String>>,
}

impl TarjanSCC {
    fn new() -> Self {
        Self {
            index: 0,
            stack: Vec::new(),
            indices: HashMap::new(),
            lowlink: HashMap::new(),
            onstack: HashSet::new(),
            sccs: Vec::new(),
        }
    }

    /// Find all strongly connected components in the graph
    fn find_sccs<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
    ) -> Vec<Vec<String>> {
        let mut tarjan = Self::new();
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

        // Build adjacency list
        for node_name in nodes.keys() {
            adjacency.entry(node_name.clone()).or_default();
        }

        for edge in edges {
            match edge {
                Edge::Fixed { from, to } => {
                    if from != crate::edge::START && to != crate::edge::END {
                        adjacency.entry(from.clone()).or_default().push(to.clone());
                    }
                }
                Edge::Conditional { from, path_map, .. } => {
                    if from != crate::edge::START {
                        let targets = adjacency.entry(from.clone()).or_default();
                        for target in path_map.iter().map(|(_, v)| v) {
                            if target != crate::edge::END {
                                targets.push(target.clone());
                            }
                        }
                    }
                }
            }
        }

        // Run Tarjan's algorithm
        for node_name in nodes.keys() {
            if !tarjan.indices.contains_key(node_name) {
                tarjan.visit(node_name, &adjacency);
            }
        }

        tarjan.sccs
    }

    fn visit(&mut self, node: &str, adjacency: &HashMap<String, Vec<String>>) {
        self.indices.insert(node.to_string(), self.index);
        self.lowlink.insert(node.to_string(), self.index);
        self.index += 1;
        self.stack.push(node.to_string());
        self.onstack.insert(node.to_string());

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                if !self.indices.contains_key(neighbor) {
                    self.visit(neighbor, adjacency);
                    let low = self.lowlink.get(node).copied().unwrap_or(0);
                    let neighbor_low = self.lowlink.get(neighbor).copied().unwrap_or(0);
                    self.lowlink.insert(node.to_string(), low.min(neighbor_low));
                } else if self.onstack.contains(neighbor) {
                    let low = self.lowlink.get(node).copied().unwrap_or(0);
                    let neighbor_idx = self.indices.get(neighbor).copied().unwrap_or(0);
                    self.lowlink.insert(node.to_string(), low.min(neighbor_idx));
                }
            }
        }

        if self.lowlink.get(node) == self.indices.get(node) {
            let mut scc = Vec::new();
            loop {
                let w = self.stack.pop().expect("stack should not be empty");
                self.onstack.remove(&w);
                scc.push(w.clone());
                if w == node {
                    break;
                }
            }
            self.sccs.push(scc);
        }
    }
}

/// Validates the topology of a `StateGraph`
///
/// Performs comprehensive checks to ensure the graph structure is valid
/// and can be executed correctly.
pub(super) struct TopologyValidator;

impl TopologyValidator {
    /// Validate the complete graph topology
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if any validation check fails.
    pub(super) fn validate<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
        entry_point: Option<&str>,
        builder_metadata: &IndexMap<String, crate::graph::builder::NodeMetadata>,
    ) -> Result<(), TopologyError> {
        Self::check_entry_point(entry_point)?;
        Self::check_edge_targets(nodes, edges)?;
        Self::check_reachability(nodes, edges, entry_point, builder_metadata)?;
        Self::check_isolated_nodes(nodes, edges)?;
        Self::check_infinite_loops(nodes, edges)?;

        Ok(())
    }

    /// Check that entry point is set
    const fn check_entry_point(entry_point: Option<&str>) -> Result<(), TopologyError> {
        if entry_point.is_none() {
            return Err(TopologyError::NoEntryPoint);
        }
        Ok(())
    }

    /// Check that all edge references point to existing nodes
    fn check_edge_targets<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
    ) -> Result<(), TopologyError> {
        for edge in edges {
            match edge {
                Edge::Fixed { from, to } => {
                    if from != crate::edge::START && !nodes.contains_key(from) {
                        return Err(TopologyError::NodeNotFound { name: from.clone() });
                    }
                    if to != crate::edge::END && !nodes.contains_key(to) {
                        return Err(TopologyError::NodeNotFound { name: to.clone() });
                    }
                }
                Edge::Conditional { from, path_map, .. } => {
                    if from != crate::edge::START && !nodes.contains_key(from) {
                        return Err(TopologyError::NodeNotFound { name: from.clone() });
                    }
                    for (branch, target) in path_map.iter() {
                        if target != crate::edge::END && !nodes.contains_key(target) {
                            return Err(TopologyError::EdgeTargetNotFound {
                                from: from.clone(),
                                branch: branch.clone(),
                                target: target.clone(),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Check that all nodes are reachable from the entry point
    fn check_reachability<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
        entry_point: Option<&str>,
        builder_metadata: &IndexMap<String, crate::graph::builder::NodeMetadata>,
    ) -> Result<(), TopologyError> {
        let entry = entry_point.expect("entry point should exist");

        // Build adjacency list for BFS
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for node_name in nodes.keys() {
            adjacency.entry(node_name.clone()).or_default();
        }

        for edge in edges {
            match edge {
                Edge::Fixed { from, to } => {
                    if from == crate::edge::START {
                        adjacency.entry(to.clone()).or_default();
                    } else if to != crate::edge::END {
                        adjacency.entry(from.clone()).or_default().push(to.clone());
                    }
                }
                Edge::Conditional { from, path_map, .. } => {
                    if from == crate::edge::START {
                        for target in path_map.iter().map(|(_, v)| v) {
                            adjacency.entry(target.clone()).or_default();
                        }
                    } else {
                        let targets = adjacency.entry(from.clone()).or_default();
                        for target in path_map.iter().map(|(_, v)| v) {
                            if target != crate::edge::END {
                                targets.push(target.clone());
                            }
                        }
                    }
                }
            }
        }

        // Add fallback edges to adjacency list
        for (node_name, meta) in builder_metadata {
            if let Some(ref fallback) = meta.fallback_node {
                adjacency
                    .entry(node_name.clone())
                    .or_default()
                    .push(fallback.clone());
            }
        }

        // BFS from entry point
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(entry.to_string());
        visited.insert(entry.to_string());

        while let Some(current) = queue.pop_front() {
            if let Some(neighbors) = adjacency.get(&current) {
                for neighbor in neighbors {
                    if visited.insert(neighbor.clone()) {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        // Check for unreachable nodes
        for node_name in nodes.keys() {
            if !visited.contains(node_name) {
                return Err(TopologyError::UnreachableNode {
                    name: node_name.clone(),
                });
            }
        }

        Ok(())
    }

    /// Check for isolated nodes (no incoming or outgoing edges)
    fn check_isolated_nodes<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
    ) -> Result<(), TopologyError> {
        let mut has_incoming: HashSet<String> = HashSet::new();
        let mut has_outgoing: HashSet<String> = HashSet::new();

        for edge in edges {
            match edge {
                Edge::Fixed { from, to } => {
                    if from != crate::edge::START {
                        has_outgoing.insert(from.clone());
                    }
                    if to != crate::edge::END {
                        has_incoming.insert(to.clone());
                    }
                }
                Edge::Conditional { from, path_map, .. } => {
                    if from != crate::edge::START {
                        has_outgoing.insert(from.clone());
                    }
                    for target in path_map.iter().map(|(_, v)| v) {
                        if target != crate::edge::END {
                            has_incoming.insert(target.clone());
                        }
                    }
                }
            }
        }

        for node_name in nodes.keys() {
            if !has_incoming.contains(node_name) && !has_outgoing.contains(node_name) {
                return Err(TopologyError::IsolatedNode {
                    name: node_name.clone(),
                });
            }
        }

        Ok(())
    }

    /// Check for potential infinite loops using SCC analysis
    ///
    /// Cycles are allowed if at least one node in the SCC has a conditional
    /// edge that can exit the cycle -- either by routing to END or to a node
    /// outside the SCC.  This covers intentional agent loops (e.g. `ReAct`
    /// agent -> tools -> agent) where the agent conditionally routes to a
    /// summary/finish node outside the loop.
    fn check_infinite_loops<S: State>(
        nodes: &IndexMap<String, Arc<dyn crate::Node<S>>>,
        edges: &[Edge<S>],
    ) -> Result<(), TopologyError> {
        let sccs = TarjanSCC::find_sccs(nodes, edges);

        for scc in sccs {
            if scc.len() <= 1 {
                continue;
            }

            let scc_set: HashSet<&str> = scc.iter().map(String::as_str).collect();

            // A cycle is safe if any node in the SCC has a conditional edge
            // whose path_map contains a target outside the SCC (including END).
            let has_exit = scc.iter().any(|node| {
                edges.iter().any(|edge| {
                    matches!(edge, Edge::Conditional { from, path_map, .. }
                    if from == node && path_map.iter().any(|(_, target)| {
                        target == crate::edge::END || !scc_set.contains(target.as_str())
                    }))
                })
            });

            if !has_exit {
                return Err(TopologyError::PotentialInfiniteLoop { cycle: scc });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::IntoNode, node::NodeFnUpdate};

    #[test]
    fn test_tarjan_scc_simple() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));
        nodes.insert("b".to_string(), mock_node("b"));

        let edges = vec![Edge::Fixed {
            from: "a".to_string(),
            to: "b".to_string(),
        }];

        let sccs = TarjanSCC::find_sccs(&nodes, &edges);
        assert_eq!(sccs.len(), 2);
    }

    #[test]
    fn test_tarjan_scc_cycle() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));
        nodes.insert("b".to_string(), mock_node("b"));

        let edges = vec![
            Edge::Fixed {
                from: "a".to_string(),
                to: "b".to_string(),
            },
            Edge::Fixed {
                from: "b".to_string(),
                to: "a".to_string(),
            },
        ];

        let sccs = TarjanSCC::find_sccs(&nodes, &edges);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
    }

    fn mock_node(name: &str) -> Arc<dyn crate::Node<StateDummy>> {
        NodeFnUpdate(|_s: &StateDummy| async move { Ok(StateDummyUpdate) }).into_node(name)
    }

    #[derive(Clone, Debug, Default)]
    struct StateDummy;

    impl crate::State for StateDummy {
        type Update = StateDummyUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, _update: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct StateDummyUpdate;
}

// Rust guideline compliant 2026-05-19
