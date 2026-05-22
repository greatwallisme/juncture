//! Tool runtime context for stateful tool execution

use juncture_core::{config::RunnableConfig, state::State};

/// Runtime context for tool execution
///
/// Provides tools with access to execution context including
/// the current state, tool call metadata, and configuration.
///
/// # Type Parameters
///
/// * `S` - The state type (must implement [`State`])
///
/// # Example
///
/// ```ignore
/// use juncture::tools::ToolRuntime;
/// use juncture_core::{State, RunnableConfig};
/// use async_trait::async_trait;
///
/// struct MyTool;
///
/// #[async_trait]
/// impl Tool for MyTool {
///     async fn invoke(
///         &self,
///         input: serde_json::Value,
///         runtime: &ToolRuntime<MyState>,
///     ) -> Result<String, ToolError> {
///         // Access state
///         let config = &runtime.config;
///         let tool_call_id = &runtime.tool_call_id;
///
///         // Execute tool logic...
///         Ok("Result".to_string())
///     }
/// }
/// ```
pub struct ToolRuntime<S: State> {
    /// Current state snapshot
    pub state: S,

    /// Tool call ID being executed
    pub tool_call_id: String,

    /// Execution configuration
    pub config: RunnableConfig,
}

impl<S: State> ToolRuntime<S> {
    /// Create a new tool runtime
    #[must_use]
    pub const fn new(state: S, tool_call_id: String, config: RunnableConfig) -> Self {
        Self {
            state,
            tool_call_id,
            config,
        }
    }

    /// Get a reference to the state
    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    /// Get the tool call ID
    #[must_use]
    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }

    /// Get the configuration
    #[must_use]
    pub const fn config(&self) -> &RunnableConfig {
        &self.config
    }
}

impl<S: State> Clone for ToolRuntime<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            tool_call_id: self.tool_call_id.clone(),
            config: self.config.clone(),
        }
    }
}

impl<S: State> std::fmt::Debug for ToolRuntime<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRuntime")
            .field("tool_call_id", &self.tool_call_id)
            .field("config", &self.config)
            .field("state", &std::any::type_name::<S>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use juncture_core::state::FieldsChanged;

    // Dummy State for testing
    #[derive(Clone, Debug)]
    struct TestState;

    impl juncture_core::State for TestState {
        type Update = TestStateUpdate;

        fn apply(&mut self, _update: Self::Update) -> FieldsChanged {
            FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct TestStateUpdate;

    #[test]
    fn test_tool_runtime_new() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new(state, "call_123".to_string(), config);

        assert_eq!(runtime.tool_call_id, "call_123");
    }

    #[test]
    fn test_tool_runtime_accessors() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new(state, "call_123".to_string(), config);

        assert_eq!(runtime.tool_call_id(), "call_123");
    }

    #[test]
    fn test_tool_runtime_clone() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new(state, "call_123".to_string(), config);

        let cloned = runtime.clone();
        assert_eq!(cloned.tool_call_id, runtime.tool_call_id);
    }

    #[test]
    fn test_tool_runtime_debug() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new(state, "call_123".to_string(), config);

        let debug_str = format!("{runtime:?}");
        assert!(debug_str.contains("ToolRuntime"));
        assert!(debug_str.contains("call_123"));
    }
}

// Rust guideline compliant 2026-05-19
