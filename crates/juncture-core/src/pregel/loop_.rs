//! Main Pregel execution loop
//!
//! This module provides the `PregelLoop` struct that orchestrates graph execution
//! using the Pregel algorithm with version tracking and task scheduling.

use crate::{
    JunctureError, Node, State,
    checkpoint::{
        Checkpoint, CheckpointMetadata, CheckpointSource, DeltaCounters, generate_checkpoint_id,
    },
    edge::TriggerTable,
    interrupt::should_interrupt,
    pregel::{
        budget::BudgetTracker,
        context::ExecutionContext,
        durability::Durability,
        runner::execute_superstep,
        scheduler::{
            FieldVersionTracker, VersionsSeen, apply_writes, compute_next_tasks,
            schedule_error_handlers,
        },
        types::{BubbleUp, LoopStatus, PendingTask, SuperstepResult},
    },
    stream::{DebugEvent, StreamEvent},
};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Graceful shutdown control for Pregel execution
///
/// Allows external callers to request drain (finish current tasks but don't start new ones).
/// Checked in `PregelLoop::tick()` before computing next tasks.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::loop_::RunControl;
///
/// let run_control = RunControl::new();
///
/// // Request drain from another thread
/// let rc_clone = run_control.clone();
/// std::thread::spawn(move || {
///     // After some condition, request drain
///     rc_clone.request_drain();
/// });
///
/// // In the main loop
/// if run_control.is_drain_requested() {
///     // Finish current tasks but don't start new ones
/// }
/// ```
#[derive(Clone, Debug)]
pub struct RunControl {
    drain_requested: Arc<AtomicBool>,
}

impl RunControl {
    /// Create a new run control instance
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::RunControl;
    ///
    /// let run_control = RunControl::new();
    /// assert!(!run_control.is_drain_requested());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            drain_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request drain (finish current tasks but don't start new ones)
    ///
    /// This is thread-safe and can be called from any thread.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::RunControl;
    ///
    /// let run_control = RunControl::new();
    /// run_control.request_drain();
    /// assert!(run_control.is_drain_requested());
    /// ```
    pub fn request_drain(&self) {
        self.drain_requested.store(true, Ordering::Release);
    }

    /// Check if drain has been requested
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::RunControl;
    ///
    /// let run_control = RunControl::new();
    /// assert!(!run_control.is_drain_requested());
    /// ```
    #[must_use]
    pub fn is_drain_requested(&self) -> bool {
        self.drain_requested.load(Ordering::Acquire)
    }
}

impl Default for RunControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Main Pregel execution loop
///
/// Orchestrates graph execution using the Pregel algorithm, managing
/// task scheduling, version tracking, and execution state.
pub struct PregelLoop<S: State> {
    /// Current execution state
    pub state: S,

    /// Graph nodes
    pub nodes: IndexMap<String, Arc<dyn Node<S>>>,

    /// Trigger table for routing
    pub trigger_table: TriggerTable<S>,

    /// Field version tracker
    pub field_versions: FieldVersionTracker,

    /// Versions seen by each node
    pub versions_seen: VersionsSeen,

    /// Execution configuration
    pub runnable_config: crate::config::RunnableConfig,

    /// Cancellation token
    pub cancellation_token: CancellationToken,

    /// Optional stream event sender
    pub stream_tx: Option<mpsc::UnboundedSender<StreamEvent<S>>>,

    /// Optional checkpoint saver for crash recovery
    pub checkpointer: Option<Arc<dyn crate::checkpoint::CheckpointSaver>>,

    /// Current step number
    pub step: usize,

    /// Loop status
    pub status: LoopStatus,

    /// Pending tasks for next superstep
    pub pending_tasks: Vec<PendingTask<S>>,

    /// Optional budget tracker (shared with `RunnableConfig` via Arc)
    budget_tracker: Option<Arc<BudgetTracker>>,

    /// Run control for graceful shutdown
    run_control: RunControl,

    /// Unique ID for this graph execution
    run_id: String,

    /// Interrupt signal receiver from the last superstep
    /// This is stored here so `after_tick` can drain it
    interrupt_rx: Option<mpsc::UnboundedReceiver<crate::interrupt::InterruptSignal>>,

    /// Interrupt signals captured during execution for checkpoint persistence
    pending_interrupts: Vec<crate::interrupt::InterruptSignal>,

    /// Scratchpad for tracking processed interrupts and transient data
    scratchpad: crate::interrupt::Scratchpad,

    /// Channel versions snapshot at the time of the last interrupt.
    /// Used by `should_interrupt` to prevent infinite interrupt loops when no
    /// state actually changed between interrupts.
    interrupt_versions_seen: HashMap<String, u64>,

    /// Superstep start time for duration tracking
    ///
    /// Set at the beginning of [`execute_superstep`], read in [`after_tick`].
    superstep_start: Option<Instant>,

    /// Maps node names to their registered error handler node names.
    ///
    /// Extracted from builder metadata during `PregelLoop` construction. When a
    /// task fails and its node has a handler in this map, the engine creates
    /// a recovery task targeting the handler instead of canceling all tasks.
    error_handler_map: HashMap<String, String>,

    /// Per-node retry policies extracted from builder metadata.
    ///
    /// When a node has an entry here, its execution in `execute_superstep` is
    /// wrapped with [`crate::graph::builder::execute_with_retry`] for automatic
    /// retries with exponential backoff and jitter.
    retry_policies: HashMap<String, crate::graph::RetryPolicy>,

    /// Per-node timeout policies extracted from builder metadata.
    ///
    /// When a node has an entry here, its execution in `execute_superstep` is
    /// wrapped with `tokio::time::timeout` using the configured `run_timeout`.
    /// The timeout wraps the entire execution (including retry attempts when a
    /// retry policy is also configured).
    timeout_policies: HashMap<String, crate::pregel::context::TimeoutPolicy>,

    /// Per-channel delta counters tracking updates and supersteps since last full snapshot.
    ///
    /// Keys are channel names (e.g. `"field_0"`), values track cumulative write counts.
    /// Populated from [`FieldsChanged`] after each superstep and reset when a full
    /// snapshot checkpoint is saved.
    delta_counters: HashMap<String, DeltaCounters>,
}

impl<S: State> std::fmt::Debug for PregelLoop<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PregelLoop")
            .field("state", &"<state>")
            .field("nodes", &self.nodes.len())
            .field("trigger_table", &self.trigger_table)
            .field("field_versions", &self.field_versions)
            .field("versions_seen", &self.versions_seen)
            .field("runnable_config", &self.runnable_config)
            .field("cancellation_token", &self.cancellation_token)
            .field("stream_tx", &self.stream_tx.is_some())
            .field("checkpointer", &self.checkpointer.is_some())
            .field("step", &self.step)
            .field("status", &self.status)
            .field("pending_tasks", &self.pending_tasks)
            .field("budget_tracker", &self.budget_tracker.is_some())
            .field("run_control", &self.run_control)
            .field("run_id", &self.run_id)
            .field("interrupt_rx", &self.interrupt_rx.is_some())
            .field("pending_interrupts", &self.pending_interrupts.len())
            .field("scratchpad", &self.scratchpad)
            .field("interrupt_versions_seen", &self.interrupt_versions_seen)
            .field("superstep_start", &self.superstep_start.is_some())
            .field("error_handler_map", &self.error_handler_map.len())
            .field(
                "retry_policies",
                &self.retry_policies.keys().collect::<Vec<_>>(),
            )
            .field(
                "timeout_policies",
                &self.timeout_policies.keys().collect::<Vec<_>>(),
            )
            .field("delta_counters", &self.delta_counters.len())
            .finish()
    }
}

impl<S: State> PregelLoop<S> {
    /// Create a new Pregel loop
    ///
    /// # Arguments
    ///
    /// * `state` - Initial state
    /// * `nodes` - Graph nodes
    /// * `trigger_table` - Trigger table for routing
    /// * `config` - Execution configuration
    /// * `num_fields` - Number of fields in the state
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The trigger table is invalid
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::PregelLoop;
    ///
    /// let loop = PregelLoop::new(
    ///     initial_state,
    ///     nodes,
    ///     trigger_table,
    ///     config,
    ///     5, // number of fields
    /// )?;
    /// ```
    pub fn new(
        state: S,
        nodes: IndexMap<String, Arc<dyn Node<S>>>,
        trigger_table: TriggerTable<S>,
        config: crate::config::RunnableConfig,
        num_fields: usize,
    ) -> Result<Self, JunctureError> {
        Self::with_error_handlers(
            state,
            nodes,
            trigger_table,
            config,
            num_fields,
            HashMap::new(),
        )
    }

    /// Create a new Pregel loop with error handler mappings
    ///
    /// Like [`new`](Self::new) but accepts a pre-built error handler map
    /// extracted from builder metadata. Nodes with entries in this map
    /// will have their failures routed to the named handler instead of
    /// canceling the entire superstep.
    ///
    /// # Arguments
    ///
    /// * `state` - Initial state
    /// * `nodes` - Graph nodes
    /// * `trigger_table` - Trigger table for routing
    /// * `config` - Execution configuration
    /// * `num_fields` - Number of fields in the state
    /// * `error_handler_map` - Maps node names to error handler node names
    ///
    /// # Errors
    ///
    /// Returns an error if the trigger table is invalid.
    pub fn with_error_handlers(
        state: S,
        nodes: IndexMap<String, Arc<dyn Node<S>>>,
        trigger_table: TriggerTable<S>,
        config: crate::config::RunnableConfig,
        num_fields: usize,
        error_handler_map: HashMap<String, String>,
    ) -> Result<Self, JunctureError> {
        let node_names: Vec<String> = nodes.keys().cloned().collect();
        let field_versions = FieldVersionTracker::new(num_fields);
        let versions_seen = VersionsSeen::new(&node_names, num_fields);
        let cancellation_token = CancellationToken::new();

        // Initialize pending tasks from entry point
        let pending_tasks = Self::compute_initial_tasks(&trigger_table);

        // Generate unique run ID for this execution
        let run_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            state,
            nodes,
            trigger_table,
            field_versions,
            versions_seen,
            runnable_config: config,
            cancellation_token,
            stream_tx: None,
            checkpointer: None,
            step: 0,
            status: LoopStatus::Running,
            pending_tasks,
            budget_tracker: None,
            run_control: RunControl::new(),
            run_id,
            interrupt_rx: None,
            pending_interrupts: Vec::new(),
            scratchpad: crate::interrupt::Scratchpad::new(),
            interrupt_versions_seen: HashMap::new(),
            superstep_start: None,
            error_handler_map,
            retry_policies: HashMap::new(),
            timeout_policies: HashMap::new(),
            delta_counters: HashMap::new(),
        })
    }

    /// Set the stream event sender
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use tokio::sync::mpsc;
    ///
    /// let (tx, _rx) = mpsc::unbounded_channel();
    /// loop.set_stream_sender(tx);
    /// ```
    pub fn set_stream_sender(&mut self, tx: mpsc::UnboundedSender<StreamEvent<S>>) {
        self.stream_tx = Some(tx);
    }

    /// Set the checkpoint saver for crash recovery during supersteps
    pub fn set_checkpointer(&mut self, saver: Arc<dyn crate::checkpoint::CheckpointSaver>) {
        self.checkpointer = Some(saver);
    }

    /// Set the budget tracker
    ///
    /// Wraps the tracker in an `Arc` so it can be shared between the
    /// `PregelLoop` (for budget checking) and the `RunnableConfig` (for
    /// node-level token reporting). Both share the same underlying
    /// counters via atomic operations.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::BudgetTracker;
    ///
    /// let budget = BudgetTracker::new(BudgetConfig::new());
    /// loop.set_budget_tracker(budget);
    /// ```
    pub fn set_budget_tracker(&mut self, tracker: BudgetTracker) {
        let shared = Arc::new(tracker);
        self.runnable_config.budget_tracker = Some(Arc::clone(&shared));
        self.budget_tracker = Some(shared);
    }

    /// Set per-node retry policies
    ///
    /// Each entry maps a node name to its [`RetryPolicy`]. During superstep
    /// execution, nodes with a configured policy are wrapped with
    /// [`crate::graph::builder::execute_with_retry`] for automatic retries
    /// with exponential backoff and jitter.
    pub fn set_retry_policies(&mut self, policies: HashMap<String, crate::graph::RetryPolicy>) {
        self.retry_policies = policies;
    }

    /// Set per-node timeout policies
    ///
    /// Each entry maps a node name to its [`TimeoutPolicy`](crate::pregel::context::TimeoutPolicy).
    /// During superstep execution, nodes with a configured policy are wrapped with
    /// `tokio::time::timeout` using the configured `run_timeout`. The timeout wraps
    /// the entire execution including any retry attempts.
    pub fn set_timeout_policies(
        &mut self,
        policies: HashMap<String, crate::pregel::context::TimeoutPolicy>,
    ) {
        self.timeout_policies = policies;
    }

    /// Compute initial tasks from entry point
    fn compute_initial_tasks(trigger_table: &TriggerTable<S>) -> Vec<PendingTask<S>> {
        // Find nodes that have incoming edges from START
        let mut initial_tasks = Vec::new();

        for (node_name, sources) in &trigger_table.incoming {
            for source in sources {
                if let crate::edge::TriggerSource::Edge { from } = source
                    && from == crate::edge::START
                {
                    initial_tasks.push(PendingTask::pull(
                        uuid::Uuid::new_v4().to_string(),
                        node_name.clone(),
                    ));
                    break;
                }
            }
        }

        initial_tasks
    }

    /// Execute one tick of the loop
    ///
    /// Returns `true` if execution should continue, `false` if done.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Recursion limit is reached
    /// - Cancellation is requested
    /// - Budget limits are exceeded
    ///
    /// # Examples
    ///
    /// ```ignore
    /// while loop.tick()? {
    ///     let result = loop.execute_superstep().await?;
    ///     loop.after_tick(result)?;
    /// }
    /// ```
    #[tracing::instrument(
        name = "juncture.graph.invoke",
        skip(self),
        fields(
            thread_id = ?std::thread::current().id(),
            step = self.step,
            recursion_limit = self.runnable_config.recursion_limit,
            graph_name = ?self.runnable_config.graph_name,
            run_id = %self.run_id,
        )
    )]
    pub fn tick(&mut self) -> Result<bool, JunctureError> {
        // Check recursion limit
        if self.step >= self.runnable_config.recursion_limit {
            self.status = LoopStatus::OutOfSteps;
            self.emit_counter("juncture.graph.errors", 1);
            let result: Result<(), JunctureError> = Err(JunctureError::recursion_limit(
                self.step,
                self.runnable_config.recursion_limit,
            ));
            self.on_graph_end(&result);
            // Extract the error from the Result for the return value.
            // This is safe because we just constructed it as Err.
            let Err(err) = result else {
                unreachable!("result was constructed as Err");
            };
            return Err(err);
        }

        // Check cancellation
        if self.cancellation_token.is_cancelled() {
            self.status = LoopStatus::Cancelled;
            self.on_graph_end(&Ok(()));
            return Ok(false);
        }

        // Check budget
        if let Some(tracker) = &self.budget_tracker
            && let Some(reason) = tracker.check()
        {
            self.status = LoopStatus::BudgetExceeded;
            self.emit_counter("juncture.graph.errors", 1);
            let result: Result<(), JunctureError> = Err(JunctureError::execution(format!(
                "Budget exceeded: {reason}"
            )));
            self.on_graph_end(&result);
            let Err(err) = result else {
                unreachable!("result was constructed as Err");
            };
            return Err(err);
        }

        // Emit budget gauges when a collector is configured
        if let Some(ref tracker) = self.budget_tracker {
            let usage = tracker.current_usage();
            if let Some(ref budget) = self.runnable_config.budget
                && let Some(max_tokens) = budget.max_tokens
            {
                self.emit_gauge(
                    "juncture.budget.remaining_tokens",
                    max_tokens.saturating_sub(usage.tokens_used),
                );
            }
        }

        // Compute next tasks if pending is empty
        if self.pending_tasks.is_empty() {
            // Check if drain is requested - if so, we're done
            if self.run_control.is_drain_requested() {
                self.status = LoopStatus::Done;
                self.on_graph_end(&Ok(()));
                return Ok(false);
            }

            // Try to compute tasks from trigger table
            // This is a no-op in the current implementation since
            // compute_next_tasks requires completed tasks
            self.status = LoopStatus::Done;
            self.on_graph_end(&Ok(()));
            return Ok(false);
        }

        // Check interrupt_before before executing next superstep
        if let Some(ref interrupt_before_nodes) = self.runnable_config.interrupt_before {
            let interrupt_before_set: HashSet<String> =
                interrupt_before_nodes.iter().cloned().collect();

            // Build channel versions map for should_interrupt
            let channel_versions: HashMap<String, u64> = self
                .field_versions
                .versions()
                .iter()
                .enumerate()
                .map(|(idx, ver)| (format!("field_{idx}"), *ver))
                .collect();

            if let Some(signals) = should_interrupt(
                &self.pending_tasks,
                &interrupt_before_set,
                &HashSet::new(), // interrupt_after not checked here
                &channel_versions,
                &self.interrupt_versions_seen,
            ) {
                self.interrupt_versions_seen = channel_versions;
                self.pending_interrupts.clone_from(&signals);
                self.status = LoopStatus::InterruptBefore(signals);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Execute one superstep
    ///
    /// Delegates to [`runner::execute_superstep`] with the current [`step`](Self::step)
    /// number for observability span attributes.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Task execution fails
    /// - Cancellation is requested
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let result = loop.execute_superstep().await?;
    /// ```
    pub async fn execute_superstep(&mut self) -> Result<SuperstepResult<S>, JunctureError>
    where
        S::Update: serde::Serialize,
    {
        let node_names: Vec<_> = self
            .pending_tasks
            .iter()
            .map(|t| t.node_name.as_str())
            .collect();
        let span = tracing::info_span!(
            "juncture.superstep",
            step = self.step,
            num_tasks = self.pending_tasks.len(),
            "juncture.step.nodes" = ?node_names,
            "juncture.step.duration_ms" = tracing::field::Empty,
        );
        let _enter = span.enter();

        // Store superstep start time for duration tracking in after_tick
        let start = Instant::now();
        self.superstep_start = Some(start);

        // Emit SuperstepStart debug event if streaming is configured
        if let Some(ref tx) = self.stream_tx {
            let _ = tx.send(StreamEvent::Debug(DebugEvent::SuperstepStart {
                step: self.step,
                nodes: node_names
                    .iter()
                    .copied()
                    .map(std::string::ToString::to_string)
                    .collect(),
            }));
        }

        // Emit graph invocation counter metric
        self.emit_counter("juncture.graph.invocations", 1);
        tracing::debug!(
            name: "juncture.graph.invocations",
            step = self.step,
            num_tasks = self.pending_tasks.len(),
        );

        let (result, interrupt_rx) = execute_superstep(
            &self.pending_tasks,
            &self.state,
            &self.nodes,
            &self.runnable_config,
            &self.cancellation_token,
            self.checkpointer.as_ref(),
            &self.pending_interrupts,
            &self.scratchpad,
            &self.error_handler_map,
            &self.retry_policies,
            &self.timeout_policies,
            self.step,
        )
        .await?;

        // Mark previously pending interrupts as processed in the scratchpad.
        // This is critical for multi-interrupt scenarios where a node has several
        // interrupt points and the user resumes with values for only a subset.
        // On subsequent re-execution, already-handled interrupt positions receive
        // a Null resume value via match_resume_to_interrupts -> scratchpad.get_null_resume(),
        // allowing the node to skip past those interrupt points without re-interrupting.
        for signal in &self.pending_interrupts {
            if let Some(ref id) = signal.id {
                self.scratchpad.mark_interrupt_processed(id);
            }
        }

        let duration = start.elapsed().as_millis();
        tracing::Span::current().record("juncture.step.duration_ms", duration);

        // Emit superstep duration histogram metric
        // duration is u128 from as_millis(), but realistic superstep durations
        // fit in u64 (millisecond precision, max ~584 million years).
        let duration_ms = u64::try_from(duration).unwrap_or(u64::MAX);
        #[allow(
            clippy::cast_precision_loss,
            reason = "millisecond durations fit well within f64 precision for histogram recording"
        )]
        let duration_f64 = duration_ms as f64;
        self.emit_histogram("juncture.superstep.duration_ms", duration_f64);

        // Emit superstep duration metric
        tracing::debug!(
            name: "juncture.superstep.duration_ms",
            step = self.step,
            duration_ms = duration,
        );

        // Store the interrupt receiver for after_tick to drain
        // We use Option to allow moving it into after_tick
        self.interrupt_rx = Some(interrupt_rx);

        Ok(result)
    }

    /// Process results after a superstep
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Task computation fails
    ///
    /// # Panics
    ///
    /// Panics if a task duration exceeds `u64::MAX` milliseconds (extremely unlikely)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// loop.after_tick(result).await?;
    /// ```
    #[expect(
        clippy::too_many_lines,
        reason = "after_tick orchestrates multiple sequential phases: apply writes, bump versions, consume channels, emit events including stream_data, compute tasks, drain interrupts, check interrupts, finish channels, increment step"
    )]
    #[allow(
        clippy::cognitive_complexity,
        reason = "after_tick orchestrates multiple sequential phases: apply writes, bump versions, consume channels, emit events including stream_data, compute tasks, drain interrupts, check interrupts, finish channels, increment step"
    )]
    pub async fn after_tick(&mut self, result: SuperstepResult<S>) -> Result<(), JunctureError>
    where
        S: Clone + serde::Serialize,
    {
        // Apply writes from completed tasks using path-based deterministic merge order.
        // apply_writes sorts by trigger type (PULL before PUSH) then by node name / send
        // index so that concurrent writes to the same field produce a deterministic result
        // matching LangGraph semantics. It also checks for replace-field conflicts before
        // applying any writes, so a double-write rejects the entire superstep.
        let total_changed = apply_writes(
            &mut self.state,
            &result.task_outputs,
            &mut self.field_versions,
        )?;

        // Increment delta counters for all tracked fields.
        // DeltaChannel fields get update+superstep increments; other changed
        // fields get update+superstep increments too, providing a consistent
        // view of write activity across all channels.
        self.update_delta_counters(&total_changed);

        // Consume all triggered channels after writes have been applied.
        // For EphemeralChannel fields, this marks the consumed flag so the
        // channel knows its value has been read by the framework. The value
        // itself is cleared by the subsequent reset_ephemeral() call.
        self.consume_triggered_channels(&total_changed);

        // Mark versions as consumed after bumping
        for task_output in &result.task_outputs {
            let current_versions = self.field_versions.versions().to_vec();
            self.versions_seen
                .mark_consumed(&task_output.node_name, &current_versions);
        }

        // Reset ephemeral fields
        self.state.reset_ephemeral();

        // Emit stream events
        if let Some(ref tx) = self.stream_tx {
            for task_output in &result.task_outputs {
                // Emit TaskStart event before TaskEnd (retroactive, but provides task_id info)
                let start_event = StreamEvent::TaskStart {
                    node: task_output.node_name.clone(),
                    task_id: task_output.task_id.clone(),
                    step: self.step,
                };
                let _ = tx.send(start_event);

                // Emit TaskEnd event
                let end_event = StreamEvent::TaskEnd {
                    node: task_output.node_name.clone(),
                    task_id: task_output.task_id.clone(),
                    step: self.step,
                    duration_ms: u64::try_from(task_output.duration.as_millis())
                        .expect("duration should fit in u64"),
                };
                let _ = tx.send(end_event);

                // Emit custom stream events from the command's stream_data.
                // Each entry in stream_data produces one StreamEvent::Custom
                // tagged with the emitting node name and empty namespace.
                for data in &task_output.command.stream_data {
                    let custom_event = StreamEvent::Custom {
                        node: task_output.node_name.clone(),
                        data: data.clone(),
                        ns: Vec::new(),
                    };
                    let _ = tx.send(custom_event);
                }

                // Emit Updates event if the task produced an update
                if let Some(ref update) = task_output.command.update {
                    let updates_event = StreamEvent::Updates {
                        node: task_output.node_name.clone(),
                        update: update.clone(),
                        step: self.step,
                    };
                    let _ = tx.send(updates_event);
                }
            }

            // Emit Values event after all updates applied
            let values_event = StreamEvent::Values {
                state: self.state.clone(),
                step: self.step,
            };
            let _ = tx.send(values_event);

            // Emit SuperstepEnd debug event with duration
            if let Some(superstep_start) = self.superstep_start {
                let duration_ms =
                    u64::try_from(superstep_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                let end_event = StreamEvent::Debug(DebugEvent::SuperstepEnd {
                    step: self.step,
                    duration_ms,
                });
                let _ = tx.send(end_event);
            }
        }

        // Compute next pending tasks
        self.pending_tasks =
            compute_next_tasks(&result.task_outputs, &self.trigger_table, &self.state).await?;

        // Schedule error handler recovery tasks for any failed nodes that have
        // a registered error handler. These tasks run the handler node which
        // receives the error context and returns a recovery Command.
        let recovery_tasks =
            schedule_error_handlers(&result.task_outputs, &self.nodes, &self.error_handler_map);
        if !recovery_tasks.is_empty() {
            tracing::debug!(
                name: "juncture.error_handler.recovery_tasks",
                step = self.step,
                count = recovery_tasks.len(),
                "Scheduling error handler recovery tasks"
            );
            self.pending_tasks.extend(recovery_tasks);
        }

        // Emit RouteDecision debug event after computing next tasks
        if let Some(ref tx) = self.stream_tx {
            let next_node_names: Vec<String> = self
                .pending_tasks
                .iter()
                .map(|t| t.node_name.clone())
                .collect();
            if !next_node_names.is_empty() {
                let route_event = StreamEvent::Debug(DebugEvent::RouteDecision {
                    from: "superstep".to_string(),
                    to: next_node_names,
                    step: self.step,
                });
                let _ = tx.send(route_event);
            }
        }

        // Save superstep checkpoint after state merge and next-task computation.
        // This provides crash recovery for normal superstep completion (B-04-002).
        // The checkpoint is only saved when no interrupts are pending; interrupt
        // paths save their own checkpoint with richer context (pending_interrupts).
        self.save_superstep_checkpoint().await;

        // Drain interrupt signals from the channel
        // These are signals sent by the interrupt!() macro during node execution
        let mut node_interrupts = Vec::new();
        if let Some(mut rx) = self.interrupt_rx.take() {
            // Collect all pending interrupt signals
            while let Ok(signal) = rx.try_recv() {
                node_interrupts.push(signal);
            }
        }

        // If we received any interrupt signals from nodes, handle them
        if !node_interrupts.is_empty() {
            self.pending_interrupts.clone_from(&node_interrupts);
            self.status = LoopStatus::InterruptAfter(node_interrupts.clone());

            // Emit interrupt events to stream (hidden nodes filtered)
            self.emit_interrupt_events(&node_interrupts);

            // Save checkpoint with Interrupt source for HITL recovery
            let node = self.interrupt_node_name().to_string();
            self.save_interrupt_checkpoint(&node).await;

            return Ok(());
        }

        // Handle BubbleUp events from subgraph execution.
        if result.has_bubble_ups() && self.handle_bubble_ups(&result.bubble_ups) {
            // Save checkpoint with Interrupt source if a BubbleUp interrupt was
            // the reason for stopping.
            if self.status.is_interrupted() {
                let node = self.interrupt_node_name().to_string();
                self.save_interrupt_checkpoint(&node).await;
            }
            return Ok(());
        }

        // Check interrupt_after after computing next tasks
        if let Some(ref interrupt_after_nodes) = self.runnable_config.interrupt_after {
            let interrupt_after_set: HashSet<String> =
                interrupt_after_nodes.iter().cloned().collect();

            // Build channel versions map for should_interrupt
            let channel_versions: HashMap<String, u64> = self
                .field_versions
                .versions()
                .iter()
                .enumerate()
                .map(|(idx, ver)| (format!("field_{idx}"), *ver))
                .collect();

            if let Some(signals) = should_interrupt(
                &self.pending_tasks,
                &HashSet::new(), // interrupt_before not checked here
                &interrupt_after_set,
                &channel_versions,
                &self.interrupt_versions_seen,
            ) {
                self.interrupt_versions_seen = channel_versions;
                self.pending_interrupts.clone_from(&signals);
                self.status = LoopStatus::InterruptAfter(signals.clone());

                // Emit interrupt events to stream (hidden nodes filtered)
                self.emit_interrupt_events(&signals);

                // Save checkpoint with Interrupt source for HITL recovery
                let node = self.interrupt_node_name().to_string();
                self.save_interrupt_checkpoint(&node).await;

                return Ok(());
            }
        }

        // Call finish() on all channels if no more tasks (execution complete)
        // This is critical for LastValueAfterFinishChannel which only makes
        // its value available after finish() is called.
        if self.pending_tasks.is_empty() {
            self.finish_all_channels();
            // In Exit durability mode, save a checkpoint on graph completion
            // so the final state is preserved in durable storage.
            if self.effective_durability() == Durability::Exit {
                self.save_exit_checkpoint().await;
            }
        }

        // Increment step
        self.step += 1;

        // Report step to budget tracker
        if let Some(ref tracker) = self.budget_tracker {
            tracker.report_step();
        }

        Ok(())
    }

    /// Process `BubbleUp` events from subgraph execution
    ///
    /// Handles interrupt propagation, drain propagation, and parent command
    /// routing from nested subgraph execution.
    ///
    /// Returns `true` if the parent loop should stop (interrupt or drain occurred),
    /// `false` if execution should continue.
    fn handle_bubble_ups(&mut self, bubble_ups: &[BubbleUp<S>]) -> bool {
        let mut should_stop = false;

        for bubble_up in bubble_ups {
            match bubble_up {
                BubbleUp::Interrupt(graph_interrupt) => {
                    self.handle_bubble_up_interrupt(graph_interrupt);
                    should_stop = true;
                }
                BubbleUp::Drained(drained) => {
                    self.handle_bubble_up_drained(drained);
                    should_stop = true;
                }
                BubbleUp::ParentCommand(cmd) => {
                    self.handle_bubble_up_parent_command(cmd);
                }
            }
        }

        should_stop
    }

    /// Handle a subgraph interrupt bubbling up to the parent graph
    fn handle_bubble_up_interrupt(
        &mut self,
        graph_interrupt: &crate::pregel::types::GraphInterrupt,
    ) {
        tracing::debug!(
            step = self.step,
            num_signals = graph_interrupt.interrupts.len(),
            interrupt_step = graph_interrupt.step,
            "Subgraph interrupt bubbling up to parent"
        );

        self.pending_interrupts
            .clone_from(&graph_interrupt.interrupts);
        self.status = LoopStatus::InterruptAfter(graph_interrupt.interrupts.clone());

        // Emit interrupt events to stream (hidden nodes filtered)
        self.emit_interrupt_events(&graph_interrupt.interrupts);
    }

    /// Handle a subgraph drain bubbling up to the parent graph
    fn handle_bubble_up_drained(&mut self, drained: &crate::pregel::types::GraphDrained) {
        tracing::debug!(
            step = self.step,
            reason = %drained.reason,
            "Subgraph drained bubbling up to parent"
        );

        self.status = LoopStatus::Drained;
    }

    /// Handle a subgraph parent command bubbling up to the parent graph
    fn handle_bubble_up_parent_command(&mut self, cmd: &crate::Command<S>) {
        tracing::debug!(
            step = self.step,
            goto = ?cmd.goto,
            "Subgraph parent command bubbling up"
        );

        if let Some(ref update) = cmd.update {
            let changed = self.state.try_apply(update.clone());
            match changed {
                Ok(changed) => self.field_versions.bump_all(&changed),
                Err(err) => {
                    tracing::warn!(
                        name: "juncture.subgraph.parent_command.apply_failed",
                        step = self.step,
                        error = %err,
                        "Failed to apply parent command from subgraph"
                    );
                }
            }
        }
    }

    /// Consume the loop and return the final state
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let final_state = loop.into_state();
    /// ```
    #[must_use]
    pub fn into_state(self) -> S {
        self.state
    }

    /// Get the current step number
    #[must_use]
    pub const fn step(&self) -> usize {
        self.step
    }

    /// Get the unique run ID for this execution
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Get the current status
    #[must_use]
    pub const fn status(&self) -> &LoopStatus {
        &self.status
    }

    /// Get the pending interrupt signals for checkpoint persistence
    #[must_use]
    pub fn pending_interrupts(&self) -> &[crate::interrupt::InterruptSignal] {
        &self.pending_interrupts
    }

    /// Get a reference to the scratchpad for interrupt tracking
    #[must_use]
    pub const fn scratchpad(&self) -> &crate::interrupt::Scratchpad {
        &self.scratchpad
    }

    /// Get a mutable reference to the scratchpad for interrupt tracking
    pub const fn scratchpad_mut(&mut self) -> &mut crate::interrupt::Scratchpad {
        &mut self.scratchpad
    }

    /// Check if the loop is still running
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self.status, LoopStatus::Running)
    }

    /// Get a clone of the current state without consuming the loop
    ///
    /// Useful for streaming execution where state snapshots are needed
    /// after each superstep without terminating the loop.
    #[must_use]
    pub fn snapshot_state(&self) -> S
    where
        S: Clone,
    {
        self.state.clone()
    }

    /// Get the run control for graceful shutdown
    ///
    /// Returns a clone of the run control that can be used to request
    /// drain from another thread or context.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::PregelLoop;
    ///
    /// let mut loop = PregelLoop::new(...)?;
    /// let run_control = loop.run_control();
    ///
    /// // From another thread
    /// std::thread::spawn(move || {
    ///     run_control.request_drain();
    /// });
    /// ```
    #[must_use]
    pub const fn run_control(&self) -> &RunControl {
        &self.run_control
    }

    /// Get a view of the current execution context
    ///
    /// Returns an `ExecutionContext` value that provides typed access
    /// to the mutable execution state (state, `field_versions`, `versions_seen`).
    /// This provides the design-intended separation between mutable context
    /// and immutable configuration.
    ///
    /// Note: Returns a cloned context, not a reference, since `ExecutionContext`
    /// is designed to own its data.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::PregelLoop;
    ///
    /// let loop = PregelLoop::new(...)?;
    /// let context = loop.as_context();
    /// let versions = context.field_versions.versions();
    /// ```
    #[must_use]
    #[allow(
        clippy::clone_on_copy,
        reason = "ExecutionContext requires owned state, not reference"
    )]
    pub fn as_context(&self) -> ExecutionContext<S>
    where
        S: Clone,
    {
        ExecutionContext {
            state: self.state.clone(),
            field_versions: self.field_versions.clone(),
            versions_seen: self.versions_seen.clone(),
            pending_writes: vec![],
        }
    }

    /// Get a view of the current execution config
    ///
    /// Returns an `ExecutionConfig` value that provides typed access
    /// to the immutable execution configuration (`recursion_limit`, interrupts, etc.).
    /// This provides the design-intended separation between mutable context
    /// and immutable configuration.
    ///
    /// Note: Returns a cloned config, not a reference, since `ExecutionConfig`
    /// is designed to own its data.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::PregelLoop;
    ///
    /// let loop = PregelLoop::new(...)?;
    /// let config = loop.as_config();
    /// let limit = config.recursion_limit;
    /// ```
    #[must_use]
    pub fn as_config(&self) -> crate::pregel::context::ExecutionConfig {
        crate::pregel::context::ExecutionConfig {
            recursion_limit: self.runnable_config.recursion_limit,
            interrupt_before: self
                .runnable_config
                .interrupt_before
                .as_ref()
                .map_or_else(HashSet::new, |v| v.iter().cloned().collect()),
            interrupt_after: self
                .runnable_config
                .interrupt_after
                .as_ref()
                .map_or_else(HashSet::new, |v| v.iter().cloned().collect()),
            budget: self.runnable_config.budget.clone(),
            durability: self.runnable_config.durability.clone().unwrap_or_default(),
            retry_policies: std::collections::HashMap::new(),
            timeout_policies: std::collections::HashMap::new(),
        }
    }

    /// Save a checkpoint with [`CheckpointSource::Interrupt`] when a checkpointer is configured.
    ///
    /// Builds a full checkpoint from the current loop state, sets the source to
    /// `Interrupt { node }`, and persists it via the checkpointer. Errors are
    /// logged but do not propagate -- interrupt checkpointing is best-effort and
    /// should not prevent the interrupt from being surfaced to the caller.
    ///
    /// # Type Parameters
    ///
    /// Requires `S: serde::Serialize` to serialize the current state into
    /// `channel_values` for the checkpoint.
    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "durability match arms and checkpoint construction logic are necessarily complex for handling Sync/Async/Exit modes"
    )]
    async fn save_interrupt_checkpoint(&mut self, node: &str)
    where
        S: serde::Serialize,
    {
        let Some(ref checkpointer) = self.checkpointer else {
            return;
        };

        let channel_values = match serde_json::to_value(&self.state) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    name: "juncture.checkpoint.interrupt.serialize_failed",
                    node = node,
                    error = %err,
                    "Failed to serialize state for interrupt checkpoint"
                );
                return;
            }
        };

        let (channel_versions, new_versions, versions_seen) = self.build_checkpoint_versions();

        let checkpoint_id = generate_checkpoint_id();
        let created_at = chrono::Utc::now().to_rfc3339();

        let checkpoint = Checkpoint {
            id: checkpoint_id,
            channel_values,
            channel_versions,
            versions_seen,
            pending_tasks: Vec::new(),
            pending_sends: Vec::new(),
            pending_interrupts: self.pending_interrupts.clone(),
            schema_version: S::schema_version(),
            created_at,
            v: 1,
            new_versions,
            counters_since_delta_snapshot: self.build_checkpoint_delta_counters(),
        };

        let metadata = CheckpointMetadata {
            source: CheckpointSource::Interrupt {
                node: node.to_string(),
            },
            step: i64::try_from(self.step).unwrap_or(i64::MAX),
            writes: HashMap::new(),
            parents: HashMap::new(),
            run_id: self.run_id.clone(),
        };

        let cp_config = self.runnable_config.clone();
        match self.effective_durability() {
            Durability::Async => {
                let step = self.step;
                let node_label = node.to_string();
                let checkpointer_arc = Arc::clone(checkpointer);
                tokio::spawn(async move {
                    match checkpointer_arc.put(&cp_config, checkpoint, metadata).await {
                        Ok(_updated_config) => {
                            tracing::info!(
                                name: "juncture.checkpoint.put",
                                checkpoint_step = step,
                                checkpoint_source = "Interrupt",
                                "Interrupt checkpoint persisted (async)"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                name: "juncture.checkpoint.interrupt.save_failed",
                                node = node_label,
                                error = %err,
                                "Failed to save interrupt checkpoint (async)"
                            );
                        }
                    }
                });
                self.reset_delta_counters();
            }
            Durability::Sync | Durability::Exit => {
                match checkpointer
                    .put(&self.runnable_config, checkpoint, metadata)
                    .await
                {
                    Ok(updated_config) => {
                        self.runnable_config.checkpoint_id = updated_config.checkpoint_id;
                        self.reset_delta_counters();
                        tracing::info!(
                            name: "juncture.checkpoint.put",
                            checkpoint_id = %self.runnable_config.checkpoint_id.as_deref().unwrap_or("unknown"),
                            checkpoint_step = self.step,
                            checkpoint_source = "Interrupt",
                            "Interrupt checkpoint persisted"
                        );
                        if let Some(ref cp_id) = self.runnable_config.checkpoint_id {
                            self.on_checkpoint_saved(cp_id, self.step);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            name: "juncture.checkpoint.interrupt.save_failed",
                            node = node,
                            error = %err,
                            "Failed to save interrupt checkpoint"
                        );
                    }
                }
            }
        }
    }

    /// Save a checkpoint with [`CheckpointSource::Loop`] after normal superstep completion.
    ///
    /// This is the second phase of two-phase persistence (B-04-002):
    /// - Phase 1: `put_writes()` after each task completes (already in runner)
    /// - Phase 2: `put()` after each superstep completes (this method)
    ///
    /// Called from [`after_tick`](Self::after_tick) after state merge and
    /// next-task computation, but before interrupt drain checks. This ensures
    /// crash recovery can resume from the last completed superstep rather than
    /// replaying from the initial state or last interrupt.
    ///
    /// No-op if no checkpointer is configured. Errors are logged but do not
    /// propagate -- superstep checkpointing is best-effort and must not prevent
    /// the graph from continuing execution.
    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "durability match arms and checkpoint construction logic are necessarily complex for handling Sync/Async/Exit modes"
    )]
    async fn save_superstep_checkpoint(&mut self)
    where
        S: serde::Serialize,
    {
        let Some(ref checkpointer) = self.checkpointer else {
            return;
        };

        // In Exit mode, skip normal superstep checkpoints. Only save on
        // graph completion or interrupt, where checkpoints are treated as
        // the final durable snapshot.
        if self.effective_durability() == Durability::Exit {
            return;
        }

        let channel_values = match serde_json::to_value(&self.state) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    name: "juncture.checkpoint.superstep.serialize_failed",
                    step = self.step,
                    error = %err,
                    "Failed to serialize state for superstep checkpoint"
                );
                return;
            }
        };

        let (channel_versions, new_versions, versions_seen) = self.build_checkpoint_versions();

        // Serialize pending tasks for crash recovery so the engine knows
        // which nodes to execute next after resuming from this checkpoint.
        let pending_tasks: Vec<crate::checkpoint::CheckpointPendingTask> = self
            .pending_tasks
            .iter()
            .map(|task| crate::checkpoint::CheckpointPendingTask {
                id: task.id.clone(),
                node: task.node_name.clone(),
                triggers: Vec::new(),
                state_override: None,
            })
            .collect();

        let checkpoint_id = generate_checkpoint_id();
        let created_at = chrono::Utc::now().to_rfc3339();

        let checkpoint = Checkpoint {
            id: checkpoint_id,
            channel_values,
            channel_versions,
            versions_seen,
            pending_tasks,
            pending_sends: Vec::new(),
            pending_interrupts: Vec::new(),
            schema_version: S::schema_version(),
            created_at,
            v: 1,
            new_versions,
            counters_since_delta_snapshot: self.build_checkpoint_delta_counters(),
        };

        let metadata = CheckpointMetadata {
            source: CheckpointSource::Loop,
            step: i64::try_from(self.step).unwrap_or(i64::MAX),
            writes: HashMap::new(),
            parents: HashMap::new(),
            run_id: self.run_id.clone(),
        };

        let cp_config = self.runnable_config.clone();
        match self.effective_durability() {
            Durability::Async => {
                let step = self.step;
                let checkpointer_arc = Arc::clone(checkpointer);
                tokio::spawn(async move {
                    match checkpointer_arc.put(&cp_config, checkpoint, metadata).await {
                        Ok(_updated_config) => {
                            tracing::info!(
                                name: "juncture.checkpoint.put",
                                checkpoint_step = step,
                                checkpoint_source = "Loop",
                                "Superstep checkpoint persisted (async)"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                name: "juncture.checkpoint.superstep.save_failed",
                                step = step,
                                error = %err,
                                "Failed to save superstep checkpoint (async)"
                            );
                        }
                    }
                });
                self.reset_delta_counters();
            }
            Durability::Sync | Durability::Exit => {
                match checkpointer
                    .put(&self.runnable_config, checkpoint, metadata)
                    .await
                {
                    Ok(updated_config) => {
                        self.runnable_config.checkpoint_id = updated_config.checkpoint_id;
                        // Reset delta counters after a successful checkpoint save.
                        // The checkpoint now carries the cumulative counters, and a
                        // fresh counting window starts for the next checkpoint cycle.
                        self.reset_delta_counters();
                        tracing::info!(
                            name: "juncture.checkpoint.put",
                            checkpoint_id = %self.runnable_config.checkpoint_id.as_deref().unwrap_or("unknown"),
                            checkpoint_step = self.step,
                            checkpoint_source = "Loop",
                            "Superstep checkpoint persisted"
                        );
                        if let Some(ref cp_id) = self.runnable_config.checkpoint_id {
                            self.on_checkpoint_saved(cp_id, self.step);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            name: "juncture.checkpoint.superstep.save_failed",
                            step = self.step,
                            error = %err,
                            "Failed to save superstep checkpoint"
                        );
                    }
                }
            }
        }
    }

    /// Save a pending interrupt checkpoint for `interrupt_before` scenarios.
    ///
    /// When `tick()` detects an `interrupt_before`, the loop exits immediately
    /// (tick is synchronous and cannot call the async checkpointer). The caller
    /// should invoke this method after the loop exits when the status is
    /// [`LoopStatus::InterruptBefore`].
    ///
    /// This is a no-op if no checkpointer is configured or if the status is not
    /// interrupted.
    ///
    /// # Type Parameters
    ///
    /// Requires `S: serde::Serialize` to serialize the current state.
    ///
    /// # Errors
    ///
    /// Does not return errors -- checkpoint save failures are logged and the
    /// interrupt is still surfaced to the caller.
    pub async fn save_pending_interrupt_checkpoint(&mut self)
    where
        S: serde::Serialize,
    {
        if !self.status.is_interrupted() || self.checkpointer.is_none() {
            return;
        }
        let node = self.interrupt_node_name().to_string();
        self.save_interrupt_checkpoint(&node).await;
    }

    /// Extract the primary interrupt node name from pending interrupts or loop status.
    ///
    /// Used for checkpoint source identification. Returns the first interrupt's
    /// associated node name, or "unknown" if not available.
    fn interrupt_node_name(&self) -> &str {
        static UNKNOWN: &str = "unknown";
        self.pending_interrupts
            .first()
            .and_then(|s| s.payload.get("node"))
            .and_then(|v| v.as_str())
            .unwrap_or(UNKNOWN)
    }

    /// Convert the current checkpoint namespace into a `Vec<String>` suitable
    /// for the `ns` field of [`StreamEvent::Interrupt`].
    ///
    /// Each [`NamespaceSegment`] contributes only its `node_name`; the
    /// invocation UUID is omitted because stream consumers only need the
    /// logical nesting path (e.g. `["review", "detail"]`).
    fn current_ns(&self) -> Vec<String> {
        self.runnable_config
            .checkpoint_ns
            .as_ref()
            .map(|ns| {
                ns.segments
                    .iter()
                    .map(|seg| seg.node_name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Emit interrupt stream events for the given signals, filtering out
    /// hidden/internal nodes (names starting and ending with `__`).
    ///
    /// Hidden nodes represent internal infrastructure (routing, error handling)
    /// that should never surface to external stream consumers.
    fn emit_interrupt_events(&self, signals: &[crate::interrupt::InterruptSignal]) {
        let Some(ref tx) = self.stream_tx else {
            return;
        };

        let ns = self.current_ns();
        for signal in signals {
            let node = signal
                .payload
                .get("node")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            // Skip hidden/internal nodes from stream emission
            if crate::interrupt::is_hidden_node(node) {
                continue;
            }

            let event = StreamEvent::Interrupt {
                node: node.to_string(),
                payload: signal.payload.clone(),
                resumable: true,
                ns: ns.clone(),
            };
            let _ = tx.send(event);
        }
    }

    /// Finish all channels in the state
    ///
    /// Called when graph execution completes (no more pending tasks).
    /// This allows channels like `LastValueAfterFinishChannel` to finalize
    /// their state and make values available to consumers.
    ///
    /// Only calls `finish_field()` for fields that use the
    /// `replace_after_finish` reducer, as indicated by
    /// [`State::replace_after_finish_field_indices`]. Other field types
    /// have no-op finish semantics.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::loop_::PregelLoop;
    ///
    /// let mut loop = PregelLoop::new(...)?;
    /// // ... execution ...
    /// if loop.pending_tasks.is_empty() {
    ///     loop.finish_all_channels();
    /// }
    /// ```
    fn finish_all_channels(&mut self) {
        for &field_idx in S::replace_after_finish_field_indices() {
            self.state.finish_field(field_idx);
        }
    }

    /// Consume all channels that were triggered (changed) in the current superstep.
    ///
    /// Called after `apply_writes()` in `after_tick()` to mark triggered channels
    /// as consumed. For ephemeral fields backed by `EphemeralChannel`, this sets
    /// the consumed flag. Other channel types (`UntrackedChannel`,
    /// `LastValueAfterFinishChannel`, `DeltaChannel`) have no-op consume semantics.
    ///
    /// Only calls `consume_field()` for fields that actually changed, as indicated
    /// by the `FieldsChanged` bitmask. This matches the design spec where all
    /// triggered channels call `consume()` after `apply_writes()`.
    fn consume_triggered_channels(&mut self, changed: &crate::FieldsChanged) {
        for &field_idx in S::consume_field_indices() {
            if changed.has_field(field_idx) {
                self.state.consume_field(field_idx);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Durability mode helpers (B-03-003)
    // -----------------------------------------------------------------------

    /// Return the effective durability mode, defaulting to `Sync` when not configured.
    #[must_use]
    fn effective_durability(&self) -> Durability {
        self.runnable_config
            .durability
            .clone()
            .unwrap_or(Durability::Sync)
    }

    /// Build the channel versions, new versions, and versions seen maps from
    /// the current execution state.
    ///
    /// Returns a tuple of `(channel_versions, new_versions, versions_seen)` for
    /// use in checkpoint construction. This refactors duplicate version-building
    /// code that appears in both `save_interrupt_checkpoint` and
    /// `save_superstep_checkpoint`.
    #[must_use]
    #[allow(
        clippy::type_complexity,
        reason = "return type is a direct mapping of the three version maps required by Checkpoint struct; factoring into a named type adds indirection without benefit"
    )]
    fn build_checkpoint_versions(
        &self,
    ) -> (
        HashMap<String, u64>,
        HashMap<String, u64>,
        HashMap<String, HashMap<String, u64>>,
    ) {
        let channel_versions: HashMap<String, u64> = self
            .field_versions
            .versions()
            .iter()
            .enumerate()
            .map(|(idx, ver)| (format!("field_{idx}"), *ver))
            .collect();

        let new_versions = channel_versions.clone();

        let versions_seen: HashMap<String, HashMap<String, u64>> = self
            .nodes
            .keys()
            .map(|node_name| {
                let versions = self.versions_seen.get_versions(node_name);
                let map: HashMap<String, u64> = versions
                    .iter()
                    .enumerate()
                    .map(|(idx, ver)| (format!("field_{idx}"), *ver))
                    .collect();
                (node_name.clone(), map)
            })
            .collect();

        (channel_versions, new_versions, versions_seen)
    }

    /// Save a final exit checkpoint when running in [`Durability::Exit`] mode.
    ///
    /// This checkpoint captures the final state after all channels are finished
    /// and no more tasks remain. It uses [`CheckpointSource::Loop`] since it
    /// represents a normal completion checkpoint, not an interrupt.
    ///
    /// No-op if no checkpointer is configured. Errors are logged but do not
    /// propagate -- exit checkpointing is best-effort.
    async fn save_exit_checkpoint(&mut self)
    where
        S: serde::Serialize,
    {
        let Some(ref checkpointer) = self.checkpointer else {
            return;
        };

        let channel_values = match serde_json::to_value(&self.state) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    name: "juncture.checkpoint.exit.serialize_failed",
                    step = self.step,
                    error = %err,
                    "Failed to serialize state for exit checkpoint"
                );
                return;
            }
        };

        let (channel_versions, new_versions, versions_seen) = self.build_checkpoint_versions();

        let pending_tasks: Vec<crate::checkpoint::CheckpointPendingTask> = self
            .pending_tasks
            .iter()
            .map(|task| crate::checkpoint::CheckpointPendingTask {
                id: task.id.clone(),
                node: task.node_name.clone(),
                triggers: Vec::new(),
                state_override: None,
            })
            .collect();

        let checkpoint_id = generate_checkpoint_id();
        let created_at = chrono::Utc::now().to_rfc3339();

        let checkpoint = Checkpoint {
            id: checkpoint_id,
            channel_values,
            channel_versions,
            versions_seen,
            pending_tasks,
            pending_sends: Vec::new(),
            pending_interrupts: Vec::new(),
            schema_version: S::schema_version(),
            created_at,
            v: 1,
            new_versions,
            counters_since_delta_snapshot: HashMap::new(),
        };

        let metadata = CheckpointMetadata {
            source: CheckpointSource::Loop,
            step: i64::try_from(self.step).unwrap_or(i64::MAX),
            writes: HashMap::new(),
            parents: HashMap::new(),
            run_id: self.run_id.clone(),
        };

        match checkpointer
            .put(&self.runnable_config, checkpoint, metadata)
            .await
        {
            Ok(updated_config) => {
                self.runnable_config.checkpoint_id = updated_config.checkpoint_id;
                tracing::info!(
                    name: "juncture.checkpoint.put",
                    checkpoint_id = %self.runnable_config.checkpoint_id.as_deref().unwrap_or("unknown"),
                    checkpoint_step = self.step,
                    checkpoint_source = "Loop",
                    "Exit checkpoint persisted"
                );
                if let Some(ref cp_id) = self.runnable_config.checkpoint_id {
                    self.on_checkpoint_saved(cp_id, self.step);
                }
            }
            Err(err) => {
                tracing::warn!(
                    name: "juncture.checkpoint.exit.save_failed",
                    step = self.step,
                    error = %err,
                    "Failed to save exit checkpoint"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Delta counter tracking (B-04-001)
    // -----------------------------------------------------------------------

    /// Increment delta counters after a superstep applies writes.
    ///
    /// For every field that changed, increment its `updates` counter. For all
    /// fields tracked in `delta_counters` (whether or not they changed), increment
    /// the `supersteps` counter. This provides a consistent view of write activity
    /// that the checkpoint builder consults to decide full-snapshot vs delta.
    fn update_delta_counters(&mut self, changed: &crate::FieldsChanged) {
        let field_names = S::field_names();
        let num_fields = field_names.len().min(self.field_versions.len());

        for field_idx in 0..num_fields {
            let channel_name = format!("field_{field_idx}");
            let entry = self.delta_counters.entry(channel_name).or_default();

            // Always increment supersteps for tracked channels
            entry.supersteps = entry.supersteps.saturating_add(1);

            // Only increment updates for channels that actually changed
            if changed.has_field(field_idx) {
                entry.updates = entry.updates.saturating_add(1);
            }
        }
    }

    /// Build the `counters_since_delta_snapshot` map for checkpoint persistence.
    ///
    /// Returns a clone of the current delta counters so the checkpoint carries an
    /// accurate snapshot of write activity since the last full snapshot.
    #[allow(
        dead_code,
        reason = "used by checkpoint delta snapshot logic (B-04-003)"
    )]
    fn build_checkpoint_delta_counters(&self) -> HashMap<String, DeltaCounters> {
        self.delta_counters.clone()
    }

    /// Decide whether a full snapshot checkpoint should be taken.
    ///
    /// Checks each `DeltaChannel` field against its configured `snapshot_frequency`.
    /// If any field's update count exceeds its frequency, returns `true` to
    /// indicate that a full snapshot is needed. Non-DeltaChannel fields are
    /// excluded from this decision since they always snapshot fully.
    #[allow(
        dead_code,
        reason = "used by checkpoint delta snapshot logic (B-04-003)"
    )]
    fn should_take_full_snapshot(&self) -> bool {
        let specs = S::delta_channel_specs();
        if specs.is_empty() {
            // No DeltaChannel fields configured -- always take full snapshots
            // since there is no delta optimization to apply.
            return true;
        }

        for &(field_idx, frequency) in specs {
            let channel_name = format!("field_{field_idx}");
            if let Some(counters) = self.delta_counters.get(&channel_name)
                && counters.exceeds_frequency(frequency)
            {
                return true;
            }
        }

        false
    }

    /// Reset delta counters after a full snapshot checkpoint has been saved.
    #[allow(
        dead_code,
        reason = "used by checkpoint delta snapshot logic (B-04-003)"
    )]
    fn reset_delta_counters(&mut self) {
        self.delta_counters.clear();
    }

    // -----------------------------------------------------------------------
    // Metric emission helpers
    // -----------------------------------------------------------------------

    /// Increment a counter metric if a collector is configured.
    #[inline]
    fn emit_counter(&self, name: &str, value: u64) {
        if let Some(ref collector) = self.runnable_config.metrics_collector {
            collector.inc_counter(name, value);
        }
    }

    /// Record a histogram value if a collector is configured.
    #[inline]
    fn emit_histogram(&self, name: &str, value: f64) {
        if let Some(ref collector) = self.runnable_config.metrics_collector {
            collector.record_histogram(name, value);
        }
    }

    /// Set a gauge value if a collector is configured.
    #[inline]
    fn emit_gauge(&self, name: &str, value: u64) {
        if let Some(ref collector) = self.runnable_config.metrics_collector {
            collector.set_gauge(name, value);
        }
    }

    // -----------------------------------------------------------------------
    // Lifecycle callback helpers
    // -----------------------------------------------------------------------

    /// Invoke graph-end callback if a handler is configured and emit
    /// the `juncture.graph.complete` tracing event with execution metrics.
    #[inline]
    fn on_graph_end(&self, result: &Result<(), JunctureError>) {
        // Extract budget metrics for the completion event.
        let (total_tokens, cost_usd) = self.budget_tracker.as_ref().map_or((0, 0.0), |tracker| {
            let usage = tracker.current_usage();
            (usage.tokens_used, usage.cost_usd)
        });

        let success = result.is_ok();
        tracing::info!(
            name: "juncture.graph.complete",
            total_steps = self.step,
            total_tokens = total_tokens,
            cost_usd = cost_usd,
            success = success,
            "Graph execution completed",
        );

        if let Some(ref handler) = self.runnable_config.callback_handler {
            handler.on_graph_end(result);
        }
    }

    /// Invoke checkpoint-saved callback if a handler is configured.
    #[inline]
    fn on_checkpoint_saved(&self, checkpoint_id: &str, step: usize) {
        if let Some(ref handler) = self.runnable_config.callback_handler {
            handler.on_checkpoint_saved(checkpoint_id, step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Command,
        node::IntoNode,
        node::NodeFnCommand,
        pregel::types::{TaskOutput, TaskTrigger},
    };
    use crate::state::FieldVersions;

    #[test]
    fn test_pregel_loop_creation() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let result = PregelLoop::new(state, nodes, trigger_table, config, 0);
        result.unwrap();
    }

    #[test]
    fn test_field_version_tracker() {
        let mut tracker = FieldVersionTracker::new(5);

        assert_eq!(tracker.get(0), 0);
        assert_eq!(tracker.global_max(), 0);

        tracker.bump(0);
        assert_eq!(tracker.get(0), 1);
        assert_eq!(tracker.global_max(), 1);

        tracker.bump(2);
        assert_eq!(tracker.get(2), 2);
        assert_eq!(tracker.global_max(), 2);
    }

    #[test]
    fn test_versions_seen() {
        let node_names = vec!["node_a".to_string(), "node_b".to_string()];
        let mut seen = VersionsSeen::new(&node_names, 3);

        assert!(!seen.should_activate("node_a", &[0], &[0, 0, 0]));

        let current = vec![1, 0, 0];
        assert!(seen.should_activate("node_a", &[0], &current));

        seen.mark_consumed("node_a", &current);
        assert!(!seen.should_activate("node_a", &[0], &current));
    }

    #[test]
    fn test_run_control() {
        let rc = RunControl::new();
        assert!(!rc.is_drain_requested());

        rc.request_drain();
        assert!(rc.is_drain_requested());
    }

    #[test]
    fn test_run_control_default() {
        let rc = RunControl::default();
        assert!(!rc.is_drain_requested());
    }

    #[test]
    fn test_handle_bubble_up_interrupt_sets_status() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let signals = vec![crate::interrupt::InterruptSignal {
            index: 0,
            id: Some("sub-int-0".to_string()),
            payload: serde_json::json!({"node": "subgraph_node"}),
        }];
        let bubble_ups = vec![BubbleUp::Interrupt(crate::pregel::types::GraphInterrupt {
            interrupts: signals,
            step: 2,
        })];

        let should_stop = loop_.handle_bubble_ups(&bubble_ups);

        assert!(should_stop);
        assert!(loop_.status.is_interrupted());
        assert_eq!(loop_.pending_interrupts.len(), 1);
        assert_eq!(loop_.pending_interrupts[0].id.as_deref(), Some("sub-int-0"));
    }

    #[test]
    fn test_handle_bubble_up_drained_sets_status() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let bubble_ups = vec![BubbleUp::Drained(crate::pregel::types::GraphDrained {
            reason: "subgraph completed".to_string(),
        })];

        let should_stop = loop_.handle_bubble_ups(&bubble_ups);

        assert!(should_stop);
        assert!(loop_.status.is_terminal());
        assert!(matches!(loop_.status, LoopStatus::Drained));
    }

    #[test]
    fn test_handle_bubble_up_parent_command_does_not_stop() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let bubble_ups = vec![BubbleUp::ParentCommand(Command::end())];

        let should_stop = loop_.handle_bubble_ups(&bubble_ups);

        assert!(!should_stop);
        assert!(loop_.status.is_running());
    }

    #[test]
    fn test_handle_bubble_up_empty_does_nothing() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let should_stop = loop_.handle_bubble_ups(&[]);

        assert!(!should_stop);
        assert!(loop_.status.is_running());
    }

    #[test]
    fn test_handle_bubble_up_interrupt_takes_priority_over_drain() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let bubble_ups = vec![
            BubbleUp::Drained(crate::pregel::types::GraphDrained {
                reason: "drained".to_string(),
            }),
            BubbleUp::Interrupt(crate::pregel::types::GraphInterrupt {
                interrupts: vec![crate::interrupt::InterruptSignal {
                    index: 0,
                    id: None,
                    payload: serde_json::Value::Null,
                }],
                step: 1,
            }),
        ];

        let should_stop = loop_.handle_bubble_ups(&bubble_ups);

        assert!(should_stop);
        // Interrupt is processed last, so status reflects the interrupt
        assert!(loop_.status.is_interrupted());
    }

    #[derive(Clone, Debug, serde::Serialize)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        type FieldVersions = FieldVersions;

        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct TestUpdate;

    // --- B-04-001: delta counter tests ---

    /// Test state with two fields to exercise delta counter tracking.
    #[derive(Clone, Debug, serde::Serialize)]
    struct DeltaTestState {
        value: i32,
        messages: Vec<String>,
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct DeltaTestUpdate {
        value: Option<i32>,
        messages: Option<Vec<String>>,
    }

    impl State for DeltaTestState {
        type Update = DeltaTestUpdate;
        type FieldVersions = FieldVersions;

        fn apply(&mut self, update: Self::Update) -> crate::FieldsChanged {
            let mut changed = crate::FieldsChanged(0);
            if let Some(v) = update.value {
                self.value = v;
                changed.set_field(0);
            }
            if let Some(msgs) = update.messages {
                self.messages.extend(msgs);
                changed.set_field(1);
            }
            changed
        }

        fn reset_ephemeral(&mut self) {}

        fn field_names() -> &'static [&'static str] {
            &["value", "messages"]
        }

        fn field_count() -> usize {
            2
        }

        /// Field 1 (messages) is a `DeltaChannel` with `snapshot_frequency` = 3
        fn delta_channel_specs() -> &'static [(usize, usize)] {
            &[(1, 3)]
        }
    }

    /// Checkpointer that captures the last saved checkpoint for inspection.
    struct CapturingCheckpointer {
        captured: Arc<std::sync::Mutex<Option<crate::checkpoint::Checkpoint>>>,
    }

    #[async_trait::async_trait]
    impl crate::checkpoint::CheckpointSaver for CapturingCheckpointer {
        async fn get_tuple(
            &self,
            _: &crate::config::RunnableConfig,
        ) -> Result<Option<crate::checkpoint::CheckpointTuple>, crate::checkpoint::CheckpointError>
        {
            Ok(None)
        }

        async fn list(
            &self,
            _: &crate::config::RunnableConfig,
            _: Option<crate::checkpoint::CheckpointFilter>,
        ) -> Result<Vec<crate::checkpoint::CheckpointTuple>, crate::checkpoint::CheckpointError>
        {
            Ok(Vec::new())
        }

        async fn put(
            &self,
            _: &crate::config::RunnableConfig,
            checkpoint: crate::checkpoint::Checkpoint,
            _metadata: crate::checkpoint::CheckpointMetadata,
        ) -> Result<crate::config::RunnableConfig, crate::checkpoint::CheckpointError> {
            *self
                .captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(checkpoint);
            let mut cfg = crate::config::RunnableConfig::new();
            cfg.checkpoint_id = Some("cp-capture".to_string());
            Ok(cfg)
        }

        async fn put_writes(
            &self,
            _: &crate::config::RunnableConfig,
            _: Vec<crate::checkpoint::PendingWrite>,
            _: &str,
        ) -> Result<(), crate::checkpoint::CheckpointError> {
            Ok(())
        }
    }

    /// Verify delta counters are incremented when fields change in a superstep.
    #[tokio::test]
    async fn test_delta_counters_increment_on_field_change() {
        let state = DeltaTestState {
            value: 0,
            messages: vec![],
        };
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 2).unwrap();

        // Simulate a superstep where both fields changed
        let changed = crate::FieldsChanged(0b11); // both field 0 and 1 changed
        loop_.update_delta_counters(&changed);

        assert_eq!(loop_.delta_counters.len(), 2, "should track both fields");

        let field_0 = loop_
            .delta_counters
            .get("field_0")
            .expect("field_0 should exist");
        assert_eq!(field_0.updates, 1, "field_0 should have 1 update");
        assert_eq!(field_0.supersteps, 1, "field_0 should have 1 superstep");

        let field_1 = loop_
            .delta_counters
            .get("field_1")
            .expect("field_1 should exist");
        assert_eq!(field_1.updates, 1, "field_1 should have 1 update");
        assert_eq!(field_1.supersteps, 1, "field_1 should have 1 superstep");
    }

    /// Verify delta counters only increment updates for fields that actually changed.
    #[tokio::test]
    async fn test_delta_counters_increment_unchanged_fields_get_superstep_only() {
        let state = DeltaTestState {
            value: 0,
            messages: vec![],
        };
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 2).unwrap();

        // Only field 0 changed, field 1 did not
        let changed = crate::FieldsChanged(0b01);
        loop_.update_delta_counters(&changed);

        let field_0 = loop_
            .delta_counters
            .get("field_0")
            .expect("field_0 should exist");
        assert_eq!(field_0.updates, 1, "field_0 should have 1 update");

        let field_1 = loop_
            .delta_counters
            .get("field_1")
            .expect("field_1 should exist");
        assert_eq!(
            field_1.updates, 0,
            "field_1 should have 0 updates (not changed)"
        );
        assert_eq!(
            field_1.supersteps, 1,
            "field_1 should still have 1 superstep"
        );
    }

    /// Verify delta counters accumulate across multiple supersteps.
    #[tokio::test]
    async fn test_delta_counters_accumulate_across_supersteps() {
        let state = DeltaTestState {
            value: 0,
            messages: vec![],
        };
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 2).unwrap();

        // First superstep: field 0 changes
        loop_.update_delta_counters(&crate::FieldsChanged(0b01));
        // Second superstep: both fields change
        loop_.update_delta_counters(&crate::FieldsChanged(0b11));

        let field_0 = loop_
            .delta_counters
            .get("field_0")
            .expect("field_0 should exist");
        assert_eq!(field_0.updates, 2, "field_0 updated in both supersteps");
        assert_eq!(field_0.supersteps, 2, "field_0 has 2 supersteps");

        let field_1 = loop_
            .delta_counters
            .get("field_1")
            .expect("field_1 should exist");
        assert_eq!(
            field_1.updates, 1,
            "field_1 updated in only second superstep"
        );
        assert_eq!(field_1.supersteps, 2, "field_1 has 2 supersteps");
    }

    /// Verify delta counters are populated in checkpoints and reset after save.
    #[tokio::test]
    async fn test_delta_counters_populated_in_checkpoint_and_reset() {
        let state = DeltaTestState {
            value: 0,
            messages: vec![],
        };
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 2).unwrap();

        let captured: Arc<std::sync::Mutex<Option<crate::checkpoint::Checkpoint>>> =
            Arc::new(std::sync::Mutex::new(None));
        let checkpointer = CapturingCheckpointer {
            captured: Arc::clone(&captured),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Manually populate delta counters to simulate a prior superstep.
        // We do NOT call update_delta_counters here because after_tick will
        // call it again, doubling the superstep count. Instead we set the
        // counters directly to model a pre-existing counter state.
        loop_.delta_counters.insert(
            "field_0".to_string(),
            DeltaCounters {
                updates: 1,
                supersteps: 1,
            },
        );
        loop_.delta_counters.insert(
            "field_1".to_string(),
            DeltaCounters {
                updates: 2,
                supersteps: 1,
            },
        );

        // Execute a superstep (empty result -- no writes, but after_tick will
        // increment superstep counters for all tracked fields).
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // Checkpoint should have populated delta counters
        let checkpoint = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("checkpoint should have been saved");
        assert!(
            !checkpoint.counters_since_delta_snapshot.is_empty(),
            "counters_since_delta_snapshot should be populated"
        );
        let field_0 = checkpoint
            .counters_since_delta_snapshot
            .get("field_0")
            .expect("field_0 should be in delta counters");
        // Pre-existing 1 update + 1 superstep, after_tick adds 0 updates (empty
        // result) and 1 superstep via update_delta_counters.
        assert_eq!(
            field_0.updates, 1,
            "field_0 should have 1 update in checkpoint"
        );
        assert_eq!(
            field_0.supersteps, 2,
            "field_0 should have 2 supersteps in checkpoint"
        );

        let field_1 = checkpoint
            .counters_since_delta_snapshot
            .get("field_1")
            .expect("field_1 should be in delta counters");
        assert_eq!(
            field_1.updates, 2,
            "field_1 should have 2 updates in checkpoint"
        );

        // After checkpoint save, delta counters should be reset
        assert!(
            loop_.delta_counters.is_empty(),
            "delta counters should be reset after checkpoint save"
        );
    }

    /// Verify `should_take_full_snapshot` returns true when no delta channels configured.
    #[test]
    fn test_should_take_full_snapshot_no_delta_channels() {
        // TestState has no delta_channel_specs override (default empty)
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        // With no delta channels, should always take full snapshot
        assert!(
            loop_.should_take_full_snapshot(),
            "should always take full snapshot with no delta channels"
        );

        // Even with some counters accumulated
        loop_.delta_counters.insert(
            "field_0".to_string(),
            DeltaCounters {
                updates: 100,
                supersteps: 50,
            },
        );
        assert!(
            loop_.should_take_full_snapshot(),
            "still full snapshot when specs are empty (no delta optimization)"
        );
    }

    /// Verify `should_take_full_snapshot` respects `snapshot_frequency` for delta channels.
    #[test]
    fn test_should_take_full_snapshot_respects_frequency() {
        let state = DeltaTestState {
            value: 0,
            messages: vec![],
        };
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 2).unwrap();

        // Below frequency threshold: field 1 has frequency 3, counters at 2
        loop_.delta_counters.insert(
            "field_1".to_string(),
            DeltaCounters {
                updates: 2,
                supersteps: 2,
            },
        );
        assert!(
            !loop_.should_take_full_snapshot(),
            "should not take full snapshot below frequency threshold"
        );

        // At frequency threshold
        loop_.delta_counters.insert(
            "field_1".to_string(),
            DeltaCounters {
                updates: 3,
                supersteps: 3,
            },
        );
        assert!(
            loop_.should_take_full_snapshot(),
            "should take full snapshot at frequency threshold"
        );

        // Above frequency threshold
        loop_.delta_counters.insert(
            "field_1".to_string(),
            DeltaCounters {
                updates: 10,
                supersteps: 5,
            },
        );
        assert!(
            loop_.should_take_full_snapshot(),
            "should take full snapshot above frequency threshold"
        );
    }

    /// Verify `DeltaCounters::exceeds_frequency` edge cases.
    #[test]
    fn test_delta_counters_exceeds_frequency() {
        let counters = DeltaCounters::new();
        assert_eq!(counters.updates, 0);
        assert_eq!(counters.supersteps, 0);

        // Frequency 0 means always snapshot
        assert!(
            counters.exceeds_frequency(0),
            "frequency 0 always snapshots"
        );

        // Below threshold
        let counters = DeltaCounters {
            updates: 2,
            supersteps: 1,
        };
        assert!(!counters.exceeds_frequency(3), "2 < 3, not exceeded");

        // At threshold
        let counters = DeltaCounters {
            updates: 3,
            supersteps: 1,
        };
        assert!(counters.exceeds_frequency(3), "3 >= 3, exceeded");

        // Above threshold
        let counters = DeltaCounters {
            updates: 10,
            supersteps: 1,
        };
        assert!(counters.exceeds_frequency(3), "10 >= 3, exceeded");
    }

    /// Verify that the scratchpad is populated with interrupt IDs after
    /// `execute_superstep` processes pending interrupts. This is the core
    /// fix for review finding B-06-006: the scratchpad must track which
    /// interrupts have been processed so that on re-execution, already-
    /// handled interrupt points receive null-resume values instead of
    /// re-interrupting.
    #[tokio::test]
    async fn test_scratchpad_populated_after_execute_superstep() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        // Simulate pending interrupts from a previous cycle
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        loop_.pending_interrupts = vec![
            crate::interrupt::InterruptSignal {
                index: 0,
                id: Some("int-alpha".to_string()),
                payload: serde_json::Value::Null,
            },
            crate::interrupt::InterruptSignal {
                index: 1,
                id: Some("int-beta".to_string()),
                payload: serde_json::Value::Null,
            },
        ];

        // Before execute_superstep, scratchpad is empty
        assert!(
            !loop_.scratchpad.is_interrupt_processed("int-alpha"),
            "scratchpad should be empty before superstep"
        );
        assert!(
            !loop_.scratchpad.is_interrupt_processed("int-beta"),
            "scratchpad should be empty before superstep"
        );

        let result = loop_.execute_superstep().await;
        assert!(result.is_ok(), "execute_superstep should succeed");

        // After execute_superstep, pending interrupts are marked as processed
        assert!(
            loop_.scratchpad.is_interrupt_processed("int-alpha"),
            "int-alpha should be marked as processed after superstep"
        );
        assert!(
            loop_.scratchpad.is_interrupt_processed("int-beta"),
            "int-beta should be marked as processed after superstep"
        );
        assert!(
            !loop_.scratchpad.is_interrupt_processed("int-gamma"),
            "unrelated interrupt should not be marked as processed"
        );
    }

    /// Verify that the scratchpad accumulates across multiple supersteps,
    /// so interrupts from different cycles are all tracked.
    #[tokio::test]
    async fn test_scratchpad_accumulates_across_supersteps() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        // First superstep with interrupt "int-1"
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        loop_.pending_interrupts = vec![crate::interrupt::InterruptSignal {
            index: 0,
            id: Some("int-1".to_string()),
            payload: serde_json::Value::Null,
        }];

        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // Second superstep with interrupt "int-2"
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        loop_.pending_interrupts = vec![crate::interrupt::InterruptSignal {
            index: 0,
            id: Some("int-2".to_string()),
            payload: serde_json::Value::Null,
        }];

        let _ = loop_.execute_superstep().await;

        // Both interrupt IDs should be tracked
        assert!(
            loop_.scratchpad.is_interrupt_processed("int-1"),
            "int-1 from first superstep should still be tracked"
        );
        assert!(
            loop_.scratchpad.is_interrupt_processed("int-2"),
            "int-2 from second superstep should be tracked"
        );
    }

    // --- B-04-002: superstep checkpoint tests ---

    /// Observed checkpointer call for test assertions
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ObservedCall {
        Put {
            source: crate::checkpoint::CheckpointSource,
            step: i64,
        },
    }

    /// Mock checkpointer that records `put()` calls for test verification
    struct TrackingCheckpointer {
        observed: Arc<std::sync::Mutex<Vec<ObservedCall>>>,
    }

    #[async_trait::async_trait]
    impl crate::checkpoint::CheckpointSaver for TrackingCheckpointer {
        async fn get_tuple(
            &self,
            _: &crate::config::RunnableConfig,
        ) -> Result<Option<crate::checkpoint::CheckpointTuple>, crate::checkpoint::CheckpointError>
        {
            Ok(None)
        }

        async fn list(
            &self,
            _: &crate::config::RunnableConfig,
            _: Option<crate::checkpoint::CheckpointFilter>,
        ) -> Result<Vec<crate::checkpoint::CheckpointTuple>, crate::checkpoint::CheckpointError>
        {
            Ok(Vec::new())
        }

        async fn put(
            &self,
            _: &crate::config::RunnableConfig,
            _checkpoint: crate::checkpoint::Checkpoint,
            metadata: crate::checkpoint::CheckpointMetadata,
        ) -> Result<crate::config::RunnableConfig, crate::checkpoint::CheckpointError> {
            self.observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(ObservedCall::Put {
                    source: metadata.source,
                    step: metadata.step,
                });
            let mut cfg = crate::config::RunnableConfig::new();
            cfg.checkpoint_id = Some("cp-test".to_string());
            Ok(cfg)
        }

        async fn put_writes(
            &self,
            _: &crate::config::RunnableConfig,
            _: Vec<crate::checkpoint::PendingWrite>,
            _: &str,
        ) -> Result<(), crate::checkpoint::CheckpointError> {
            Ok(())
        }
    }

    /// Verify that `after_tick` saves a checkpoint with `CheckpointSource::Loop`
    /// after a normal (non-interrupt) superstep completes (B-04-002).
    #[tokio::test]
    async fn test_superstep_checkpoint_saved_on_normal_completion() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Execute one superstep (no interrupts)
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];

        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // Verify a checkpoint with Loop source was saved
        let has_loop_checkpoint = {
            let calls = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.iter().any(|c| {
                matches!(
                    c,
                    ObservedCall::Put {
                        source: crate::checkpoint::CheckpointSource::Loop,
                        step: 0,
                    }
                )
            })
        };
        assert!(has_loop_checkpoint, "expected a Loop checkpoint at step 0");
    }

    /// Verify that superstep checkpoint is saved at the correct step number
    /// across multiple supersteps (B-04-002).
    #[tokio::test]
    async fn test_superstep_checkpoint_step_increments() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // First superstep at step 0
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // Second superstep at step 1
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        let loop_steps: Vec<i64> = {
            let calls = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls
                .iter()
                .filter_map(|c| match c {
                    ObservedCall::Put {
                        source: crate::checkpoint::CheckpointSource::Loop,
                        step,
                    } => Some(*step),
                    ObservedCall::Put { .. } => None,
                })
                .collect()
        };

        assert_eq!(
            loop_steps,
            vec![0, 1],
            "expected Loop checkpoints at steps 0 and 1, got: {loop_steps:?}"
        );
    }

    /// Verify that NO superstep checkpoint is saved when no checkpointer is configured
    /// (B-04-002 -- should be a silent no-op).
    #[tokio::test]
    async fn test_superstep_checkpoint_noop_without_checkpointer() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();
        assert!(
            loop_.checkpointer.is_none(),
            "no checkpointer should be configured by default"
        );

        // Execute one superstep without checkpointer -- should succeed without error
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];

        let result = loop_.execute_superstep().await;
        assert!(result.is_ok(), "execute_superstep should succeed");

        let after_result = loop_.after_tick(SuperstepResult::empty()).await;
        assert!(
            after_result.is_ok(),
            "after_tick should succeed without checkpointer"
        );
    }

    // --- B-06-003: current_ns tests ---

    #[test]
    fn test_current_ns_empty_when_no_checkpoint_ns() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();
        assert!(
            loop_.current_ns().is_empty(),
            "root-level graph should have empty ns"
        );
    }

    #[test]
    fn test_current_ns_extracts_node_names_from_checkpoint_ns() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new().with_checkpoint_ns(
            crate::checkpoint::CheckpointNamespace::new(vec![
                crate::checkpoint::NamespaceSegment::new(
                    "review".to_string(),
                    "uuid-1".to_string(),
                ),
                crate::checkpoint::NamespaceSegment::new(
                    "detail".to_string(),
                    "uuid-2".to_string(),
                ),
            ]),
        );

        let loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();
        let ns = loop_.current_ns();
        assert_eq!(ns, vec!["review", "detail"]);
    }

    #[test]
    fn test_current_ns_single_segment() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new().with_checkpoint_ns(
            crate::checkpoint::CheckpointNamespace::new(vec![
                crate::checkpoint::NamespaceSegment::new(
                    "agent".to_string(),
                    "uuid-single".to_string(),
                ),
            ]),
        );

        let loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();
        let ns = loop_.current_ns();
        assert_eq!(ns, vec!["agent"]);
    }

    /// Verify that a bubble-up interrupt emitted to the stream carries the
    /// namespace from the execution context (fix for B-06-003).
    #[test]
    fn test_bubble_up_interrupt_emits_ns_from_checkpoint_ns() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let checkpoint_ns = crate::checkpoint::CheckpointNamespace::new(vec![
            crate::checkpoint::NamespaceSegment::new(
                "review".to_string(),
                "uuid-parent".to_string(),
            ),
        ]);
        let config = crate::config::RunnableConfig::new().with_checkpoint_ns(checkpoint_ns);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        // Attach a stream receiver to capture emitted events
        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        let signals = vec![crate::interrupt::InterruptSignal {
            index: 0,
            id: Some("int-ns-0".to_string()),
            payload: serde_json::json!({"node": "child_node"}),
        }];
        let bubble_ups = vec![BubbleUp::Interrupt(crate::pregel::types::GraphInterrupt {
            interrupts: signals,
            step: 1,
        })];

        let _ = loop_.handle_bubble_ups(&bubble_ups);

        // The emitted event should carry the checkpoint namespace
        let event = rx
            .try_recv()
            .expect("should have received an interrupt event");
        match event {
            StreamEvent::Interrupt { ns, .. } => {
                assert_eq!(ns, vec!["review"]);
            }
            other => panic!("expected Interrupt event, got {other:?}"),
        }
    }

    // --- B-06-005: HIDDEN_TAG stream filtering tests ---

    /// Verify that hidden nodes (names starting/ending with `__`) are filtered
    /// from bubble-up interrupt stream events.
    #[test]
    fn test_hidden_node_filtered_from_bubble_up_interrupt_stream() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        // Mix of visible and hidden node signals
        let signals = vec![
            crate::interrupt::InterruptSignal {
                index: 0,
                id: Some("int-visible".to_string()),
                payload: serde_json::json!({"node": "agent"}),
            },
            crate::interrupt::InterruptSignal {
                index: 1,
                id: Some("int-hidden".to_string()),
                payload: serde_json::json!({"node": "__route__"}),
            },
            crate::interrupt::InterruptSignal {
                index: 2,
                id: Some("int-also-visible".to_string()),
                payload: serde_json::json!({"node": "review"}),
            },
        ];
        let bubble_ups = vec![BubbleUp::Interrupt(crate::pregel::types::GraphInterrupt {
            interrupts: signals,
            step: 1,
        })];

        let _ = loop_.handle_bubble_ups(&bubble_ups);

        // Should receive exactly 2 events (agent and review), __route__ filtered
        let mut received_nodes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                StreamEvent::Interrupt { node, .. } => received_nodes.push(node),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(
            received_nodes,
            vec!["agent", "review"],
            "hidden node __route__ should be filtered from stream"
        );
    }

    /// Verify that all-hidden-node signals produce zero stream events.
    #[test]
    fn test_all_hidden_nodes_produce_no_stream_events() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        let signals = vec![
            crate::interrupt::InterruptSignal {
                index: 0,
                id: Some("int-h1".to_string()),
                payload: serde_json::json!({"node": "__route__"}),
            },
            crate::interrupt::InterruptSignal {
                index: 1,
                id: Some("int-h2".to_string()),
                payload: serde_json::json!({"node": "__handler__"}),
            },
        ];
        let bubble_ups = vec![BubbleUp::Interrupt(crate::pregel::types::GraphInterrupt {
            interrupts: signals,
            step: 1,
        })];

        let _ = loop_.handle_bubble_ups(&bubble_ups);

        // No events should be emitted
        assert!(
            rx.try_recv().is_err(),
            "all-hidden signals should produce no stream events"
        );
        // But pending_interrupts and status still reflect all signals (internal state)
        assert_eq!(loop_.pending_interrupts.len(), 2);
    }

    // --- B-03-003: Durability mode tests ---

    /// Verify that `effective_durability` defaults to `Sync` when no durability
    /// is configured in `RunnableConfig`.
    #[test]
    fn test_effective_durability_defaults_to_sync() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();
        assert_eq!(
            loop_.effective_durability(),
            Durability::Sync,
            "default durability should be Sync"
        );
    }

    /// Verify that `Durability::Exit` skips superstep checkpoints but saves
    /// a final checkpoint on clean completion.
    #[tokio::test]
    async fn test_durability_exit_skips_superstep_saves_final() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());
        config.durability = Some(Durability::Exit);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Execute one superstep -- no superstep checkpoint should be saved
        // in Exit mode; only the final exit checkpoint (when pending_tasks
        // is empty) should be persisted.
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // Exactly one checkpoint should be saved (the final exit checkpoint,
        // since compute_next_tasks returns empty for an end() command).
        let calls = observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            calls.len(),
            1,
            "Exit mode should save exactly one final checkpoint"
        );
        assert!(
            matches!(
                &calls[0],
                ObservedCall::Put {
                    source: crate::checkpoint::CheckpointSource::Loop,
                    step: 0
                }
            ),
            "Final exit checkpoint should have Loop source at step 0"
        );
    }

    /// Verify that `Durability::Sync` saves a superstep checkpoint (default behavior).
    #[tokio::test]
    async fn test_durability_sync_saves_superstep_checkpoint() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());
        config.durability = Some(Durability::Sync);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Execute one superstep -- a Loop checkpoint should be saved
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        let has_loop_checkpoint = {
            let calls = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.iter().any(|c| {
                matches!(
                    c,
                    ObservedCall::Put {
                        source: crate::checkpoint::CheckpointSource::Loop,
                        step: 0,
                    }
                )
            })
        };
        assert!(
            has_loop_checkpoint,
            "Sync mode should save a Loop checkpoint at step 0"
        );
    }

    /// Verify that `Durability::Exit` still saves interrupt checkpoints.
    #[tokio::test]
    async fn test_durability_exit_saves_interrupt_checkpoint() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());
        config.durability = Some(Durability::Exit);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Simulate an interrupt scenario
        loop_.pending_interrupts = vec![crate::interrupt::InterruptSignal {
            index: 0,
            id: Some("int-exit-test".to_string()),
            payload: serde_json::json!({"node": "test_node"}),
        }];
        loop_.save_interrupt_checkpoint("test_node").await;

        let has_interrupt_checkpoint = {
            let calls = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.iter().any(|c| {
                matches!(
                    c,
                    ObservedCall::Put {
                        source: crate::checkpoint::CheckpointSource::Interrupt { .. },
                        step: 0,
                    }
                )
            })
        };
        assert!(
            has_interrupt_checkpoint,
            "Exit mode should still save interrupt checkpoints"
        );
    }

    // --- B-08-001: Budget tracker Arc sharing tests ---

    /// Verify that `BudgetTracker` is shared between `PregelLoop` and `RunnableConfig`
    /// via Arc, so tokens reported through `config.budget_tracker()` are visible
    /// to the loop's budget check method.
    #[tokio::test]
    async fn test_budget_tracker_arc_sharing() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let budget = crate::pregel::budget::BudgetConfig::new().with_max_tokens(100);
        let config = crate::config::RunnableConfig::new().with_budget(budget);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        // Set up the shared budget tracker (normally done in compiled.rs)
        let tracker_config = loop_.runnable_config.budget.clone().unwrap();
        loop_.set_budget_tracker(BudgetTracker::new(tracker_config));

        // Initially, no tokens reported, budget not exceeded
        assert!(loop_.budget_tracker.as_ref().unwrap().check().is_none());

        // Report tokens via the RunnableConfig's budget_tracker (the node's view)
        if let Some(ref tracker) = loop_.runnable_config.budget_tracker {
            tracker.report_model_call(30, 20); // 50 total tokens
        }

        // The loop's budget tracker should reflect the same usage (Arc sharing)
        let usage = loop_.budget_tracker.as_ref().unwrap().current_usage();
        assert_eq!(usage.tokens_used, 50);

        // Budget not exceeded yet
        assert!(loop_.budget_tracker.as_ref().unwrap().check().is_none());

        // Report more tokens to exceed the limit via the same shared tracker
        if let Some(ref tracker) = loop_.runnable_config.budget_tracker {
            tracker.report_model_call(40, 30); // 70 more, total 120 > 100
        }

        // Budget should now be exceeded
        assert!(loop_.budget_tracker.as_ref().unwrap().check().is_some());
        assert_eq!(
            loop_
                .budget_tracker
                .as_ref()
                .unwrap()
                .current_usage()
                .tokens_used,
            120
        );

        // tick() should detect the exceeded budget and return an error
        let _ = loop_.tick().unwrap_err();
        assert!(loop_.status.is_terminal());
    }

    /// Verify that multiple token reports via the `RunnableConfig` path
    /// accumulate correctly and pass through budget checks when a
    /// cost limit is configured.
    #[tokio::test]
    async fn test_budget_tracker_cost_via_config() {
        let state = TestState;
        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let budget = crate::pregel::budget::BudgetConfig::new().with_max_cost_usd(0.01);
        let config = crate::config::RunnableConfig::new().with_budget(budget);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let tracker_config = loop_.runnable_config.budget.clone().unwrap();
        loop_.set_budget_tracker(BudgetTracker::new(tracker_config));

        // Report costs via the RunnableConfig (simulating multiple LLM calls)
        if let Some(ref tracker) = loop_.runnable_config.budget_tracker {
            tracker.report_cost(0.003);
            tracker.report_cost(0.004);
        }

        // Combined cost is below limit
        let usage = loop_.budget_tracker.as_ref().unwrap().current_usage();
        assert!((usage.cost_usd - 0.007).abs() < 0.0001);
        assert!(loop_.budget_tracker.as_ref().unwrap().check().is_none());

        // Third call pushes cost over the limit
        if let Some(ref tracker) = loop_.runnable_config.budget_tracker {
            tracker.report_cost(0.004); // total now 0.011 > 0.01
        }

        assert!(loop_.budget_tracker.as_ref().unwrap().check().is_some());

        // tick() should detect the exceeded budget
        let _ = loop_.tick().unwrap_err();
        assert!(loop_.status.is_terminal());
    }

    /// Verify that `Durability::Async` does not block on checkpoint persistence.
    #[tokio::test]
    async fn test_durability_async_does_not_block() {
        let state = TestState;

        let mut nodes = IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(Command::end()) }).into_node("test_node"),
        );

        let trigger_table = TriggerTable::new();
        let mut config = crate::config::RunnableConfig::new();
        config.thread_id = Some("test-thread".to_string());
        config.durability = Some(Durability::Async);

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let checkpointer = TrackingCheckpointer {
            observed: Arc::clone(&observed),
        };
        loop_.set_checkpointer(Arc::new(checkpointer));

        // Execute one superstep
        loop_.pending_tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];
        let _ = loop_.execute_superstep().await;
        let _ = loop_.after_tick(SuperstepResult::empty()).await;

        // In Async mode, the put() is spawned as a background task. Give it
        // a brief moment to execute before checking.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // The checkpoint should eventually be persisted by the spawned task.
        let has_checkpoint = {
            let calls = observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.iter().any(|c| {
                matches!(
                    c,
                    ObservedCall::Put {
                        source: crate::checkpoint::CheckpointSource::Loop,
                        step: 0,
                    }
                )
            })
        };
        assert!(
            has_checkpoint,
            "Async mode should eventually persist the checkpoint via spawned task"
        );
    }

    // --- B-05-002: Command stream_data tests ---

    /// Verify that a task output with `stream_data` produces `StreamEvent::Custom`
    /// events during `after_tick`.
    #[tokio::test]
    async fn test_stream_data_emits_custom_events() {
        let state = TestState;
        let nodes = IndexMap::new();
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        // Build a SuperstepResult with a task output that has stream_data
        let result = SuperstepResult {
            task_outputs: vec![TaskOutput {
                task_id: "task-1".to_string(),
                node_name: "test_node".to_string(),
                command: Command::end()
                    .with_stream_data(serde_json::json!({"event": "first"}))
                    .with_stream_data(serde_json::json!({"event": "second"})),
                duration: std::time::Duration::from_millis(1),
                trigger: TaskTrigger::Pull,
                error: None,
            }],
            bubble_ups: Vec::new(),
        };

        let () = loop_.after_tick(result).await.unwrap();

        // Collect Custom events from the stream
        let mut custom_data = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let StreamEvent::Custom { node, data, ns } = event {
                assert_eq!(node, "test_node");
                assert!(ns.is_empty());
                custom_data.push(data);
            }
        }

        assert_eq!(custom_data.len(), 2, "should emit two custom events");
        assert_eq!(custom_data[0], serde_json::json!({"event": "first"}));
        assert_eq!(custom_data[1], serde_json::json!({"event": "second"}));
    }

    /// Verify that a task output without `stream_data` produces no Custom events.
    #[tokio::test]
    async fn test_stream_data_empty_produces_no_custom_events() {
        let state = TestState;
        let nodes = IndexMap::new();
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        // Build a SuperstepResult with a task output that has NO stream_data
        let result = SuperstepResult {
            task_outputs: vec![TaskOutput {
                task_id: "task-1".to_string(),
                node_name: "test_node".to_string(),
                command: Command::end(),
                duration: std::time::Duration::from_millis(1),
                trigger: TaskTrigger::Pull,
                error: None,
            }],
            bubble_ups: Vec::new(),
        };

        let () = loop_.after_tick(result).await.unwrap();

        // No Custom events should be emitted
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, StreamEvent::Custom { .. }),
                "no Custom events expected for empty stream_data"
            );
        }
    }

    /// Verify that `stream_data` from multiple task outputs are all emitted.
    #[tokio::test]
    async fn test_stream_data_multiple_tasks() {
        let state = TestState;
        let nodes = IndexMap::new();
        let trigger_table = TriggerTable::new();
        let config = crate::config::RunnableConfig::new();

        let mut loop_ = PregelLoop::new(state, nodes, trigger_table, config, 0).unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        loop_.stream_tx = Some(tx);

        // Build a SuperstepResult with two task outputs, one with stream_data
        let result = SuperstepResult {
            task_outputs: vec![
                TaskOutput {
                    task_id: "task-1".to_string(),
                    node_name: "node_a".to_string(),
                    command: Command::end().with_stream_data(serde_json::json!("from_a")),
                    duration: std::time::Duration::from_millis(1),
                    trigger: TaskTrigger::Pull,
                    error: None,
                },
                TaskOutput {
                    task_id: "task-2".to_string(),
                    node_name: "node_b".to_string(),
                    command: Command::end(),
                    duration: std::time::Duration::from_millis(2),
                    trigger: TaskTrigger::Pull,
                    error: None,
                },
            ],
            bubble_ups: Vec::new(),
        };

        let () = loop_.after_tick(result).await.unwrap();

        // Collect Custom events from the stream
        let mut custom_events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let StreamEvent::Custom { node, data, .. } = event {
                custom_events.push((node, data));
            }
        }

        assert_eq!(
            custom_events.len(),
            1,
            "only node_a should emit a custom event"
        );
        assert_eq!(custom_events[0].0, "node_a");
        assert_eq!(custom_events[0].1, serde_json::json!("from_a"));
    }
}

// Rust guideline compliant 2026-05-22
