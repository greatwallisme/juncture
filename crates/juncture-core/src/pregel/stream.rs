//! Stream events for Pregel execution
//!
//! This module defines events that can be streamed during graph execution
//! for monitoring and debugging purposes.

use crate::{Command, State};
use std::time::Duration;

/// Stream mode for execution events
///
/// Controls what information is included in stream events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamMode {
    /// Stream only final state values
    #[default]
    Values,

    /// Stream only state updates
    Updates,

    /// Stream all debug information
    Debug,
}

/// Event emitted during graph execution
///
/// Represents various events that occur during Pregel execution,
/// including state changes, task completions, and errors.
#[derive(Debug)]
pub enum StreamEvent<S: State> {
    /// State snapshot (Values mode)
    Values {
        /// Current state
        state: S,
    },

    /// State updates (Updates mode)
    Updates {
        /// Updates by node name
        updates: Vec<(String, S::Update)>,
    },

    /// Task completed (Debug mode)
    TaskEnd {
        /// Task identifier
        task_id: String,

        /// Node name
        node_name: String,

        /// Execution duration
        duration: Duration,
    },

    /// Error occurred (all modes)
    Error {
        /// The error
        error: crate::JunctureError,
    },
}

impl<S: State + Clone> Clone for StreamEvent<S>
where
    S::Update: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Self::Values { state } => Self::Values {
                state: state.clone(),
            },
            Self::Updates { updates } => Self::Updates {
                updates: updates.clone(),
            },
            Self::TaskEnd {
                task_id,
                node_name,
                duration,
            } => Self::TaskEnd {
                task_id: task_id.clone(),
                node_name: node_name.clone(),
                duration: *duration,
            },
            Self::Error { error } => Self::Error {
                error: crate::JunctureError::execution(format!("{error}")),
            },
        }
    }
}

impl<S: State> StreamEvent<S> {
    /// Check if this is a values event
    #[must_use]
    pub const fn is_values(&self) -> bool {
        matches!(self, Self::Values { .. })
    }

    /// Check if this is an updates event
    #[must_use]
    pub const fn is_updates(&self) -> bool {
        matches!(self, Self::Updates { .. })
    }

    /// Check if this is a task end event
    #[must_use]
    pub const fn is_task_end(&self) -> bool {
        matches!(self, Self::TaskEnd { .. })
    }

    /// Check if this is an error event
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Get the state if this is a values event
    #[must_use]
    pub const fn as_values(&self) -> Option<&S> {
        match self {
            Self::Values { state } => Some(state),
            _ => None,
        }
    }

    /// Get the updates if this is an updates event
    #[must_use]
    pub fn as_updates(&self) -> Option<&[(String, S::Update)]> {
        match self {
            Self::Updates { updates } => Some(updates),
            _ => None,
        }
    }
}

/// Helper to create stream events from command results
///
/// This trait provides convenience methods for creating stream events
/// from the results of node execution.
#[allow(dead_code, reason = "trait provided for future use")]
pub trait IntoStreamEvent<S: State> {
    /// Convert this result into a stream event
    fn into_stream_event(self, mode: StreamMode) -> Option<StreamEvent<S>>;
}

impl<S: State> IntoStreamEvent<S> for (String, String, Command<S>, Duration) {
    fn into_stream_event(self, mode: StreamMode) -> Option<StreamEvent<S>> {
        let (task_id, node_name, _command, duration) = self;

        match mode {
            StreamMode::Debug => Some(StreamEvent::TaskEnd {
                task_id,
                node_name,
                duration,
            }),
            StreamMode::Values | StreamMode::Updates => {
                // Values and Updates modes are handled at the superstep level
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_stream_event_is_values() {
        let event = StreamEvent::<TestState>::Values { state: TestState };
        assert!(event.is_values());
        assert!(!event.is_updates());
        assert!(!event.is_task_end());
        assert!(!event.is_error());
    }

    #[test]
    fn test_stream_event_is_updates() {
        let event = StreamEvent::<TestState>::Updates { updates: vec![] };
        assert!(!event.is_values());
        assert!(event.is_updates());
        assert!(!event.is_task_end());
        assert!(!event.is_error());
    }

    #[test]
    fn test_stream_event_is_task_end() {
        let event = StreamEvent::<TestState>::TaskEnd {
            task_id: "task-123".to_string(),
            node_name: "test_node".to_string(),
            duration: Duration::from_millis(100),
        };
        assert!(!event.is_values());
        assert!(!event.is_updates());
        assert!(event.is_task_end());
        assert!(!event.is_error());
    }

    #[test]
    fn test_stream_event_is_error() {
        let event = StreamEvent::<TestState>::Error {
            error: crate::JunctureError::execution("test error"),
        };
        assert!(!event.is_values());
        assert!(!event.is_updates());
        assert!(!event.is_task_end());
        assert!(event.is_error());
    }

    #[test]
    fn test_stream_event_as_values() {
        let state = TestState;
        let event = StreamEvent::<TestState>::Values { state };
        assert!(event.as_values().is_some());
        assert!(event.as_updates().is_none());
    }

    #[test]
    fn test_stream_event_as_updates() {
        let updates = vec![("node_a".to_string(), TestUpdate)];
        let event = StreamEvent::<TestState>::Updates { updates };
        assert!(event.as_values().is_none());
        assert!(event.as_updates().is_some());
    }

    #[test]
    fn test_stream_mode_default() {
        let mode = StreamMode::default();
        assert_eq!(mode, StreamMode::Values);
    }
}

// Rust guideline compliant 2026-05-19
