//! Main Pregel execution loop
//!
//! This module provides the `PregelLoop` struct that orchestrates graph execution
//! using the Pregel algorithm with version tracking and task scheduling.

use crate::{
    JunctureError, Node, State,
    edge::TriggerTable,
    interrupt::should_interrupt,
    pregel::{
        budget::BudgetTracker,
        context::ExecutionContext,
        runner::execute_superstep,
        scheduler::{FieldVersionTracker, VersionsSeen, compute_next_tasks},
        types::{LoopStatus, PendingTask, SuperstepResult},
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

    /// Optional budget tracker
    budget_tracker: Option<BudgetTracker>,

    /// Run control for graceful shutdown
    run_control: RunControl,

    /// Unique ID for this graph execution
    run_id: String,

    /// Interrupt signal receiver from the last superstep
    /// This is stored here so `after_tick` can drain it
    interrupt_rx: Option<mpsc::UnboundedReceiver<crate::interrupt::InterruptSignal>>,

    /// Superstep start time for duration tracking
    ///
    /// Set at the beginning of [`execute_superstep`], read in [`after_tick`].
    superstep_start: Option<Instant>,
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
            .field("superstep_start", &self.superstep_start.is_some())
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
            superstep_start: None,
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
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::budget::BudgetTracker;
    ///
    /// let budget = BudgetTracker::new(BudgetConfig::new());
    /// loop.set_budget_tracker(budget);
    /// ```
    pub fn set_budget_tracker(&mut self, tracker: BudgetTracker) {
        self.budget_tracker = Some(tracker);
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
            return Err(JunctureError::recursion_limit(
                self.step,
                self.runnable_config.recursion_limit,
            ));
        }

        // Check cancellation
        if self.cancellation_token.is_cancelled() {
            self.status = LoopStatus::Cancelled;
            return Ok(false);
        }

        // Check budget
        if let Some(tracker) = &self.budget_tracker
            && let Some(reason) = tracker.check()
        {
            self.status = LoopStatus::BudgetExceeded;
            return Err(JunctureError::execution(format!(
                "Budget exceeded: {reason}"
            )));
        }

        // Compute next tasks if pending is empty
        if self.pending_tasks.is_empty() {
            // Check if drain is requested - if so, we're done
            if self.run_control.is_drain_requested() {
                self.status = LoopStatus::Done;
                return Ok(false);
            }

            // Try to compute tasks from trigger table
            // This is a no-op in the current implementation since
            // compute_next_tasks requires completed tasks
            self.status = LoopStatus::Done;
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

            // Build versions_seen map for should_interrupt
            let versions_seen_map: HashMap<String, Vec<u64>> = self
                .nodes
                .keys()
                .map(|node_name| {
                    let versions = self.versions_seen.get_versions(node_name);
                    (node_name.clone(), versions.to_vec())
                })
                .collect();

            if let Some(signals) = should_interrupt(
                &self.pending_tasks,
                &interrupt_before_set,
                &HashSet::new(), // interrupt_after not checked here
                &channel_versions,
                &versions_seen_map,
            ) {
                self.status = LoopStatus::InterruptBefore(signals);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Execute one superstep
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
    pub async fn execute_superstep(&mut self) -> Result<SuperstepResult<S>, JunctureError> {
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
        )
        .await?;

        let duration = start.elapsed().as_millis();
        tracing::Span::current().record("juncture.step.duration_ms", duration);

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
        reason = "after_tick requires multiple steps: apply writes, bump versions, emit events, compute tasks, drain interrupts, check interrupts, finish channels, increment step"
    )]
    pub async fn after_tick(&mut self, result: SuperstepResult<S>) -> Result<(), JunctureError>
    where
        S: Clone,
    {
        // Apply writes from completed tasks
        let mut total_changed = crate::FieldsChanged(0);

        for task_output in &result.task_outputs {
            if let Some(ref update) = task_output.command.update {
                let changed = self.state.apply(update.clone());
                total_changed.merge(&changed);
            }
        }

        // Bump field versions for changed fields
        self.field_versions.bump_all(&total_changed);

        // Mark versions as consumed
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
            self.status = LoopStatus::InterruptAfter(node_interrupts.clone());

            // Emit interrupt events to stream
            if let Some(ref tx) = self.stream_tx {
                for signal in &node_interrupts {
                    let event = StreamEvent::Interrupt {
                        node: signal
                            .payload
                            .get("node")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        payload: signal.payload.clone(),
                        resumable: true,
                        ns: Vec::new(),
                    };
                    let _ = tx.send(event);
                }
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

            // Build versions_seen map for should_interrupt
            let versions_seen_map: HashMap<String, Vec<u64>> = self
                .nodes
                .keys()
                .map(|node_name| {
                    let versions = self.versions_seen.get_versions(node_name);
                    (node_name.clone(), versions.to_vec())
                })
                .collect();

            if let Some(signals) = should_interrupt(
                &self.pending_tasks,
                &HashSet::new(), // interrupt_before not checked here
                &interrupt_after_set,
                &channel_versions,
                &versions_seen_map,
            ) {
                self.status = LoopStatus::InterruptAfter(signals.clone());

                // Emit interrupt events to stream
                if let Some(ref tx) = self.stream_tx {
                    for signal in &signals {
                        let event = StreamEvent::Interrupt {
                            node: signal
                                .payload
                                .get("node")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            payload: signal.payload.clone(),
                            resumable: true,
                            ns: Vec::new(),
                        };
                        let _ = tx.send(event);
                    }
                }

                return Ok(());
            }
        }

        // Call finish() on all channels if no more tasks (execution complete)
        // This is critical for LastValueAfterFinishChannel which only makes
        // its value available after finish() is called.
        if self.pending_tasks.is_empty() {
            self.finish_all_channels();
        }

        // Increment step
        self.step += 1;

        // Report step to budget tracker
        if let Some(ref tracker) = self.budget_tracker {
            tracker.report_step();
        }

        Ok(())
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

    /// Get the current status
    #[must_use]
    pub const fn status(&self) -> &LoopStatus {
        &self.status
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

    /// Finish all channels in the state
    ///
    /// Called when graph execution completes (no more pending tasks).
    /// This allows channels like `LastValueAfterFinishChannel` to finalize
    /// their state and make values available to consumers.
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
        // Finish all fields by index
        // The number of fields is tracked by field_versions
        let field_count = self.field_versions.len();
        for field_idx in 0..field_count {
            self.state.finish_field(field_idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, node::IntoNode, node::NodeFnCommand};

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

    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        type FieldVersions = ();

        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct TestUpdate;
}

// Rust guideline compliant 2026-05-20
// Rust guideline compliant 2026-05-20
