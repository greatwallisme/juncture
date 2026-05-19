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
/// Contains the outputs from all tasks executed in a single superstep.
#[derive(Clone, Debug)]
pub struct SuperstepResult<S: State> {
    /// Outputs from each completed task
    pub task_outputs: Vec<TaskOutput<S>>,
}

/// Output from a single completed task
///
/// Contains the task ID, node name, command returned by the node,
/// trigger type, and execution duration.
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
}

impl<S: State> std::fmt::Debug for TaskOutput<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskOutput")
            .field("task_id", &self.task_id)
            .field("node_name", &self.node_name)
            .field("command", &"<command>")
            .field("duration", &self.duration)
            .field("trigger", &self.trigger)
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
        }
    }
}

/// Result that bubbles up from subgraph execution
///
/// Represents various outcomes that can occur when executing a subgraph,
/// including interrupts, draining, and normal command returns.
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
    /// #     type FieldVersions = ();
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
        }
    }

    /// Check if this superstep had any tasks
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.task_outputs.is_empty()
    }

    /// Get the number of tasks in this superstep
    #[must_use]
    pub const fn len(&self) -> usize {
        self.task_outputs.len()
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
}

/// A task result that may be synchronously ready or asynchronously computed
///
/// In functional API (@task/@entrypoint), results are wrapped as `SyncAsyncFuture`.
/// Callers use `.await` uniformly regardless of sync/async nature.
///
/// # Type Parameters
///
/// * `T` - The result type
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::types::SyncAsyncFuture;
///
/// let ready = SyncAsyncFuture::ready(42);
/// assert!(ready.is_ready());
/// ```
pub enum SyncAsyncFuture<T> {
    /// Synchronous result (e.g., cache hit)
    Ready(Option<T>),

    /// Asynchronous result (e.g., requires computation)
    Future(futures::future::BoxFuture<'static, T>),
}

impl<T> std::fmt::Debug for SyncAsyncFuture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(v) => f
                .debug_tuple("Ready")
                .field(&v.as_ref().map(|_| "<value>"))
                .finish(),
            Self::Future(_) => f.debug_tuple("Future").field(&"<future>").finish(),
        }
    }
}

impl<T> SyncAsyncFuture<T> {
    /// Create a synchronous ready result
    #[must_use]
    pub const fn ready(value: T) -> Self {
        Self::Ready(Some(value))
    }

    /// Create an empty ready result (no value available)
    #[must_use]
    pub const fn empty() -> Self {
        Self::Ready(None)
    }

    /// Await the result, returning an error if the value is empty
    ///
    /// # Errors
    ///
    /// Returns [`crate::JunctureError::EmptyChannel`] if the result
    /// is `Ready(None)` (no value available).
    pub async fn result(self) -> Result<T, crate::JunctureError> {
        match self {
            Self::Ready(Some(value)) => Ok(value),
            Self::Ready(None) => Err(crate::JunctureError::empty_channel()),
            Self::Future(fut) => Ok(fut.await),
        }
    }

    /// Check if the result is synchronously available
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

impl<T> From<futures::future::BoxFuture<'static, T>> for SyncAsyncFuture<T> {
    fn from(fut: futures::future::BoxFuture<'static, T>) -> Self {
        Self::Future(fut)
    }
}

// Rust guideline compliant 2026-05-19
// Rust guideline compliant 2026-05-20
