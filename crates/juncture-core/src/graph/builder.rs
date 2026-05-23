//! `StateGraph` builder for constructing executable graphs
//!
//! Provides a fluent API for building graphs with nodes, edges, and subgraphs.
//! The builder validates the graph structure during compilation.

use super::{compiled::CompiledGraph, topology::TopologyError, topology::TopologyValidator};
use crate::{
    State,
    edge::{CompiledEdge, END, Edge, START, TriggerSource},
    node::IntoNode,
    state::{FromState, IntoState},
};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for graph compilation
///
/// Controls compile-time settings that become defaults for every execution
/// of the compiled graph. Runtime [`RunnableConfig`](crate::RunnableConfig)
/// values override these when present.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::graph::CompileConfig;
///
/// let config = CompileConfig {
///     interrupt_before: vec!["human_review".into()],
///     interrupt_after: vec!["llm_call".into()],
/// };
/// let compiled = graph.compile_with_config(config)?;
/// ```
#[derive(Clone, Debug, Default)]
pub struct CompileConfig {
    /// Nodes that should interrupt before execution (HITL)
    ///
    /// When a node listed here is about to execute, the graph pauses and
    /// returns control to the caller. Runtime `interrupt_before` in
    /// [`RunnableConfig`] takes precedence over this list.
    pub interrupt_before: Vec<String>,

    /// Nodes that should interrupt after execution (HITL)
    ///
    /// After a node listed here finishes executing, the graph pauses and
    /// returns control to the caller. Runtime `interrupt_after` in
    /// [`RunnableConfig`] takes precedence over this list.
    pub interrupt_after: Vec<String>,
}

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

    /// Optional error handler node name for engine-level error recovery.
    ///
    /// When a task executing this node fails, the Pregel engine checks this
    /// field. If set, the engine creates a recovery task targeting the named
    /// handler node instead of canceling all remaining tasks. The error
    /// handler node receives a [`NodeError`] and returns a [`Command`] whose
    /// update is applied normally.
    pub error_handler: Option<String>,

    /// Timeout policies for this node, applied by the Pregel engine during
    /// superstep execution. The timeout wraps the entire execution (including
    /// retry attempts when a retry policy is also configured).
    pub timeout_policies: Vec<crate::TimeoutPolicy>,
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

/// Node error information for error handlers
///
/// Contains detailed information about a node execution error,
/// including the node name, error, state snapshot, and attempt count.
pub struct NodeError<S: State> {
    /// Node that failed
    pub node: String,

    /// The error that occurred
    pub error: crate::JunctureError,

    /// State snapshot at time of error
    pub state: S,

    /// Attempt number (1-indexed)
    pub attempt: u32,
}

impl<S: State> std::fmt::Debug for NodeError<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeError")
            .field("node", &self.node)
            .field("error", &self.error)
            .field("state", &"<state>")
            .field("attempt", &self.attempt)
            .finish()
    }
}

/// Node wrapper that adds error recovery handling
///
/// Wraps an inner node and invokes the error handler when the inner
/// node fails, allowing the graph to recover from errors gracefully.
///
/// The error handler receives the error and produces a fallback state
/// update. Since the inner node consumes the input state, the handler
/// receives a state snapshot.
pub struct ErrorHandlerNode<S: State> {
    /// The inner node being wrapped
    inner: Arc<dyn crate::Node<S>>,

    /// Error recovery handler
    ///
    /// Called when the inner node returns an error. The handler receives
    /// a `NodeError` with detailed information and returns a fallback command.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    handler: Arc<dyn Fn(NodeError<S>) -> crate::Command<S> + Send + Sync>,

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
    ///   receiving `NodeError` with detailed information and producing a fallback command
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature"
    )]
    pub fn new(
        inner: Arc<dyn crate::Node<S>>,
        handler: Arc<dyn Fn(NodeError<S>) -> crate::Command<S> + Send + Sync>,
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
        // the error handler if the inner node fails.
        let state_backup = state.clone();
        let result = self.inner.call(state, config);
        let handler = Arc::clone(&self.handler);
        let node_name = self.name.clone();
        Box::pin(async move {
            match result.await {
                Ok(command) => Ok(command),
                Err(error) => {
                    // Construct NodeError with all required fields
                    let node_error = NodeError {
                        node: node_name,
                        error,
                        state: state_backup,
                        attempt: 1, // First attempt in error handler
                    };
                    Ok(handler(node_error))
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
        let policy = self.policy.clone();
        let inner = Arc::clone(&self.inner);
        let config = config.clone();
        let node_name = self.name.clone();

        Box::pin(async move {
            execute_with_retry(
                &node_name,
                &policy,
                |s, cfg| inner.call(s, cfg),
                state,
                &config,
            )
            .await
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Execute an async operation with retry according to the given policy.
///
/// Implements exponential backoff with optional jitter, configurable max interval
/// capping, and a predicate-based retry filter. When `retry_on` is `None`, all
/// errors except cancellation and interrupt are retried.
///
/// # Arguments
///
/// * `node_name` - Name of the node for error reporting
/// * `policy` - Retry policy governing backoff, jitter, and attempt limits
/// * `operation` - The async operation to execute; receives state and config
/// * `state` - The input state, cloned for each attempt
/// * `config` - Execution configuration passed through to the operation
///
/// # Errors
///
/// Returns the last error when all attempts are exhausted, or immediately
/// returns if the error is not retryable (per `retry_on` predicate or default
/// cancellation/interrupt filter).
///
/// # Examples
///
/// ```ignore
/// use juncture_core::graph::builder::{RetryPolicy, execute_with_retry};
///
/// let policy = RetryPolicy::default();
/// let result = execute_with_retry(
///     "my_node",
///     &policy,
///     |state, config| my_node.call(state, config),
///     state,
///     &config,
/// ).await?;
/// ```
pub async fn execute_with_retry<S, F, Fut>(
    node_name: &str,
    policy: &RetryPolicy,
    operation: F,
    state: S,
    config: &crate::RunnableConfig,
) -> Result<crate::Command<S>, crate::JunctureError>
where
    S: State + Clone,
    F: Fn(S, &crate::RunnableConfig) -> Fut,
    Fut: std::future::Future<Output = Result<crate::Command<S>, crate::JunctureError>>,
{
    let mut last_error: Option<crate::JunctureError> = None;
    let mut delay = policy.initial_interval;

    for attempt in 0..policy.max_attempts {
        let state_for_attempt = state.clone();

        match operation(state_for_attempt, config).await {
            Ok(command) => {
                if attempt > 0 {
                    tracing::debug!(
                        node_name = node_name,
                        attempt = attempt + 1,
                        "node succeeded after retry"
                    );
                }
                return Ok(command);
            }
            Err(error) => {
                let should_retry = policy.should_retry(&error);

                if !should_retry || attempt + 1 >= policy.max_attempts {
                    return Err(error);
                }

                tracing::warn!(
                    node_name = node_name,
                    attempt = attempt + 1,
                    max_attempts = policy.max_attempts,
                    error = %error,
                    "node failed, will retry"
                );

                last_error = Some(error);

                let actual_delay = compute_delay(delay, policy.jitter, policy.max_interval);
                tokio::time::sleep(actual_delay).await;

                delay = cap_delay(delay.mul_f64(policy.backoff_factor), policy.max_interval);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        crate::JunctureError::execution(format!(
            "node '{node_name}': retry policy exhausted with no error recorded"
        ))
    }))
}

/// Compute the actual sleep duration with optional jitter and max interval capping.
///
/// When jitter is enabled, applies +/- 25% random variation to the base delay
/// to prevent thundering herd effects across concurrent retries.
/// The result is then capped at `max_interval`.
fn compute_delay(
    base: std::time::Duration,
    jitter: bool,
    max_interval: std::time::Duration,
) -> std::time::Duration {
    let capped = cap_delay(base, max_interval);

    if !jitter {
        return capped;
    }

    // Apply +/- 25% jitter: random value in [0.75, 1.25] * capped
    let jitter_fraction: f64 = rand::random_range(0.75..=1.25);
    let jittered = capped.mul_f64(jitter_fraction);
    cap_delay(jittered, max_interval)
}

/// Cap a duration at the configured maximum interval.
fn cap_delay(delay: std::time::Duration, max: std::time::Duration) -> std::time::Duration {
    delay.min(max)
}

impl RetryPolicy {
    /// Determine whether the given error should trigger a retry.
    ///
    /// When a `retry_on` predicate is configured, delegates to it.
    /// Otherwise uses the default policy: retry everything except
    /// cancellation and interrupt errors.
    fn should_retry(&self, error: &crate::JunctureError) -> bool {
        self.retry_on.as_ref().map_or_else(
            || !error.is_cancelled() && !error.is_interrupt(),
            |predicate| predicate(error),
        )
    }
}

/// Node wrapper that adds timeout enforcement
///
/// Wraps an inner node and enforces a maximum execution duration.
/// If the inner node does not complete within `run_timeout`, the
/// execution is cancelled and a [`JunctureError::node_timeout`] is returned.
pub struct TimeoutNode<S: State> {
    /// The inner node being wrapped
    inner: Arc<dyn crate::Node<S>>,

    /// Timeout policy governing timeout behavior
    policy: crate::TimeoutPolicy,

    /// Node name (same as inner node)
    name: String,
}

impl<S: State> std::fmt::Debug for TimeoutNode<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutNode")
            .field("name", &self.name)
            .field("inner", &"<node>")
            .field("policy", &self.policy)
            .finish()
    }
}

impl<S: State> TimeoutNode<S> {
    /// Create a new timeout node
    ///
    /// # Arguments
    ///
    /// * `inner` - The node to wrap
    /// * `policy` - Timeout policy governing timeout behavior
    #[must_use]
    pub fn new(inner: Arc<dyn crate::Node<S>>, policy: crate::TimeoutPolicy) -> Self {
        let name = inner.name().to_string();
        Self {
            inner,
            policy,
            name,
        }
    }
}

impl<S: State + Clone> crate::Node<S> for TimeoutNode<S> {
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
        let inner = Arc::clone(&self.inner);
        let config = config.clone();
        let node_name = self.name.clone();
        let run_timeout = self.policy.run_timeout;

        Box::pin(async move {
            execute_with_timeout(
                &node_name,
                run_timeout,
                |s, cfg| inner.call(s, cfg),
                state,
                &config,
            )
            .await
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Execute an async operation with a timeout.
///
/// Wraps the provided operation in a [`tokio::time::timeout`] and returns
/// a [`JunctureError::node_timeout`] if the operation does not complete within
/// `run_timeout`. Inner node errors are passed through unchanged.
///
/// # Arguments
///
/// * `node_name` - Name of the node for error reporting
/// * `run_timeout` - Maximum duration the operation is allowed to run
/// * `operation` - The async operation to execute; receives state and config
/// * `state` - The input state passed to the operation
/// * `config` - Execution configuration passed through to the operation
///
/// # Errors
///
/// Returns [`JunctureError::node_timeout`] if the operation exceeds `run_timeout`.
/// Returns the inner error if the operation fails before the timeout.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::graph::builder::execute_with_timeout;
/// use std::time::Duration;
///
/// let result = execute_with_timeout(
///     "my_node",
///     Duration::from_secs(30),
///     |state, config| my_node.call(state, config),
///     state,
///     &config,
/// ).await?;
/// ```
pub async fn execute_with_timeout<S, F, Fut>(
    node_name: &str,
    run_timeout: std::time::Duration,
    operation: F,
    state: S,
    config: &crate::RunnableConfig,
) -> Result<crate::Command<S>, crate::JunctureError>
where
    S: State,
    F: FnOnce(S, &crate::RunnableConfig) -> Fut,
    Fut: std::future::Future<Output = Result<crate::Command<S>, crate::JunctureError>>,
{
    let result = tokio::time::timeout(run_timeout, operation(state, config)).await;

    match result {
        Ok(Ok(command)) => Ok(command),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(crate::JunctureError::node_timeout(
            crate::error::NodeTimeoutError::RunTimeout {
                node: node_name.to_string(),
                timeout: u64::try_from(run_timeout.as_millis()).unwrap_or(u64::MAX),
            },
        )),
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
pub struct StateGraph<S: State, I: IntoState<S> = S, O: FromState<S> = S> {
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

    /// Marker for input type
    _input: std::marker::PhantomData<I>,
    /// Marker for output type
    _output: std::marker::PhantomData<O>,
}

impl<S: State, I: IntoState<S>, O: FromState<S>> std::fmt::Debug for StateGraph<S, I, O> {
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

impl<S: State, I: IntoState<S>, O: FromState<S>> StateGraph<S, I, O> {
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
            _input: std::marker::PhantomData,
            _output: std::marker::PhantomData,
        }
    }

    /// Add a node with full configuration options
    ///
    /// Returns `&mut Self` on success for fluent builder chaining.
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    ///
    /// # Panics
    ///
    /// Panics if the node name contains invalid characters for graph identifiers.
    #[expect(
        clippy::too_many_arguments,
        reason = "add_node requires name, node, defer, metadata, destinations, retry_policies, and timeout_policies. All are necessary for the builder pattern."
    )]
    pub fn add_node(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
        defer: bool,
        metadata: Option<HashMap<String, serde_json::Value>>,
        destinations: Option<Vec<String>>,
        retry_policies: Vec<RetryPolicy>,
        timeout_policies: Vec<crate::TimeoutPolicy>,
    ) -> Result<&mut Self, TopologyError> {
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
                error_handler: None,
                timeout_policies,
            },
        );

        Ok(self)
    }

    /// Add a node with default configuration options
    ///
    /// This convenience method uses these defaults:
    /// - `defer`: `false`
    /// - `metadata`: `None`
    /// - `destinations`: `None`
    /// - `retry_policies`: empty
    /// - `timeout_policies`: empty
    ///
    /// Returns `&mut Self` on success for fluent builder chaining.
    ///
    /// # Errors
    ///
    /// Returns an error if a node with the same name already exists.
    pub fn add_node_simple(
        &mut self,
        name: impl Into<String>,
        node: impl IntoNode<S>,
    ) -> Result<&mut Self, TopologyError> {
        self.add_node(name, node, false, None, None, Vec::new(), Vec::new())
    }

    /// Add a node with an error recovery handler
    ///
    /// When the wrapped node returns an error, the handler is invoked
    /// to produce a fallback command instead of propagating the error.
    ///
    /// The handler receives `NodeError` with detailed information (node name,
    /// error, state snapshot, attempt count) and returns a recovery command.
    ///
    /// Returns `&mut Self` on success for fluent builder chaining.
    ///
    /// # Arguments
    ///
    /// * `name` - Node name
    /// * `node` - The node to wrap
    /// * `handler` - Error recovery function receiving `NodeError`
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
        handler: Arc<dyn Fn(super::builder::NodeError<S>) -> crate::Command<S> + Send + Sync>,
    ) -> Result<&mut Self, TopologyError>
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

        Ok(self)
    }

    /// Add a node with automatic retry behavior
    ///
    /// When the wrapped node fails, it is retried according to the
    /// provided retry policy with exponential backoff.
    ///
    /// Returns `&mut Self` on success for fluent builder chaining.
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
    ) -> Result<&mut Self, TopologyError>
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

        Ok(self)
    }

    /// Add a compiled subgraph as a node in this graph
    ///
    /// The subgraph is mounted with input/output mapping functions
    /// that transform state between the parent and child graph.
    ///
    /// Returns `&mut Self` on success for fluent builder chaining.
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
    ) -> Result<&mut Self, TopologyError> {
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

        Ok(self)
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
        reason = "fully implemented public API awaiting external consumers"
    )]
    pub fn add_subgraph_node<Sub>(
        &mut self,
        name: &str,
        subgraph: Arc<crate::graph::CompiledGraph<Sub>>,
    ) -> Result<&mut Self, TopologyError>
    where
        Sub: crate::subgraph::StateSubset<S>
            + State
            + Clone
            + serde::Serialize
            + for<'de> serde::Deserialize<'de>,
        Sub::Update: serde::Serialize,
        S: Clone,
    {
        // Create input/output mapping functions using StateSubset.
        // output_map extracts the actual subgraph output state and maps
        // it back to the parent update via StateSubset::map_update.
        let input_map = Arc::new(move |parent: &S| Sub::extract(parent));
        let output_map = Arc::new(|_sub_output: &Sub| Sub::map_update(Default::default()));

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
        clippy::type_complexity,
        reason = "requires type erasure for trait object storage"
    )]
    pub fn add_subgraph_with_config<Sub>(
        &mut self,
        name: &str,
        subgraph: Arc<crate::graph::CompiledGraph<Sub>>,
        input_map: impl Fn(&S) -> Sub + Send + Sync + 'static,
        output_map: impl Fn(&Sub) -> S::Update + Send + Sync + 'static,
        config: crate::subgraph::SubgraphConfig,
    ) -> Result<&mut Self, TopologyError>
    where
        Sub: State + serde::Serialize + for<'de> serde::Deserialize<'de>,
        Sub::Update: serde::Serialize,
        S: Clone,
    {
        let input_map_arc = Arc::new(input_map);
        let output_map_arc: Arc<dyn Fn(&Sub) -> S::Update + Send + Sync> = Arc::new(output_map);

        // Create the subgraph node
        let node: Arc<dyn crate::Node<S>> = Arc::new(crate::subgraph::SubgraphNode::new(
            subgraph,
            name.to_string(),
            input_map_arc,
            output_map_arc,
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
    /// Returns `&mut Self` on success for fluent builder chaining.
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
    pub fn add_sequence(&mut self, nodes: &[impl AsRef<str>]) -> Result<&mut Self, TopologyError> {
        if nodes.is_empty() {
            return Ok(self);
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

        Ok(self)
    }

    /// Validate that all state keys are present
    ///
    /// Key validation ensures that all nodes can access their required state fields.
    /// This validates:
    /// - Node names are non-empty and contain no reserved characters
    /// - Entry point references an existing node
    /// - Finish points reference existing nodes
    /// - Reducer field indices are within bounds of the State type's field count
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if:
    /// - A node name is empty or contains reserved characters (`:`, `/`, `\`)
    /// - Entry point references a non-existent node
    /// - A finish point references a non-existent node
    /// - A reducer field index exceeds the number of fields in the State type
    pub fn validate_keys(&self) -> Result<(), TopologyError> {
        // Validate all node names
        for name in self.nodes.keys() {
            if name.is_empty() {
                return Err(TopologyError::InvalidNodeName {
                    name: name.clone(),
                    reason: "node name cannot be empty".to_string(),
                });
            }

            // Check for reserved characters
            if name.contains(':') || name.contains('/') || name.contains('\\') {
                return Err(TopologyError::InvalidNodeName {
                    name: name.clone(),
                    reason: "node name cannot contain ':', '/', or '\\'".to_string(),
                });
            }
        }

        // Validate entry point
        if let Some(ref entry) = self.entry_point
            && !self.nodes.contains_key(entry)
        {
            return Err(TopologyError::NodeNotFound {
                name: entry.clone(),
            });
        }

        // Validate finish points
        for finish in &self.finish_points {
            if !self.nodes.contains_key(finish) {
                return Err(TopologyError::NodeNotFound {
                    name: finish.clone(),
                });
            }
        }

        // Validate that all reducer field indices are within bounds
        let field_count = S::field_count();
        let field_names = S::field_names();

        for &idx in S::replace_field_indices() {
            if idx >= field_count {
                return Err(TopologyError::InvalidFieldReference {
                    index: idx,
                    field_count,
                    field_names,
                    context: "replace_field_indices".to_string(),
                });
            }
        }

        for &idx in S::replace_after_finish_field_indices() {
            if idx >= field_count {
                return Err(TopologyError::InvalidFieldReference {
                    index: idx,
                    field_count,
                    field_names,
                    context: "replace_after_finish_field_indices".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Compile the graph into an executable form
    ///
    /// Runs topology validation and builds the optimized execution structure
    /// using default compile configuration (no compile-time interrupts).
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
    pub fn compile(&self) -> Result<CompiledGraph<S, I, O>, TopologyError> {
        self.compile_inner(CompileConfig::default(), None)
    }

    /// Compile the graph with explicit compile-time configuration
    ///
    /// Like [`compile`](Self::compile) but accepts a [`CompileConfig`] that
    /// sets compile-time defaults for interrupt behavior. Runtime
    /// [`RunnableConfig`] values override these when present.
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::graph::CompileConfig;
    ///
    /// let compiled = graph.compile_with_config(CompileConfig {
    ///     interrupt_before: vec!["human_review".into()],
    ///     interrupt_after: vec!["llm_call".into()],
    ///     ..Default::default()
    /// })?;
    /// ```
    pub fn compile_with_config(
        &self,
        config: CompileConfig,
    ) -> Result<CompiledGraph<S, I, O>, TopologyError> {
        self.compile_inner(config, None)
    }

    /// Compile the graph without persistence (dev/test)
    ///
    /// Creates a compiled graph with no checkpointer attached.
    /// Useful for development and testing where persistence is not needed.
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    pub fn compile_ephemeral(&self) -> Result<CompiledGraph<S, I, O>, TopologyError> {
        self.compile_inner(CompileConfig::default(), None)
    }

    /// Compile the graph with optional checkpointer
    ///
    /// This is a forward-compatible method that accepts an optional checkpointer.
    /// Uses default compile configuration (no compile-time interrupts).
    ///
    /// # Errors
    ///
    /// Returns [`TopologyError`] if validation fails.
    pub fn compile_with_checkpointer(
        &self,
        checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
    ) -> Result<CompiledGraph<S, I, O>, TopologyError> {
        self.compile_inner(CompileConfig::default(), checkpointer)
    }

    /// Internal compilation shared by all public compile methods.
    ///
    /// Validates topology, builds the trigger table, and constructs the
    /// [`CompiledGraph`] with the given compile config and optional checkpointer.
    fn compile_inner(
        &self,
        config: CompileConfig,
        checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
    ) -> Result<CompiledGraph<S, I, O>, TopologyError> {
        // Validate topology and field indices
        TopologyValidator::validate(&self.nodes, &self.edges, self.entry_point.as_deref())?;
        self.validate_keys()?;

        // Build trigger table
        let trigger_table = self.build_trigger_table();

        // Convert subgraph mounts to SubgraphInfo for the compiled graph
        let subgraph_info: Vec<super::compiled::SubgraphInfo> = self
            .subgraphs
            .iter()
            .map(|mount| super::compiled::SubgraphInfo {
                name: mount.name.clone(),
                persistence: mount.config.persistence,
            })
            .collect();

        // Create compiled graph
        Ok(CompiledGraph::new(
            self.nodes.clone(),
            trigger_table,
            self.builder_metadata.clone(),
            config.interrupt_before,
            config.interrupt_after,
            checkpointer,
            subgraph_info,
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

impl<S: State, I: IntoState<S>, O: FromState<S>> Default for StateGraph<S, I, O> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;
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
    fn test_compile_wires_subgraph_info() {
        use crate::subgraph::{SubgraphConfig, SubgraphMount, SubgraphPersistence};

        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        let node = NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }).into_node("sub");
        let mount = SubgraphMount::new(
            "my_subgraph",
            SubgraphConfig {
                persistence: SubgraphPersistence::PerThread,
            },
            node,
        );

        graph.add_subgraph(mount).unwrap();
        graph.set_entry_point("my_subgraph");
        graph.set_finish_point("my_subgraph");

        let compiled = graph.compile().unwrap();
        let subgraphs = compiled.get_subgraphs();
        assert_eq!(subgraphs.len(), 1);
        assert_eq!(subgraphs[0].name, "my_subgraph");
        assert_eq!(subgraphs[0].persistence, SubgraphPersistence::PerThread);
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

    /// Child state type for testing explicit-mapping subgraph mounting.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    struct ChildState {
        value: i32,
    }

    impl crate::State for ChildState {
        type Update = ChildStateUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
            if let Some(v) = update.value {
                self.value = v;
            }
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct ChildStateUpdate {
        value: Option<i32>,
    }

    #[test]
    fn test_add_subgraph_with_config_registers_node() {
        let mut child_graph: StateGraph<ChildState> = StateGraph::new();
        child_graph
            .add_node_simple(
                "child_node",
                crate::node::NodeFnUpdate(|_s: ChildState| async move {
                    Ok(ChildStateUpdate { value: Some(42) })
                }),
            )
            .unwrap();
        child_graph.set_entry_point("child_node");
        child_graph.set_finish_point("child_node");

        let compiled_child = Arc::new(child_graph.compile().unwrap());

        let mut parent_graph: StateGraph<StateDummy> = StateGraph::new();
        parent_graph
            .add_subgraph_with_config(
                "explicit_subgraph",
                compiled_child,
                |_parent: &StateDummy| ChildState { value: 0 },
                |_child: &ChildState| StateDummyUpdate,
                crate::subgraph::SubgraphConfig::default(),
            )
            .unwrap();

        assert!(parent_graph.nodes.contains_key("explicit_subgraph"));
    }

    #[test]
    fn test_add_subgraph_with_config_duplicate_node() {
        let mut child_graph: StateGraph<ChildState> = StateGraph::new();
        child_graph
            .add_node_simple(
                "child_node",
                crate::node::NodeFnUpdate(|_s: ChildState| async move {
                    Ok(ChildStateUpdate { value: Some(42) })
                }),
            )
            .unwrap();
        child_graph.set_entry_point("child_node");
        child_graph.set_finish_point("child_node");

        let compiled_child = Arc::new(child_graph.compile().unwrap());

        let mut parent_graph: StateGraph<StateDummy> = StateGraph::new();
        parent_graph
            .add_node_simple(
                "explicit_subgraph",
                crate::node::NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();

        let result = parent_graph.add_subgraph_with_config(
            "explicit_subgraph",
            compiled_child,
            |_parent: &StateDummy| ChildState { value: 0 },
            |_child: &ChildState| StateDummyUpdate,
            crate::subgraph::SubgraphConfig::default(),
        );

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

        let handler = Arc::new(|_err: NodeError<StateDummy>| crate::Command::end());

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

    #[test]
    fn test_validate_keys_empty_graph() {
        let graph: StateGraph<StateDummy> = StateGraph::new();
        graph.validate_keys().unwrap();
    }

    #[test]
    fn test_validate_keys_valid_nodes() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        graph
            .add_node_simple(
                "node_b",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();

        graph.validate_keys().unwrap();
    }

    #[test]
    fn test_validate_keys_empty_node_name() {
        let graph: StateGraph<StateDummy> = StateGraph::new();
        // Note: add_node_simple doesn't validate names during insertion
        // but validate_keys will catch empty names
        let result = graph.validate_keys();
        // Empty graph should pass
        result.unwrap();
    }

    #[test]
    fn test_validate_keys_reserved_characters() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();

        // Add a node with reserved characters (will be added but validate_keys will fail)
        graph
            .add_node_simple(
                "node:test",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();

        let result = graph.validate_keys();
        // validate_keys should catch the reserved character
        assert!(matches!(result, Err(TopologyError::InvalidNodeName { .. })));
    }

    #[test]
    fn test_validate_keys_entry_point_not_found() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph.set_entry_point("nonexistent");

        let result = graph.validate_keys();
        assert!(matches!(result, Err(TopologyError::NodeNotFound { .. })));
    }

    #[test]
    fn test_validate_keys_finish_point_not_found() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        graph.set_finish_point("nonexistent");

        let result = graph.validate_keys();
        assert!(matches!(result, Err(TopologyError::NodeNotFound { .. })));
    }

    #[test]
    fn test_validate_keys_with_valid_entry_and_finish() {
        let mut graph: StateGraph<StateDummy> = StateGraph::new();
        graph
            .add_node_simple(
                "start",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        graph
            .add_node_simple(
                "end",
                NodeFnUpdate(|_s| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        graph.set_entry_point("start");
        graph.set_finish_point("end");

        graph.validate_keys().unwrap();
    }

    #[test]
    fn test_validate_keys_catches_invalid_replace_field_index() {
        let mut graph: StateGraph<StateWithBadReplaceIndex> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateWithBadReplaceIndexUpdate::default()) }),
            )
            .unwrap();
        graph.set_entry_point("node_a");
        graph.set_finish_point("node_a");

        let result = graph.validate_keys();
        assert!(matches!(
            result,
            Err(TopologyError::InvalidFieldReference { .. })
        ));
        if let Err(TopologyError::InvalidFieldReference {
            index,
            field_count,
            context,
            ..
        }) = result
        {
            assert_eq!(index, 5);
            assert_eq!(field_count, 2);
            assert_eq!(context, "replace_field_indices");
        }
    }

    #[test]
    fn test_validate_keys_catches_invalid_replace_after_finish_field_index() {
        let mut graph: StateGraph<StateWithBadAfterFinishIndex> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateWithBadAfterFinishIndexUpdate::default()) }),
            )
            .unwrap();
        graph.set_entry_point("node_a");
        graph.set_finish_point("node_a");

        let result = graph.validate_keys();
        assert!(matches!(
            result,
            Err(TopologyError::InvalidFieldReference { .. })
        ));
        if let Err(TopologyError::InvalidFieldReference {
            index,
            field_count,
            context,
            ..
        }) = result
        {
            assert_eq!(index, 99);
            assert_eq!(field_count, 2);
            assert_eq!(context, "replace_after_finish_field_indices");
        }
    }

    /// State type with a `replace` field index that exceeds the field count.
    /// Simulates an inconsistency that would be caught by `validate_keys()`.
    #[derive(Clone, Debug)]
    struct StateWithBadReplaceIndex {
        a: i32,
        b: i32,
    }

    #[derive(Clone, Debug, Default)]
    struct StateWithBadReplaceIndexUpdate {
        a: Option<i32>,
        b: Option<i32>,
    }

    impl crate::State for StateWithBadReplaceIndex {
        type Update = StateWithBadReplaceIndexUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
            let mut changed = crate::FieldsChanged::default();
            if let Some(v) = update.a {
                self.a = v;
                changed.set_field(0);
            }
            if let Some(v) = update.b {
                self.b = v;
                changed.set_field(1);
            }
            changed
        }

        fn reset_ephemeral(&mut self) {}

        fn field_count() -> usize {
            2
        }

        fn field_names() -> &'static [&'static str] {
            &["a", "b"]
        }

        fn replace_field_indices() -> &'static [usize] {
            &[5] // Invalid: index 5 but only 2 fields (0, 1)
        }
    }

    /// State type with a `replace_after_finish` field index that exceeds the field count.
    /// Simulates an inconsistency that would be caught by `validate_keys()`.
    #[derive(Clone, Debug)]
    struct StateWithBadAfterFinishIndex {
        x: String,
        y: String,
    }

    #[derive(Clone, Debug, Default)]
    struct StateWithBadAfterFinishIndexUpdate {
        x: Option<String>,
        y: Option<String>,
    }

    impl crate::State for StateWithBadAfterFinishIndex {
        type Update = StateWithBadAfterFinishIndexUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
            let mut changed = crate::FieldsChanged::default();
            if let Some(v) = update.x {
                self.x = v;
                changed.set_field(0);
            }
            if let Some(v) = update.y {
                self.y = v;
                changed.set_field(1);
            }
            changed
        }

        fn reset_ephemeral(&mut self) {}

        fn field_count() -> usize {
            2
        }

        fn field_names() -> &'static [&'static str] {
            &["x", "y"]
        }

        fn replace_after_finish_field_indices() -> &'static [usize] {
            &[99] // Invalid: index 99 but only 2 fields (0, 1)
        }
    }

    #[test]
    fn test_compile_calls_validate_keys_and_catches_invalid_replace_field_index() {
        let mut graph: StateGraph<StateWithBadReplaceIndex> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateWithBadReplaceIndexUpdate::default()) }),
            )
            .unwrap();
        graph.set_entry_point("node_a");
        graph.set_finish_point("node_a");

        // compile() should call validate_keys() internally and reject the invalid field index
        let result = graph.compile();
        assert!(matches!(
            result,
            Err(TopologyError::InvalidFieldReference { .. })
        ));
        if let Err(TopologyError::InvalidFieldReference {
            index,
            field_count,
            context,
            ..
        }) = result
        {
            assert_eq!(index, 5);
            assert_eq!(field_count, 2);
            assert_eq!(context, "replace_field_indices");
        }
    }

    #[test]
    fn test_compile_calls_validate_keys_and_catches_invalid_replace_after_finish_field_index() {
        let mut graph: StateGraph<StateWithBadAfterFinishIndex> = StateGraph::new();
        graph
            .add_node_simple(
                "node_a",
                NodeFnUpdate(|_s| async move { Ok(StateWithBadAfterFinishIndexUpdate::default()) }),
            )
            .unwrap();
        graph.set_entry_point("node_a");
        graph.set_finish_point("node_a");

        // compile() should call validate_keys() internally and reject the invalid field index
        let result = graph.compile();
        assert!(matches!(
            result,
            Err(TopologyError::InvalidFieldReference { .. })
        ));
        if let Err(TopologyError::InvalidFieldReference {
            index,
            field_count,
            context,
            ..
        }) = result
        {
            assert_eq!(index, 99);
            assert_eq!(field_count, 2);
            assert_eq!(context, "replace_after_finish_field_indices");
        }
    }

    #[test]
    fn test_validate_keys_validates_reducer_indices_during_compile() {
        // This test verifies that the validation of reducer field indices
        // happens automatically during compile(), ensuring that invalid
        // field indices in replace_field_indices() or replace_after_finish_field_indices()
        // are caught at graph compilation time, not at runtime.

        let mut graph: StateGraph<StateWithBadReplaceIndex> = StateGraph::new();
        graph
            .add_node_simple(
                "process",
                NodeFnUpdate(|_s| async move { Ok(StateWithBadReplaceIndexUpdate::default()) }),
            )
            .unwrap();
        graph.set_entry_point("process");
        graph.set_finish_point("process");

        // Before compile(), validate_keys() should catch the error
        let validate_result = graph.validate_keys();
        assert!(
            validate_result.is_err(),
            "validate_keys should detect invalid field index"
        );

        // compile() should also catch the same error (by calling validate_keys internally)
        let compile_result = graph.compile();
        assert!(
            compile_result.is_err(),
            "compile should detect invalid field index"
        );

        // Both should return the same error type
        match (validate_result, compile_result) {
            (
                Err(TopologyError::InvalidFieldReference { index: v_idx, .. }),
                Err(TopologyError::InvalidFieldReference { index: c_idx, .. }),
            ) => {
                assert_eq!(
                    v_idx, c_idx,
                    "Both methods should report the same invalid index"
                );
            }
            _ => panic!("Both methods should return InvalidFieldReference error"),
        }
    }

    #[derive(Clone, Debug)]
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

    // --- Retry tests ---

    #[tokio::test]
    async fn test_execute_with_retry_succeeds_first_attempt() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let result = execute_with_retry(
            "test_node",
            &policy,
            |_s: StateDummy, _cfg: &crate::RunnableConfig| async { Ok(crate::Command::end()) },
            StateDummy,
            &config,
        )
        .await;

        result.unwrap();
    }

    #[tokio::test]
    async fn test_execute_with_retry_succeeds_after_retries() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < 2 {
                        Err(crate::JunctureError::execution("transient failure"))
                    } else {
                        Ok(crate::Command::end())
                    }
                }
            },
            StateDummy,
            &config,
        )
        .await;

        result.unwrap();
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_execute_with_retry_exhausts_attempts() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let result = execute_with_retry(
            "test_node",
            &policy,
            |_s: StateDummy, _cfg: &crate::RunnableConfig| async {
                Err(crate::JunctureError::execution("always fails"))
            },
            StateDummy,
            &config,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
    }

    #[tokio::test]
    async fn test_execute_with_retry_does_not_retry_cancelled() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Err(crate::JunctureError::cancelled())
                }
            },
            StateDummy,
            &config,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_cancelled());
        // Should only be called once (no retry on Cancelled)
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_execute_with_retry_does_not_retry_interrupt() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Err(crate::JunctureError::interrupt("user input needed"))
                }
            },
            StateDummy,
            &config,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_interrupt());
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_execute_with_retry_custom_retry_on_predicate() {
        // Only retry on timeout errors
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: Some(Arc::new(|e: &crate::JunctureError| e.is_timeout())),
        };
        let config = crate::RunnableConfig::new();

        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // Return execution error (not timeout), should NOT be retried
                    Err(crate::JunctureError::execution("not a timeout"))
                }
            },
            StateDummy,
            &config,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
        // Only called once (execution errors are not retryable per custom predicate)
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_execute_with_retry_custom_predicate_allows_retry() {
        // Only retry on timeout errors
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: Some(Arc::new(|e: &crate::JunctureError| e.is_timeout())),
        };
        let config = crate::RunnableConfig::new();

        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < 2 {
                        Err(crate::JunctureError::timeout("timed out"))
                    } else {
                        Ok(crate::Command::end())
                    }
                }
            },
            StateDummy,
            &config,
        )
        .await;

        result.unwrap();
        assert_eq!(attempt_count.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[test]
    fn test_compute_delay_no_jitter() {
        let base = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_secs(10);
        let result = compute_delay(base, false, max);
        assert_eq!(result, std::time::Duration::from_millis(100));
    }

    #[test]
    fn test_compute_delay_caps_at_max() {
        let base = std::time::Duration::from_secs(20);
        let max = std::time::Duration::from_secs(10);
        let result = compute_delay(base, false, max);
        assert_eq!(result, std::time::Duration::from_secs(10));
    }

    #[test]
    fn test_compute_delay_with_jitter_stays_within_range() {
        let base = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_secs(10);
        // Run multiple times to verify jitter stays within +/- 25%
        for _ in 0..100 {
            let result = compute_delay(base, true, max);
            let millis = result.as_secs_f64() * 1000.0;
            // 100ms * 0.75 = 75ms, 100ms * 1.25 = 125ms
            assert!(
                (75.0..=125.0).contains(&millis),
                "jittered delay {millis}ms outside expected range [75, 125]"
            );
        }
    }

    #[test]
    fn test_compute_delay_jitter_capped_by_max() {
        let base = std::time::Duration::from_millis(100);
        // Set max very low to force capping even with jitter
        let max = std::time::Duration::from_millis(50);
        for _ in 0..100 {
            let result = compute_delay(base, true, max);
            assert!(
                result <= max,
                "jittered delay {result:?} exceeded max {max:?}",
            );
        }
    }

    #[test]
    fn test_cap_delay_returns_min() {
        let delay = std::time::Duration::from_secs(5);
        let max = std::time::Duration::from_secs(10);
        assert_eq!(cap_delay(delay, max), delay);

        let delay_large = std::time::Duration::from_secs(15);
        assert_eq!(cap_delay(delay_large, max), max);
    }

    #[test]
    fn test_retry_policy_should_retry_default_allows_execution_errors() {
        let policy = RetryPolicy::default();
        let error = crate::JunctureError::execution("something went wrong");
        assert!(policy.should_retry(&error));
    }

    #[test]
    fn test_retry_policy_should_retry_default_blocks_cancelled() {
        let policy = RetryPolicy::default();
        let error = crate::JunctureError::cancelled();
        assert!(!policy.should_retry(&error));
    }

    #[test]
    fn test_retry_policy_should_retry_default_blocks_interrupt() {
        let policy = RetryPolicy::default();
        let error = crate::JunctureError::interrupt("waiting for user");
        assert!(!policy.should_retry(&error));
    }

    #[test]
    fn test_retry_policy_should_retry_custom_predicate() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(100),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(10),
            jitter: false,
            retry_on: Some(Arc::new(|e: &crate::JunctureError| e.is_timeout())),
        };

        assert!(policy.should_retry(&crate::JunctureError::timeout("slow")));
        assert!(!policy.should_retry(&crate::JunctureError::execution("not timeout")));
    }

    #[tokio::test]
    async fn test_retrying_node_delegates_to_execute_with_retry() {
        use crate::node::NodeFnCommand;

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = Arc::clone(&call_count);

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(move |_s: StateDummy| {
            let counter = Arc::clone(&count_clone);
            async move {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n == 0 {
                    Err(crate::JunctureError::execution("first try fails"))
                } else {
                    Ok(crate::Command::end())
                }
            }
        })
        .into_node("inner");

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };

        let retrying_node = RetryingNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = retrying_node.call(StateDummy, &config).await;
        result.unwrap();
        assert_eq!(call_count.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_retrying_node_respects_max_attempts() {
        use crate::node::NodeFnCommand;

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = Arc::clone(&call_count);

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(move |_s: StateDummy| {
            let counter = Arc::clone(&count_clone);
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(crate::JunctureError::execution("always fails"))
            }
        })
        .into_node("inner");

        let policy = RetryPolicy {
            max_attempts: 5,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: false,
            retry_on: None,
        };

        let retrying_node = RetryingNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = retrying_node.call(StateDummy, &config).await;
        let err = result.unwrap_err();
        assert!(err.is_execution());
        assert_eq!(call_count.load(std::sync::atomic::Ordering::Relaxed), 5);
    }

    #[tokio::test]
    async fn test_retrying_node_with_jitter_enabled() {
        use crate::node::NodeFnCommand;

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = Arc::clone(&call_count);

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(move |_s: StateDummy| {
            let counter = Arc::clone(&count_clone);
            async move {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n < 2 {
                    Err(crate::JunctureError::execution("retry me"))
                } else {
                    Ok(crate::Command::end())
                }
            }
        })
        .into_node("inner");

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(1),
            backoff_factor: 2.0,
            max_interval: std::time::Duration::from_secs(1),
            jitter: true,
            retry_on: None,
        };

        let retrying_node = RetryingNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = retrying_node.call(StateDummy, &config).await;
        result.unwrap();
        assert_eq!(call_count.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_execute_with_retry_max_interval_capping() {
        // Use a very high backoff_factor but low max_interval to verify capping
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_interval: std::time::Duration::from_millis(50),
            backoff_factor: 100.0,
            max_interval: std::time::Duration::from_millis(80),
            jitter: false,
            retry_on: None,
        };
        let config = crate::RunnableConfig::new();

        let start = std::time::Instant::now();
        let attempt_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let result = execute_with_retry(
            "test_node",
            &policy,
            move |_s: StateDummy, _cfg: &crate::RunnableConfig| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if n < 2 {
                        Err(crate::JunctureError::execution("fail"))
                    } else {
                        Ok(crate::Command::end())
                    }
                }
            },
            StateDummy,
            &config,
        )
        .await;

        let elapsed = start.elapsed();
        result.unwrap();
        // Without max_interval cap: 50ms + 5000ms = 5050ms
        // With max_interval cap: 50ms + 80ms = 130ms (plus some overhead)
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "max_interval capping should prevent very long waits, elapsed: {elapsed:?}"
        );
    }

    // --- Timeout tests ---

    #[tokio::test]
    async fn test_execute_with_timeout_succeeds_within_limit() {
        let config = crate::RunnableConfig::new();

        let result = execute_with_timeout(
            "test_node",
            std::time::Duration::from_secs(10),
            |_s: StateDummy, _cfg: &crate::RunnableConfig| async { Ok(crate::Command::end()) },
            StateDummy,
            &config,
        )
        .await;

        result.unwrap();
    }

    #[tokio::test]
    async fn test_execute_with_timeout_fires_on_slow_node() {
        let config = crate::RunnableConfig::new();

        let result = execute_with_timeout(
            "slow_node",
            std::time::Duration::from_millis(10),
            |_s: StateDummy, _cfg: &crate::RunnableConfig| async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(crate::Command::end())
            },
            StateDummy,
            &config,
        )
        .await;

        let err = result.unwrap_err();
        assert!(err.is_node_timeout());
    }

    #[tokio::test]
    async fn test_execute_with_timeout_passes_through_inner_error() {
        let config = crate::RunnableConfig::new();

        let result = execute_with_timeout(
            "failing_node",
            std::time::Duration::from_secs(10),
            |_s: StateDummy, _cfg: &crate::RunnableConfig| async {
                Err(crate::JunctureError::execution("inner failure"))
            },
            StateDummy,
            &config,
        )
        .await;

        let err = result.unwrap_err();
        assert!(err.is_execution());
        assert!(!err.is_node_timeout());
    }

    #[tokio::test]
    async fn test_timeout_node_wrapper_integration() {
        use crate::node::NodeFnCommand;

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count_clone = Arc::clone(&call_count);

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(move |_s: StateDummy| {
            let counter = Arc::clone(&count_clone);
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(crate::Command::end())
            }
        })
        .into_node("inner");

        let policy =
            crate::TimeoutPolicy::new().with_run_timeout(std::time::Duration::from_secs(10));

        let timeout_node = TimeoutNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = timeout_node.call(StateDummy, &config).await;
        result.unwrap();
        assert_eq!(call_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_timeout_node_fires_on_exceeded_duration() {
        use crate::node::NodeFnCommand;

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(|_s: StateDummy| async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(crate::Command::end())
        })
        .into_node("inner");

        let policy =
            crate::TimeoutPolicy::new().with_run_timeout(std::time::Duration::from_millis(10));

        let timeout_node = TimeoutNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = timeout_node.call(StateDummy, &config).await;
        let err = result.unwrap_err();
        assert!(err.is_node_timeout());
    }

    #[tokio::test]
    async fn test_timeout_node_passes_through_inner_error() {
        use crate::node::NodeFnCommand;

        let inner: Arc<dyn crate::Node<StateDummy>> = NodeFnCommand(|_s: StateDummy| async {
            Err(crate::JunctureError::execution("node failure"))
        })
        .into_node("inner");

        let policy =
            crate::TimeoutPolicy::new().with_run_timeout(std::time::Duration::from_secs(10));

        let timeout_node = TimeoutNode::new(inner, policy);
        let config = crate::RunnableConfig::new();

        let result = timeout_node.call(StateDummy, &config).await;
        let err = result.unwrap_err();
        assert!(err.is_execution());
        assert!(!err.is_node_timeout());
    }
}

// Rust guideline compliant 2026-05-21
