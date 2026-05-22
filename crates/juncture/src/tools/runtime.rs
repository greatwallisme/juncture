//! Tool runtime context for stateful tool execution

use std::sync::Arc;

use juncture_core::{config::RunnableConfig, state::State, store::Store};

/// Runtime context for stateful tool execution
///
/// Provides tools with access to execution context including
/// the current state, tool call metadata, configuration, and
/// optional cross-thread persistent store.
///
/// # Type Parameters
///
/// * `S` - The state type (must implement [`State`])
///
/// # Example
///
/// ```ignore
/// use juncture::tools::{StatefulTool, ToolRuntime};
/// use juncture_core::{State, RunnableConfig};
/// use async_trait::async_trait;
///
/// struct MyStatefulTool;
///
/// #[async_trait]
/// impl<S: State + 'static> StatefulTool<S> for MyStatefulTool {
///     async fn invoke_with_runtime(
///         &self,
///         input: serde_json::Value,
///         runtime: &ToolRuntime<S>,
///     ) -> Result<String, ToolError> {
///         // Access state
///         let state = &runtime.state;
///         let config = runtime.config();
///
///         // Access store if available
///         if let Some(store) = runtime.store() {
///             let item = store.get("namespace", "key").await?;
///         }
///
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

    /// Optional cross-thread persistent store for long-term memory
    pub store: Option<Arc<dyn Store>>,
}

impl<S: State> ToolRuntime<S> {
    /// Create a new tool runtime with all fields
    #[must_use]
    pub fn new(
        state: S,
        tool_call_id: String,
        config: RunnableConfig,
        store: Option<Arc<dyn Store>>,
    ) -> Self {
        Self {
            state,
            tool_call_id,
            config,
            store,
        }
    }

    /// Create a new tool runtime without a store
    #[must_use]
    pub fn new_without_store(state: S, tool_call_id: String, config: RunnableConfig) -> Self {
        Self {
            state,
            tool_call_id,
            config,
            store: None,
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

    /// Get the optional store
    #[must_use]
    pub const fn store(&self) -> Option<&Arc<dyn Store>> {
        self.store.as_ref()
    }

    /// Emit an incremental output delta during tool execution.
    ///
    /// Sends a partial result chunk for streaming tool output observation.
    /// Currently logs the delta at debug level; will be connected to the
    /// graph's event stream via `StreamEvent::Tools(ToolsEvent::ToolOutputDelta)`
    /// when tool streaming is fully integrated with the Pregel execution
    /// pipeline.
    pub fn emit_output_delta(&self, delta: &str) {
        tracing::debug!(
            tool_call_id = %self.tool_call_id,
            delta_len = delta.len(),
            "tool output delta emitted"
        );
    }
}

impl<S: State> Clone for ToolRuntime<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            tool_call_id: self.tool_call_id.clone(),
            config: self.config.clone(),
            store: self.store.clone(),
        }
    }
}

impl<S: State> std::fmt::Debug for ToolRuntime<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRuntime")
            .field("tool_call_id", &self.tool_call_id)
            .field("config", &self.config)
            .field(
                "store",
                &self.store.as_ref().map_or("None", |_| "Some(...)"),
            )
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
        let runtime = ToolRuntime::new_without_store(state, "call_123".to_string(), config);

        assert_eq!(runtime.tool_call_id, "call_123");
    }

    #[test]
    fn test_tool_runtime_accessors() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_123".to_string(), config);

        assert_eq!(runtime.tool_call_id(), "call_123");
    }

    #[test]
    fn test_tool_runtime_clone() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_123".to_string(), config);

        let cloned = runtime.clone();
        assert_eq!(cloned.tool_call_id, runtime.tool_call_id);
    }

    #[test]
    fn test_tool_runtime_debug() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_123".to_string(), config);

        let debug_str = format!("{runtime:?}");
        assert!(debug_str.contains("ToolRuntime"));
        assert!(debug_str.contains("call_123"));
    }

    #[test]
    fn test_emit_output_delta_does_not_panic() {
        let state = TestState;
        let config = RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_456".to_string(), config);

        runtime.emit_output_delta("partial result chunk");
        runtime.emit_output_delta("");
        runtime.emit_output_delta("another delta");
    }
}

// Rust guideline compliant 2026-05-22
