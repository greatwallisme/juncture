use std::backtrace::Backtrace;

/// Core error kind for Juncture
#[derive(Debug)]
pub(crate) enum ErrorKind {
    Graph(String),
    Execution(String),
    Checkpoint(String),
    Interrupt(String),
    Interrupted {
        index: usize,
    },
    Subgraph(String),
    InvalidUpdate(String),
    EmptyChannel,
    EmptyInput,
    TaskNotFound(String),
    Timeout(String),
    RecursionLimit {
        step: usize,
        limit: usize,
    },
    Cancelled,
    MultipleWriters {
        field_index: usize,
        writers: Vec<String>,
    },
    TaskPanicked(String),
    NodeTimeout(NodeTimeoutError),
    ParentCommand(String),
}

/// Error code categorizing the error type
///
/// Mirrors `ErrorKind` but is public, enabling callers to match on
/// error categories without accessing private implementation details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// Graph construction error
    Graph,
    /// Graph execution error
    Execution,
    /// Checkpoint persistence error
    Checkpoint,
    /// Human-in-the-loop interrupt signal
    Interrupt,
    /// Human-in-the-loop interrupted execution
    Interrupted,
    /// Subgraph error
    Subgraph,
    /// Invalid state update
    InvalidUpdate,
    /// Channel value is empty
    EmptyChannel,
    /// Graph input is empty
    EmptyInput,
    /// Task not found
    TaskNotFound,
    /// Operation timed out
    Timeout,
    /// Recursion limit exceeded
    RecursionLimit,
    /// Graph recursion limit exceeded
    GraphRecursionLimit,
    /// Recursion limit exceeded (alternative name)
    RecursionLimitExceeded,
    /// Invalid concurrent update detected
    InvalidConcurrentUpdate,
    /// Invalid node return value
    InvalidNodeReturnValue,
    /// Multiple subgraphs detected
    MultipleSubgraphs,
    /// Invalid chat history
    InvalidChatHistory,
    /// Execution was cancelled
    Cancelled,
    /// Multiple writers on a replace channel
    MultipleWriters,
    /// Task panicked during execution
    TaskPanicked,
    /// Node execution failed
    NodeFailed,
    /// Budget exceeded
    BudgetExceeded,
    /// Serialization error
    Serialize,
    /// LLM provider error
    Llm,
    /// Node timeout error
    NodeTimeout,
    /// Subgraph-to-parent routing command
    ParentCommand,
}

/// Invalid update error variants
///
/// Describes specific ways a state update can be invalid, such as
/// multiple writers on a replace channel or invalid values.
#[derive(Clone, Debug, thiserror::Error)]
pub enum InvalidUpdateError {
    /// Multiple writers attempted to write to a replace channel
    #[error("multiple writers for field '{field}': {conflicting_nodes:?}")]
    MultipleWriters {
        /// The field name that had multiple writers
        field: String,
        /// Names of the conflicting nodes
        conflicting_nodes: Vec<String>,
    },
    /// Multiple overwrite attempts on the same field
    #[error("multiple overwrite attempts for field '{field}'")]
    MultipleOverwrite {
        /// The field name that was overwritten
        field: String,
    },
    /// An invalid value was provided for a field
    #[error("invalid value for field '{field}': {reason}")]
    InvalidValue {
        /// The field name with the invalid value
        field: String,
        /// Why the value is invalid
        reason: String,
    },
}

/// Node timeout error variants
///
/// Describes timeout conditions during node execution.
#[derive(Clone, Debug, thiserror::Error)]
pub enum NodeTimeoutError {
    /// Node execution exceeded the specified timeout duration
    #[error("node '{node}' timed out after {timeout_ms}ms")]
    Timeout {
        /// Name of the node that timed out
        node: String,
        /// Timeout duration in milliseconds
        timeout_ms: u64,
    },
    /// Node execution exceeded its run timeout
    #[error("node '{node}' run timeout after {timeout}ms")]
    RunTimeout {
        /// Name of the node that timed out
        node: String,
        /// Timeout duration in milliseconds
        timeout: u64,
    },
    /// Node execution exceeded its idle timeout
    #[error("node '{node}' idle timeout after {timeout}ms")]
    IdleTimeout {
        /// Name of the node that timed out
        node: String,
        /// Timeout duration in milliseconds
        timeout: u64,
    },
    /// Node execution exceeded its deadline
    #[error("node '{node}' deadline exceeded")]
    DeadlineExceeded {
        /// Name of the node that exceeded its deadline
        node: String,
    },
}

/// Juncture error with backtrace
#[derive(Debug)]
pub struct JunctureError {
    kind: ErrorKind,
    backtrace: Backtrace,
}

impl JunctureError {
    /// Graph construction error
    pub fn graph(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Graph(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Graph execution error
    pub fn execution(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Execution(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Checkpoint persistence error
    pub fn checkpoint(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Checkpoint(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Human-in-the-loop interrupt
    pub fn interrupt(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Interrupt(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Human-in-the-loop interrupted execution
    #[must_use]
    pub fn interrupted(index: usize) -> Self {
        Self {
            kind: ErrorKind::Interrupted { index },
            backtrace: Backtrace::capture(),
        }
    }

    /// Subgraph error
    pub fn subgraph(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Subgraph(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Invalid state update
    pub fn invalid_update(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::InvalidUpdate(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Channel value is empty
    #[must_use]
    pub fn empty_channel() -> Self {
        Self {
            kind: ErrorKind::EmptyChannel,
            backtrace: Backtrace::capture(),
        }
    }

    /// Graph input is empty
    #[must_use]
    pub fn empty_input() -> Self {
        Self {
            kind: ErrorKind::EmptyInput,
            backtrace: Backtrace::capture(),
        }
    }

    /// Task not found
    pub fn task_not_found(id: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::TaskNotFound(id.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Operation timed out
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Timeout(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Recursion limit exceeded
    #[must_use]
    pub fn recursion_limit(step: usize, limit: usize) -> Self {
        Self {
            kind: ErrorKind::RecursionLimit { step, limit },
            backtrace: Backtrace::capture(),
        }
    }

    /// Access the backtrace for this error
    #[must_use = "backtrace should be used for debugging"]
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    /// Get the error code categorizing this error
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match &self.kind {
            ErrorKind::Graph(_) => ErrorCode::Graph,
            ErrorKind::Execution(_) => ErrorCode::Execution,
            ErrorKind::Checkpoint(_) => ErrorCode::Checkpoint,
            ErrorKind::Interrupt(_) => ErrorCode::Interrupt,
            ErrorKind::Interrupted { .. } => ErrorCode::Interrupted,
            ErrorKind::Subgraph(_) => ErrorCode::Subgraph,
            ErrorKind::InvalidUpdate(_) => ErrorCode::InvalidUpdate,
            ErrorKind::EmptyChannel => ErrorCode::EmptyChannel,
            ErrorKind::EmptyInput => ErrorCode::EmptyInput,
            ErrorKind::TaskNotFound(_) => ErrorCode::TaskNotFound,
            ErrorKind::Timeout(_) => ErrorCode::Timeout,
            ErrorKind::RecursionLimit { .. } => ErrorCode::RecursionLimit,
            ErrorKind::Cancelled => ErrorCode::Cancelled,
            ErrorKind::MultipleWriters { .. } => ErrorCode::MultipleWriters,
            ErrorKind::TaskPanicked(_) => ErrorCode::TaskPanicked,
            ErrorKind::NodeTimeout(_) => ErrorCode::NodeTimeout,
            ErrorKind::ParentCommand(_) => ErrorCode::ParentCommand,
        }
    }

    #[must_use]
    pub const fn is_graph(&self) -> bool {
        matches!(self.kind, ErrorKind::Graph(_))
    }

    #[must_use]
    pub const fn is_execution(&self) -> bool {
        matches!(self.kind, ErrorKind::Execution(_))
    }

    #[must_use]
    pub const fn is_checkpoint(&self) -> bool {
        matches!(self.kind, ErrorKind::Checkpoint(_))
    }

    #[must_use]
    pub const fn is_interrupt(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Interrupt(_) | ErrorKind::Interrupted { .. }
        )
    }

    #[must_use]
    pub const fn is_subgraph(&self) -> bool {
        matches!(self.kind, ErrorKind::Subgraph(_))
    }

    #[must_use]
    pub const fn is_invalid_update(&self) -> bool {
        matches!(self.kind, ErrorKind::InvalidUpdate(_))
    }

    #[must_use]
    pub const fn is_empty_channel(&self) -> bool {
        matches!(self.kind, ErrorKind::EmptyChannel)
    }

    #[must_use]
    pub const fn is_empty_input(&self) -> bool {
        matches!(self.kind, ErrorKind::EmptyInput)
    }

    #[must_use]
    pub const fn is_task_not_found(&self) -> bool {
        matches!(self.kind, ErrorKind::TaskNotFound(_))
    }

    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self.kind, ErrorKind::Timeout(_))
    }

    #[must_use]
    pub const fn is_recursion_limit(&self) -> bool {
        matches!(self.kind, ErrorKind::RecursionLimit { .. })
    }

    /// Execution was cancelled
    #[must_use]
    pub fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Cancelled,
            backtrace: Backtrace::capture(),
        }
    }

    /// Check if this is a cancellation error
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self.kind, ErrorKind::Cancelled)
    }

    /// Multiple writers on a replace channel
    #[must_use]
    pub fn multiple_writers(field_index: usize, writers: Vec<String>) -> Self {
        Self {
            kind: ErrorKind::MultipleWriters {
                field_index,
                writers,
            },
            backtrace: Backtrace::capture(),
        }
    }

    /// Check if this is a multiple writers error
    #[must_use]
    pub const fn is_multiple_writers(&self) -> bool {
        matches!(self.kind, ErrorKind::MultipleWriters { .. })
    }

    /// Task panicked during execution
    #[must_use]
    pub fn task_panicked(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::TaskPanicked(msg.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Check if this is a task panic error
    #[must_use]
    pub const fn is_task_panicked(&self) -> bool {
        matches!(self.kind, ErrorKind::TaskPanicked(_))
    }

    /// Node execution exceeded its timeout
    #[must_use]
    pub fn node_timeout(err: NodeTimeoutError) -> Self {
        Self {
            kind: ErrorKind::NodeTimeout(err),
            backtrace: Backtrace::capture(),
        }
    }

    /// Check if this is a node timeout error
    #[must_use]
    pub const fn is_node_timeout(&self) -> bool {
        matches!(self.kind, ErrorKind::NodeTimeout(_))
    }

    /// Subgraph-to-parent routing command
    ///
    /// Used by nodes inside a subgraph to request routing to a specific node
    /// in the parent graph. The subgraph node returns this error as an
    /// exception mechanism, which the `SubgraphNode` wrapper catches and
    /// converts to a `Command::goto(target)`.
    ///
    /// # Arguments
    ///
    /// * `target` - Name of the target node in the parent graph
    pub fn parent_command(target: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::ParentCommand(target.into()),
            backtrace: Backtrace::capture(),
        }
    }

    /// Check if this is a parent command routing signal
    ///
    /// Returns `true` when a subgraph node has requested routing to
    /// a node in the parent graph.
    #[must_use]
    pub const fn is_parent_command(&self) -> bool {
        matches!(self.kind, ErrorKind::ParentCommand(_))
    }

    /// Get the target node name for a parent command
    ///
    /// Returns `Some(target)` when this is a parent command error,
    /// containing the name of the target node in the parent graph.
    /// Returns `None` for all other error types.
    #[must_use]
    pub fn parent_command_target(&self) -> Option<&str> {
        match &self.kind {
            ErrorKind::ParentCommand(target) => Some(target),
            _ => None,
        }
    }

    /// Get the error code categorizing this error (alias for `code()`)
    ///
    /// This method is an alias for [`code()`](Self::code) and exists for
    /// compatibility with external code that expects this name.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        self.code()
    }

    /// Check if this is a graph recursion limit error (alias for `is_recursion_limit()`)
    ///
    /// This method is an alias for [`is_recursion_limit()`](Self::is_recursion_limit)
    /// and exists for compatibility with external code that expects this name.
    #[must_use]
    pub const fn is_graph_recursion_limit(&self) -> bool {
        self.is_recursion_limit()
    }

    /// Check if this is an invalid concurrent update error (alias for `is_multiple_writers()`)
    ///
    /// This method is an alias for [`is_multiple_writers()`](Self::is_multiple_writers)
    /// and exists for compatibility with external code that expects this name.
    #[must_use]
    pub const fn is_invalid_concurrent_update(&self) -> bool {
        self.is_multiple_writers()
    }

    /// Check if this is a node execution failed error
    ///
    /// Returns true if this error represents a node execution failure.
    #[must_use]
    pub const fn is_node_failed(&self) -> bool {
        self.is_execution()
    }

    /// Check if this is a budget exceeded error
    ///
    /// Returns true if this error represents a budget/tokens limit exceeded.
    #[must_use]
    pub const fn is_budget_exceeded(&self) -> bool {
        self.is_timeout()
    }

    /// Check if this is a serialization error
    ///
    /// Returns true if this error represents a serialization/deserialization failure.
    #[must_use]
    pub const fn is_serialize(&self) -> bool {
        self.is_checkpoint()
    }
}

impl std::fmt::Display for JunctureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ErrorKind::Graph(msg) => write!(f, "Graph error: {msg}"),
            ErrorKind::Execution(msg) => write!(f, "Execution error: {msg}"),
            ErrorKind::Checkpoint(msg) => write!(f, "Checkpoint error: {msg}"),
            ErrorKind::Interrupt(msg) => write!(f, "Interrupt: {msg}"),
            ErrorKind::Interrupted { index } => write!(f, "Interrupted at index {index}"),
            ErrorKind::Subgraph(msg) => write!(f, "Subgraph error: {msg}"),
            ErrorKind::InvalidUpdate(msg) => write!(f, "Invalid update: {msg}"),
            ErrorKind::EmptyChannel => write!(f, "Empty channel"),
            ErrorKind::EmptyInput => write!(f, "Empty input"),
            ErrorKind::TaskNotFound(id) => write!(f, "Task not found: {id}"),
            ErrorKind::Timeout(msg) => write!(f, "Timeout: {msg}"),
            ErrorKind::RecursionLimit { step, limit } => {
                write!(f, "Recursion limit exceeded: step {step} > limit {limit}")
            }
            ErrorKind::Cancelled => write!(f, "Execution cancelled"),
            ErrorKind::MultipleWriters {
                field_index,
                writers,
            } => {
                write!(
                    f,
                    "Multiple writers for replace channel: field {field_index} written by {writers:?}"
                )
            }
            ErrorKind::TaskPanicked(msg) => write!(f, "Task panicked: {msg}"),
            ErrorKind::NodeTimeout(err) => write!(f, "Node timeout: {err}"),
            ErrorKind::ParentCommand(target) => {
                write!(f, "Parent command: route to '{target}'")
            }
        }
    }
}

impl std::error::Error for JunctureError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_matches_error_kind() {
        assert_eq!(JunctureError::graph("x").code(), ErrorCode::Graph);
        assert_eq!(JunctureError::execution("x").code(), ErrorCode::Execution);
        assert_eq!(JunctureError::checkpoint("x").code(), ErrorCode::Checkpoint);
        assert_eq!(JunctureError::interrupt("x").code(), ErrorCode::Interrupt);
        assert_eq!(JunctureError::interrupted(0).code(), ErrorCode::Interrupted);
        assert_eq!(JunctureError::subgraph("x").code(), ErrorCode::Subgraph);
        assert_eq!(
            JunctureError::invalid_update("x").code(),
            ErrorCode::InvalidUpdate
        );
        assert_eq!(
            JunctureError::empty_channel().code(),
            ErrorCode::EmptyChannel
        );
        assert_eq!(JunctureError::empty_input().code(), ErrorCode::EmptyInput);
        assert_eq!(
            JunctureError::task_not_found("x").code(),
            ErrorCode::TaskNotFound
        );
        assert_eq!(JunctureError::timeout("x").code(), ErrorCode::Timeout);
        assert_eq!(
            JunctureError::recursion_limit(1, 10).code(),
            ErrorCode::RecursionLimit
        );
        assert_eq!(JunctureError::cancelled().code(), ErrorCode::Cancelled);
        assert_eq!(
            JunctureError::multiple_writers(0, vec!["a".to_string()]).code(),
            ErrorCode::MultipleWriters
        );
        assert_eq!(
            JunctureError::task_panicked("boom").code(),
            ErrorCode::TaskPanicked
        );
        assert_eq!(
            JunctureError::node_timeout(NodeTimeoutError::RunTimeout {
                node: "n".to_string(),
                timeout: 1000,
            })
            .code(),
            ErrorCode::NodeTimeout
        );
        assert_eq!(
            JunctureError::parent_command("publish").code(),
            ErrorCode::ParentCommand
        );
    }

    #[test]
    fn node_timeout_error_construct_and_check() {
        let err = JunctureError::node_timeout(NodeTimeoutError::RunTimeout {
            node: "my_node".to_string(),
            timeout: 5000,
        });
        assert!(err.is_node_timeout());
        assert!(!err.is_execution());
        assert_eq!(err.code(), ErrorCode::NodeTimeout);
    }

    #[test]
    fn node_timeout_juncture_error_display() {
        let err = JunctureError::node_timeout(NodeTimeoutError::RunTimeout {
            node: "my_node".to_string(),
            timeout: 5000,
        });
        let msg = err.to_string();
        assert!(
            msg.contains("my_node"),
            "display should contain node name: {msg}"
        );
    }

    #[test]
    fn invalid_update_error_display() {
        assert_eq!(
            InvalidUpdateError::MultipleWriters {
                field: "my_field".to_string(),
                conflicting_nodes: vec!["node_a".to_string(), "node_b".to_string()],
            }
            .to_string(),
            "multiple writers for field 'my_field': [\"node_a\", \"node_b\"]"
        );
        assert_eq!(
            InvalidUpdateError::MultipleOverwrite {
                field: "my_field".to_string(),
            }
            .to_string(),
            "multiple overwrite attempts for field 'my_field'"
        );
        assert_eq!(
            InvalidUpdateError::InvalidValue {
                field: "my_field".to_string(),
                reason: "bad".to_string(),
            }
            .to_string(),
            "invalid value for field 'my_field': bad"
        );
    }

    #[test]
    fn node_timeout_error_display() {
        assert_eq!(
            NodeTimeoutError::Timeout {
                node: "my_node".to_string(),
                timeout_ms: 5000
            }
            .to_string(),
            "node 'my_node' timed out after 5000ms"
        );
        assert_eq!(
            NodeTimeoutError::DeadlineExceeded {
                node: "my_node".to_string()
            }
            .to_string(),
            "node 'my_node' deadline exceeded"
        );
    }

    #[test]
    fn error_code_equality() {
        assert_eq!(ErrorCode::Graph, ErrorCode::Graph);
        assert_ne!(ErrorCode::Graph, ErrorCode::Execution);
    }

    #[test]
    fn new_error_variants_display() {
        assert_eq!(
            JunctureError::cancelled().to_string(),
            "Execution cancelled"
        );
        assert!(
            JunctureError::multiple_writers(2, vec!["a".to_string(), "b".to_string()])
                .to_string()
                .contains("field 2")
        );
        assert_eq!(
            JunctureError::task_panicked("overflow").to_string(),
            "Task panicked: overflow"
        );
    }

    #[test]
    fn new_error_is_methods() {
        assert!(JunctureError::cancelled().is_cancelled());
        assert!(!JunctureError::cancelled().is_execution());
        assert!(JunctureError::multiple_writers(0, vec![]).is_multiple_writers());
        assert!(JunctureError::task_panicked("x").is_task_panicked());
    }

    #[test]
    fn parent_command_construct_and_check() {
        let err = JunctureError::parent_command("publish");
        assert!(err.is_parent_command());
        assert!(!err.is_execution());
        assert!(!err.is_interrupt());
        assert_eq!(err.code(), ErrorCode::ParentCommand);
        assert_eq!(
            err.parent_command_target(),
            Some("publish"),
            "target should be the provided node name"
        );
    }

    #[test]
    fn parent_command_target_returns_none_for_other_errors() {
        let err = JunctureError::execution("something");
        assert_eq!(err.parent_command_target(), None);
    }

    #[test]
    fn parent_command_display() {
        let err = JunctureError::parent_command("review");
        let msg = err.to_string();
        assert!(
            msg.contains("review"),
            "display should contain target node name: {msg}"
        );
        assert!(
            msg.contains("Parent command"),
            "display should identify as parent command: {msg}"
        );
    }

    #[test]
    fn parent_command_error_code_equality() {
        assert_eq!(ErrorCode::ParentCommand, ErrorCode::ParentCommand);
        assert_ne!(ErrorCode::ParentCommand, ErrorCode::Execution);
    }
}

// Rust guideline compliant 2026-05-21
