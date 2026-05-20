//! Pregel execution engine
//!
//! This module implements the Pregel algorithm for executing compiled graphs.
//! It provides parallel task execution, version tracking, and budget management.
//!
//! # Overview
//!
//! The Pregel engine executes graphs using the following algorithm:
//!
//! 1. **Initialization**: Create a [`PregelLoop`] with initial state and graph
//! 2. **Superstep Execution**: Execute pending tasks in parallel using [`execute_superstep`]
//! 3. **State Updates**: Apply node outputs to state using [`PregelLoop::after_tick`]
//! 4. **Task Scheduling**: Compute next tasks using [`compute_next_tasks`]
//! 5. **Repeat**: Continue until no more tasks or termination condition
//!
//! # Examples
//!
//! ```ignore
//! use juncture_core::pregel::{PregelLoop, LoopStatus};
//!
//! let mut loop = PregelLoop::new(
//!     initial_state,
//!     nodes,
//!     trigger_table,
//!     config,
//!     num_fields,
//! )?;
//!
//! while loop.tick()? {
//!     let result = loop.execute_superstep().await?;
//!     loop.after_tick(result)?;
//! }
//!
//! let final_state = loop.into_state();
//! ```

mod budget;
mod context;
mod durability;
mod loop_;
mod protocol;
mod runner;
mod scheduler;
mod types;

pub use crate::stream::{StreamEvent, StreamMode};
pub use budget::{
    BudgetConfig, BudgetExceededAction, BudgetExceededReason, BudgetTracker, BudgetUsage,
};
pub use context::{ExecutionConfig, ExecutionContext, TimeoutPolicy};
pub use durability::Durability;
pub use loop_::{PregelLoop, RunControl};
pub use protocol::PregelProtocol;
pub use runner::execute_superstep;
pub use scheduler::{
    FieldVersionTracker, TriggerToNodes, VersionsSeen, apply_writes, check_replace_conflicts,
    compute_next_tasks, consume_triggered_channels, schedule_error_handlers,
};
pub use types::{
    BubbleUp, GraphDrained, GraphInterrupt, LoopStatus, PendingTask, SuperstepResult,
    SyncAsyncFuture, TaskOutput, TaskTrigger,
};

// Rust guideline compliant 2026-05-20
