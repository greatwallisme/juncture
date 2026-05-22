//! Core types for Pregel execution engine
//!
//! This module defines the fundamental types used by the Pregel engine
//! for executing compiled graphs, including loop status, tasks, and results.

use crate::{State, interrupt::InterruptSignal};
use std::time::Duration;

/// Status of the Pregel execution loop
///
/// Represents the current state of graph execution, including completion,
/// interruption, and error conditions.
#[derive(Clone, Debug)]
pub enum LoopStatus {
    /// Loop is still running
    Running,

    /// Graph execution completed normally
    Done,

    /// Recursion limit exceeded
    OutOfSteps,

    /// Interrupt before executing next superstep
    InterruptBefore(Vec<InterruptSignal>),

    /// Interrupt after executing a superstep
    InterruptAfter(Vec<InterruptSignal>),

    /// Budget limit exceeded
    BudgetExceeded,

    /// Execution was cancelled
    Cancelled,

    /// Graph drained (no more tasks to execute)
    Drained,
}

/// Pending task for execution
///
/// Represents a node that has been scheduled for execution in the next superstep.
#[derive(Clone, Debug)]
pub struct PendingTask<S: State> {
    /// Unique task identifier
    pub id: String,

    /// Name of the node to execute
    pub node_name: String,

    /// What triggered this task
    pub trigger: TaskTrigger,

    /// Optional state override for this task (as JSON for Send operations)
    pub state_override: Option<S>,

    /// Optional state override as JSON (for Send operations)
    ///
    /// Stored separately because Send operations provide state as `serde_json::Value`,
    /// which can't be deserialized to S without S: `DeserializeOwned` bound.
    /// This is deserialized during task execution in the runner.
    pub state_json: Option<serde_json::Value>,
}

/// What triggered a task to be scheduled
///
/// Distinguishes between pull-based (normal edge routing) and push-based
/// (Send operation) task scheduling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskTrigger {
    /// Normal edge routing (pull-based)
    Pull,

    /// Push-based routing via Send operation
    Push {
        /// Index in the Send targets list
        index: usize,
    },
}

/// Result of executing one superstep
///
/// Contains the outputs from all tasks executed in a single superstep,
/// as well as any subgraph propagation events that need to be handled
/// by the parent `PregelLoop`.
#[derive(Clone, Debug)]
pub struct SuperstepResult<S: State> {
    /// Outputs from each completed task
    pub task_outputs: Vec<TaskOutput<S>>,

    /// Subgraph propagation events (interrupts, drains, parent commands)
    /// that bubble up from nested subgraph execution.
    pub bubble_ups: Vec<BubbleUp<S>>,
}

/// Output from a single completed task
///
/// Contains the task ID, node name, command returned by the node,
/// trigger type, execution duration, and optional error information
/// when the node has a registered error handler.
pub struct TaskOutput<S: State> {
    /// Task identifier
    pub task_id: String,

    /// Name of the node that was executed
    pub node_name: String,

    /// Command returned by the node
    pub command: crate::Command<S>,

    /// Execution duration
    pub duration: Duration,

    /// What triggered this task
    pub trigger: TaskTrigger,

    /// Error that occurred during execution, if the node has a registered
    /// error handler. When present, `command` contains the error handler's
    /// output. When absent but the task failed, the error propagates
    /// immediately (no recovery).
    pub error: Option<crate::JunctureError>,
}

impl<S: State> std::fmt::Debug for TaskOutput<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskOutput")
            .field("task_id", &self.task_id)
            .field("node_name", &self.node_name)
            .field("command", &"<command>")
            .field("duration", &self.duration)
            .field("trigger", &self.trigger)
            .field("error", &self.error)
            .finish()
    }
}

impl<S: State> Clone for TaskOutput<S>
where
    S::Update: Clone,
{
    fn clone(&self) -> Self {
        Self {
            task_id: self.task_id.clone(),
            node_name: self.node_name.clone(),
            command: self.command.clone(),
            duration: self.duration,
            trigger: self.trigger.clone(),
            // JunctureError is not Clone (contains Backtrace). For cloned
            // TaskOutputs the error is reconstructed from the display string
            // so that recovery scheduling still works after cloning.
            error: self
                .error
                .as_ref()
                .map(|e| crate::JunctureError::execution(e.to_string())),
        }
    }
}

/// Result that bubbles up from subgraph execution
///
/// Represents various outcomes that can occur when executing a subgraph,
/// including interrupts, draining, and normal command returns.
#[derive(Clone)]
pub enum BubbleUp<S: State> {
    /// Human-in-the-loop interrupt occurred
    Interrupt(GraphInterrupt),

    /// Graph was drained
    Drained(GraphDrained),

    /// Normal command return from subgraph
    ParentCommand(crate::Command<S>),
}

impl<S: State> std::fmt::Debug for BubbleUp<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interrupt(arg0) => f.debug_tuple("Interrupt").field(arg0).finish(),
            Self::Drained(arg0) => f.debug_tuple("Drained").field(arg0).finish(),
            Self::ParentCommand(_) => f.debug_tuple("ParentCommand").field(&"<command>").finish(),
        }
    }
}

/// Interrupt information from graph execution
///
/// Contains interrupt signals and the step at which they occurred.
#[derive(Clone, Debug)]
pub struct GraphInterrupt {
    /// Interrupt signals
    pub interrupts: Vec<InterruptSignal>,

    /// Step at which interrupt occurred
    pub step: usize,
}

/// Information about graph being drained
///
/// Contains the reason why the graph was drained.
#[derive(Clone, Debug)]
pub struct GraphDrained {
    /// Reason for draining
    pub reason: String,
}

impl<S: State> PendingTask<S> {
    /// Create a new pending task
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::types::{PendingTask, TaskTrigger};
    /// use juncture_core::State;
    ///
    /// # #[derive(Clone, Debug)]
    /// # struct MyState;
    /// # impl State for MyState {
    /// #     type Update = ();
    /// #     fn apply(&mut self, _: ()) -> juncture_core::FieldsChanged { juncture_core::FieldsChanged(0) }
    /// #     fn reset_ephemeral(&mut self) {}
    /// # }
    /// let task = PendingTask::<MyState> {
    ///     id: "task-123".to_string(),
    ///     node_name: "my_node".to_string(),
    ///     trigger: TaskTrigger::Pull,
    ///     state_override: None,
    ///     state_json: None,
    /// };
    /// ```
    #[must_use]
    pub const fn new(
        id: String,
        node_name: String,
        trigger: TaskTrigger,
        state_override: Option<S>,
    ) -> Self {
        Self {
            id,
            node_name,
            trigger,
            state_override,
            state_json: None,
        }
    }

    /// Create a pull-based task
    #[must_use]
    pub const fn pull(id: String, node_name: String) -> Self {
        Self {
            id,
            node_name,
            trigger: TaskTrigger::Pull,
            state_override: None,
            state_json: None,
        }
    }

    /// Create a push-based task with state override as JSON
    ///
    /// This is used for Send operations where the state override comes from
    /// `SendTarget.state` (a `serde_json::Value`).
    #[must_use]
    pub const fn push(
        id: String,
        node_name: String,
        index: usize,
        state_json: serde_json::Value,
    ) -> Self {
        Self {
            id,
            node_name,
            trigger: TaskTrigger::Push { index },
            state_override: None,
            state_json: Some(state_json),
        }
    }

    /// Create a push-based task with typed state override
    ///
    /// This variant is used when the state override is already available
    /// as the typed state S (not just JSON).
    #[must_use]
    pub const fn push_typed(
        id: String,
        node_name: String,
        index: usize,
        state_override: S,
    ) -> Self {
        Self {
            id,
            node_name,
            trigger: TaskTrigger::Push { index },
            state_override: Some(state_override),
            state_json: None,
        }
    }
}

impl TaskTrigger {
    /// Check if this is a pull-based trigger
    #[must_use]
    pub const fn is_pull(&self) -> bool {
        matches!(self, Self::Pull)
    }

    /// Check if this is a push-based trigger
    #[must_use]
    pub const fn is_push(&self) -> bool {
        matches!(self, Self::Push { .. })
    }
}

impl<S: State> SuperstepResult<S> {
    /// Create an empty superstep result
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            task_outputs: Vec::new(),
            bubble_ups: Vec::new(),
        }
    }

    /// Check if this superstep had any tasks or bubble-up events
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.task_outputs.is_empty() && self.bubble_ups.is_empty()
    }

    /// Get the number of tasks in this superstep
    #[must_use]
    pub const fn len(&self) -> usize {
        self.task_outputs.len()
    }

    /// Check if there are any bubble-up events from subgraph execution
    #[must_use]
    pub const fn has_bubble_ups(&self) -> bool {
        !self.bubble_ups.is_empty()
    }
}

impl<S: State> Default for SuperstepResult<S> {
    fn default() -> Self {
        Self::empty()
    }
}

impl LoopStatus {
    /// Check if the loop is still running
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Check if the loop has terminated
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::OutOfSteps | Self::BudgetExceeded | Self::Cancelled | Self::Drained
        )
    }

    /// Check if the loop is interrupted
    #[must_use]
    pub const fn is_interrupted(&self) -> bool {
        matches!(self, Self::InterruptBefore(_) | Self::InterruptAfter(_))
    }

    /// Extract interrupt signals if interrupted
    #[must_use]
    pub fn interrupt_signals(&self) -> &[InterruptSignal] {
        match self {
            Self::InterruptBefore(signals) | Self::InterruptAfter(signals) => signals,
            _ => &[],
        }
    }
}

/// A task result that may be synchronously ready or asynchronously computed.
///
/// In the functional API (`@task`/`@entrypoint`), results are wrapped as
/// `SyncAsyncFuture`. Callers use `.result().await` uniformly regardless of
/// whether the underlying computation was synchronous (cache hit) or
/// asynchronous (computation required).
///
/// # Type Parameters
///
/// * `T` - The success value type produced by this future
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::types::SyncAsyncFuture;
///
/// // Synchronous (cache hit)
/// let ready = SyncAsyncFuture::ready(42);
/// assert!(ready.is_ready());
/// assert_eq!(ready.result().await?, 42);
///
/// // Asynchronous (needs computation)
/// let pending = SyncAsyncFuture::pending(async { Ok(99) });
/// assert!(!pending.is_ready());
/// assert_eq!(pending.result().await?, 99);
/// ```
pub enum SyncAsyncFuture<T> {
    /// Synchronous result (e.g., cache hit).
    Ready(Option<T>),

    /// Asynchronous result that resolves to `Result<T, JunctureError>`.
    Future(futures::future::BoxFuture<'static, Result<T, crate::JunctureError>>),
}

impl<T: std::fmt::Debug> std::fmt::Debug for SyncAsyncFuture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(value) => f.debug_tuple("Ready").field(value).finish(),
            Self::Future(_) => f.debug_tuple("Future").field(&"<pending>").finish(),
        }
    }
}

impl<T> SyncAsyncFuture<T> {
    /// Create a synchronous ready result with a value.
    ///
    /// Use this when the result is already available (e.g., cache hit).
    #[must_use]
    pub const fn ready(value: T) -> Self {
        Self::Ready(Some(value))
    }

    /// Create an empty ready result indicating no value is available.
    ///
    /// Calling [`result()`](Self::result) on this will return
    /// [`JunctureError::empty_channel()`].
    #[must_use]
    pub const fn empty() -> Self {
        Self::Ready(None)
    }

    /// Create an asynchronous result from a future that resolves to
    /// `Result<T, JunctureError>`.
    ///
    /// Use this when the result requires computation (e.g., cache miss,
    /// network call, or LLM invocation).
    pub fn pending(
        fut: impl std::future::Future<Output = Result<T, crate::JunctureError>> + Send + 'static,
    ) -> Self {
        Self::Future(Box::pin(fut))
    }

    /// Await the result, returning `Ok(T)` on success.
    ///
    /// # Errors
    ///
    /// - Returns [`JunctureError::empty_channel()`] if the variant is
    ///   `Ready(None)` (no value available).
    /// - Returns the error from the inner future for the `Future` variant.
    pub async fn result(self) -> Result<T, crate::JunctureError> {
        match self {
            Self::Ready(Some(value)) => Ok(value),
            Self::Ready(None) => Err(crate::JunctureError::empty_channel()),
            Self::Future(fut) => fut.await,
        }
    }

    /// Check if the result is synchronously available.
    ///
    /// Returns `true` for the `Ready` variant (regardless of whether the
    /// inner value is `Some` or `None`), and `false` for `Future`.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

impl<T> From<T> for SyncAsyncFuture<T> {
    fn from(value: T) -> Self {
        Self::Ready(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::SyncAsyncFuture;
    use crate::JunctureError;

    #[tokio::test]
    async fn ready_some_returns_value() {
        let saf = SyncAsyncFuture::ready(42_i32);
        assert!(saf.is_ready());
        let result = saf.result().await;
        assert_eq!(result.expect("Ready(Some) should yield Ok"), 42);
    }

    #[tokio::test]
    async fn ready_none_returns_empty_channel_error() {
        let saf: SyncAsyncFuture<i32> = SyncAsyncFuture::empty();
        assert!(saf.is_ready());
        let err = saf
            .result()
            .await
            .expect_err("Ready(None) should yield Err");
        assert!(
            err.is_empty_channel(),
            "expected EmptyChannel error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn from_trait_creates_ready_some() {
        let saf = SyncAsyncFuture::from("hello");
        assert!(saf.is_ready());
        assert_eq!(
            saf.result().await.expect("From<T> should yield Ok"),
            "hello"
        );
    }

    #[tokio::test]
    async fn pending_future_returns_ok() {
        let saf = SyncAsyncFuture::pending(async { Ok::<_, JunctureError>(99_u64) });
        assert!(!saf.is_ready());
        assert_eq!(saf.result().await.expect("pending Ok should yield Ok"), 99);
    }

    #[tokio::test]
    async fn pending_future_propagates_error() {
        let error = JunctureError::execution("computation failed");
        let saf = SyncAsyncFuture::pending(async {
            Err::<i32, _>(JunctureError::execution("computation failed"))
        });
        let err = saf
            .result()
            .await
            .expect_err("pending Err should yield Err");
        assert!(err.is_execution(), "expected Execution error, got {err:?}");
        assert_eq!(format!("{error}"), format!("{err}"));
    }

    #[tokio::test]
    async fn pending_from_boxed_future() {
        let saf: SyncAsyncFuture<String> =
            SyncAsyncFuture::pending(async { Ok("from boxed".to_string()) });
        assert!(!saf.is_ready());
        assert_eq!(
            saf.result().await.expect("boxed future should yield Ok"),
            "from boxed"
        );
    }

    #[test]
    fn debug_ready_some_shows_value() {
        let saf = SyncAsyncFuture::ready(42_i32);
        let debug = format!("{saf:?}");
        assert!(
            debug.contains("Ready") && debug.contains("Some(42)"),
            "Debug should show the value: {debug}"
        );
    }

    #[test]
    fn debug_ready_none_shows_none() {
        let saf: SyncAsyncFuture<i32> = SyncAsyncFuture::empty();
        let debug = format!("{saf:?}");
        assert!(
            debug.contains("Ready") && debug.contains("None"),
            "Debug should show None: {debug}"
        );
    }

    #[test]
    fn debug_future_shows_pending() {
        let saf = SyncAsyncFuture::pending(async { Ok::<_, JunctureError>(1_i32) });
        let debug = format!("{saf:?}");
        assert!(
            debug.contains("Future") && debug.contains("pending"),
            "Debug should show <pending> for Future variant: {debug}"
        );
    }
}

// Rust guideline compliant 2026-05-21
