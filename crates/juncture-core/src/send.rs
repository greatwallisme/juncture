//! Send API for dynamic fan-out
//!
//! The Send API allows nodes to dynamically create multiple parallel tasks,
//! each with its own state snapshot.

use crate::State;

/// Dynamic fan-out target
///
/// Represents a single task in a dynamic fan-out operation. Each Send target
/// specifies which node to execute and provides a custom state for that task.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::{Send, State, command::SendTarget};
/// use serde_json::json;
///
/// struct MyState;
/// impl State for MyState {
///     type Update = MyStateUpdate;
/// }
///
/// struct MyStateUpdate;
///
/// // Create a send target with custom state
/// let send = Send {
///     node: "worker".to_string(),
///     state: MyState { /* ... */ },
/// };
/// ```
#[derive(Debug)]
pub struct Send<S: State> {
    /// Target node name to execute
    pub node: String,

    /// Custom state for this task (overrides current state)
    pub state: S,
}

impl<S: State + serde::Serialize> From<Send<S>> for crate::command::SendTarget {
    fn from(send: Send<S>) -> Self {
        Self {
            node: send.node,
            // Convert state to JSON value
            state: serde_json::to_value(send.state)
                .expect("state must be serializable for Send API"),
        }
    }
}

// Rust guideline compliant 2025-01-18
