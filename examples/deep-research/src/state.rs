//! Research state for tracking multi-agent research workflow.

use juncture_core::state::Message;
use juncture_derive::State;
use serde::{Deserialize, Serialize};

/// A research finding from a sub-task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    /// Sub-task identifier that produced this finding.
    pub sub_task: String,

    /// Content of the finding.
    pub content: String,

    /// Source references for this finding.
    pub sources: Vec<String>,
}

/// A sub-task in the research plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubTask {
    /// Unique identifier for this sub-task.
    pub id: usize,

    /// Description of what to research.
    pub description: String,

    /// Current status of this sub-task.
    pub status: TaskStatus,
}

/// Status of a sub-task in the research plan.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is pending execution.
    #[default]
    Pending,
    /// Task is currently being researched.
    InProgress,
    /// Task has been completed.
    Completed,
}

/// Research state tracking the multi-agent workflow.
#[derive(State, Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResearchState {
    /// Conversation history with append semantics.
    #[reducer(append)]
    pub messages: Vec<Message>,

    /// Original research query.
    pub query: String,

    /// Research plan with sub-tasks to execute.
    pub plan: Vec<SubTask>,

    /// Research findings from sub-tasks with append semantics.
    #[reducer(append)]
    pub findings: Vec<Finding>,

    /// Final research report (when available).
    pub report: Option<String>,
}

// Rust guideline compliant 2026-05-27
