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

impl<S: State + serde::Serialize> TryFrom<Send<S>> for crate::command::SendTarget {
    type Error = crate::JunctureError;

    fn try_from(send: Send<S>) -> Result<Self, Self::Error> {
        // Convert state to a JSON value, propagating serialization errors
        // instead of panicking. `serde_json::to_value` can fail even with
        // `S: serde::Serialize` (e.g. `f64::NAN`, or a custom `Serialize` impl
        // that returns an error); a non-serializable fan-out target must surface
        // as a `JunctureError` the engine can handle rather than crashing the
        // worker (audit #4).
        let state = serde_json::to_value(send.state).map_err(|err| {
            crate::JunctureError::execution(format!("Send state serialization failed: {err}"))
        })?;
        Ok(Self {
            node: send.node,
            state,
            timeout: None,
        })
    }
}

// Rust guideline compliant 2026-07-27
