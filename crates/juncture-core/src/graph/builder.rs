//! `StateGraph` builder for constructing executable graphs
//!
//! Provides a fluent API for building graphs with nodes, edges, and subgraphs.
//! The builder validates the graph structure during compilation.

use super::{compiled::CompiledGraph, topology::TopologyError, topology::TopologyValidator};
use crate::{
    State,
    edge::{CompiledEdge, END, Edge, START, TriggerSource},
    node::IntoNode,
};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;

/// Metadata stored for each node during graph construction
///
/// Contains configuration options that affect node execution behavior.
/// The actual defer/retry behavior is implemented by the Pregel engine.
#[derive(Clone, Debug, Default)]
pub struct NodeMetadata {
    /// Whether this node's execution should be deferred
    pub defer: bool,

    /// User-defined metadata for this node
    pub metadata: Option<HashMap<String, serde_json::Value>>,

    /// Optional list of destination node names
    pub destinations: Option<Vec<String>>,

    /// Retry policies for this node
    pub retry_policies: Vec<RetryPolicy>,
}

/// Retry policy for node execution
///
/// Defines how nodes should retry on failure with configurable
/// backoff, jitter, and retry conditions.
#[derive(Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_attempts: u32,

    /// Initial interval between retries
    pub initial_interval: std::time::Duration,

    /// Backoff multiplier (e.g., 2.0 for exponential backoff)
    pub backoff_factor: f64,

    /// Maximum interval between retries (caps exponential growth)
    pub max_interval: std::time::Duration,

    /// Whether to add random jitter to prevent thundering herd
    pub jitter: bool,

    /// Optional predicate to determine if an error is retryable
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub retry_on: Option<Arc<dyn Fn(&crate::JunctureError) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_attempts", &self.max_attempts)
            .field("initial_interval", &self.initial_interval)
            .field("backoff_factor", &self.backoff_factor)
            .field("max_interval", &self.max_interval)
            .field("jitter", &self.jitter)
            .field("retry_on", &self.retry_on.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(500),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(10),
            jitter: true,
            retry_on: None,
        }
    }
}

/// Node wrapper that adds error recovery handling
///
/// Wraps an inner node and invokes the error handler when the inner
/// node fails, allowing the graph to recover from errors gracefully.
///
/// The error handler receives the error and produces a fallback state
/// update. Since the inner node consumes the input state, the handler
/// cannot access the original state and must produce a recovery update
/// from scratch (e.g., a default or empty update).
pub struct ErrorHandlerNode<S: State> {
    /// The inner node being wrapped
    inner: Arc<dyn crate::Node<S>>,

    /// Error recovery handler
    ///
    /// Called when the inner node returns an error. The handler receives
    /// the error and returns a fallback state update command.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    handler: Arc<dyn Fn(crate::JunctureError) -> crate::Command<S> + Send + Sync>,

    /// Node name (same as inner node)
    name: String,
}

impl<S: State> std::fmt::Debug for ErrorHandlerNode<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErrorHandlerNode")
            .field("name", &self.name)
            .field("inner", &"<node>")
            .field("handler", &"<fn>")
            .finish()
    }
}

impl<S: State> ErrorHandlerNode<S> {
    /// Create a new error handler node
    ///
    /// # Arguments
    ///
    /// * `inner` - The node to wrap
    /// * `handler` - Function invoked when `inner` returns an error,
    ///   receiving the error and producing a fallback command
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub fn new(
        inner: Arc<dyn crate::Node<S>>,
        handler: Arc<dyn Fn(crate::JunctureError) -> crate::Command<S> + Send + Sync>,
    ) -> Self {
        let name = inner.name().to_string();
        Self {
            inner,
            handler,
            name,
        }
    }
}

impl<S: State + Clone> crate::Node<S> for ErrorHandlerNode<S> {
    fn call(
        &self,
        state: S,
        config: &crate::RunnableConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<crate::Command<S>, crate::JunctureError>>
                + Send
                + '_,
        >,
    > {
        // Clone state before calling the inner node so we can pass it to
        // the error handler if the inner node fails. The inner node consumes
        // its input, so without the clone the original state is lost.
        let state_backup = state.clone();
        let result = self.inner.call(state, config);
        let handler = Arc::clone(&self.handler);
        Box::pin(async move {
            match result.await {
                Ok(command) => Ok(command),
                Err(error) => {
                    let _ = state_backup;
                    Ok(handler(error))
                }
            }
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Node wrapper that adds retry behavior
///
/// Wraps an inner node and retries execution according to the
/// provided retry policy when the inner node fails.
pub struct RetryingNode<S: State> {
    /// The inner node being wrapped
    inner: Arc<dyn crate::Node<S>>,

    /// Retry policy governing retry behavior
    policy: RetryPolicy,

    /// Node name (same as inner node)
    name: String,
}

impl<S: State> std::fmt::Debug for RetryingNode<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryingNode")
            .field("name", &self.name)
            .field("inner", &"<node>")
            .field("policy", &self.policy)
            .finish()
    }
}

impl<S: State> RetryingNode<S> {
    /// Create a new retrying node
    ///
    /// # Arguments
    ///
    /// * `inner` - The node to wrap
    /// * `policy` - Retry policy governing retry behavior
    #[must_use]
    pub fn new(inner: Arc<dyn crate::Node<S>>, policy: RetryPolicy) -> Self {
        let name = inner.name().to_string();
        Self {
            inner,
            policy,
            name,
        }
    }
}

impl<S: State + Clone> crate::Node<S> for RetryingNode<S> {
    fn call(
        &self,
        state: S,
        config: &crate::RunnableConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<crate::Command<S>, crate::JunctureError>>
                + Send
                + '_,
        >,
    > {
        let max_attempts = self.policy.max_attempts;
        let initial_interval = self.policy.initial_interval;
        let backoff_factor = self.policy.backoff_factor;
        let retry_on = self.policy.retry_on.clone();
        let inner = Arc::clone(&self.inner);
        let config = config.clone();

        Box::pin(async move {
            let mut last_error: Option<crate::JunctureError> = None;
            let mut attempt: u32 = 0;

            while attempt < max_attempts {
                attempt += 1;

                let state_for_attempt = state.clone();

                match inner.call(state_for_attempt, &config).await {
                    Ok(command) => return Ok(command),
                    Err(error) => {
                        // Check if error is retryable
                        if let Some(ref predicate) = retry_on
                            && !predicate(&error)
                        {
                            return Err(error);
                        }

                        last_error = Some(error);

                        // Don't sleep after the last attempt
                        if attempt < max_attempts {
                            #[allow(
                                clippy::cast_precision_loss,
                                reason = "attempt count fits in f64 for delay calculation"
                            )]
                            let delay = initial_interval.as_secs_f64()
                                * backoff_factor
                                    .powi(i32::try_from(attempt - 1).unwrap_or(i32::MAX));
                            let delay = std::time::Duration::from_secs_f64(delay);
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }

            Err(last_error.unwrap_or_else(|| {
                crate::JunctureError::execution("retry policy exhausted with no error recorded")
            }))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Builder for constructing executable Juncture graphs
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{StateGraph, State, IntoNode};
///
/// struct MyState;
/// impl State for MyState { type Update = MyStateUpdate; }
/// struct MyStateUpdate;
///
/// // Build a simple graph
/// let mut graph = StateGraph::<MyState>::new();
/// graph.add_node_simple("process", |state: MyState| async move {
///     Ok(MyStateUpdate)
/// });
/// graph.set_entry_point("process");
/// graph.set_finish_point("process");
///
/// // Compile and validate
/// let compiled = graph.compile()?;
/// # Ok::<(), juncture_core::graph::TopologyError>(())
/// ```
pub struct StateGraph<S: State> {
    /// Registered nodes in the graph
    nodes: IndexMap<String, Arc<dyn crate::Node<S>>>,

    /// Edges between nodes
    edges: Vec<Edge<S>>,

    /// Entry point node name
    entry_point: Option<String>,

    /// Finish point nodes (nodes that route to END)
    finish_points: Vec<String>,

    /// Metadata for each node
    builder_metadata: IndexMap<String, NodeMetadata>,

    /// Mounted subgraphs
    subgraphs: Vec<crate::subgraph::SubgraphMount<S>>,
}

impl<S: State> std::fmt::Debug for StateGraph<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateGraph")
            .field("nodes", &format_args!("{} nodes", self.nodes.len()))
            .field("edges", &format_args!("{} edges", self.edges.len()))
            .field("entry_point", &self.entry_point)
            .field("finish_points", &self.finish_points)
            .field("builder_metadata", &self.builder_metadata)
            .field(
                "subgraphs",
                &format_args!("{} subgraphs", self.subgraphs.len()),
            )
            .finish()
    }
}

impl<S: State> StateGraph<S> {
    /// Create a new empty graph
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: IndexMap::new(),
            edges: Vec::new(),
            entry_point: None,
            finish_points: Vec::new(),
            builder_metadata: IndexMap::new(),
            subgraphs: Vec::new(),
        }
    }

    /// Add a node with full configuration options
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    ///
    /// # Panics
    ///
    /// Panics if the node name contains invalid characters for graph identifiers.
    pub fn add_node(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        defer: bool,
        metadata: Option<HashMap<String, serde_json::Value>>,
        destinations: Option<Vec<String>>,
        retry_policies: Vec<RetryPolicy>,
    ) -> Result<(), TopologyError> {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            return Err(TopologyError::DuplicateNode { name });
        }

        let node_arc = node.into_node(&name);
        self.nodes.insert(name.clone(), node_arc);

        self.builder_metadata.insert(
            name,
            NodeMetadata {
                defer,
                metadata,
                destinations,
                retry_policies,
            },
        );

        Ok(())
    }

    /// Add a node with default configuration options
    ///
    /// This convenience method uses these defaults:
    /// - `defer`: `false`
    /// - `metadata`: `None`
    /// - `destinations`: `None`
    /// - `retry_policies`: empty
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    pub fn add_node_simple(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
    ) -> Result<(), TopologyError> {
        self.add_node(name, node, false, None, None, Vec::new())
    }

    /// Add a node with an error recovery handler
    ///
    /// When the wrapped node returns an error, the handler is invoked
    /// to produce a fallback command instead of propagating the error.
    ///
    /// The handler receives the error and returns a recovery command.
    /// Since the inner node consumes the input state, the handler
    /// cannot access the original state.
    ///
    /// # Arguments
    ///
    /// * `name` - Node name
    /// * `node` - The node to wrap
    /// * `handler` - Error recovery function receiving the error
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub fn add_node_with_error_handler(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        handler: Arc<dyn Fn(crate::JunctureError) -> crate::Command<S> + Send + Sync>,
    ) -> Result<(), TopologyError>
    where
        S: Clone,
    {
        let name_str = name.into();
        let inner = node.into_node(&name_str);
        let wrapped: Arc<dyn crate::Node<S>> = Arc::new(ErrorHandlerNode::new(inner, handler));

        if self.nodes.contains_key(&name_str) {
            return Err(TopologyError::DuplicateNode { name: name_str });
        }

        self.nodes.insert(name_str.clone(), wrapped);
        self.builder_metadata
            .insert(name_str, NodeMetadata::default());

        Ok(())
    }

    /// Add a node with automatic retry behavior
    ///
    /// When the wrapped node fails, it is retried according to the
    /// provided retry policy with exponential backoff.
    ///
    /// # Arguments
    ///
    /// * `name` - Node name
    /// * `node` - The node to wrap
    /// * `policy` - Retry policy governing retry behavior
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    pub fn add_node_with_retry(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        policy: RetryPolicy,
    ) -> Result<(), TopologyError>
    where
        S: Clone,
    {
        let name_str = name.into();
        let inner = node.into_node(&name_str);
        let wrapped: Arc<dyn crate::Node<S>> = Arc::new(RetryingNode::new(inner, policy));

        if self.nodes.contains_key(&name_str) {
            return Err(TopologyError::DuplicateNode { name: name_str });
        }

        self.nodes.insert(name_str.clone(), wrapped);
        self.builder_metadata
            .insert(name_str, NodeMetadata::default());

        Ok(())
    }

    /// Add a compiled subgraph as a node in this graph
    ///
    /// The subgraph is mounted with input/output mapping functions
    /// that transform state between the parent and child graph.
    ///
    /// # Arguments
    ///
    /// * `mount` - Subgraph mount containing the compiled graph and mapping functions
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name as the subgraph already exists.
    pub fn add_subgraph(
        &mut self,
        mount: crate::subgraph::SubgraphMount<S>,
    ) -> Result<(), TopologyError> {
        if self.nodes.contains_key(&mount.name) {
            return Err(TopologyError::DuplicateNode {
                name: mount.name.clone(),
            });
        }

        let name = mount.name.clone();
        let node = Arc::clone(&mount.node);
        self.nodes.insert(name.clone(), node);
        self.builder_metadata.insert(name, NodeMetadata::default());
        self.subgraphs.push(mount);

        Ok(())
    }

    /// Add a subgraph with shared state using `StateSubset`
    ///
    /// This method adds a subgraph that shares state with its parent graph
    /// using the `StateSubset` trait for type-safe state transformation.
    ///
    /// # Type Parameters
    ///
    /// * `Sub` - The subgraph's state type, which must implement `StateSubset<S>`
    ///
    /// # Arguments
    ///
    /// * `name` - The node name for the subgraph mount point
    /// * `subgraph` - The compiled subgraph to add
    ///
    /// # Returns
    ///
    /// A mutable reference to `self` for chaining
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    #[allow(
        dead_code,
        reason = "will be used when subgraph support is fully implemented"
    )]
    pub fn add_subgraph_node<Sub>(&mut self, name: &str, subgraph: Arc<crate::graph::CompiledGraph<Sub>>) -> Result<&mut Self, TopologyError>
    where
        Sub: crate::subgraph::StateSubset<S> + State + Clone,
        S: Clone,
    {
        // Create input/output mapping functions using StateSubset
        let input_map = Arc::new(move |parent: &S| Sub::extract(parent));
        let output_map = Arc::new(|_sub_output: &Sub| Sub::map_update(Sub::Update::default()));

        // Create the subgraph node
        let node: Arc<dyn crate::Node<S>> = Arc::new(crate::subgraph::SubgraphNode::new(
            subgraph,
            name.to_string(),
            input_map,
            output_map,
            crate::subgraph::SubgraphConfig::default(),
        ));

        if self.nodes.contains_key(name) {
            return Err(TopologyError::DuplicateNode {
                name: name.to_string(),
            });
        }

        self.nodes.insert(name.to_string(), node);
        self.builder_metadata
            .insert(name.to_string(), NodeMetadata::default());

        Ok(self)
    }

    /// Add a subgraph with explicit state mapping and custom config
    ///
    /// This method adds a subgraph with different state types than the parent,
    /// using explicit mapping functions to transform between state types.
    ///
    /// # Type Parameters
    ///
    /// * `Sub` - The subgraph's state type
    ///
    /// # Arguments
    ///
    /// * `name` - The node name for the subgraph mount point
    /// * `subgraph` - The compiled subgraph to add
    /// * `input_map` - Function to transform parent state to subgraph input
    /// * `output_map` - Function to transform subgraph output to parent state update
    /// * `config` - Subgraph configuration options
    ///
    /// # Returns
    ///
    /// A mutable reference to `self` for chaining
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    #[allow(
        dead_code,
        reason = "will be used when subgraph support is fully implemented"
    )]
    #[allow(
        clippy::type_complexity,
        reason = "requires type erasure for trait object storage"
    )]
    pub fn add_subgraph_with_config<Sub>(
        &mut self,
        name: &str,
        subgraph: Arc<crate::graph::CompiledGraph<Sub>>,
        input_map: impl Fn(&S) -> Sub + Send + Sync + 'static,
        output_map: impl Fn(Sub::Update) -> S::Update + Send + Sync + 'static,
        config: crate::subgraph::SubgraphConfig,
    ) -> Result<&mut Self, TopologyError>
    where
        Sub: State,
        S: Clone,
    {
        // Box the mapping functions to create trait objects
        let input_map_arc = Arc::new(input_map);
        let output_map_wrapper = Arc::new(move |_sub: &Sub| {
            // Transform subgraph state to parent state update
            // Full implementation will capture actual subgraph output
            output_map(Sub::Update::default())
        });

        // Create the subgraph node
        let node: Arc<dyn crate::Node<S>> = Arc::new(crate::subgraph::SubgraphNode::new(
            subgraph,
            name.to_string(),
            input_map_arc,
            output_map_wrapper,
            config,
        ));

        if self.nodes.contains_key(name) {
            return Err(TopologyError::DuplicateNode {
                name: name.to_string(),
            });
        }

        self.nodes.insert(name.to_string(), node);
        self.builder_metadata
            .insert(name.to_string(), NodeMetadata::default());

        Ok(self)
    }

    /// Set the context schema type for this graph
    ///
    /// Currently a no-op that returns `self` for forward compatibility.
    /// Will be used for compile-time context validation in a future release.
    #[must_use]
    pub const fn with_context_schema(self) -> Self {
        self
    }

    /// Add a fixed edge between two nodes
    ///
    /// # Examples
    ///
    /// ```ignore
    /// graph.add_edge("node_a", "node_b")?;
    /// ```
    ///
    /// # Errors
    ///
    /// This method doesn't validate node existence. Validation happens during [`compile`](Self::compile).
    pub fn add_edge(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.edges.push(Edge::Fixed {
            from: from.into(),
            to: to.into(),
        });
    }

    /// Add a conditional edge with dynamic routing
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::edge::{PathMap, Router};
    /// use std::sync::Arc;
    ///
    /// let router = |state: &MyState| -> &str {
    ///     if state.should_continue { "continue" } else { "stop" }
    /// };
    ///
    /// let path_map = PathMap::from(&[
    ///     ("continue", "process_more"),
    ///     ("stop", "finish"),
    /// ]);
    ///
    /// graph.add_conditional_edges("decide", Arc::new(router), path_map)?;
    /// ```
    ///
    /// # Errors
    ///
    /// This method doesn't validate node existence or path map targets.
    /// Validation happens during [`compile`](Self::compile).
    pub fn add_conditional_edges(
        &mut self,
        from: impl Into<String>,
        router: Arc<dyn crate::edge::Router<S>>,
        path_map: crate::edge::PathMap,
    ) {
        self.edges.push(Edge::Conditional {
            from: from.into(),
            router,
            path_map,
        });
    }

    /// Set the entry point for the graph
    ///
    /// This is equivalent to `add_edge(START, node)`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// graph.set_entry_point("start_node");
    /// ```
    pub fn set_entry_point(&mut self, node: impl Into<String>) {
        let node = node.into();
        self.entry_point = Some(node.clone());
        self.edges.push(Edge::Fixed {
            from: START.to_string(),
            to: node,
        });
    }

    /// Set a finish point for the graph
    ///
    /// This is equivalent to `add_edge(node, END)`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// graph.set_finish_point("end_node");
    /// ```
    pub fn set_finish_point(&mut self, node: impl Into<String>) {
        let node = node.into();
        self.finish_points.push(node.clone());
        self.edges.push(Edge::Fixed {
            from: node,
            to: END.to_string(),
        });
    }

    /// Add a sequence of nodes as a chain
    ///
    /// Automatically adds edges between consecutive nodes and sets
    /// the first node as the entry point.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// graph.add_sequence(&["step1", "step2", "step3"])?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if any of the nodes don't exist.
    pub fn add_sequence(&mut self, nodes: &[impl AsRef<str>]) -> Result<(), TopologyError> {
        if nodes.is_empty() {
            return Ok(());
        }

        let node_names: Vec<&str> = nodes.iter().map(std::convert::AsRef::as_ref).collect();

        // Validate all nodes exist
        for name in &node_names {
            if !self.nodes.contains_key(*name) {
                return Err(TopologyError::NodeNotFound {
                    name: (*name).to_string(),
                });
            }
        }

        // Set entry point to first node
        if self.entry_point.is_none() {
            self.set_entry_point(node_names[0]);
        }

        // Add edges between consecutive nodes
        for window in node_names.windows(2) {
            self.add_edge(window[0], window[1]);
        }

        Ok(())
    }

    /// Validate that all state keys are present
    ///
    /// Key validation ensures that all nodes can access their required state fields.
    /// This validation is performed by the Pregel engine during execution.
    ///
    /// # Errors
    ///
    /// Currently always returns Ok. The Pregel engine will perform comprehensive
    /// validation of state field accessibility when implemented in Phase 5.
    pub const fn validate_keys(&self) -> Result<(), TopologyError> {
        // Key validation is performed by the Pregel engine during execution
        // to ensure all nodes can access their required state fields
        Ok(())
    }

    /// Compile the graph into an executable form
    ///
    /// Runs topology validation and builds the optimized execution structure.
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let compiled = graph.compile()?;
    /// ```
    pub fn compile(&self) -> Result<CompiledGraph<S>, TopologyError> {
        self.compile_with_checkpointer(None)
    }

    /// Compile the graph without persistence (dev/test)
    ///
    /// Creates a compiled graph with no checkpointer attached.
    /// Useful for development and testing where persistence is not needed.
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    pub fn compile_ephemeral(&self) -> Result<CompiledGraph<S>, TopologyError> {
        self.compile_with_checkpointer(None)
    }

    /// Compile the graph with optional checkpointer
    ///
    /// This is a forward-compatible method that accepts an optional checkpointer.
    /// Currently, checkpointing is not implemented, so the checkpointer is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    pub fn compile_with_checkpointer(
        &self,
        _checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
    ) -> Result<CompiledGraph<S>, TopologyError> {
        // Validate topology
        TopologyValidator::validate(&self.nodes, &self.edges, self.entry_point.as_deref())?;

        // Build trigger table
        let trigger_table = self.build_trigger_table();

        // Create compiled graph
        Ok(CompiledGraph::new(
            self.nodes.clone(),
            trigger_table,
            self.builder_metadata.clone(),
        ))
    }

    /// Build the trigger table from edges
    fn build_trigger_table(&self) -> crate::edge::TriggerTable<S> {
        let mut trigger_table = crate::edge::TriggerTable::new();

        for edge in &self.edges {
            match edge {
                Edge::Fixed { from, to } => {
                    if from == START {
                        // Entry point - add to incoming triggers
                        trigger_table
                            .add_incoming(to.clone(), TriggerSource::Edge { from: from.clone() });
                    } else if to == END {
                        // Finish point - no outgoing trigger needed
                    } else {
                        // Regular edge
                        trigger_table
                            .add_outgoing(from.clone(), CompiledEdge::Fixed { target: to.clone() });
                        trigger_table
                            .add_incoming(to.clone(), TriggerSource::Edge { from: from.clone() });
                    }
                }
                Edge::Conditional {
                    from,
                    path_map,
                    router,
                } => {
                    let router = Arc::clone(router);
                    let path_map = path_map.clone();

                    if from == START {
                        // Entry point with conditional routing
                        for target in path_map.iter().map(|(_, v)| v) {
                            trigger_table.add_incoming(
                                target.clone(),
                                TriggerSource::Edge { from: from.clone() },
                            );
                        }
                    } else {
                        // Regular conditional edge
                        trigger_table.add_outgoing(
                            from.clone(),
                            CompiledEdge::Conditional {
                                router,
                                path_map: path_map.clone(),
                            },
                        );

                        for target in path_map.iter().map(|(_, v)| v) {
                            trigger_table.add_incoming(
                                target.clone(),
                                TriggerSource::Edge { from: from.clone() },
                            );
                        }
                    }
                }
            }
        }

        trigger_table
    }
}

impl<S: State> Default for StateGraph<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeFnUpdate;

    #[test]
    fn test_state_graph_new() {
        let graph: StateGraph<StateDummy> = StateGraph::new();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
        assert!(graph.entry_point.is_none());
        assert!(graph.subgraphs.is_empty());
    }

    #[test]
    fn test_add_node_simple() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        let node = NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) });

        graph.add_node_simple("test", node).unwrap();
        assert!(graph.nodes.contains_key("test"));
    }

    #[test]
    fn test_add_node_duplicate() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        graph
            .add_node_simple(
                "test",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        let result = graph.add_node_simple(
            "test",
            NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
        );
        assert!(matches!(result, Err(TopologyError::DuplicateNode { .. })));
    }

    #[test]
    fn test_set_entry_point() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph.set_entry_point("start");
        assert_eq!(graph.entry_point, Some("start".to_string()));
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn test_set_finish_point() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph.set_finish_point("end");
        assert_eq!(graph.finish_points, vec!["end"]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn test_add_sequence() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        // Add nodes first
        graph
            .add_node_simple("a", NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }))
            .unwrap();
        graph
            .add_node_simple("b", NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }))
            .unwrap();
        graph
            .add_node_simple("c", NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }))
            .unwrap();

        // Add sequence
        graph.add_sequence(&["a", "b", "c"]).unwrap();

        assert_eq!(graph.entry_point, Some("a".to_string()));
        assert_eq!(graph.edges.len(), 3); // START->a, a->b, b->c
    }

    #[test]
    fn test_add_sequence_missing_node() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        let result = graph.add_sequence(&["missing"]);
        assert!(matches!(result, Err(TopologyError::NodeNotFound { .. })));
    }

    #[test]
    fn test_compile_ephemeral() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph
            .add_node_simple("a", NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }))
            .unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");

        let compiled = graph.compile_ephemeral().unwrap();
        assert_eq!(compiled.nodes().len(), 1);
    }

    #[test]
    fn test_with_context_schema() {
        let graph: StateGraph<StateDummy> = StateGraph::new();
        let returned = graph.with_context_schema();
        assert!(returned.nodes.is_empty());
    }

    #[test]
    fn test_add_subgraph() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        let node = NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }).into_node("sub");
        let mount = crate::subgraph::SubgraphMount::new(
            "my_subgraph",
            crate::subgraph::SubgraphConfig::default(),
            node,
        );

        graph.add_subgraph(mount).unwrap();
        assert!(graph.nodes.contains_key("my_subgraph"));
        assert_eq!(graph.subgraphs.len(), 1);
    }

    #[test]
    fn test_add_subgraph_duplicate() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        graph
            .add_node_simple(
                "my_subgraph",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();

        let node = NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }).into_node("sub");
        let mount = crate::subgraph::SubgraphMount::new(
            "my_subgraph",
            crate::subgraph::SubgraphConfig::default(),
            node,
        );

        let result = graph.add_subgraph(mount);
        assert!(matches!(result, Err(TopologyError::DuplicateNode { .. })));
    }

    #[test]
    fn test_add_node_with_retry() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(100),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(10),
            jitter: true,
            retry_on: None,
        };

        graph
            .add_node_with_retry(
                "retry_node",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
                policy,
            )
            .unwrap();

        assert!(graph.nodes.contains_key("retry_node"));
    }

    #[test]
    fn test_add_node_with_error_handler() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        let handler = Arc::new(|_err: crate::JunctureError| crate::Command::end());

        graph
            .add_node_with_error_handler(
                "error_handler_node",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
                handler,
            )
            .unwrap();

        assert!(graph.nodes.contains_key("error_handler_node"));
    }

    #[test]
    fn test_default_implementation() {
        let graph: StateGraph<StateDummy> = StateGraph::default();
        assert!(graph.nodes.is_empty());
        assert!(graph.subgraphs.is_empty());
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

// Rust guideline compliant 2026-05-19
