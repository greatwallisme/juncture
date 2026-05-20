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
    pregel::PregelLoop,
    stream::{EventEmitter, StreamEvent, StreamMode},
};
use futures::Stream;
use indexmap::IndexMap;
use std::{pin::Pin, sync::Arc};
use tokio::sync::mpsc;

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
    /// # Returns
    ///
    /// A pinned stream of results, where each result is either a `StreamEvent` or a `JunctureError`.
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
    /// let mut stream = compiled.stream(initial_state, &config, StreamMode::Values).await?;
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
    #[expect(
        clippy::unused_async,
        reason = "function signature follows async convention for consistency with invoke_async"
    )]
    pub async fn stream(
        &self,
        input: S,
        config: &RunnableConfig,
        mode: StreamMode,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>,
        JunctureError,
    >
    where
        S: Clone + Send + 'static,
    {
        use futures::stream;

        let num_fields = 64;

        // Create channel for Result<StreamEvent, JunctureError>
        // Using unbounded channel for non-blocking sends
        let (tx, rx) = mpsc::unbounded_channel();

        // Create Pregel loop
        let mut pregel = PregelLoop::new(
            input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            config.clone(),
            num_fields,
        )?;

        // Create a separate channel for PregelLoop's internal stream events
        let (pregel_tx, mut pregel_rx) = mpsc::unbounded_channel();
        pregel.set_stream_sender(pregel_tx);

        // Spawn graph execution in background task
        tokio::spawn(async move {
            // Task to forward PregelLoop events to the main stream
            let tx_forward = tx.clone();
            let mode_forward = mode.clone();
            tokio::spawn(async move {
                // Create a temporary bounded channel for EventEmitter filtering
                let (temp_tx, _temp_rx) = mpsc::channel(1);
                let emitter = EventEmitter::new(temp_tx, mode_forward);

                while let Some(event) = pregel_rx.recv().await {
                    // Filter events based on stream mode using EventEmitter's should_emit
                    if emitter.should_emit(&event) {
                        let _ = tx_forward.send(Ok(event));
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
                    let _ = tx.send(Ok(event));
                }

                // Execute superstep
                match pregel.execute_superstep().await {
                    Ok(result) => {
                        // Process results and emit events
                        if let Err(e) = pregel.after_tick(result).await {
                            // Emit final state before error
                            let _ = tx.send(Ok(StreamEvent::End {
                                output: pregel.snapshot_state(),
                            }));
                            // Send error through channel
                            let _ = tx.send(Err(e));
                            return;
                        }
                    }
                    Err(e) => {
                        // Emit final state before error
                        let _ = tx.send(Ok(StreamEvent::End {
                            output: pregel.snapshot_state(),
                        }));
                        // Send error through channel
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }

            // Send End event with final state
            let final_state = pregel.into_state();
            let _ = tx.send(Ok(StreamEvent::End {
                output: final_state,
            }));
        });

        // Return stream using futures::stream::unfold to convert UnboundedReceiver to Stream
        Ok(Box::pin(stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })))
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
        S: Clone + Send + 'static,
    {
        let num_fields = 64;

        let mut pregel = PregelLoop::new(
            input,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            config.clone(),
            num_fields,
        )?;

        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        let mode = emitter.mode().clone();

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
    ) -> Result<GraphOutput<S>, JunctureError>
    where
        S: for<'de> serde::Deserialize<'de>,
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

        // Deserialize state from checkpoint
        let state: S = serde_json::from_value(tuple.checkpoint.channel_values)
            .map_err(|e| JunctureError::checkpoint(format!("failed to deserialize state: {e}")))?;

        // Create a new config with resume value
        let mut resume_config = config.clone();
        resume_config.resume_value = Some(resume_value);

        // Create Pregel loop with restored state
        let num_fields = 64; // Maximum number of fields
        let mut pregel = crate::pregel::PregelLoop::new(
            state,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            resume_config,
            num_fields,
        )?;

        // Set checkpointer
        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        // Execute the loop from the restored state
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
    ) -> Result<GraphOutput<S>, JunctureError>
    where
        S: for<'de> serde::Deserialize<'de>,
    {
        self.resume(config, ResumeValue::Single(value)).await
    }

    /// Resume execution from an interrupt checkpoint with streaming events
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
    /// A pinned stream of results, where each result is either a
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
    /// let mut stream = compiled.resume_stream(
    ///     &config,
    ///     ResumeValue::Single(json!("approved")),
    ///     StreamMode::Values,
    /// ).await?;
    ///
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
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<StreamEvent<S>, JunctureError>> + Send>>,
        JunctureError,
    >
    where
        S: Clone + Send + for<'de> serde::Deserialize<'de> + 'static,
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

        // Deserialize state from checkpoint
        let state: S = serde_json::from_value(tuple.checkpoint.channel_values)
            .map_err(|e| JunctureError::checkpoint(format!("failed to deserialize state: {e}")))?;

        // Create resume config
        let mut resume_config = config.clone();
        resume_config.resume_value = Some(resume_value);

        // Create Pregel loop with restored state
        let num_fields = 64;
        let mut pregel = PregelLoop::new(
            state,
            self.inner.nodes.clone(),
            self.inner.trigger_table.clone(),
            resume_config,
            num_fields,
        )?;

        // Set checkpointer on the Pregel loop
        if let Some(cp) = self.inner.checkpointer.clone() {
            pregel.set_checkpointer(cp);
        }

        // Create channel for Result<StreamEvent, JunctureError>
        let (tx, rx) = mpsc::unbounded_channel();

        // Create a separate channel for PregelLoop's internal stream events
        let (pregel_tx, mut pregel_rx) = mpsc::unbounded_channel();
        pregel.set_stream_sender(pregel_tx);

        // Spawn graph execution in background task
        tokio::spawn(async move {
            // Task to forward PregelLoop events to the main stream
            let tx_forward = tx.clone();
            let mode_forward = mode.clone();
            tokio::spawn(async move {
                // Create a temporary bounded channel for EventEmitter filtering
                let (temp_tx, _temp_rx) = mpsc::channel(1);
                let emitter = EventEmitter::new(temp_tx, mode_forward);

                while let Some(event) = pregel_rx.recv().await {
                    if emitter.should_emit(&event) {
                        let _ = tx_forward.send(Ok(event));
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
                    let _ = tx.send(Ok(event));
                }

                // Execute superstep
                match pregel.execute_superstep().await {
                    Ok(result) => {
                        if let Err(e) = pregel.after_tick(result).await {
                            // Emit final state before error
                            let _ = tx.send(Ok(StreamEvent::End {
                                output: pregel.snapshot_state(),
                            }));
                            let _ = tx.send(Err(e));
                            return;
                        }
                    }
                    Err(e) => {
                        // Emit final state before error
                        let _ = tx.send(Ok(StreamEvent::End {
                            output: pregel.snapshot_state(),
                        }));
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }

            // Send End event with final state
            let final_state = pregel.into_state();
            let _ = tx.send(Ok(StreamEvent::End {
                output: final_state,
            }));
        });

        // Return stream using futures::stream::unfold to convert UnboundedReceiver to Stream
        Ok(Box::pin(stream::unfold(rx, |mut receiver| async move {
            receiver.recv().await.map(|item| (item, receiver))
        })))
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

        // Deserialize channel values into S
        let values: S = serde_json::from_value(tuple.checkpoint.channel_values)
            .map_err(|e| JunctureError::checkpoint(format!("failed to deserialize state: {e}")))?;

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

        // Deserialize current state from checkpoint
        let mut state: S = serde_json::from_value(tuple.checkpoint.channel_values)
            .map_err(|e| JunctureError::checkpoint(format!("failed to deserialize state: {e}")))?;

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

        let compiled = CompiledGraph::with_checkpointer(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Input,
            })),
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
        let compiled = CompiledGraph::with_checkpointer(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Loop,
            })),
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
        let compiled = CompiledGraph::with_checkpointer(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Interrupt {
                    node: "test_node".to_string(),
                },
            })),
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

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
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

        let compiled = CompiledGraph::new(nodes, TriggerTable::new(), IndexMap::new());
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
        let compiled = CompiledGraph::with_checkpointer(
            nodes.clone(),
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Input,
            })),
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
        let compiled = CompiledGraph::with_checkpointer(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                checkpoint_source: CheckpointSource::Interrupt {
                    node: "test_node".to_string(),
                },
            })),
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

        let compiled = CompiledGraph::with_checkpointer(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(NoCheckpointCheckpointer)),
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

        let compiled = CompiledGraph::with_checkpointer(
            nodes,
            TriggerTable::new(),
            IndexMap::new(),
            Some(Arc::new(MockCheckpointer {
                observed: Arc::clone(&observed),
            })),
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

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let config = RunnableConfig::new();

        let mut stream = compiled
            .stream(StateDummy, &config, StreamMode::Values)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
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

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let config = RunnableConfig::new();

        let mut stream = compiled
            .stream(StateDummy, &config, StreamMode::Updates)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
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

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let config = RunnableConfig::new();

        let mut stream = compiled
            .stream(StateDummy, &config, StreamMode::Debug)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
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

        let compiled = CompiledGraph::new(nodes, trigger_table, IndexMap::new());
        let config = RunnableConfig::new();

        let mut stream = compiled
            .stream(StateDummy, &config, StreamMode::Values)
            .await
            .expect("stream should succeed");

        // Collect events
        let mut events = Vec::new();
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
