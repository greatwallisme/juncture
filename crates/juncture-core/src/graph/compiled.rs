//! Compiled graph for efficient execution
//!
//! Provides the optimized execution structure produced by [`StateGraph::compile`].
//! The compiled graph includes validated topology, trigger tables, and metadata
//! for execution by the Pregel engine.

use super::builder::NodeMetadata;
use crate::{
    JunctureError, State,
    checkpoint::{CheckpointFilter, StateSnapshot},
    config::RunnableConfig,
    edge::TriggerTable,
    pregel::PregelLoop,
};
use indexmap::IndexMap;
use std::sync::Arc;

/// Compiled and validated graph ready for execution
///
/// This is the output of [`StateGraph::compile`] and contains all information
/// needed for graph execution by the Pregel engine.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{StateGraph, State};
///
/// let mut graph = StateGraph::<MyState>::new();
/// // ... add nodes and edges ...
///
/// let compiled = graph.compile()?;
/// let output = compiled.invoke(initial_state, &config)?;
/// # Ok::<(), juncture_core::JunctureError>(())
/// ```
#[derive(Clone)]
pub struct CompiledGraph<S: State> {
    inner: Arc<CompiledGraphInner<S>>,
}

impl<S: State> std::fmt::Debug for CompiledGraph<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledGraph")
            .field("node_count", &self.inner.nodes.len())
            .field("has_checkpointer", &self.inner.checkpointer.is_some())
            .finish()
    }
}

impl<S: State> CompiledGraph<S> {
    /// Create a new compiled graph
    #[must_use]
    pub(crate) fn new(
        nodes: IndexMap<String, Arc<dyn crate::Node<S>>>,
        trigger_table: TriggerTable<S>,
        builder_metadata: IndexMap<String, NodeMetadata>,
    ) -> Self {
        Self {
            inner: Arc::new(CompiledGraphInner {
                nodes,
                trigger_table,
                builder_metadata,
                checkpointer: None,
            }),
        }
    }

    /// Create a new compiled graph with checkpointer
    #[must_use]
    #[allow(
        dead_code,
        reason = "will be used when checkpointing is implemented in Phase 6"
    )]
    pub(crate) fn with_checkpointer(
        nodes: IndexMap<String, Arc<dyn crate::Node<S>>>,
        trigger_table: TriggerTable<S>,
        builder_metadata: IndexMap<String, NodeMetadata>,
        checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
    ) -> Self {
        Self {
            inner: Arc::new(CompiledGraphInner {
                nodes,
                trigger_table,
                builder_metadata,
                checkpointer,
            }),
        }
    }

    /// Invoke the graph synchronously
    ///
    /// Executes the graph from the given input state and returns the final output.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if execution fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let output = compiled.invoke(initial_state, &config)?;
    /// let final_state = output.value;
    /// ```
    pub fn invoke(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> Result<GraphOutput<S>, JunctureError> {
        // Use blocking executor to run async Pregel loop
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| JunctureError::execution(format!("Failed to create runtime: {e}")))?;

        runtime.block_on(self.invoke_async(input, config))
    }

    /// Invoke the graph asynchronously
    ///
    /// Async version of [`invoke`](Self::invoke) for use in async contexts.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if execution fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let output = compiled.invoke_async(initial_state, &config).await?;
    /// let final_state = output.value;
    /// ```
    pub async fn invoke_async(
        &self,
        input: S,
        config: &RunnableConfig,
    ) -> Result<GraphOutput<S>, JunctureError> {
        // Maximum number of fields supported (u64 bitmask in FieldsChanged)
        let num_fields = 64;

        // Create Pregel loop
        let mut pregel = PregelLoop::new(
            input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            config.clone(),
            num_fields,
        )?;

        // Execute the loop
        while pregel.tick()? {
            let result = pregel.execute_superstep().await?;
            pregel.after_tick(result).await?;
        }

        // Extract step before consuming pregel
        let steps = pregel.step();

        // Return final state
        let final_state = pregel.into_state();

        Ok(GraphOutput {
            value: final_state,
            interrupts: Vec::new(),
            metadata: GraphOutputMetadata {
                steps,
                checkpoint_id: config.checkpoint_id.clone(),
                budget_usage: None,
            },
        })
    }

    /// Stream graph execution as a sequence of events
    ///
    /// Executes the graph and emits [`StreamEvent`](crate::stream::StreamEvent)s
    /// as each superstep completes, enabling real-time monitoring of execution progress.
    ///
    /// # Arguments
    ///
    /// * `input` - Initial state for execution
    /// * `config` - Execution configuration
    /// * `mode` - Stream mode controlling what events are emitted
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if execution fails.
    pub async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: crate::stream::StreamMode,
    ) -> Result<Vec<crate::stream::StreamEvent<S>>, JunctureError>
    where
        S: Clone,
    {
        let num_fields = 64;

        let mut pregel = PregelLoop::new(
            input.clone(),
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            config.clone(),
            num_fields,
        )?;

        let mut events = Vec::new();

        while pregel.tick()? {
            let step = pregel.step();

            let result = pregel.execute_superstep().await?;
            pregel.after_tick(result).await?;

            match mode {
                crate::stream::StreamMode::Values => {
                    events.push(crate::stream::StreamEvent::Values {
                        state: pregel.snapshot_state(),
                        step,
                    });
                }
                crate::stream::StreamMode::Updates => {
                    events.push(crate::stream::StreamEvent::Updates {
                        node: String::new(),
                        update: S::Update::default(),
                        step,
                    });
                }
                crate::stream::StreamMode::Debug => {
                    events.push(crate::stream::StreamEvent::TaskDetail {
                        task_id: format!("step-{step}"),
                        node: String::new(),
                        step,
                        attempt: 0,
                        event: crate::stream::TaskEventType::Started,
                    });
                }
                _ => {}
            }
        }

        // Add End event with final state
        events.push(crate::stream::StreamEvent::End {
            output: pregel.into_state(),
        });

        Ok(events)
    }

    /// Resume execution from an interrupt point
    ///
    /// Continues graph execution from where it was interrupted by a
    /// human-in-the-loop interaction, using the provided resume value.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `resume_value` - Value to pass to the interrupted node
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the checkpoint cannot be found.
    #[expect(
        clippy::unused_async,
        reason = "async API consistency for checkpoint operations"
    )]
    pub async fn resume(
        &self,
        _config: &RunnableConfig,
        _resume_value: serde_json::Value,
    ) -> Result<GraphOutput<S>, JunctureError> {
        let checkpointer =
            self.inner.checkpointer.as_ref().ok_or_else(|| {
                JunctureError::checkpoint("no checkpointer configured for resume")
            })?;

        let _ = checkpointer;

        // Full implementation requires checkpoint state recovery, which will
        // be completed in Phase 6 (checkpoint integration).
        Err(JunctureError::checkpoint(
            "resume not yet implemented: requires checkpoint state recovery",
        ))
    }

    /// Get the current state snapshot for a thread
    ///
    /// Returns the state at the latest checkpoint for the given configuration.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the state cannot be retrieved.
    #[expect(
        clippy::unused_async,
        reason = "async API consistency for checkpoint operations"
    )]
    pub async fn get_state(
        &self,
        _config: &RunnableConfig,
    ) -> Result<Option<StateSnapshot<S>>, JunctureError> {
        let checkpointer =
            self.inner.checkpointer.as_ref().ok_or_else(|| {
                JunctureError::checkpoint("no checkpointer configured for get_state")
            })?;

        let _ = checkpointer;

        // Full implementation requires deserialization of checkpoint state,
        // which will be completed in Phase 6 (checkpoint integration).
        Err(JunctureError::checkpoint(
            "get_state not yet implemented: requires checkpoint state recovery",
        ))
    }

    /// Get the full state history for a thread
    ///
    /// Returns all checkpointed state snapshots for the given configuration,
    /// optionally filtered by the provided filter.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` set
    /// * `filter` - Optional filter for narrowing history results
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the history cannot be retrieved.
    #[expect(
        clippy::unused_async,
        reason = "async API consistency for checkpoint operations"
    )]
    pub async fn get_state_history(
        &self,
        _config: &RunnableConfig,
        filter: Option<CheckpointFilter>,
    ) -> Result<Vec<StateSnapshot<S>>, JunctureError> {
        let checkpointer = self.inner.checkpointer.as_ref().ok_or_else(|| {
            JunctureError::checkpoint("no checkpointer configured for get_state_history")
        })?;

        let _ = (checkpointer, filter);

        // Full implementation requires deserialization of checkpoint history,
        // which will be completed in Phase 6 (checkpoint integration).
        Err(JunctureError::checkpoint(
            "get_state_history not yet implemented: requires checkpoint state recovery",
        ))
    }

    /// Manually update the state at a checkpoint
    ///
    /// Applies the provided state update to the current checkpoint state.
    /// Used for administrative state modifications outside of normal execution.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `update` - State update to apply
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the update cannot be applied.
    #[expect(
        clippy::unused_async,
        reason = "async API consistency for checkpoint operations"
    )]
    pub async fn update_state(
        &self,
        _config: &RunnableConfig,
        update: StateUpdate<S>,
    ) -> Result<RunnableConfig, JunctureError> {
        let checkpointer = self.inner.checkpointer.as_ref().ok_or_else(|| {
            JunctureError::checkpoint("no checkpointer configured for update_state")
        })?;

        let _ = (checkpointer, update);

        // Full implementation requires checkpoint state modification,
        // which will be completed in Phase 6 (checkpoint integration).
        Err(JunctureError::checkpoint(
            "update_state not yet implemented: requires checkpoint state recovery",
        ))
    }

    /// Bulk update state across multiple checkpoints
    ///
    /// Applies multiple state updates atomically. If any update fails,
    /// none of the updates are applied.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` set
    /// * `updates` - List of state updates to apply
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if any update cannot be applied.
    #[expect(
        clippy::unused_async,
        reason = "async API consistency for checkpoint operations"
    )]
    pub async fn bulk_update_state(
        &self,
        _config: &RunnableConfig,
        updates: Vec<StateUpdate<S>>,
    ) -> Result<Vec<RunnableConfig>, JunctureError> {
        let checkpointer = self.inner.checkpointer.as_ref().ok_or_else(|| {
            JunctureError::checkpoint("no checkpointer configured for bulk_update_state")
        })?;

        let _ = (checkpointer, updates);

        // Full implementation requires atomic checkpoint state modification,
        // which will be completed in Phase 6 (checkpoint integration).
        Err(JunctureError::checkpoint(
            "bulk_update_state not yet implemented: requires checkpoint state recovery",
        ))
    }

    /// Get a drawable graph representation
    ///
    /// Returns the graph structure for visualization, optionally
    /// including nested subgraph detail up to the specified depth.
    ///
    /// # Arguments
    ///
    /// * `xray` - Optional depth for subgraph x-ray visualization.
    ///   `None` renders only the top-level graph; `Some(n)` expands
    ///   subgraphs up to `n` levels deep.
    #[must_use]
    pub fn get_graph(&self, xray: Option<usize>) -> DrawableGraph {
        let _ = xray;

        // Currently ignores xray depth; subgraph expansion will be
        // implemented when subgraph visualization is fully supported.
        self.to_drawable()
    }

    /// Get information about subgraphs in this compiled graph
    ///
    /// Returns metadata about each mounted subgraph, including its
    /// name and persistence configuration.
    #[must_use]
    pub const fn get_subgraphs(&self) -> Vec<SubgraphInfo> {
        // The compiled graph does not currently store subgraph mounts
        // directly. This returns an empty list; subgraph tracking will
        // be wired through when StateGraph passes mounts to CompiledGraph.
        Vec::new()
    }

    /// Get the nodes in this graph
    #[must_use]
    pub fn nodes(&self) -> &IndexMap<String, Arc<dyn crate::Node<S>>> {
        &self.inner.nodes
    }

    /// Get the trigger table
    #[must_use]
    pub fn trigger_table(&self) -> &TriggerTable<S> {
        &self.inner.trigger_table
    }

    /// Get the checkpointer (if configured)
    #[must_use]
    pub fn checkpointer(&self) -> Option<&Arc<dyn crate::checkpoint::CheckpointSaver>> {
        self.inner.checkpointer.as_ref()
    }

    /// Get the builder metadata for nodes
    #[must_use]
    pub fn builder_metadata(&self) -> &IndexMap<String, NodeMetadata> {
        &self.inner.builder_metadata
    }

    /// Export graph as Mermaid diagram
    ///
    /// Returns a string in Mermaid format that can be rendered by Mermaid.js.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let mermaid = compiled.to_mermaid();
    /// let diagram = format!("```mermaid\n{mermaid}\n```");
    /// ```
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        let mut lines = vec!["graph TD".to_string()];

        // Add nodes
        for node_name in self.inner.nodes.keys() {
            lines.push(format!("    {node_name}[{node_name}]"));
        }

        // Add edges from trigger table
        for (from, edges) in &self.inner.trigger_table.outgoing {
            for edge in edges {
                match edge {
                    crate::edge::CompiledEdge::Fixed { target } => {
                        lines.push(format!("    {from} --> {target}"));
                    }
                    crate::edge::CompiledEdge::Conditional { path_map, .. } => {
                        for (branch, target) in path_map.iter() {
                            lines.push(format!("    {from} -->|{branch}| {target}"));
                        }
                    }
                }
            }
        }

        // Add entry point
        if let Some(entry) = self.find_entry_point() {
            lines.push(format!("    START((start)) --> {entry}"));
        }

        lines.join("\n")
    }

    /// Export graph as DOT format
    ///
    /// Returns a string in Graphviz DOT format.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let dot = compiled.to_dot();
    /// // Use the DOT format with Graphviz or other tools
    /// ```
    #[must_use]
    pub fn to_dot(&self) -> String {
        let mut lines = vec!["digraph juncture_graph {".to_string()];
        lines.push("    rankdir=LR;".to_string());
        lines.push("    node [shape=box];".to_string());
        lines.push("    START [shape=circle];".to_string());
        lines.push("    END [shape=doublecircle];".to_string());
        lines.push(String::new());

        // Add nodes
        for node_name in self.inner.nodes.keys() {
            lines.push(format!("    {node_name};"));
        }

        lines.push(String::new());

        // Add edges from trigger table
        for (from, edges) in &self.inner.trigger_table.outgoing {
            for edge in edges {
                match edge {
                    crate::edge::CompiledEdge::Fixed { target } => {
                        lines.push(format!("    {from} -> {target};"));
                    }
                    crate::edge::CompiledEdge::Conditional { path_map, .. } => {
                        for (branch, target) in path_map.iter() {
                            lines.push(format!("    {from} -> {target} [label=\"{branch}\"];"));
                        }
                    }
                }
            }
        }

        // Add entry point
        if let Some(entry) = self.find_entry_point() {
            lines.push(format!("    START -> {entry};"));
        }

        lines.push("}".to_string());
        lines.join("\n")
    }

    /// Export graph structure as JSON
    ///
    /// Returns a JSON value representing the graph structure.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let json = compiled.to_json();
    /// let pretty = serde_json::to_string_pretty(&json)?;
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let drawable = self.to_drawable();

        serde_json::json!({
            "nodes": drawable.nodes.into_iter().map(|n| {
                serde_json::json!({
                    "name": n.name,
                    "metadata": n.metadata,
                })
            }).collect::<Vec<_>>(),
            "edges": drawable.edges.into_iter().map(|e| {
                let mut edge = serde_json::json!({
                    "from": e.from,
                    "to": e.to,
                    "conditional": e.conditional,
                });
                if let Some(label) = e.label {
                    edge["label"] = serde_json::Value::String(label);
                }
                edge
            }).collect::<Vec<_>>(),
        })
    }

    /// Convert to drawable graph representation
    fn to_drawable(&self) -> DrawableGraph {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // Add nodes
        for node_name in self.inner.nodes.keys() {
            let metadata = self
                .inner
                .builder_metadata
                .get(node_name)
                .and_then(|m| m.metadata.clone())
                .unwrap_or_default();

            nodes.push(DrawableNode {
                name: node_name.clone(),
                metadata,
            });
        }

        // Add edges from trigger table
        for (from, edge_list) in &self.inner.trigger_table.outgoing {
            for edge in edge_list {
                match edge {
                    crate::edge::CompiledEdge::Fixed { target } => {
                        edges.push(DrawableEdge {
                            from: from.clone(),
                            to: target.clone(),
                            conditional: false,
                            label: None,
                        });
                    }
                    crate::edge::CompiledEdge::Conditional { path_map, .. } => {
                        for (branch, target) in path_map.iter() {
                            edges.push(DrawableEdge {
                                from: from.clone(),
                                to: target.clone(),
                                conditional: true,
                                label: Some(branch.clone()),
                            });
                        }
                    }
                }
            }
        }

        DrawableGraph { nodes, edges }
    }

    /// Find the entry point node from the trigger table
    fn find_entry_point(&self) -> Option<String> {
        for (target, sources) in &self.inner.trigger_table.incoming {
            for source in sources {
                if matches!(source, crate::edge::TriggerSource::Edge { from } if from == "START") {
                    return Some(target.clone());
                }
            }
        }
        None
    }
}

/// Inner data of compiled graph
#[allow(dead_code, reason = "fields used through Arc, not directly")]
struct CompiledGraphInner<S: State> {
    /// Registered nodes
    nodes: IndexMap<String, Arc<dyn crate::Node<S>>>,

    /// Trigger table for execution
    trigger_table: TriggerTable<S>,

    /// Metadata from builder
    builder_metadata: IndexMap<String, NodeMetadata>,

    /// Optional checkpointer
    checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
}

/// Output from graph execution
///
/// Contains the final state, any interrupts, and execution metadata.
#[derive(Debug)]
pub struct GraphOutput<S: State> {
    /// Final state value
    pub value: S,

    /// Interrupt information if execution was interrupted
    pub interrupts: Vec<InterruptInfo>,

    /// Execution metadata
    pub metadata: GraphOutputMetadata,
}

/// Information about a human-in-the-loop interrupt
///
/// Contains details about where and why execution was interrupted.
#[derive(Clone, Debug)]
pub struct InterruptInfo {
    /// Node that raised the interrupt
    pub node: String,

    /// Interrupt payload value
    pub value: serde_json::Value,

    /// Optional interrupt identifier
    pub id: Option<String>,
}

/// Metadata about graph execution
///
/// Contains information about the execution run.
#[derive(Clone, Debug)]
pub struct GraphOutputMetadata {
    /// Number of supersteps executed
    pub steps: usize,

    /// Checkpoint ID if checkpointing was enabled
    pub checkpoint_id: Option<String>,

    /// Budget usage if budget tracking was enabled
    pub budget_usage: Option<crate::pregel::BudgetUsage>,
}

/// State update for manual checkpoint modifications
///
/// Used by [`CompiledGraph::update_state`] and [`CompiledGraph::bulk_update_state`]
/// to apply state changes outside of normal graph execution.
#[derive(Clone, Debug)]
pub struct StateUpdate<S: State> {
    /// State update to apply
    pub update: S::Update,

    /// Optional label for this update (shown in state history)
    pub label: Option<String>,

    /// Optional node name credited with this update
    pub as_node: Option<String>,
}

/// Information about a subgraph in a compiled graph
///
/// Contains metadata about a mounted subgraph for inspection
/// and visualization purposes.
#[derive(Clone, Debug)]
pub struct SubgraphInfo {
    /// Subgraph name
    pub name: String,

    /// Checkpoint persistence mode
    pub persistence: crate::subgraph::SubgraphPersistence,
}

/// Filter for state history queries
///
/// Used to narrow down the results of [`CompiledGraph::get_state_history`].
#[derive(Clone, Debug, Default)]
pub struct StateFilter {
    /// Only include states after this superstep
    pub after_step: Option<usize>,

    /// Only include states before this superstep
    pub before_step: Option<usize>,

    /// Maximum number of states to return
    pub limit: Option<usize>,
}

/// Drawable graph representation for export
///
/// Provides a structure optimized for visualization and export to external formats.
#[derive(Clone, Debug)]
pub struct DrawableGraph {
    /// Nodes in the graph
    pub nodes: Vec<DrawableNode>,

    /// Edges in the graph
    pub edges: Vec<DrawableEdge>,
}

/// Drawable node for visualization
///
/// Contains node name and optional metadata.
#[derive(Clone, Debug)]
pub struct DrawableNode {
    /// Node name
    pub name: String,

    /// Optional metadata
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

/// Drawable edge for visualization
///
/// Contains edge connection information and optional label.
#[derive(Clone, Debug)]
pub struct DrawableEdge {
    /// Source node name
    pub from: String,

    /// Target node name
    pub to: String,

    /// Whether this is a conditional edge
    pub conditional: bool,

    /// Optional edge label
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::IntoNode, node::NodeFnUpdate};

    #[test]
    fn test_compiled_graph_creation() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("test".to_string(), mock_node("test"));

        let trigger_table = TriggerTable::new();
        let builder_metadata = IndexMap::new();

        let compiled = CompiledGraph::new(nodes, trigger_table, builder_metadata);
        assert_eq!(compiled.nodes().len(), 1);
    }

    #[test]
    fn test_to_mermaid() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));
        nodes.insert("b".to_string(), mock_node("b"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_outgoing(
            "a".to_string(),
            crate::edge::CompiledEdge::Fixed {
                target: "b".to_string(),
            },
        );

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let mermaid = compiled.to_mermaid();

        assert!(mermaid.contains("graph TD"));
        assert!(mermaid.contains("a --> b"));
    }

    #[test]
    fn test_to_dot() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));
        nodes.insert("b".to_string(), mock_node("b"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_outgoing(
            "a".to_string(),
            crate::edge::CompiledEdge::Fixed {
                target: "b".to_string(),
            },
        );

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let dot = compiled.to_dot();

        assert!(dot.contains("digraph juncture_graph"));
        assert!(dot.contains("a -> b"));
    }

    #[test]
    fn test_to_json() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));
        nodes.insert("b".to_string(), mock_node("b"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_outgoing(
            "a".to_string(),
            crate::edge::CompiledEdge::Fixed {
                target: "b".to_string(),
            },
        );

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let json = compiled.to_json();

        assert!(json.is_object());
        assert!(json.get("nodes").is_some());
        assert!(json.get("edges").is_some());
    }

    #[test]
    fn test_get_graph() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let drawable = compiled.get_graph(None);
        assert_eq!(drawable.nodes.len(), 1);

        let drawable_xray = compiled.get_graph(Some(2));
        assert_eq!(drawable_xray.nodes.len(), 1);
    }

    #[test]
    fn test_get_subgraphs_empty() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let subgraphs = compiled.get_subgraphs();
        assert!(subgraphs.is_empty());
    }

    #[tokio::test]
    async fn test_resume_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let config = RunnableConfig::new();

        let result = compiled.resume(&config, serde_json::Value::Null).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_get_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let config = RunnableConfig::new();

        let result = compiled.get_state(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_get_state_history_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let config = RunnableConfig::new();

        let result = compiled.get_state_history(&config, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_update_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let config = RunnableConfig::new();

        let update = StateUpdate {
            update: StateDummyUpdate,
            label: None,
            as_node: None,
        };

        let result = compiled.update_state(&config, update).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_bulk_update_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
        let config = RunnableConfig::new();

        let updates = vec![StateUpdate {
            update: StateDummyUpdate,
            label: None,
            as_node: None,
        }];

        let result = compiled.bulk_update_state(&config, updates).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[test]
    fn test_state_update_creation() {
        let update: StateUpdate<StateDummy> = StateUpdate {
            update: StateDummyUpdate,
            label: Some("test update".to_string()),
            as_node: Some("my_node".to_string()),
        };

        assert!(update.label.is_some());
        assert!(update.as_node.is_some());
    }

    #[test]
    fn test_subgraph_info_creation() {
        let info = SubgraphInfo {
            name: "my_subgraph".to_string(),
            persistence: crate::subgraph::SubgraphPersistence::Inherit,
        };

        assert_eq!(info.name, "my_subgraph");
    }

    #[test]
    fn test_state_filter_default() {
        let filter = StateFilter::default();
        assert!(filter.after_step.is_none());
        assert!(filter.before_step.is_none());
        assert!(filter.limit.is_none());
    }

    #[test]
    fn test_state_filter_with_values() {
        let filter = StateFilter {
            after_step: Some(5),
            before_step: Some(10),
            limit: Some(20),
        };

        assert_eq!(filter.after_step, Some(5));
        assert_eq!(filter.before_step, Some(10));
        assert_eq!(filter.limit, Some(20));
    }

    fn mock_node(name: &str) -> Arc<dyn crate::Node<StateDummy>> {
        NodeFnUpdate(|_s: StateDummy| async move { Ok(StateDummyUpdate) }).into_node(name)
    }

    #[derive(Clone, Debug)]
    struct StateDummy;

    impl crate::State for StateDummy {
        type Update = StateDummyUpdate;
        type FieldVersions = ();

        fn apply(&mut self, _update: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct StateDummyUpdate;
}

// Rust guideline compliant 2026-05-20
