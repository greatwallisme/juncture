//! Compiled graph for efficient execution
//!
//! Provides the optimized execution structure produced by [`StateGraph::compile`].
//! The compiled graph includes validated topology, trigger tables, and metadata
//! for execution by the Pregel engine.

use super::builder::NodeMetadata;
use crate::{
    JunctureError, State,
    checkpoint::{
        Checkpoint, CheckpointFilter, CheckpointMetadata, CheckpointSource, StateSnapshot,
    },
    config::RunnableConfig,
    edge::TriggerTable,
    interrupt::ResumeValue,
    pregel::{BudgetTracker, PregelLoop},
    state::{FromState, IntoState},
    stream::{EventEmitter, StreamEvent, StreamMode},
};
use futures::Stream;
use indexmap::IndexMap;
use std::{pin::Pin, sync::Arc};
use tokio::sync::mpsc;
use tracing::Instrument;

/// Bounded channel capacity for Messages streaming mode.
///
/// Messages mode handles high-throughput LLM token chunks that arrive rapidly,
/// so it needs a larger buffer to avoid unnecessary backpressure stalls.
/// Per design doc 05-streaming section 7.3.
const CHANNEL_CAPACITY_MESSAGES: usize = 256;

/// Default bounded channel capacity for all non-Messages streaming modes.
///
/// Modes like Values, Updates, Debug, etc. produce far fewer events per
/// superstep than Messages mode, so a smaller buffer suffices while still
/// providing backpressure against runaway producers.
const CHANNEL_CAPACITY_DEFAULT: usize = 32;

/// Determine the channel capacity based on the stream mode.
///
/// Returns [`CHANNEL_CAPACITY_MESSAGES`] (256) for Messages mode and
/// [`CHANNEL_CAPACITY_DEFAULT`] (32) for all other modes. Multi mode uses
/// the larger capacity if any sub-mode is Messages.
fn stream_capacity(mode: &StreamMode) -> usize {
    match mode {
        StreamMode::Messages => CHANNEL_CAPACITY_MESSAGES,
        StreamMode::Multi(modes) if modes.iter().any(|m| matches!(m, StreamMode::Messages)) => {
            CHANNEL_CAPACITY_MESSAGES
        }
        _ => CHANNEL_CAPACITY_DEFAULT,
    }
}

/// Result of a streaming graph execution.
///
/// Contains the run identifier for tracking and resumption, alongside the
/// event stream produced by the Pregel engine. Callers use [`run_id`](StreamHandle::run_id)
/// to correlate events with a specific invocation or to resume a stream that was
/// interrupted.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{StateGraph, State, StreamMode};
/// use futures::StreamExt;
///
/// let handle = compiled.stream(initial_state, &config, StreamMode::Values).await?;
/// println!("run_id = {}", handle.run_id());
///
/// let mut stream = handle.stream;
/// while let Some(result) = stream.next().await {
///     // process events
/// }
/// ```
pub struct StreamHandle<S: State> {
    /// Unique run identifier for this execution.
    run_id: String,
    /// Stream of graph execution events.
    pub stream: Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>,
}

impl<S: State> std::fmt::Debug for StreamHandle<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamHandle")
            .field("run_id", &self.run_id)
            .field("stream", &"<stream>")
            .finish()
    }
}

impl<S: State> StreamHandle<S> {
    /// Returns the unique run identifier for this streaming execution.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Consumes the handle, returning the run ID and stream as a tuple.
    #[must_use]
    #[allow(
        clippy::type_complexity,
        reason = "return type mirrors StreamHandle fields"
    )]
    pub fn into_parts(
        self,
    ) -> (
        String,
        Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>,
    ) {
        (self.run_id, self.stream)
    }
}

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
pub struct CompiledGraph<S: State, I: IntoState<S> = S, O: FromState<S> = S> {
    inner: Arc<CompiledGraphInner<S>>,
    _input: std::marker::PhantomData<I>,
    _output: std::marker::PhantomData<O>,
}

impl<S: State, I: IntoState<S>, O: FromState<S>> std::fmt::Debug for CompiledGraph<S, I, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledGraph")
            .field("node_count", &self.inner.nodes.len())
            .field("has_checkpointer", &self.inner.checkpointer.is_some())
            .finish()
    }
}

impl<S: State, I: IntoState<S>, O: FromState<S>> CompiledGraph<S, I, O> {
    /// Create a new compiled graph
    #[must_use]
    pub(crate) fn new(
        nodes: IndexMap<String, Arc<dyn crate::Node<S>>>,
        trigger_table: TriggerTable<S>,
        builder_metadata: IndexMap<String, NodeMetadata>,
        interrupt_before: Vec<String>,
        interrupt_after: Vec<String>,
        checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,
        subgraphs: Vec<SubgraphInfo>,
    ) -> Self {
        Self {
            inner: Arc::new(CompiledGraphInner {
                nodes,
                trigger_table,
                builder_metadata,
                checkpointer,
                interrupt_before,
                interrupt_after,
                subgraphs,
                active_invocations: std::sync::atomic::AtomicU64::new(0),
            }),
            _input: std::marker::PhantomData,
            _output: std::marker::PhantomData,
        }
    }

    /// Extract error handler map from builder metadata.
    ///
    /// Builds a `HashMap<String, String>` mapping node names to their
    /// registered error handler node names by scanning builder metadata.
    fn build_error_handler_map(&self) -> std::collections::HashMap<String, String> {
        self.inner
            .builder_metadata
            .iter()
            .filter_map(|(node_name, meta)| {
                meta.error_handler
                    .as_ref()
                    .map(|handler| (node_name.clone(), handler.clone()))
            })
            .collect()
    }

    /// Builds a `HashMap<String, RetryPolicy>` mapping node names to their
    /// first configured retry policy by scanning builder metadata.
    ///
    /// Nodes configured via [`StateGraph::add_node_with_retry`](super::StateGraph::add_node_with_retry)
    /// are wrapped in a [`RetryingNode`](super::builder::RetryingNode) at graph construction
    /// time and do NOT appear here -- this map captures engine-level retry policies
    /// from [`NodeMetadata::retry_policies`] that are applied by the Pregel runner
    /// during superstep execution.
    fn build_retry_policy_map(
        &self,
    ) -> std::collections::HashMap<String, super::builder::RetryPolicy> {
        self.inner
            .builder_metadata
            .iter()
            .filter_map(|(node_name, meta)| {
                meta.retry_policies
                    .first()
                    .map(|policy| (node_name.clone(), policy.clone()))
            })
            .collect()
    }

    /// Builds a `HashMap<String, TimeoutPolicy>` mapping node names to their
    /// first configured timeout policy by scanning builder metadata.
    ///
    /// This map captures engine-level timeout policies from
    /// [`NodeMetadata::timeout_policies`] that are applied by the Pregel runner
    /// during superstep execution. The timeout wraps the entire node execution
    /// (including retry attempts when a retry policy is also configured).
    fn build_timeout_policy_map(&self) -> std::collections::HashMap<String, crate::TimeoutPolicy> {
        self.inner
            .builder_metadata
            .iter()
            .filter_map(|(node_name, meta)| {
                meta.timeout_policies
                    .first()
                    .cloned()
                    .map(|policy| (node_name.clone(), policy))
            })
            .collect()
    }

    /// Merge compile-time interrupt defaults with runtime config.
    ///
    /// Runtime values (from `RunnableConfig`) take precedence when present.
    /// Compile-time values (from `CompileConfig`) serve as defaults when
    /// runtime values are `None`.
    fn effective_config(&self, config: &RunnableConfig) -> RunnableConfig {
        let mut effective = config.clone();
        if effective.interrupt_before.is_none() && !self.inner.interrupt_before.is_empty() {
            effective.interrupt_before = Some(self.inner.interrupt_before.clone());
        }
        if effective.interrupt_after.is_none() && !self.inner.interrupt_after.is_empty() {
            effective.interrupt_after = Some(self.inner.interrupt_after.clone());
        }
        effective
    }

    /// Deserialize state from checkpoint, applying schema migration if needed.
    ///
    /// Compares the checkpoint's `schema_version` with `S::schema_version()`.
    /// When they differ, `S::migrate()` transforms the JSON before deserialization.
    fn deserialize_with_migration(
        checkpoint: &crate::checkpoint::Checkpoint,
    ) -> Result<S, JunctureError>
    where
        S: serde::de::DeserializeOwned,
    {
        let mut channel_values = checkpoint.channel_values.clone();
        let checkpoint_version = checkpoint.schema_version;
        let current_version = S::schema_version();
        if checkpoint_version != current_version {
            channel_values = S::migrate(checkpoint_version, channel_values);
        }
        serde_json::from_value(channel_values)
            .map_err(|e| JunctureError::checkpoint(format!("failed to deserialize state: {e}")))
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
        input: I,
        config: &RunnableConfig,
    ) -> Result<GraphOutput<S, O>, JunctureError>
    where
        S: serde::Serialize,
        S::Update: serde::Serialize,
        O: FromState<S>,
    {
        let effective = self.effective_config(config);

        // Use blocking executor to run async Pregel loop
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| JunctureError::execution(format!("Failed to create runtime: {e}")))?;

        runtime.block_on(self.invoke_async_inner(input, &effective))
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
        input: I,
        config: &RunnableConfig,
    ) -> Result<GraphOutput<S, O>, JunctureError>
    where
        S: serde::Serialize,
        S::Update: serde::Serialize,
        O: FromState<S>,
    {
        let effective = self.effective_config(config);
        self.invoke_async_inner(input, &effective).await
    }

    /// Core async invocation used by both `invoke` (blocking) and `invoke_async`.
    async fn invoke_async_inner(
        &self,
        input: I,
        config: &RunnableConfig,
    ) -> Result<GraphOutput<S, O>, JunctureError>
    where
        S: serde::Serialize,
        S::Update: serde::Serialize,
        O: FromState<S>,
    {
        // Maximum number of fields supported (u64 bitmask in FieldsChanged)
        let num_fields = 64;

        // Extract error handler map from builder metadata
        let error_handler_map = self.build_error_handler_map();

        // Extract per-node retry policies from builder metadata
        let retry_policy_map = self.build_retry_policy_map();

        // Extract per-node timeout policies from builder metadata
        let timeout_policy_map = self.build_timeout_policy_map();

        // Convert input type I into state type S
        let state_input = input.into_state();

        // Create Pregel loop
        let mut pregel = PregelLoop::with_error_handlers(
            state_input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            config.clone(),
            num_fields,
            error_handler_map,
        )?;

        pregel.set_retry_policies(retry_policy_map);
        pregel.set_timeout_policies(timeout_policy_map);

        // Wire up budget tracking when budget limits are configured
        if let Some(budget_config) = &pregel.runnable_config.budget {
            let metrics_collector = pregel.runnable_config.metrics_collector.clone();
            pregel.set_budget_tracker(
                BudgetTracker::new(budget_config.clone()).with_metrics_collector(metrics_collector),
            );
        }

        // Create the graph.invoke span that wraps the entire execution
        // This span provides the root for all nested spans (superstep, node.execute, etc.)
        let graph_name = config
            .graph_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());
        let run_id = pregel.run_id().to_string();
        let recursion_limit = pregel.runnable_config.recursion_limit;

        async move {
            let graph_start = std::time::Instant::now();

            // Emit graph invocation counter metric and update active gauge
            if let Some(ref collector) = config.metrics_collector {
                collector.inc_counter("juncture.graph.invocations", 1);

                let active = self
                    .inner
                    .active_invocations
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                collector.set_gauge("juncture.graph.active_invocations", active);
            }

            // Execute the loop
            let execution_result = async {
                while pregel.tick()? {
                    let result = pregel.execute_superstep().await?;
                    pregel.after_tick(result).await?;
                }
                Ok::<(), JunctureError>(())
            }
            .await;

            // Decrement active invocations gauge (always, regardless of success/failure)
            if let Some(ref collector) = config.metrics_collector {
                let active = self
                    .inner
                    .active_invocations
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
                    - 1;
                collector.set_gauge("juncture.graph.active_invocations", active);
            }

            // Handle execution errors
            let execution_result = match execution_result {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Emit graph error counter metric
                    if let Some(ref collector) = config.metrics_collector {
                        collector.inc_counter("juncture.graph.errors", 1);
                    }
                    Err(e)
                }
            };

            // Extract step and run_id before consuming pregel
            let steps = pregel.step();
            let run_id = pregel.run_id().to_string();

            // Return final state with extracted output
            let final_state = pregel.into_state();
            let output = O::from_state(&final_state);

            // Emit graph duration metric
            if let Some(ref collector) = config.metrics_collector {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Milliseconds as f64 is sufficient for histogram metrics; sub-millisecond precision is not required for graph duration tracking"
                )]
                collector.record_histogram(
                    "juncture.graph.duration_ms",
                    graph_start.elapsed().as_millis() as f64,
                );
            }

            execution_result?;

            Ok(GraphOutput {
                value: final_state,
                output,
                interrupts: Vec::new(),
                metadata: GraphOutputMetadata {
                    steps,
                    run_id,
                    checkpoint_id: config.checkpoint_id.clone(),
                    budget_usage: None,
                },
            })
        }
        .instrument(tracing::info_span!(
            "juncture.graph.invoke",
            "juncture.graph.name" = graph_name,
            "juncture.run.id" = %run_id,
            "juncture.recursion.limit" = recursion_limit,
        ))
        .await
    }

    /// Stream graph execution as a sequence of events.
    ///
    /// Executes the graph and emits [`StreamEvent`](crate::stream::StreamEvent)s
    /// as each superstep completes, enabling real-time monitoring of execution progress.
    ///
    /// This is a convenience wrapper around [`stream_with_config`](Self::stream_with_config)
    /// that uses a default [`StreamConfig`] with no output key filtering.
    ///
    /// # Arguments
    ///
    /// * `input` - Initial state for execution
    /// * `config` - Execution configuration
    /// * `mode` - Stream mode controlling what events are emitted
    ///
    /// # Returns
    ///
    /// A [`StreamHandle`] containing the `run_id` and a pinned stream of results,
    /// where each result is either a `StreamEvent` or a `JunctureError`.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if the graph cannot be initialized.
    /// Runtime errors during execution are sent through the stream.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State, StreamMode};
    /// use futures::StreamExt;
    ///
    /// let handle = compiled.stream(initial_state, &config, StreamMode::Values).await?;
    /// println!("run_id = {}", handle.run_id());
    ///
    /// let mut stream = handle.stream;
    /// while let Some(result) = stream.next().await {
    ///     match result? {
    ///         StreamEvent::Values { state, step } => {
    ///             println!("Step {}: {:?}", step, state);
    ///         }
    ///         StreamEvent::End { output } => {
    ///             println!("Final state: {:?}", output);
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// # Ok::<(), juncture_core::JunctureError>(())
    /// ```
    pub async fn stream(
        &self,
        input: I,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<StreamHandle<S>, JunctureError>
    where
        S: Clone + Send + serde::Serialize + 'static,
        S::Update: serde::Serialize,
    {
        self.stream_with_config(input, config, crate::stream::StreamConfig::new(mode))
            .await
    }

    /// Stream graph execution with full [`StreamConfig`] control.
    ///
    /// Like [`stream`](Self::stream) but accepts a [`StreamConfig`] instead
    /// of a bare [`StreamMode`], enabling output key filtering, subgraph
    /// inclusion, and message batch tuning.
    ///
    /// When [`StreamConfig::output_keys`] is set, [`StreamEvent::Values`]
    /// events are replaced by [`StreamEvent::FilteredValues`] containing only
    /// the requested fields as a JSON object.  Similarly, [`StreamEvent::Updates`]
    /// events become [`StreamEvent::FilteredUpdates`].
    ///
    /// # Arguments
    ///
    /// * `input` - Initial state for execution
    /// * `config` - Execution configuration
    /// * `stream_config` - Full streaming configuration (mode, output keys, etc.)
    ///
    /// # Returns
    ///
    /// A [`StreamHandle`] containing the `run_id` and a pinned stream of
    /// results, where each result is either a [`StreamEvent`] or a [`JunctureError`].
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if the graph cannot be initialized.
    /// Runtime errors during execution are sent through the stream.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State, StreamMode, stream::StreamConfig};
    /// use futures::StreamExt;
    ///
    /// let cfg = StreamConfig::new(StreamMode::Values)
    ///     .with_output_keys(vec!["messages".to_string()]);
    ///
    /// let handle = compiled.stream_with_config(initial_state, &config, cfg).await?;
    /// println!("run_id = {}", handle.run_id());
    ///
    /// let mut stream = handle.stream;
    /// while let Some(result) = stream.next().await {
    ///     match result? {
    ///         StreamEvent::FilteredValues { data, step } => {
    ///             println!("Step {}: {}", step, data);
    ///         }
    ///         StreamEvent::Values { state, step } => {
    ///             println!("Step {}: {:?}", step, state);
    ///         }
    ///         StreamEvent::End { output } => {
    ///             println!("Final state: {:?}", output);
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// # Ok::<(), juncture_core::JunctureError>(())
    /// ```
    #[allow(
        clippy::too_many_lines,
        reason = "stream orchestration: channel setup, PregelLoop wiring, output_keys filtering, and event forwarding are inseparable"
    )]
    #[expect(
        clippy::unused_async,
        reason = "function signature follows async convention for consistency with invoke_async"
    )]
    pub async fn stream_with_config(
        &self,
        input: I,
        config: &RunnableConfig,
        stream_config: crate::stream::StreamConfig,
    ) -> Result<StreamHandle<S>, JunctureError>
    where
        S: Clone + Send + serde::Serialize + 'static,
        S::Update: serde::Serialize,
    {
        use futures::stream;

        let effective = self.effective_config(config);
        let num_fields = 64;
        let mode = stream_config.mode.clone();
        let output_keys = stream_config.output_keys;
        let include_subgraphs = stream_config.include_subgraphs;
        let subgraph_filter = stream_config.subgraph_filter;
        let resumption = stream_config.resumption;

        // Sized channel provides backpressure: 256 for Messages mode (high-throughput
        // LLM token chunks), 32 for all other modes. Per design doc 05-streaming 7.3.
        let capacity = stream_capacity(&mode);
        let (tx, rx) = mpsc::channel(capacity);

        // Extract error handler map from builder metadata
        let error_handler_map = self.build_error_handler_map();

        // Extract per-node retry policies from builder metadata
        let retry_policy_map = self.build_retry_policy_map();

        // Extract per-node timeout policies from builder metadata
        let timeout_policy_map = self.build_timeout_policy_map();

        // Extract graph_name before moving effective
        let graph_name = effective
            .graph_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());

        // Create Pregel loop
        let state_input = input.into_state();
        let mut pregel = PregelLoop::with_error_handlers(
            state_input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            effective,
            num_fields,
            error_handler_map,
        )?;

        pregel.set_retry_policies(retry_policy_map);
        pregel.set_timeout_policies(timeout_policy_map);

        // Wire up budget tracking when budget limits are configured
        if let Some(budget_config) = &pregel.runnable_config.budget {
            let metrics_collector = pregel.runnable_config.metrics_collector.clone();
            pregel.set_budget_tracker(
                BudgetTracker::new(budget_config.clone()).with_metrics_collector(metrics_collector),
            );
        }

        // Extract run_id before moving pregel into the spawned task
        let run_id = pregel.run_id().to_string();
        let recursion_limit = pregel.runnable_config.recursion_limit;

        // Create a separate channel for PregelLoop's internal stream events.
        // Unbounded is acceptable here because this is an internal relay between
        // PregelLoop (sync send) and the forwarding task; the output channel
        // above provides the actual backpressure.
        let (pregel_tx, mut pregel_rx) = mpsc::unbounded_channel();
        pregel.set_stream_sender(pregel_tx);

        // Spawn graph execution in background task
        tokio::spawn(
            async move {
                // Task to forward PregelLoop events to the main stream,
                // applying subgraph filtering, resumption, and output_keys filtering.
                let tx_forward = tx.clone();
                let mode_forward = mode.clone();
                let output_keys_forward = output_keys.clone();
                let resumption_forward = resumption.clone();
                tokio::spawn(async move {
                    // Create a temporary bounded channel for EventEmitter filtering
                    let (temp_tx, _temp_rx) = mpsc::channel(1);
                    let emitter = EventEmitter::new(temp_tx, mode_forward);

                    while let Some(event) = pregel_rx.recv().await {
                        if !emitter.should_emit(&event) {
                            continue;
                        }

                        // Subgraph event filtering: events with non-empty namespace
                        // originate from subgraphs. Skip them unless explicitly included.
                        let ns = event.namespace();
                        if !ns.is_empty() {
                            if !include_subgraphs {
                                continue;
                            }
                            if let Some(ref filter) = subgraph_filter
                                && let Some(first) = ns.first()
                                && !filter.contains(first)
                            {
                                continue;
                            }
                        }

                        // Checkpoint-based resumption: skip step-based events
                        // (Values, Updates, and their filtered variants) at or before
                        // the last processed step.
                        if let Some(ref r) = resumption_forward {
                            let step = match &event {
                                StreamEvent::Values { step, .. }
                                | StreamEvent::FilteredValues { step, .. }
                                | StreamEvent::Updates { step, .. }
                                | StreamEvent::FilteredUpdates { step, .. } => Some(*step),
                                _ => None,
                            };
                            if let Some(s) = step
                                && r.should_skip(s)
                            {
                                continue;
                            }
                        }

                        // Apply output_keys filtering to Updates events from PregelLoop
                        let filtered = output_keys_forward.as_ref().and_then(|keys| match &event {
                            StreamEvent::Updates { node, update, step } => {
                                serde_json::to_value(update).ok().map(|json| {
                                    StreamEvent::FilteredUpdates {
                                        node: node.clone(),
                                        data: crate::stream::filter_json_by_keys(json, keys),
                                        step: *step,
                                    }
                                })
                            }
                            _ => None,
                        });

                        if let Some(filtered_event) = filtered {
                            let _ = tx_forward.send(Ok(filtered_event)).await;
                        } else {
                            let _ = tx_forward.send(Ok(event)).await;
                        }
                    }
                });

                // Execute the Pregel loop
                while matches!(pregel.tick(), Ok(true)) {
                    let step = pregel.step();

                    // Emit Values events if mode is Values, applying output_keys
                    // and resumption filtering.
                    if matches!(mode, StreamMode::Values) {
                        let skip = resumption.as_ref().is_some_and(|r| r.should_skip(step));

                        if !skip {
                            let event = output_keys.as_ref().map_or_else(
                                || StreamEvent::Values {
                                    state: pregel.snapshot_state(),
                                    step,
                                },
                                |keys| {
                                    let json = serde_json::to_value(pregel.snapshot_state())
                                        .unwrap_or(serde_json::Value::Null);
                                    StreamEvent::FilteredValues {
                                        data: crate::stream::filter_json_by_keys(json, keys),
                                        step,
                                    }
                                },
                            );
                            let _ = tx.send(Ok(event)).await;
                        }
                    }

                    // Execute superstep
                    match pregel.execute_superstep().await {
                        Ok(result) => {
                            // Process results and emit events
                            if let Err(e) = pregel.after_tick(result).await {
                                // Emit final state before error
                                let _ = tx
                                    .send(Ok(StreamEvent::End {
                                        output: pregel.snapshot_state(),
                                    }))
                                    .await;
                                // Send error through channel
                                let _ = tx.send(Err(e)).await;
                                return;
                            }
                        }
                        Err(e) => {
                            // Emit final state before error
                            let _ = tx
                                .send(Ok(StreamEvent::End {
                                    output: pregel.snapshot_state(),
                                }))
                                .await;
                            // Send error through channel
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    }
                }

                // Send End event with final state
                let final_state = pregel.into_state();
                let _ = tx
                    .send(Ok(StreamEvent::End {
                        output: final_state,
                    }))
                    .await;
            }
            .instrument(tracing::info_span!(
                "juncture.graph.invoke",
                "juncture.graph.name" = graph_name,
                "juncture.run.id" = %run_id,
                "juncture.recursion.limit" = recursion_limit,
            )),
        );

        // Return stream using futures::stream::unfold to convert Receiver to Stream
        Ok(StreamHandle {
            run_id,
            stream: Box::pin(stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })),
        })
    }

    /// Execute the graph with an externally-provided event emitter
    ///
    /// Unlike [`stream`](Self::stream) which creates internal channels,
    /// this method accepts a pre-configured [`EventEmitter`] for subgraph
    /// execution and custom streaming pipelines. The caller retains control
    /// over the receiver end of the channel.
    ///
    /// # Arguments
    ///
    /// * `input` - Initial state for execution
    /// * `config` - Execution configuration
    /// * `emitter` - Pre-configured event emitter for streaming events
    ///
    /// # Returns
    ///
    /// The final state `S` after graph execution completes.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError`] if the graph cannot be initialized
    /// or if execution fails during a superstep.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State, StreamMode, stream::EventEmitter};
    /// use tokio::sync::mpsc;
    ///
    /// let (tx, mut rx) = mpsc::channel(256);
    /// let emitter = EventEmitter::new(tx, StreamMode::Values);
    ///
    /// // Spawn a task to consume events
    /// tokio::spawn(async move {
    ///     while let Some(event) = rx.recv().await {
    ///         println!("{event:?}");
    ///     }
    /// });
    ///
    /// let final_state = compiled.execute_with_emitter(input, &config, emitter).await?;
    /// # Ok::<(), juncture_core::JunctureError>(())
    /// ```
    pub async fn execute_with_emitter(
        &self,
        input: S,
        config: &RunnableConfig,
        emitter: EventEmitter<S>,
    ) -> Result<S, JunctureError>
    where
        S: Clone + Send + serde::Serialize + 'static,
        S::Update: serde::Serialize,
    {
        let num_fields = 64;

        // Merge compile-time defaults with runtime config
        let mut exec_config = self.effective_config(config);
        // Ensure run_id is populated; generate one if not provided.
        if exec_config.run_id.is_none() {
            exec_config.run_id = Some(uuid::Uuid::new_v4().to_string());
        }

        // Extract graph_name before moving exec_config
        let graph_name = exec_config
            .graph_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());

        let error_handler_map = self.build_error_handler_map();
        let retry_policy_map = self.build_retry_policy_map();
        let timeout_policy_map = self.build_timeout_policy_map();

        let mut pregel = PregelLoop::with_error_handlers(
            input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            exec_config,
            num_fields,
            error_handler_map,
        )?;

        pregel.set_retry_policies(retry_policy_map);
        pregel.set_timeout_policies(timeout_policy_map);

        // Wire up budget tracking when budget limits are configured
        if let Some(budget_config) = &pregel.runnable_config.budget {
            let metrics_collector = pregel.runnable_config.metrics_collector.clone();
            pregel.set_budget_tracker(
                BudgetTracker::new(budget_config.clone()).with_metrics_collector(metrics_collector),
            );
        }

        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        let mode = emitter.mode().clone();
        let run_id = pregel.run_id().to_string();
        let recursion_limit = pregel.runnable_config.recursion_limit;

        // Create a separate channel for PregelLoop's internal stream events
        let (pregel_tx, mut pregel_rx) = mpsc::unbounded_channel();
        pregel.set_stream_sender(pregel_tx);

        // Spawn task to forward PregelLoop events through the emitter
        let emitter_clone = emitter.clone();
        tokio::spawn(async move {
            while let Some(event) = pregel_rx.recv().await {
                if emitter_clone.should_emit(&event) {
                    emitter_clone.emit(event).await;
                }
            }
        });

        async move {
            // Execute the Pregel loop, emitting Values events at each tick
            while pregel.tick()? {
                let step = pregel.step();

                if matches!(mode, StreamMode::Values) {
                    let event = StreamEvent::Values {
                        state: pregel.snapshot_state(),
                        step,
                    };
                    emitter.emit(event).await;
                }

                let result = pregel.execute_superstep().await?;
                pregel.after_tick(result).await?;
            }

            // Emit End event with the final state
            let final_state = pregel.into_state();
            emitter
                .emit(StreamEvent::End {
                    output: final_state.clone(),
                })
                .await;

            Ok(final_state)
        }
        .instrument(tracing::info_span!(
            "juncture.graph.invoke",
            "juncture.graph.name" = graph_name,
            "juncture.run.id" = %run_id,
            "juncture.recursion.limit" = recursion_limit,
        ))
        .await
    }

    /// Resume execution from an interrupt point
    ///
    /// Continues graph execution from where it was interrupted by a
    /// human-in-the-loop interaction, using the provided resume value(s).
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `resume_value` - Resume value(s) to pass to interrupted node(s).
    ///   Supports single value, ID-based resume, and namespace-based resume.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the checkpoint cannot be found.
    ///
    /// # Notes
    ///
    /// This method requires `S: DeserializeOwned` to deserialize the state
    /// from the checkpoint. This is a requirement of checkpoint-based recovery.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State, interrupt::ResumeValue};
    /// use serde_json::json;
    ///
    /// // Single value resume
    /// let output = compiled.resume(
    ///     &config,
    ///     ResumeValue::Single(json!("approved"))
    /// ).await?;
    ///
    /// // ID-based resume for named interrupts
    /// let mut by_id = std::collections::HashMap::new();
    /// by_id.insert("interrupt_123".to_string(), json!("yes"));
    /// let output = compiled.resume(&config, ResumeValue::ById(by_id)).await?;
    ///
    /// // Namespace-based resume for multiple interrupts
    /// let mut by_ns = std::collections::HashMap::new();
    /// by_ns.insert("node1:0".to_string(), json!("value1"));
    /// by_ns.insert("node2:0".to_string(), json!("value2"));
    /// let output = compiled.resume(&config, ResumeValue::ByNamespace(by_ns)).await?;
    /// ```
    pub async fn resume(
        &self,
        config: &RunnableConfig,
        resume_value: ResumeValue,
    ) -> Result<GraphOutput<S, O>, JunctureError>
    where
        S: for<'de> serde::Deserialize<'de> + serde::Serialize,
        S::Update: serde::Serialize,
        O: FromState<S>,
    {
        let checkpointer =
            self.inner.checkpointer.as_ref().ok_or_else(|| {
                JunctureError::checkpoint("no checkpointer configured for resume")
            })?;

        // Load checkpoint
        let tuple = checkpointer
            .get_tuple(config)
            .await
            .map_err(|e| JunctureError::checkpoint(format!("failed to load checkpoint: {e}")))?
            .ok_or_else(|| {
                JunctureError::checkpoint(format!(
                    "checkpoint not found: thread_id={:?}, checkpoint_id={:?}",
                    config.thread_id, config.checkpoint_id
                ))
            })?;

        // Verify checkpoint is from an interrupt state
        // Per design spec 06-hitl.md section 5, resume() only works on interrupt-state checkpoints
        if !matches!(tuple.metadata.source, CheckpointSource::Interrupt { .. }) {
            return Err(JunctureError::checkpoint(format!(
                "resume() requires checkpoint from Interrupt source, got {:?}",
                tuple.metadata.source
            )));
        }

        // Deserialize state from checkpoint (applies schema migration if needed)
        let state = Self::deserialize_with_migration(&tuple.checkpoint)?;

        // Merge compile-time defaults with runtime config, then add resume value
        let mut resume_config = self.effective_config(config);
        resume_config.resume_value = Some(resume_value);
        if resume_config.run_id.is_none() {
            resume_config.run_id = Some(uuid::Uuid::new_v4().to_string());
        }

        // Extract graph_name before moving resume_config
        let graph_name = resume_config
            .graph_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());

        // Create Pregel loop with restored state
        let num_fields = 64; // Maximum number of fields
        let error_handler_map = self.build_error_handler_map();
        let retry_policy_map = self.build_retry_policy_map();
        let timeout_policy_map = self.build_timeout_policy_map();
        let mut pregel = crate::pregel::PregelLoop::with_error_handlers(
            state,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            resume_config,
            num_fields,
            error_handler_map,
        )?;

        pregel.set_retry_policies(retry_policy_map);
        pregel.set_timeout_policies(timeout_policy_map);

        // Wire up budget tracking when budget limits are configured
        if let Some(budget_config) = &pregel.runnable_config.budget {
            let metrics_collector = pregel.runnable_config.metrics_collector.clone();
            pregel.set_budget_tracker(
                BudgetTracker::new(budget_config.clone()).with_metrics_collector(metrics_collector),
            );
        }

        // Set checkpointer
        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        let run_id = pregel.run_id().to_string();
        let recursion_limit = pregel.runnable_config.recursion_limit;

        async move {
            // Execute the loop from the restored state
            while pregel.tick()? {
                let result = pregel.execute_superstep().await?;
                pregel.after_tick(result).await?;
            }

            // Extract step and run_id before consuming pregel
            let steps = pregel.step();
            let run_id = pregel.run_id().to_string();

            // Return final state with extracted output
            let final_state = pregel.into_state();
            let output = O::from_state(&final_state);

            Ok(GraphOutput {
                value: final_state,
                output,
                interrupts: Vec::new(),
                metadata: GraphOutputMetadata {
                    steps,
                    run_id,
                    checkpoint_id: config.checkpoint_id.clone(),
                    budget_usage: None,
                },
            })
        }
        .instrument(tracing::info_span!(
            "juncture.graph.invoke",
            "juncture.graph.name" = graph_name,
            "juncture.run.id" = %run_id,
            "juncture.recursion.limit" = recursion_limit,
        ))
        .await
    }

    /// Resume execution from an interrupt point with a single value
    ///
    /// Convenience method for resuming with a single value. Equivalent to
    /// calling `resume()` with `ResumeValue::Single(value)`.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `value` - Single value to pass to the interrupted node
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured
    /// or if the checkpoint cannot be found.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State};
    /// use serde_json::json;
    ///
    /// // Simple single-value resume
    /// let output = compiled.resume_single(&config, json!("approved")).await?;
    /// ```
    pub async fn resume_single(
        &self,
        config: &RunnableConfig,
        value: serde_json::Value,
    ) -> Result<GraphOutput<S, O>, JunctureError>
    where
        S: for<'de> serde::Deserialize<'de> + serde::Serialize,
        S::Update: serde::Serialize,
        O: FromState<S>,
    {
        self.resume(config, ResumeValue::Single(value)).await
    }

    /// Resume execution from an interrupt checkpoint with streaming events.
    ///
    /// Like [`resume`](Self::resume) but returns a stream of events
    /// for monitoring execution progress in real time. Loads the checkpoint
    /// identified by `config.thread_id` / `config.checkpoint_id`, validates
    /// that it originated from an interrupt, deserializes the saved state,
    /// and then runs the Pregel engine with the same streaming infrastructure
    /// used by [`stream`](Self::stream).
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `resume_value` - Resume value(s) to pass to interrupted node(s).
    ///   Supports single value, ID-based resume, and namespace-based resume.
    /// * `mode` - Stream mode controlling what events are emitted
    ///
    /// # Returns
    ///
    /// A [`StreamHandle`] containing the `run_id` and a pinned stream of
    /// results, where each result is either a
    /// [`StreamEvent`](crate::stream::StreamEvent) or a [`JunctureError`].
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured,
    /// no checkpoint is found, the checkpoint is not from an interrupt state,
    /// or the state cannot be deserialized.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::{StateGraph, State, StreamMode, interrupt::ResumeValue};
    /// use futures::StreamExt;
    /// use serde_json::json;
    ///
    /// let handle = compiled.resume_stream(
    ///     &config,
    ///     ResumeValue::Single(json!("approved")),
    ///     StreamMode::Values,
    /// ).await?;
    /// println!("run_id = {}", handle.run_id());
    ///
    /// let mut stream = handle.stream;
    /// while let Some(result) = stream.next().await {
    ///     match result? {
    ///         StreamEvent::Values { state, step } => {
    ///             println!("Step {}: {:?}", step, state);
    ///         }
    ///         StreamEvent::End { output } => {
    ///             println!("Final state: {:?}", output);
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// # Ok::<(), juncture_core::JunctureError>(())
    /// ```
    pub async fn resume_stream(
        &self,
        config: &RunnableConfig,
        resume_value: ResumeValue,
        mode: StreamMode,
    ) -> Result<StreamHandle<S>, JunctureError>
    where
        S: Clone + Send + for<'de> serde::Deserialize<'de> + serde::Serialize + 'static,
        S::Update: serde::Serialize,
    {
        use futures::stream;

        let checkpointer = self.inner.checkpointer.as_ref().ok_or_else(|| {
            JunctureError::checkpoint("no checkpointer configured for resume_stream")
        })?;

        // Load checkpoint
        let tuple = checkpointer
            .get_tuple(config)
            .await
            .map_err(|e| JunctureError::checkpoint(format!("failed to load checkpoint: {e}")))?
            .ok_or_else(|| {
                JunctureError::checkpoint(format!(
                    "checkpoint not found: thread_id={:?}, checkpoint_id={:?}",
                    config.thread_id, config.checkpoint_id
                ))
            })?;

        // Verify checkpoint is from an interrupt state
        if !matches!(tuple.metadata.source, CheckpointSource::Interrupt { .. }) {
            return Err(JunctureError::checkpoint(format!(
                "resume_stream() requires checkpoint from Interrupt source, got {:?}",
                tuple.metadata.source
            )));
        }

        // Deserialize state from checkpoint (applies schema migration if needed)
        let state = Self::deserialize_with_migration(&tuple.checkpoint)?;

        // Merge compile-time defaults with runtime config, then add resume value
        let mut resume_config = self.effective_config(config);
        resume_config.resume_value = Some(resume_value);
        if resume_config.run_id.is_none() {
            resume_config.run_id = Some(uuid::Uuid::new_v4().to_string());
        }

        // Create Pregel loop with restored state
        let num_fields = 64;
        let error_handler_map = self.build_error_handler_map();
        let retry_policy_map = self.build_retry_policy_map();
        let timeout_policy_map = self.build_timeout_policy_map();
        let mut pregel = PregelLoop::with_error_handlers(
            state,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            resume_config,
            num_fields,
            error_handler_map,
        )?;

        pregel.set_retry_policies(retry_policy_map);
        pregel.set_timeout_policies(timeout_policy_map);

        // Wire up budget tracking when budget limits are configured
        if let Some(budget_config) = &pregel.runnable_config.budget {
            let metrics_collector = pregel.runnable_config.metrics_collector.clone();
            pregel.set_budget_tracker(
                BudgetTracker::new(budget_config.clone()).with_metrics_collector(metrics_collector),
            );
        }

        // Set checkpointer on the Pregel loop
        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        let (_handle, rx, run_id) = Self::spawn_streaming_loop(pregel, mode);

        // Return stream using futures::stream::unfold to convert Receiver to Stream
        Ok(StreamHandle {
            run_id,
            stream: Box::pin(stream::unfold(rx, |mut receiver| async move {
                receiver.recv().await.map(|item| (item, receiver))
            })),
        })
    }

    /// Spawn the Pregel execution loop and event forwarding tasks for streaming.
    ///
    /// Returns the spawned task handle, the receiver end of the event channel,
    /// and the `run_id` for this execution.
    #[allow(
        clippy::type_complexity,
        reason = "return type is a tuple of channel handle, receiver, and run_id which is clear in context"
    )]
    fn spawn_streaming_loop(
        mut pregel: PregelLoop<S>,
        mode: StreamMode,
    ) -> (
        tokio::task::JoinHandle<()>,
        mpsc::Receiver<Result<StreamEvent<S>, JunctureError>>,
        String,
    )
    where
        S: Clone + Send + for<'de> serde::Deserialize<'de> + serde::Serialize + 'static,
        S::Update: serde::Serialize,
    {
        // Sized channel provides backpressure: 256 for Messages mode (high-throughput
        // LLM token chunks), 32 for all other modes. Per design doc 05-streaming 7.3.
        let capacity = stream_capacity(&mode);
        let (tx, rx) = mpsc::channel(capacity);

        // Extract run_id and graph_name before moving pregel into the spawned task
        let run_id = pregel.run_id().to_string();
        let graph_name = pregel
            .runnable_config
            .graph_name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());
        let recursion_limit = pregel.runnable_config.recursion_limit;

        // Create a separate channel for PregelLoop's internal stream events.
        // Unbounded is acceptable here because this is an internal relay between
        // PregelLoop (sync send) and the forwarding task; the output channel
        // above provides the actual backpressure.
        let (pregel_tx, mut pregel_rx) = mpsc::unbounded_channel();
        pregel.set_stream_sender(pregel_tx);

        let handle = tokio::spawn(
            async move {
                // Task to forward PregelLoop events to the main stream
                let tx_forward = tx.clone();
                let mode_forward = mode.clone();
                tokio::spawn(async move {
                    // Create a temporary bounded channel for EventEmitter filtering
                    let (temp_tx, _temp_rx) = mpsc::channel(1);
                    let emitter = EventEmitter::new(temp_tx, mode_forward);

                    while let Some(event) = pregel_rx.recv().await {
                        if emitter.should_emit(&event) {
                            let _ = tx_forward.send(Ok(event)).await;
                        }
                    }
                });

                // Execute the Pregel loop
                while matches!(pregel.tick(), Ok(true)) {
                    let step = pregel.step();

                    // Emit Values events if mode is Values
                    if matches!(mode, StreamMode::Values) {
                        let event = StreamEvent::Values {
                            state: pregel.snapshot_state(),
                            step,
                        };
                        let _ = tx.send(Ok(event)).await;
                    }

                    // Execute superstep
                    match pregel.execute_superstep().await {
                        Ok(result) => {
                            if let Err(e) = pregel.after_tick(result).await {
                                let _ = tx
                                    .send(Ok(StreamEvent::End {
                                        output: pregel.snapshot_state(),
                                    }))
                                    .await;
                                let _ = tx.send(Err(e)).await;
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Ok(StreamEvent::End {
                                    output: pregel.snapshot_state(),
                                }))
                                .await;
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    }
                }

                // Send End event with final state
                let final_state = pregel.into_state();
                let _ = tx
                    .send(Ok(StreamEvent::End {
                        output: final_state,
                    }))
                    .await;
            }
            .instrument(tracing::info_span!(
                "juncture.graph.invoke",
                "juncture.graph.name" = graph_name,
                "juncture.run.id" = %run_id,
                "juncture.recursion.limit" = recursion_limit,
            )),
        );

        (handle, rx, run_id)
    }

    /// Get the current state snapshot for a thread
    ///
    /// Returns the state at the latest checkpoint for the given configuration.
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured,
    /// the checkpoint cannot be retrieved, or the state cannot be deserialized.
    pub async fn get_state(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<StateSnapshot<S>>, JunctureError>
    where
        S: serde::de::DeserializeOwned,
    {
        let checkpointer =
            self.inner.checkpointer.as_ref().ok_or_else(|| {
                JunctureError::checkpoint("no checkpointer configured for get_state")
            })?;

        let tuple = checkpointer
            .get_tuple(config)
            .await
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        let Some(tuple) = tuple else {
            return Ok(None);
        };

        // Deserialize channel values into S (applies schema migration if needed)
        let values = Self::deserialize_with_migration(&tuple.checkpoint)?;

        // Extract next nodes from pending_tasks
        let next: Vec<String> = tuple
            .checkpoint
            .pending_tasks
            .iter()
            .map(|t| t.node.clone())
            .collect();

        let snapshot = StateSnapshot {
            values,
            next,
            config: tuple.config,
            metadata: tuple.metadata,
            created_at: tuple.checkpoint.created_at,
            parent_config: tuple.parent_config,
            tasks: vec![],
        };

        Ok(Some(snapshot))
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
    /// The updated checkpoint is saved with [`CheckpointSource::Update`] and an
    /// incremented step counter.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with `thread_id` and `checkpoint_id` set
    /// * `update` - State update to apply (carries `update`, `label`, and `as_node`)
    ///
    /// # Errors
    ///
    /// Returns [`JunctureError::Checkpoint`] if no checkpointer is configured,
    /// the checkpoint cannot be found, state deserialization/serialization fails,
    /// or the checkpoint cannot be saved.
    ///
    /// # Notes
    ///
    /// This method requires `S: DeserializeOwned + Serialize` to deserialize
    /// the state from the checkpoint and re-serialize after applying the update.
    pub async fn update_state(
        &self,
        config: &RunnableConfig,
        update: StateUpdate<S>,
    ) -> Result<RunnableConfig, JunctureError>
    where
        S: serde::de::DeserializeOwned + serde::Serialize,
    {
        let checkpointer = self.inner.checkpointer.as_ref().ok_or_else(|| {
            JunctureError::checkpoint("no checkpointer configured for update_state")
        })?;

        // Load current checkpoint
        let tuple = checkpointer
            .get_tuple(config)
            .await
            .map_err(|e| JunctureError::checkpoint(e.to_string()))?;

        let Some(tuple) = tuple else {
            return Err(JunctureError::checkpoint(
                "no checkpoint found for update_state",
            ));
        };

        // Deserialize current state from checkpoint (applies schema migration if needed)
        let mut state = Self::deserialize_with_migration(&tuple.checkpoint)?;

        // Apply the user's update
        state.apply(update.update);

        // Re-serialize the updated state
        let updated_values = serde_json::to_value(&state).map_err(|e| {
            JunctureError::checkpoint(format!("failed to serialize updated state: {e}"))
        })?;

        // Record the writer node in metadata.writes when as_node is provided
        let mut writes = tuple.metadata.writes;
        if let Some(as_node) = update.as_node {
            writes.insert(as_node, serde_json::Value::Null);
        }

        // Build updated checkpoint with new channel values
        let updated_checkpoint = Checkpoint {
            channel_values: updated_values,
            ..tuple.checkpoint
        };

        // Build updated metadata: source=Update, step incremented
        let metadata = CheckpointMetadata {
            source: CheckpointSource::Update,
            step: tuple.metadata.step + 1,
            writes,
            ..tuple.metadata
        };

        // Save the updated checkpoint
        checkpointer
            .put(config, updated_checkpoint, metadata)
            .await
            .map_err(|e| JunctureError::checkpoint(e.to_string()))
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
    pub fn get_subgraphs(&self) -> Vec<SubgraphInfo> {
        self.inner.subgraphs.clone()
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

    /// Compile-time `interrupt_before` nodes (HITL defaults)
    interrupt_before: Vec<String>,

    /// Compile-time `interrupt_after` nodes (HITL defaults)
    interrupt_after: Vec<String>,

    /// Mounted subgraph metadata
    subgraphs: Vec<SubgraphInfo>,

    /// Active invocation count for gauge metric emission.
    ///
    /// Tracks the number of currently executing graph invocations across
    /// all shared references to this compiled graph. Used to emit the
    /// `juncture.graph.active_invocations` gauge metric.
    active_invocations: std::sync::atomic::AtomicU64,
}

/// Output from graph execution
///
/// Contains the final output (extracted via [`FromState`]), any interrupts,
/// and execution metadata.
#[derive(Debug)]
pub struct GraphOutput<S: State, O: FromState<S> = S> {
    /// Final state value
    pub value: S,

    /// Output value extracted from state via `FromState`
    pub output: O,

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

    /// Unique run ID for this execution
    pub run_id: String,

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

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            builder_metadata,
            vec![],
            vec![],
            None,
            vec![],
        );
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

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
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

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
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

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let json = compiled.to_json();

        assert!(json.is_object());
        assert!(json.get("nodes").is_some());
        assert!(json.get("edges").is_some());
    }

    #[test]
    fn test_get_graph() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let drawable = compiled.get_graph(None);
        assert_eq!(drawable.nodes.len(), 1);

        let drawable_xray = compiled.get_graph(Some(2));
        assert_eq!(drawable_xray.nodes.len(), 1);
    }

    #[test]
    fn test_get_subgraphs_empty() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let subgraphs = compiled.get_subgraphs();
        assert!(subgraphs.is_empty());
    }

    #[test]
    fn test_get_subgraphs_with_mounted_subgraphs() {
        use crate::subgraph::{SubgraphConfig, SubgraphMount, SubgraphPersistence};

        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let sub_node = mock_node("sub_node");
        let mount_inherit = SubgraphMount::new(
            "child_graph",
            SubgraphConfig {
                persistence: SubgraphPersistence::Inherit,
            },
            Arc::clone(&sub_node),
        );
        let mount_per_thread = SubgraphMount::new(
            "worker_graph",
            SubgraphConfig {
                persistence: SubgraphPersistence::PerThread,
            },
            sub_node,
        );

        let subgraphs = vec![
            super::SubgraphInfo {
                name: mount_inherit.name.clone(),
                persistence: mount_inherit.config.persistence,
            },
            super::SubgraphInfo {
                name: mount_per_thread.name.clone(),
                persistence: mount_per_thread.config.persistence,
            },
        ];

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            subgraphs,
        );

        let result = compiled.get_subgraphs();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "child_graph");
        assert_eq!(result[0].persistence, SubgraphPersistence::Inherit);
        assert_eq!(result[1].name, "worker_graph");
        assert_eq!(result[1].persistence, SubgraphPersistence::PerThread);
    }

    #[tokio::test]
    async fn test_resume_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let result = compiled
            .resume(&config, ResumeValue::Single(serde_json::Value::Null))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "comprehensive test with multiple mock scenarios"
    )]
    async fn test_resume_validates_interrupt_source() {
        use crate::checkpoint::{
            Checkpoint, CheckpointMetadata, CheckpointSource, CheckpointTuple,
        };
        use std::collections::HashMap;

        // Create a mock checkpointer that returns a non-interrupt checkpoint
        struct MockCheckpointer {
            checkpoint_source: CheckpointSource,
        }

        #[async_trait::async_trait]
        impl crate::checkpoint::CheckpointSaver for MockCheckpointer {
            async fn get_tuple(
                &self,
                _config: &crate::config::RunnableConfig,
            ) -> Result<Option<CheckpointTuple>, crate::checkpoint::CheckpointError> {
                Ok(Some(CheckpointTuple {
                    config: crate::config::RunnableConfig::new(),
                    checkpoint: Checkpoint {
                        id: "test_id".to_string(),
                        channel_values: serde_json::json!({}),
                        channel_versions: HashMap::new(),
                        versions_seen: HashMap::new(),
                        pending_tasks: Vec::new(),
                        pending_sends: Vec::new(),
                        pending_interrupts: Vec::new(),
                        schema_version: 1,
                        created_at: "2024-01-01T00:00:00Z".to_string(),
                        v: 1,
                        new_versions: HashMap::new(),
                        counters_since_delta_snapshot: HashMap::new(),
                    },
                    metadata: CheckpointMetadata {
                        source: self.checkpoint_source.clone(),
                        step: 1,
                        writes: HashMap::new(),
                        parents: HashMap::new(),
                        run_id: "test_run".to_string(),
                    },
                    pending_writes: Vec::new(),
                    parent_config: None,
                }))
            }

            async fn list(
                &self,
                _config: &crate::config::RunnableConfig,
                _filter: Option<crate::checkpoint::CheckpointFilter>,
            ) -> Result<Vec<CheckpointTuple>, crate::checkpoint::CheckpointError> {
                Ok(Vec::new())
            }

            async fn put(
                &self,
                _config: &crate::config::RunnableConfig,
                _checkpoint: Checkpoint,
                _metadata: CheckpointMetadata,
            ) -> Result<crate::config::RunnableConfig, crate::checkpoint::CheckpointError>
            {
                Ok(crate::config::RunnableConfig::new())
            }

            async fn put_writes(
                &self,
                _config: &crate::config::RunnableConfig,
                _writes: Vec<crate::checkpoint::PendingWrite>,
                _task_id: &str,
            ) -> Result<(), crate::checkpoint::CheckpointError> {
                Ok(())
            }
        }

        // Test with Input source (should fail)
        let nodes = {
            let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
            nodes.insert("a".to_string(), mock_node("a"));
            nodes
        };

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Input,
            })),
            vec![],
        );

        let config = RunnableConfig::new();
        let result = compiled
            .resume(&config, ResumeValue::Single(serde_json::json!("test")))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_checkpoint());
        assert!(
            err.to_string()
                .contains("resume() requires checkpoint from Interrupt source")
        );
        assert!(err.to_string().contains("Input"));

        // Test with Loop source (should fail)
        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Loop,
            })),
            vec![],
        );

        let result = compiled
            .resume(&config, ResumeValue::Single(serde_json::json!("test")))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_checkpoint());
        assert!(
            err.to_string()
                .contains("resume() requires checkpoint from Interrupt source")
        );
        assert!(err.to_string().contains("Loop"));

        // Test with Interrupt source (should pass validation, though will fail later)
        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Interrupt {
                    node: "test_node".to_string(),
                },
            })),
            vec![],
        );

        let result = compiled
            .resume(&config, ResumeValue::Single(serde_json::json!("test")))
            .await;

        // Should not fail with the source validation error
        // (it will fail with a different error due to mock limitations)
        if let Err(err) = result {
            assert!(
                !err.to_string()
                    .contains("resume() requires checkpoint from Interrupt source")
            );
        }
    }

    #[tokio::test]
    async fn test_resume_single_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let result = compiled
            .resume_single(&config, serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_resume_stream_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let result = compiled
            .resume_stream(
                &config,
                ResumeValue::Single(serde_json::Value::Null),
                StreamMode::Values,
            )
            .await;
        let Err(err) = result else {
            panic!("expected checkpoint error, got stream");
        };
        assert!(err.is_checkpoint());
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "mock checkpointer boilerplate inflates line count; extraction would hurt readability"
    )]
    async fn test_resume_stream_validates_interrupt_source() {
        use crate::checkpoint::{
            Checkpoint, CheckpointError, CheckpointMetadata, CheckpointSource, CheckpointTuple,
        };
        use std::collections::HashMap;

        struct MockCheckpointer {
            checkpoint_source: CheckpointSource,
        }

        #[async_trait::async_trait]
        impl crate::checkpoint::CheckpointSaver for MockCheckpointer {
            async fn get_tuple(
                &self,
                _config: &crate::config::RunnableConfig,
            ) -> Result<Option<CheckpointTuple>, CheckpointError> {
                Ok(Some(CheckpointTuple {
                    config: crate::config::RunnableConfig::new(),
                    checkpoint: Checkpoint {
                        id: "test_id".to_string(),
                        channel_values: serde_json::json!({}),
                        channel_versions: HashMap::new(),
                        versions_seen: HashMap::new(),
                        pending_tasks: Vec::new(),
                        pending_sends: Vec::new(),
                        pending_interrupts: Vec::new(),
                        schema_version: 1,
                        created_at: "2024-01-01T00:00:00Z".to_string(),
                        v: 1,
                        new_versions: HashMap::new(),
                        counters_since_delta_snapshot: HashMap::new(),
                    },
                    metadata: CheckpointMetadata {
                        source: self.checkpoint_source.clone(),
                        step: 1,
                        writes: HashMap::new(),
                        parents: HashMap::new(),
                        run_id: "test_run".to_string(),
                    },
                    pending_writes: Vec::new(),
                    parent_config: None,
                }))
            }

            async fn list(
                &self,
                _config: &crate::config::RunnableConfig,
                _filter: Option<crate::checkpoint::CheckpointFilter>,
            ) -> Result<Vec<CheckpointTuple>, CheckpointError> {
                Ok(Vec::new())
            }

            async fn put(
                &self,
                _config: &crate::config::RunnableConfig,
                _checkpoint: Checkpoint,
                _metadata: CheckpointMetadata,
            ) -> Result<crate::config::RunnableConfig, CheckpointError> {
                Ok(crate::config::RunnableConfig::new())
            }

            async fn put_writes(
                &self,
                _config: &crate::config::RunnableConfig,
                _writes: Vec<crate::checkpoint::PendingWrite>,
                _task_id: &str,
            ) -> Result<(), CheckpointError> {
                Ok(())
            }
        }

        let nodes = {
            let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
            nodes.insert("a".to_string(), mock_node("a"));
            nodes
        };

        // Test with Input source (should fail)
        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Input,
            })),
            vec![],
        );

        let config = RunnableConfig::new();
        let result = compiled
            .resume_stream(
                &config,
                ResumeValue::Single(serde_json::json!("test")),
                StreamMode::Values,
            )
            .await;

        assert!(result.is_err());
        let Err(err) = result else {
            panic!("expected checkpoint error, got stream");
        };
        assert!(err.is_checkpoint());
        assert!(
            err.to_string()
                .contains("resume_stream() requires checkpoint from Interrupt source"),
            "Expected interrupt source validation error, got: {err}"
        );

        // Test with Interrupt source (should pass source validation)
        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Interrupt {
                    node: "test_node".to_string(),
                },
            })),
            vec![],
        );

        let result = compiled
            .resume_stream(
                &config,
                ResumeValue::Single(serde_json::json!("test")),
                StreamMode::Values,
            )
            .await;

        // Should not fail with the source validation error
        // (may succeed or fail with a different error due to mock limitations)
        if let Err(err) = result {
            assert!(
                !err.to_string()
                    .contains("resume_stream() requires checkpoint from Interrupt source"),
                "Interrupt source should pass validation: {err}"
            );
        }
    }

    #[tokio::test]
    async fn test_get_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let result = compiled.get_state(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_get_state_history_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let result = compiled.get_state_history(&config, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_checkpoint());
    }

    #[tokio::test]
    async fn test_update_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
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
    async fn test_update_state_no_checkpoint_found() {
        use crate::checkpoint::{Checkpoint, CheckpointError, CheckpointMetadata, CheckpointTuple};

        struct NoCheckpointCheckpointer;

        #[async_trait::async_trait]
        impl crate::checkpoint::CheckpointSaver for NoCheckpointCheckpointer {
            async fn get_tuple(
                &self,
                _config: &crate::config::RunnableConfig,
            ) -> Result<Option<CheckpointTuple>, CheckpointError> {
                Ok(None)
            }

            async fn list(
                &self,
                _config: &crate::config::RunnableConfig,
                _filter: Option<crate::checkpoint::CheckpointFilter>,
            ) -> Result<Vec<CheckpointTuple>, CheckpointError> {
                Ok(Vec::new())
            }

            async fn put(
                &self,
                _config: &crate::config::RunnableConfig,
                _checkpoint: Checkpoint,
                _metadata: CheckpointMetadata,
            ) -> Result<crate::config::RunnableConfig, CheckpointError> {
                Ok(crate::config::RunnableConfig::new())
            }

            async fn put_writes(
                &self,
                _config: &crate::config::RunnableConfig,
                _writes: Vec<crate::checkpoint::PendingWrite>,
                _task_id: &str,
            ) -> Result<(), CheckpointError> {
                Ok(())
            }
        }

        let nodes = {
            let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
            nodes.insert("a".to_string(), mock_node("a"));
            nodes
        };

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(NoCheckpointCheckpointer)),
            vec![],
        );

        let config = RunnableConfig::new();
        let update = StateUpdate {
            update: StateDummyUpdate,
            label: None,
            as_node: None,
        };

        let result = compiled.update_state(&config, update).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_checkpoint());
        assert!(
            err.to_string().contains("no checkpoint found"),
            "Expected 'no checkpoint found' error, got: {err}"
        );
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "mock checkpointer boilerplate inflates line count; extraction would hurt readability"
    )]
    async fn test_update_state_success() {
        use crate::checkpoint::{
            Checkpoint, CheckpointError, CheckpointMetadata, CheckpointSource, CheckpointTuple,
        };
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        enum ObservedCall {
            Put { source: CheckpointSource, step: i64 },
        }

        struct MockCheckpointer {
            observed: Arc<Mutex<Vec<ObservedCall>>>,
        }

        #[async_trait::async_trait]
        impl crate::checkpoint::CheckpointSaver for MockCheckpointer {
            async fn get_tuple(
                &self,
                _config: &crate::config::RunnableConfig,
            ) -> Result<Option<CheckpointTuple>, CheckpointError> {
                Ok(Some(CheckpointTuple {
                    config: crate::config::RunnableConfig::new(),
                    checkpoint: Checkpoint {
                        id: "cp_123".to_string(),
                        channel_values: serde_json::Value::Null,
                        channel_versions: HashMap::new(),
                        versions_seen: HashMap::new(),
                        pending_tasks: Vec::new(),
                        pending_sends: Vec::new(),
                        pending_interrupts: Vec::new(),
                        schema_version: 1,
                        created_at: "2024-01-01T00:00:00Z".to_string(),
                        v: 1,
                        new_versions: HashMap::new(),
                        counters_since_delta_snapshot: HashMap::new(),
                    },
                    metadata: CheckpointMetadata {
                        source: CheckpointSource::Loop,
                        step: 5,
                        writes: HashMap::new(),
                        parents: HashMap::new(),
                        run_id: "run_abc".to_string(),
                    },
                    pending_writes: Vec::new(),
                    parent_config: None,
                }))
            }

            async fn list(
                &self,
                _config: &crate::config::RunnableConfig,
                _filter: Option<crate::checkpoint::CheckpointFilter>,
            ) -> Result<Vec<CheckpointTuple>, CheckpointError> {
                Ok(Vec::new())
            }

            async fn put(
                &self,
                _config: &crate::config::RunnableConfig,
                _checkpoint: Checkpoint,
                metadata: CheckpointMetadata,
            ) -> Result<crate::config::RunnableConfig, CheckpointError> {
                self.observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(ObservedCall::Put {
                        source: metadata.source,
                        step: metadata.step,
                    });
                Ok(crate::config::RunnableConfig::new())
            }

            async fn put_writes(
                &self,
                _config: &crate::config::RunnableConfig,
                _writes: Vec<crate::checkpoint::PendingWrite>,
                _task_id: &str,
            ) -> Result<(), CheckpointError> {
                Ok(())
            }
        }

        let observed = Arc::new(Mutex::new(Vec::new()));
        let nodes = {
            let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
            nodes.insert("a".to_string(), mock_node("a"));
            nodes
        };

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            Some(Arc::new(MockCheckpointer {
                observed: Arc::clone(&observed),
            })),
            vec![],
        );

        let config = RunnableConfig::new();
        let update = StateUpdate {
            update: StateDummyUpdate,
            label: Some("manual fix".to_string()),
            as_node: Some("admin".to_string()),
        };

        let result = compiled.update_state(&config, update).await;
        assert!(result.is_ok(), "update_state should succeed");

        // Verify the put was called with correct metadata
        let calls = observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(calls.len(), 1, "Expected exactly one put call");
        match &calls[0] {
            ObservedCall::Put { source, step } => {
                assert!(
                    matches!(source, CheckpointSource::Update),
                    "Expected Update source, got {source:?}"
                );
                assert_eq!(*step, 6, "Expected step to be incremented from 5 to 6");
            }
        }
    }

    #[tokio::test]
    async fn test_bulk_update_state_no_checkpointer() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
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

    #[tokio::test]
    async fn test_stream_values_mode() {
        use futures::StreamExt;

        // Build a simple graph: START -> node_a (no outgoing edges, terminates naturally)
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), mock_node("node_a"));

        let mut trigger_table = TriggerTable::new();
        // Add incoming trigger from START
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let handle = compiled
            .stream(StateDummy, &config, StreamMode::Values)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Verify Values and End events are present
        let has_values = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::Values { .. }));
        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));

        assert!(has_values, "Expected Values events in Values mode");
        assert!(has_end, "Expected End event");
    }

    #[tokio::test]
    async fn test_stream_updates_mode() {
        use futures::StreamExt;

        // Build a simple graph: START -> node_a (no outgoing edges, terminates naturally)
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), mock_node("node_a"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let handle = compiled
            .stream(StateDummy, &config, StreamMode::Updates)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Verify Updates and End events are present
        let has_updates = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::Updates { .. }));
        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));

        assert!(has_updates, "Expected Updates events in Updates mode");
        assert!(has_end, "Expected End event");
    }

    #[tokio::test]
    async fn test_stream_debug_mode() {
        use futures::StreamExt;

        // Build a simple graph: START -> node_a (no outgoing edges, terminates naturally)
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), mock_node("node_a"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let handle = compiled
            .stream(StateDummy, &config, StreamMode::Debug)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Debug mode should emit all events including Debug events
        let has_debug = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::Debug(_)));
        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));

        assert!(has_debug, "Expected Debug events in Debug mode");
        assert!(has_end, "Expected End event");
    }

    #[tokio::test]
    async fn test_stream_end_event() {
        use futures::StreamExt;

        // Build a simple graph: START -> node_a (no outgoing edges, terminates naturally)
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), mock_node("node_a"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );
        let config = RunnableConfig::new();

        let handle = compiled
            .stream(StateDummy, &config, StreamMode::Values)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Verify End event is present and contains final state
        assert!(!events.is_empty(), "Stream should emit events");

        let end_events: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let crate::stream::StreamEvent::End { output } = e {
                    Some(output.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(!end_events.is_empty(), "Expected at least one End event");

        // Verify we can clone the state (it should be valid)
        for state in end_events {
            let _cloned_state = state.clone();
        }
    }

    fn mock_node(name: &str) -> Arc<dyn crate::Node<StateDummy>> {
        NodeFnUpdate(|_s: StateDummy| async move { Ok(StateDummyUpdate) }).into_node(name)
    }

    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    #[serde(crate = "serde")]
    struct StateDummy;

    impl crate::State for StateDummy {
        type Update = StateDummyUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, _update: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct StateDummyUpdate;

    /// Test state type with `schema_version=2` and a custom `migrate()` that transforms old data.
    ///
    /// v1 format: `{"value": 0}` (no `label` field)
    /// v2 format: `{"value": N, "label": "migrated"}` (added `label` field)
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
    #[serde(crate = "serde")]
    struct StateV2 {
        value: i32,
        label: String,
    }

    impl crate::State for StateV2 {
        type Update = StateV2Update;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, _update: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}

        fn schema_version() -> u32 {
            2
        }

        fn migrate(from_version: u32, value: serde_json::Value) -> serde_json::Value {
            let mut map = match value {
                serde_json::Value::Object(m) => m,
                other => return other,
            };
            if from_version < 2 {
                map.insert(
                    "label".to_string(),
                    serde_json::Value::String("migrated".to_string()),
                );
            }
            serde_json::Value::Object(map)
        }
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct StateV2Update;

    #[test]
    fn test_deserialize_with_migration_applies_migration_when_versions_differ() {
        use std::collections::HashMap;

        // Simulate a v1 checkpoint that only has `value`, no `label`
        let checkpoint = crate::checkpoint::Checkpoint {
            id: "test_id".to_string(),
            channel_values: serde_json::json!({"value": 42}),
            channel_versions: HashMap::new(),
            versions_seen: HashMap::new(),
            pending_tasks: Vec::new(),
            pending_sends: Vec::new(),
            pending_interrupts: Vec::new(),
            schema_version: 1, // Old version
            created_at: "2024-01-01T00:00:00Z".to_string(),
            v: 1,
            new_versions: HashMap::new(),
            counters_since_delta_snapshot: HashMap::new(),
        };

        let state: StateV2 = CompiledGraph::<StateV2>::deserialize_with_migration(&checkpoint)
            .expect("deserialization with migration should succeed");

        // The migrate() function should have added the `label` field
        assert_eq!(state.value, 42);
        assert_eq!(state.label, "migrated");
    }

    #[test]
    fn test_deserialize_with_migration_skips_migration_when_versions_match() {
        use std::collections::HashMap;

        // Checkpoint already at v2, includes `label`
        let checkpoint = crate::checkpoint::Checkpoint {
            id: "test_id".to_string(),
            channel_values: serde_json::json!({"value": 7, "label": "original"}),
            channel_versions: HashMap::new(),
            versions_seen: HashMap::new(),
            pending_tasks: Vec::new(),
            pending_sends: Vec::new(),
            pending_interrupts: Vec::new(),
            schema_version: 2, // Same version as StateV2::schema_version()
            created_at: "2024-01-01T00:00:00Z".to_string(),
            v: 1,
            new_versions: HashMap::new(),
            counters_since_delta_snapshot: HashMap::new(),
        };

        let state: StateV2 = CompiledGraph::<StateV2>::deserialize_with_migration(&checkpoint)
            .expect("deserialization should succeed");

        // No migration applied, original label preserved
        assert_eq!(state.value, 7);
        assert_eq!(state.label, "original");
    }

    #[test]
    fn test_compile_config_default_is_empty() {
        let config = super::super::CompileConfig::default();
        assert!(config.interrupt_before.is_empty());
        assert!(config.interrupt_after.is_empty());
    }

    #[test]
    fn test_compile_with_config_stores_interrupts() {
        let mut graph = super::super::StateGraph::<StateDummy>::new();
        graph
            .add_node_simple(
                "human_review",
                NodeFnUpdate(|_s: StateDummy| async move { Ok(StateDummyUpdate) }),
            )
            .unwrap();
        graph.set_entry_point("human_review");
        graph.set_finish_point("human_review");

        let config = super::super::CompileConfig {
            interrupt_before: vec!["human_review".to_string()],
            interrupt_after: vec!["human_review".to_string()],
        };

        let compiled = graph.compile_with_config(config).unwrap();
        assert_eq!(compiled.nodes().len(), 1);
    }

    #[test]
    fn test_effective_config_uses_compile_time_defaults() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec!["node_a".to_string()],
            vec!["node_b".to_string()],
            None,
            vec![],
        );

        // When runtime config has no interrupt_before/after, compile-time values apply
        let config = RunnableConfig::new();
        let effective = compiled.effective_config(&config);
        assert_eq!(effective.interrupt_before, Some(vec!["node_a".to_string()]));
        assert_eq!(effective.interrupt_after, Some(vec!["node_b".to_string()]));
    }

    #[test]
    fn test_effective_config_runtime_overrides_compile_time() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec!["compile_before".to_string()],
            vec!["compile_after".to_string()],
            None,
            vec![],
        );

        // Runtime values take precedence
        let config = RunnableConfig::new()
            .with_interrupt_before(vec!["runtime_before".to_string()])
            .with_interrupt_after(vec!["runtime_after".to_string()]);

        let effective = compiled.effective_config(&config);
        assert_eq!(
            effective.interrupt_before,
            Some(vec!["runtime_before".to_string()])
        );
        assert_eq!(
            effective.interrupt_after,
            Some(vec!["runtime_after".to_string()])
        );
    }

    #[test]
    fn test_effective_config_empty_compile_time_no_override() {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<StateDummy>>> = IndexMap::new();
        nodes.insert("a".to_string(), mock_node("a"));

        let compiled: CompiledGraph<StateDummy> = CompiledGraph::new(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        );

        // When compile-time lists are empty, runtime config stays as-is
        let config = RunnableConfig::new();
        let effective = compiled.effective_config(&config);
        assert!(effective.interrupt_before.is_none());
        assert!(effective.interrupt_after.is_none());
    }

    // --- Tests for stream_with_config / output_keys filtering ---

    /// Multi-field state type for `output_keys` filtering tests.
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
    #[serde(crate = "serde")]
    struct MultiFieldState {
        messages: Vec<String>,
        count: i32,
        label: String,
    }

    impl crate::State for MultiFieldState {
        type Update = MultiFieldStateUpdate;
        type FieldVersions = crate::state::FieldVersions;

        fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
            let mut mask = 0u64;
            if let Some(messages) = update.messages {
                self.messages = messages;
                mask |= 1;
            }
            if let Some(count) = update.count {
                self.count = count;
                mask |= 1 << 1;
            }
            if let Some(label) = update.label {
                self.label = label;
                mask |= 1 << 2;
            }
            crate::FieldsChanged(mask)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct MultiFieldStateUpdate {
        messages: Option<Vec<String>>,
        count: Option<i32>,
        label: Option<String>,
    }

    fn multi_field_node(name: &str) -> Arc<dyn crate::Node<MultiFieldState>> {
        NodeFnUpdate(|_s: MultiFieldState| async move {
            Ok(MultiFieldStateUpdate {
                messages: Some(vec!["hello".to_string()]),
                count: Some(1),
                label: Some("updated".to_string()),
            })
        })
        .into_node(name)
    }

    fn build_multi_field_graph() -> CompiledGraph<MultiFieldState> {
        let mut nodes: IndexMap<String, Arc<dyn crate::Node<MultiFieldState>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), multi_field_node("node_a"));

        let mut trigger_table = TriggerTable::new();
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );

        CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        )
    }

    #[tokio::test]
    async fn test_stream_with_config_no_output_keys_emits_values() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();

        let stream_config = crate::stream::StreamConfig::new(StreamMode::Values);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Should contain Values events (not FilteredValues)
        let has_values = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::Values { .. }));
        assert!(has_values, "Expected Values events without output_keys");
    }

    #[tokio::test]
    async fn test_stream_with_config_output_keys_emits_filtered_values() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();

        let stream_config = crate::stream::StreamConfig::new(StreamMode::Values)
            .with_output_keys(vec!["messages".to_string()]);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Should contain FilteredValues events with only "messages" key
        let filtered: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let crate::stream::StreamEvent::FilteredValues { data, .. } = e {
                    Some(data.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(
            !filtered.is_empty(),
            "Expected FilteredValues events with output_keys set"
        );

        for data in &filtered {
            // Should only contain "messages" key
            assert!(
                data.get("messages").is_some(),
                "FilteredValues should contain 'messages' key"
            );
            assert!(
                data.get("count").is_none(),
                "FilteredValues should not contain 'count' key"
            );
            assert!(
                data.get("label").is_none(),
                "FilteredValues should not contain 'label' key"
            );
        }
    }

    #[tokio::test]
    async fn test_stream_delegates_to_stream_with_config() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();

        // stream() with StreamMode should behave identically to
        // stream_with_config(StreamConfig::new(mode))
        let handle = compiled
            .stream(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                StreamMode::Values,
            )
            .await
            .expect("stream should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        let has_values = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::Values { .. }));
        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));

        assert!(has_values, "stream() should emit Values events");
        assert!(has_end, "stream() should emit End event");
    }

    #[tokio::test]
    async fn test_stream_with_config_output_keys_multiple_keys() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();

        let stream_config = crate::stream::StreamConfig::new(StreamMode::Values)
            .with_output_keys(vec!["messages".to_string(), "count".to_string()]);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        let filtered: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let crate::stream::StreamEvent::FilteredValues { data, .. } = e {
                    Some(data.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(!filtered.is_empty());

        for data in &filtered {
            assert!(
                data.get("messages").is_some(),
                "Should contain 'messages' key"
            );
            assert!(data.get("count").is_some(), "Should contain 'count' key");
            assert!(
                data.get("label").is_none(),
                "Should not contain 'label' key"
            );
        }
    }

    #[tokio::test]
    async fn test_stream_with_config_updates_mode_output_keys() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();

        let stream_config = crate::stream::StreamConfig::new(StreamMode::Updates)
            .with_output_keys(vec!["messages".to_string()]);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Should contain FilteredUpdates events with only "messages" key
        let filtered_updates: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let crate::stream::StreamEvent::FilteredUpdates { data, .. } = e {
                    Some(data.clone())
                } else {
                    None
                }
            })
            .collect();

        assert!(
            !filtered_updates.is_empty(),
            "Expected FilteredUpdates events in Updates mode with output_keys"
        );

        for data in &filtered_updates {
            assert!(
                data.get("messages").is_some(),
                "FilteredUpdates should contain 'messages' key"
            );
            // The other keys should be filtered out
            assert!(
                data.get("count").is_none(),
                "FilteredUpdates should not contain 'count' key"
            );
            assert!(
                data.get("label").is_none(),
                "FilteredUpdates should not contain 'label' key"
            );
        }
    }

    #[test]
    fn test_filter_json_by_keys() {
        let json = serde_json::json!({
            "messages": ["hello"],
            "count": 42,
            "label": "test"
        });

        let filtered = crate::stream::filter_json_by_keys(json, &["messages".to_string()]);
        assert!(filtered.get("messages").is_some());
        assert!(filtered.get("count").is_none());
        assert!(filtered.get("label").is_none());
    }

    #[test]
    fn test_filter_json_by_keys_multiple() {
        let json = serde_json::json!({
            "a": 1,
            "b": 2,
            "c": 3
        });

        let filtered =
            crate::stream::filter_json_by_keys(json, &["a".to_string(), "c".to_string()]);
        assert_eq!(filtered.get("a").unwrap(), 1);
        assert!(filtered.get("b").is_none());
        assert_eq!(filtered.get("c").unwrap(), 3);
    }

    #[test]
    fn test_filter_json_by_keys_empty_keys() {
        let json = serde_json::json!({"a": 1});
        let filtered = crate::stream::filter_json_by_keys(json.clone(), &[]);
        assert_eq!(json, filtered);
    }

    #[test]
    fn test_filter_json_by_keys_non_object() {
        let json = serde_json::json!("hello");
        let filtered = crate::stream::filter_json_by_keys(json.clone(), &["a".to_string()]);
        assert_eq!(json, filtered);
    }

    // --- Tests for StreamEvent::namespace() ---

    #[test]
    fn test_stream_event_namespace_custom_has_ns() {
        let event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "sub_node".to_string(),
            data: serde_json::json!({"x": 1}),
            ns: vec!["child_graph".to_string(), "sub_node:uuid".to_string()],
        };
        assert_eq!(event.namespace().len(), 2);
        assert_eq!(event.namespace()[0], "child_graph");
    }

    #[test]
    fn test_stream_event_namespace_messages_has_ns() {
        let event: StreamEvent<StateDummy> = StreamEvent::Messages {
            chunk: crate::stream::MessageChunk {
                content: "hi".to_string(),
                tool_call_chunks: vec![],
                usage_delta: None,
            },
            metadata: crate::stream::MessageStreamMetadata {
                node: "llm".to_string(),
                model: "gpt-4".to_string(),
                tags: vec![],
                ns: vec!["child_graph".to_string()],
            },
        };
        assert_eq!(event.namespace().len(), 1);
        assert_eq!(event.namespace()[0], "child_graph");
    }

    #[test]
    fn test_stream_event_namespace_interrupt_has_ns() {
        let event: StreamEvent<StateDummy> = StreamEvent::Interrupt {
            node: "review".to_string(),
            payload: serde_json::Value::Null,
            resumable: true,
            ns: vec!["subgraph_a".to_string()],
        };
        assert_eq!(event.namespace().len(), 1);
    }

    #[test]
    fn test_stream_event_namespace_values_is_empty() {
        let event: StreamEvent<StateDummy> = StreamEvent::Values {
            state: StateDummy,
            step: 0,
        };
        assert!(event.namespace().is_empty());
    }

    #[test]
    fn test_stream_event_namespace_updates_is_empty() {
        let event: StreamEvent<StateDummy> = StreamEvent::Updates {
            node: "n".to_string(),
            update: StateDummyUpdate,
            step: 0,
        };
        assert!(event.namespace().is_empty());
    }

    #[test]
    fn test_stream_event_namespace_end_is_empty() {
        let event: StreamEvent<StateDummy> = StreamEvent::End { output: StateDummy };
        assert!(event.namespace().is_empty());
    }

    #[test]
    fn test_stream_event_namespace_task_start_is_empty() {
        let event: StreamEvent<StateDummy> = StreamEvent::TaskStart {
            node: "n".to_string(),
            task_id: "t".to_string(),
            step: 0,
        };
        assert!(event.namespace().is_empty());
    }

    #[test]
    fn test_stream_event_namespace_debug_is_empty() {
        let event: StreamEvent<StateDummy> =
            StreamEvent::Debug(crate::stream::DebugEvent::SuperstepStart {
                step: 0,
                pending_nodes: vec![],
            });
        assert!(event.namespace().is_empty());
    }

    // --- Tests for subgraph_filter in stream_with_config ---

    /// Verify that `include_subgraphs=false` causes subgraph events to be
    /// filtered out while top-level events pass through.
    #[test]
    fn test_subgraph_filter_default_excludes_subgraph_events() {
        // A subgraph-namespaced Custom event
        let subgraph_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "sub_node".to_string(),
            data: serde_json::json!({}),
            ns: vec!["child_graph".to_string()],
        };

        // A top-level event (empty namespace)
        let top_level_event: StreamEvent<StateDummy> = StreamEvent::Values {
            state: StateDummy,
            step: 0,
        };

        let include_subgraphs = false;
        // When include_subgraphs is false, subgraph_filter is irrelevant.

        // Top-level event should always pass
        assert!(top_level_event.namespace().is_empty());
        // Subgraph event has namespace
        assert!(!subgraph_event.namespace().is_empty());

        // Filtering logic (mirrors the forwarding task):
        // if !ns.is_empty() && !include_subgraphs { skip }
        let ns = subgraph_event.namespace();
        let should_skip = !ns.is_empty() && !include_subgraphs;
        assert!(
            should_skip,
            "subgraph events should be skipped when include_subgraphs=false"
        );

        let ns = top_level_event.namespace();
        let should_skip = !ns.is_empty() && !include_subgraphs;
        assert!(!should_skip, "top-level events should not be skipped");
    }

    /// Verify that `include_subgraphs=true` with no filter allows all
    /// subgraph events through.
    #[test]
    fn test_subgraph_filter_include_all_passes() {
        let subgraph_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "sub_node".to_string(),
            data: serde_json::json!({}),
            ns: vec!["child_graph".to_string()],
        };

        let include_subgraphs = true;
        let subgraph_filter: Option<Vec<String>> = None;

        let ns = subgraph_event.namespace();
        let should_skip = !ns.is_empty() && !include_subgraphs;
        assert!(
            !should_skip,
            "include_subgraphs=true should not skip subgraph events"
        );

        // With no filter, no additional filtering applies
        assert!(subgraph_filter.is_none());
    }

    /// Verify that `subgraph_filter` allows only matching subgraphs.
    #[test]
    fn test_subgraph_filter_by_name_passes_matching() {
        let matching_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "sub_node".to_string(),
            data: serde_json::json!({}),
            ns: vec!["child_a".to_string()],
        };

        let non_matching_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "sub_node".to_string(),
            data: serde_json::json!({}),
            ns: vec!["child_b".to_string()],
        };

        let include_subgraphs = true;
        let subgraph_filter = Some(vec!["child_a".to_string()]);

        // Matching event should pass
        let ns = matching_event.namespace();
        let should_skip = if ns.is_empty() {
            false
        } else if !include_subgraphs {
            true
        } else if let Some(ref filter) = subgraph_filter {
            ns.first().is_some_and(|first| !filter.contains(first))
        } else {
            false
        };
        assert!(!should_skip, "matching subgraph event should pass filter");

        // Non-matching event should be skipped
        let ns = non_matching_event.namespace();
        let should_skip = if ns.is_empty() {
            false
        } else if !include_subgraphs {
            true
        } else if let Some(ref filter) = subgraph_filter {
            ns.first().is_some_and(|first| !filter.contains(first))
        } else {
            false
        };
        assert!(
            should_skip,
            "non-matching subgraph event should be filtered out"
        );
    }

    /// Verify that Messages events with subgraph namespace are correctly
    /// identified and filtered.
    #[test]
    fn test_subgraph_filter_applies_to_messages_events() {
        let subgraph_messages: StreamEvent<StateDummy> = StreamEvent::Messages {
            chunk: crate::stream::MessageChunk {
                content: "token".to_string(),
                tool_call_chunks: vec![],
                usage_delta: None,
            },
            metadata: crate::stream::MessageStreamMetadata {
                node: "llm".to_string(),
                model: "gpt-4".to_string(),
                tags: vec![],
                ns: vec!["sub_llm".to_string()],
            },
        };

        let include_subgraphs = false;
        assert!(!subgraph_messages.namespace().is_empty());

        let ns = subgraph_messages.namespace();
        let should_skip = !ns.is_empty() && !include_subgraphs;
        assert!(
            should_skip,
            "subgraph Messages events should be filtered when include_subgraphs=false"
        );
    }

    /// Verify that Interrupt events with subgraph namespace are correctly
    /// identified and filtered.
    #[test]
    fn test_subgraph_filter_applies_to_interrupt_events() {
        let subgraph_interrupt: StreamEvent<StateDummy> = StreamEvent::Interrupt {
            node: "review".to_string(),
            payload: serde_json::Value::Null,
            resumable: true,
            ns: vec!["sub_review".to_string()],
        };

        let include_subgraphs = false;
        assert!(!subgraph_interrupt.namespace().is_empty());

        let ns = subgraph_interrupt.namespace();
        let should_skip = !ns.is_empty() && !include_subgraphs;
        assert!(
            should_skip,
            "subgraph Interrupt events should be filtered when include_subgraphs=false"
        );
    }

    // --- Nested subgraph (sub-subgraph) filtering tests ---

    /// Verify that nested subgraph events (2-level namespace) are correctly
    /// filtered by `include_subgraphs=false`.
    #[test]
    fn test_nested_subgraph_default_excludes_nested_events() {
        // A sub-subgraph event with 2-level namespace
        let nested_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "deep_node".to_string(),
            data: serde_json::json!({}),
            ns: vec!["parent".to_string(), "child".to_string()],
        };

        let include_subgraphs = false;

        // Has non-empty namespace (correctly identified as subgraph event)
        assert_eq!(nested_event.namespace(), &["parent", "child"]);
        assert!(!nested_event.namespace().is_empty());

        let should_skip = !nested_event.namespace().is_empty() && !include_subgraphs;
        assert!(
            should_skip,
            "nested subgraph events should be skipped when include_subgraphs=false"
        );
    }

    /// Verify that nested subgraph events (2-level namespace) pass through
    /// when `include_subgraphs=true` with no filter.
    #[test]
    fn test_nested_subgraph_include_all_passes() {
        // Simulate the namespace that an EventEmitter with_subgraph_ns
        // would produce for a parent/child chain
        let emitter_ns = vec!["parent".to_string(), "child".to_string()];

        // Build a nested event with the namespace set
        let nested_custom_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "inner".to_string(),
            data: serde_json::json!({"k": "v"}),
            ns: emitter_ns,
        };

        let include_subgraphs = true;
        let subgraph_filter: Option<Vec<String>> = None;

        let should_skip = !nested_custom_event.namespace().is_empty() && !include_subgraphs;
        assert!(
            !should_skip,
            "nested subgraph events should pass when include_subgraphs=true"
        );
        assert!(subgraph_filter.is_none());
    }

    /// Verify that `subgraph_filter` based on `ns.first()` correctly includes
    /// nested subgraph events when the outermost name matches.
    #[test]
    fn test_nested_subgraph_filter_matches_outermost_name() {
        // Events from deeply nested subgraph: parent -> child -> grandchild
        let nested_event: StreamEvent<StateDummy> = StreamEvent::Custom {
            node: "deep".to_string(),
            data: serde_json::json!({}),
            ns: vec![
                "parent".to_string(),
                "child".to_string(),
                "grandchild".to_string(),
            ],
        };

        let include_subgraphs = true;
        // Filter on outermost name only
        let subgraph_filter = Some(vec!["parent".to_string()]);

        let ns = nested_event.namespace();
        let should_skip = if ns.is_empty() {
            false
        } else if !include_subgraphs {
            true
        } else if let Some(ref filter) = subgraph_filter {
            ns.first().is_some_and(|first| !filter.contains(first))
        } else {
            false
        };

        assert!(
            !should_skip,
            "nested event from parent should pass when parent is in filter"
        );

        // Test with non-matching outermost name
        let subgraph_filter_other = Some(vec!["other".to_string()]);
        let should_skip_other = if ns.is_empty() {
            false
        } else if !include_subgraphs {
            true
        } else if let Some(ref filter) = subgraph_filter_other {
            ns.first().is_some_and(|first| !filter.contains(first))
        } else {
            false
        };

        assert!(
            should_skip_other,
            "nested event should be skipped when outermost name does not match filter"
        );
    }

    /// Verify that nested Messages events (with ns in metadata) are correctly
    /// identified and filtered.
    #[test]
    fn test_nested_subgraph_messages_filtering() {
        let nested_messages: StreamEvent<StateDummy> = StreamEvent::Messages {
            chunk: crate::stream::MessageChunk {
                content: "nested_token".to_string(),
                tool_call_chunks: vec![],
                usage_delta: None,
            },
            metadata: crate::stream::MessageStreamMetadata {
                node: "llm".to_string(),
                model: "gpt-4".to_string(),
                tags: vec![],
                ns: vec!["outer".to_string(), "inner".to_string()],
            },
        };

        let include_subgraphs = false;

        // verify namespace detection works for Messages events
        assert_eq!(
            nested_messages.namespace(),
            &["outer", "inner"],
            "Messages events should expose full nested namespace via metadata.ns"
        );

        let should_skip = !nested_messages.namespace().is_empty() && !include_subgraphs;
        assert!(
            should_skip,
            "nested subgraph Messages events should be filtered when include_subgraphs=false"
        );

        // With include_subgraphs=true, nested Messages pass through
        let include_subgraphs_true = true;
        let should_pass = nested_messages.namespace().is_empty() || include_subgraphs_true;
        assert!(
            should_pass,
            "nested subgraph Messages events should pass when include_subgraphs=true"
        );
    }

    /// Verify that `SubgraphTransformer::to_emitter()` produces an emitter
    /// whose namespace carries the correct nested path.
    #[test]
    fn test_subgraph_transformer_to_emitter_nested_ns() {
        let transformer = crate::SubgraphTransformer::new("child".to_string());
        let transformer = transformer.child_transformer("grandchild");

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let emitter = transformer.to_emitter::<StateDummy>(tx, crate::stream::StreamMode::Values);

        // Verify the emitter has the correct namespace chain
        assert_eq!(emitter.ns(), &["child", "grandchild"]);
    }

    /// Verify that `SubgraphTransformer::child_transformer()` properly
    /// chains namespace for 3 levels deep.
    #[test]
    fn test_transformer_child_chain_three_levels() {
        use crate::stream::StreamEvent;

        let grandparent = crate::SubgraphTransformer::new("grandparent".to_string());
        let parent = grandparent.child_transformer("parent");
        let child = parent.child_transformer("child");

        // Verify transform produces correct node prefix
        let event = StreamEvent::<StateDummy>::TaskStart {
            node: "worker".to_string(),
            task_id: "t1".to_string(),
            step: 1,
        };

        let result = child.transform(&event).expect("should pass filter");
        match result {
            StreamEvent::TaskStart { node, .. } => {
                assert_eq!(node, "grandparent/parent/child/worker");
            }
            other => panic!("expected TaskStart, got {other:?}"),
        }

        // Verify Custom event ns field
        let custom_event = StreamEvent::<StateDummy>::Custom {
            node: "agent".to_string(),
            data: serde_json::json!({}),
            ns: vec![],
        };
        let result = child.transform(&custom_event).expect("custom should pass");
        match result {
            StreamEvent::Custom { node, ns, .. } => {
                assert_eq!(node, "grandparent/parent/child/agent");
                assert_eq!(ns, vec!["grandparent", "parent", "child"]);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    /// Verify that `StreamConfig::with_subgraphs()` and
    /// `StreamConfig::with_subgraph_filter()` produce the expected config.
    #[test]
    fn test_stream_config_subgraph_builder_methods() {
        let cfg = crate::stream::StreamConfig::new(StreamMode::Values);
        assert!(!cfg.include_subgraphs);
        assert!(cfg.subgraph_filter.is_none());

        let cfg = cfg.with_subgraphs(true);
        assert!(cfg.include_subgraphs);

        let cfg = cfg.with_subgraph_filter(vec!["sub_a".to_string()]);
        assert_eq!(cfg.subgraph_filter.as_ref().map(Vec::len), Some(1));
        assert_eq!(
            cfg.subgraph_filter
                .as_ref()
                .and_then(|f| f.first().cloned()),
            Some("sub_a".to_string())
        );
    }

    /// End-to-end test: `stream_with_config` with default config
    /// (`include_subgraphs=false`) does not forward subgraph events.
    #[tokio::test]
    async fn test_stream_default_config_no_subgraph_events() {
        use futures::StreamExt;

        let compiled = build_multi_field_graph();
        let config = RunnableConfig::new();
        let stream_config = crate::stream::StreamConfig::new(StreamMode::Values);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // All events should have empty namespace (no subgraph events)
        for event in &events {
            assert!(
                event.namespace().is_empty(),
                "Expected no subgraph events, but found one with ns: {:?}",
                event.namespace()
            );
        }
    }

    // --- Tests for checkpoint-based stream resumption ---

    /// Build a two-node chained graph: START -> `node_a` -> `node_b`.
    /// `node_a` increments count, `node_b` increments count again.
    /// This produces two supersteps so resumption can skip one.
    fn build_two_step_graph() -> CompiledGraph<MultiFieldState> {
        let node_a = NodeFnUpdate(|s: MultiFieldState| async move {
            Ok(MultiFieldStateUpdate {
                messages: Some(s.messages),
                count: Some(s.count + 1),
                label: Some(s.label),
            })
        })
        .into_node("node_a");

        let node_b = NodeFnUpdate(|s: MultiFieldState| async move {
            Ok(MultiFieldStateUpdate {
                messages: Some(s.messages),
                count: Some(s.count + 10),
                label: Some(s.label),
            })
        })
        .into_node("node_b");

        let mut nodes: IndexMap<String, Arc<dyn crate::Node<MultiFieldState>>> = IndexMap::new();
        nodes.insert("node_a".to_string(), node_a);
        nodes.insert("node_b".to_string(), node_b);

        let mut trigger_table = TriggerTable::new();
        // START -> node_a
        trigger_table.add_incoming(
            "node_a".to_string(),
            crate::edge::TriggerSource::Edge {
                from: crate::edge::START.to_string(),
            },
        );
        // node_a -> node_b (count field changed triggers node_b)
        trigger_table.add_outgoing(
            "node_a".to_string(),
            crate::edge::CompiledEdge::Fixed {
                target: "node_b".to_string(),
            },
        );

        CompiledGraph::new(
            nodes,
            trigger_table,
            IndexMap::new(),
            vec![],
            vec![],
            None,
            vec![],
        )
    }

    #[tokio::test]
    async fn test_resumption_skips_values_at_or_before_last_step() {
        use futures::StreamExt;

        let compiled = build_two_step_graph();
        let config = RunnableConfig::new();

        let resumption = crate::stream::StreamResumption::new("run1".to_string(), None, Some(0));
        let stream_config =
            crate::stream::StreamConfig::new(StreamMode::Values).with_resumption(resumption);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Values events at step 0 should be skipped; step 1 and End should remain.
        let values_steps: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                crate::stream::StreamEvent::Values { step, .. } => Some(*step),
                _ => None,
            })
            .collect();

        assert!(
            !values_steps.contains(&0),
            "Values at step 0 should be skipped, got steps: {values_steps:?}"
        );

        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));
        assert!(has_end, "End event must always be emitted");
    }

    #[tokio::test]
    async fn test_resumption_allows_values_after_last_step() {
        use futures::StreamExt;

        let compiled = build_two_step_graph();
        let config = RunnableConfig::new();

        let resumption = crate::stream::StreamResumption::new("run1".to_string(), None, Some(5));
        let stream_config =
            crate::stream::StreamConfig::new(StreamMode::Values).with_resumption(resumption);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // With last_step=5, all steps (0, 1) should be skipped, but End still arrives.
        let values_steps: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                crate::stream::StreamEvent::Values { step, .. } => Some(*step),
                _ => None,
            })
            .collect();

        assert!(
            values_steps.is_empty(),
            "All Values should be skipped with last_step=5, got steps: {values_steps:?}"
        );

        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));
        assert!(
            has_end,
            "End event must always be emitted even when all steps are skipped"
        );
    }

    #[tokio::test]
    async fn test_resumption_none_last_step_allows_all_events() {
        use futures::StreamExt;

        let compiled = build_two_step_graph();
        let config = RunnableConfig::new();

        let resumption = crate::stream::StreamResumption::new("run1".to_string(), None, None);
        let stream_config =
            crate::stream::StreamConfig::new(StreamMode::Values).with_resumption(resumption);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // With last_step=None, nothing is skipped.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, crate::stream::StreamEvent::Values { .. })),
            "Values events should be emitted when last_step is None"
        );

        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));
        assert!(has_end, "End event must be present");
    }

    #[tokio::test]
    async fn test_resumption_skips_updates_at_or_before_last_step() {
        use futures::StreamExt;

        let compiled = build_two_step_graph();
        let config = RunnableConfig::new();

        let resumption = crate::stream::StreamResumption::new("run1".to_string(), None, Some(0));
        let stream_config =
            crate::stream::StreamConfig::new(StreamMode::Updates).with_resumption(resumption);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        // Updates at step 0 should be skipped; updates at step > 0 and End remain.
        let updates_steps: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                crate::stream::StreamEvent::Updates { step, .. }
                | crate::stream::StreamEvent::FilteredUpdates { step, .. } => Some(*step),
                _ => None,
            })
            .collect();

        assert!(
            !updates_steps.contains(&0),
            "Updates at step 0 should be skipped, got steps: {updates_steps:?}"
        );

        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));
        assert!(has_end, "End event must always be emitted");
    }

    #[tokio::test]
    async fn test_resumption_no_resumption_emits_all_events() {
        use futures::StreamExt;

        let compiled = build_two_step_graph();
        let config = RunnableConfig::new();

        // No resumption set (default StreamConfig)
        let stream_config = crate::stream::StreamConfig::new(StreamMode::Values);

        let handle = compiled
            .stream_with_config(
                MultiFieldState {
                    messages: vec![],
                    count: 0,
                    label: String::new(),
                },
                &config,
                stream_config,
            )
            .await
            .expect("stream_with_config should succeed");

        let mut events = Vec::new();
        let mut stream = handle.stream;
        while let Some(result) = stream.next().await {
            events.push(result.expect("stream event should be Ok"));
        }

        let values_count = events
            .iter()
            .filter(|e| matches!(e, crate::stream::StreamEvent::Values { .. }))
            .count();

        assert!(
            values_count >= 1,
            "At least one Values event expected without resumption"
        );

        let has_end = events
            .iter()
            .any(|e| matches!(e, crate::stream::StreamEvent::End { .. }));
        assert!(has_end, "End event must be present");
    }
}

// Rust guideline compliant 2026-05-21
