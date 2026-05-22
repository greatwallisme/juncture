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

    /// Custom streaming data to emit during execution
    ///
    /// Nodes can attach arbitrary JSON values here that will be emitted as
    /// [`StreamEvent::Custom`] events during graph execution. Each entry in
    /// the vector produces one custom stream event tagged with the emitting
    /// node name. Use [`Command::with_stream_data`] to append items.
    pub stream_data: Vec<serde_json::Value>,
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
            stream_data: Vec::new(),
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
            stream_data: Vec::new(),
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
            stream_data: Vec::new(),
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
            stream_data: Vec::new(),
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
            stream_data: Vec::new(),
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
            stream_data: Vec::new(),
        }
    }

    /// Create command that routes to parent graph
    pub fn goto_parent(target: impl Into<String>) -> Self {
        Self {
            update: None,
            goto: Goto::Next(target.into()),
            graph: GraphTarget::Parent,
            resume: None,
            stream_data: Vec::new(),
        }
    }

    /// Attach a resume value to this command
    #[must_use]
    pub fn with_resume(mut self, value: ResumeValue) -> Self {
        self.resume = Some(value);
        self
    }

    /// Attach custom streaming data to this command
    ///
    /// The given value is appended to the command's streaming data list.
    /// During graph execution, each entry is emitted as a
    /// [`StreamEvent::Custom`] event, allowing nodes to push custom JSON
    /// payloads to the stream consumer alongside state updates and routing.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::Command;
    /// use serde_json::json;
    ///
    /// // In a node returning Command:
    /// Command::end().with_stream_data(json!({"progress": 0.75}));
    /// ```
    #[must_use]
    pub fn with_stream_data(mut self, data: serde_json::Value) -> Self {
        self.stream_data.push(data);
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
            stream_data: Vec::new(),
        }
    }
}

// Rust guideline compliant 2026-05-22

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }
        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct TestUpdate;

    #[test]
    fn command_default_has_empty_stream_data() {
        let cmd = Command::<TestState>::default();
        assert!(cmd.stream_data.is_empty());
    }

    #[test]
    fn command_update_has_empty_stream_data() {
        let cmd = Command::<TestState>::update(TestUpdate);
        assert!(cmd.stream_data.is_empty());
    }

    #[test]
    fn command_goto_has_empty_stream_data() {
        let cmd = Command::<TestState>::goto("target");
        assert!(cmd.stream_data.is_empty());
    }

    #[test]
    fn command_end_has_empty_stream_data() {
        let cmd = Command::<TestState>::end();
        assert!(cmd.stream_data.is_empty());
    }

    #[test]
    fn command_with_stream_data_appends_single_item() {
        let cmd = Command::<TestState>::end().with_stream_data(json!({"key": "value"}));
        assert_eq!(cmd.stream_data.len(), 1);
        assert_eq!(cmd.stream_data[0], json!({"key": "value"}));
    }

    #[test]
    fn command_with_stream_data_appends_multiple_items() {
        let cmd = Command::<TestState>::end()
            .with_stream_data(json!({"step": 1}))
            .with_stream_data(json!({"step": 2}))
            .with_stream_data(json!({"step": 3}));
        assert_eq!(cmd.stream_data.len(), 3);
        assert_eq!(cmd.stream_data[0], json!({"step": 1}));
        assert_eq!(cmd.stream_data[1], json!({"step": 2}));
        assert_eq!(cmd.stream_data[2], json!({"step": 3}));
    }

    #[test]
    fn command_with_stream_data_preserves_other_fields() {
        let cmd = Command::<TestState>::update(TestUpdate)
            .with_stream_data(json!("progress"))
            .with_resume(ResumeValue::Single(json!("resumed")));
        assert!(cmd.update.is_some());
        assert_eq!(cmd.stream_data.len(), 1);
        assert!(cmd.resume.is_some());
    }

    #[test]
    fn command_with_stream_data_works_with_goto() {
        let cmd = Command::<TestState>::goto("next_node").with_stream_data(json!("data"));
        assert!(matches!(cmd.goto, Goto::Next(ref target) if target == "next_node"));
        assert_eq!(cmd.stream_data.len(), 1);
    }

    #[test]
    fn command_send_has_empty_stream_data() {
        let cmd = Command::<TestState>::send(vec![]);
        assert!(cmd.stream_data.is_empty());
    }

    #[test]
    fn command_goto_parent_has_empty_stream_data() {
        let cmd = Command::<TestState>::goto_parent("parent");
        assert!(cmd.stream_data.is_empty());
    }
}
