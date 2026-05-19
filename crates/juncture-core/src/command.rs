use crate::interrupt::ResumeValue;
use crate::state::State;

/// Command: node return value combining state update and routing
///
/// Nodes can return `S::Update` (simple case) or `Command<S>` (for routing control).
#[derive(Clone, Debug)]
pub struct Command<S: State> {
    /// State update to apply
    pub update: Option<S::Update>,

    /// Routing instruction
    pub goto: Goto,

    /// Target graph (current or parent)
    pub graph: GraphTarget,

    /// Resume value for HITL interrupt resumption
    ///
    /// When provided, this value is used to resume from a previously triggered
    /// interrupt. Supports single values, ID-based mapping, and namespace-based
    /// mapping via [`ResumeValue`].
    pub resume: Option<ResumeValue>,
}

/// Routing instruction from node
#[derive(Clone, Debug)]
pub enum Goto {
    /// No routing (use external edges)
    None,

    /// Route to single node
    Next(String),

    /// Route to multiple nodes (parallel)
    Multiple(Vec<String>),

    /// Dynamic fan-out to multiple targets
    Send(Vec<SendTarget>),

    /// Terminate this path
    End,
}

/// Send target for dynamic fan-out
#[derive(Clone, Debug)]
pub struct SendTarget {
    /// Target node name
    pub node: String,

    /// State to use for this task (overrides current state)
    pub state: serde_json::Value,

    /// Optional per-task timeout override
    pub timeout: Option<std::time::Duration>,
}

/// Target graph for routing
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphTarget {
    /// Current graph (default)
    Current,

    /// Parent graph (for subgraph navigation)
    Parent,
}

/// Final value distinguishing return value from saved state
///
/// Used in entrypoint functions to separate what's returned to caller
/// from what's saved to checkpoint.
#[derive(Debug)]
pub struct Final<V, S> {
    /// Value returned to caller
    pub value: V,

    /// State update to save to checkpoint
    pub save: S,
}

/// Routing command from node to parent or child graph
///
/// This type is used in subgraph communication to control execution flow
/// between parent and child graphs.
#[derive(Clone, Debug)]
pub enum CommandGoto {
    /// Route to a single node
    One(String),

    /// Route to multiple nodes (parallel execution)
    Many(Vec<String>),

    /// Route to parent graph
    Parent(String),

    /// Dynamic fan-out to multiple targets with state overrides
    Send(Vec<SendTarget>),
}

/// Command wrapper for subgraph-to-parent communication
///
/// This newtype wrapper is used as an exception mechanism for subgraph nodes
/// to send commands to their parent graph. It wraps a `Command<S>` where `S`
/// is the parent graph's state type.
///
/// # Type Parameters
///
/// * `S` - The parent graph's state type
pub struct ParentCommand<S: State>(pub Command<S>);

impl<S: State> std::fmt::Debug for ParentCommand<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ParentCommand").field(&"<command>").finish()
    }
}

impl<S: State> Command<S> {
    /// Create command with only state update
    #[must_use]
    pub const fn update(update: S::Update) -> Self {
        Self {
            update: Some(update),
            goto: Goto::None,
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command with only routing
    #[must_use]
    pub fn goto(target: impl Into<String>) -> Self {
        Self {
            update: None,
            goto: Goto::Next(target.into()),
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command with update and routing
    #[must_use]
    pub fn update_and_goto(update: S::Update, target: impl Into<String>) -> Self {
        Self {
            update: Some(update),
            goto: Goto::Next(target.into()),
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command with dynamic fan-out
    #[must_use]
    pub const fn send(targets: Vec<SendTarget>) -> Self {
        Self {
            update: None,
            goto: Goto::Send(targets),
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command with update and fan-out
    #[must_use]
    pub const fn update_and_send(update: S::Update, targets: Vec<SendTarget>) -> Self {
        Self {
            update: Some(update),
            goto: Goto::Send(targets),
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command that terminates current path
    #[must_use]
    pub const fn end() -> Self {
        Self {
            update: None,
            goto: Goto::End,
            graph: GraphTarget::Current,
            resume: None,
        }
    }

    /// Create command that routes to parent graph
    pub fn goto_parent(target: impl Into<String>) -> Self {
        Self {
            update: None,
            goto: Goto::Next(target.into()),
            graph: GraphTarget::Parent,
            resume: None,
        }
    }

    /// Attach a resume value to this command
    #[must_use]
    pub fn with_resume(mut self, value: ResumeValue) -> Self {
        self.resume = Some(value);
        self
    }
}

impl<S: State> Default for Command<S> {
    fn default() -> Self {
        Self {
            update: None,
            goto: Goto::None,
            graph: GraphTarget::Current,
            resume: None,
        }
    }
}

// Rust guideline compliant 2026-05-20
