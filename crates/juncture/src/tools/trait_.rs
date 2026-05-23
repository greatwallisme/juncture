//! Tool trait and related types for LLM function calling

use async_trait::async_trait;
use juncture_core::state::State;
use std::sync::Arc;

use crate::tools::error::ToolError;
use crate::tools::runtime::ToolRuntime;

/// Tool definition for LLM function calling
///
/// Contains the metadata needed to describe a tool to an LLM,
/// including its name, description, and parameter schema.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    /// Unique tool identifier
    pub name: String,

    /// Human-readable tool description
    pub description: String,

    /// JSON Schema for tool input parameters
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Convert to `OpenAI` function call format
    #[must_use]
    pub fn to_openai_format(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
    }

    /// Convert to Anthropic tool format
    #[must_use]
    pub fn to_anthropic_format(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters,
        })
    }
}

/// Core Tool trait for LLM function calling
///
/// Tools represent executable functions that LLMs can invoke during agent execution.
/// Each tool has a name, description, JSON schema for parameters, and execution logic.
///
/// # Example
///
/// ```ignore
/// use async_trait::async_trait;
/// use juncture::tools::{Tool, ToolError};
/// use serde_json::json;
///
/// struct Calculator;
///
/// #[async_trait]
/// impl Tool for Calculator {
///     fn name(&self) -> &str {
///         "calculator"
///     }
///
///     fn description(&self) -> &str {
///         "Performs basic arithmetic operations"
///     }
///
///     fn schema(&self) -> serde_json::Value {
///         json!({
///             "type": "object",
///             "properties": {
///                 "operation": {
///                     "type": "string",
///                     "enum": ["add", "subtract", "multiply", "divide"],
///                 },
///                 "a": {"type": "number"},
///                 "b": {"type": "number"},
///             },
///             "required": ["operation", "a", "b"],
///         })
///     }
///
///     async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
///         // Parse and execute the calculation
///         Ok("42".to_string())
///     }
/// }
/// ```
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Tool name (must be unique within a `ToolNode`)
    fn name(&self) -> &str;

    /// Tool description (shown to LLM)
    ///
    /// This should clearly explain what the tool does and when to use it.
    fn description(&self) -> &str;

    /// JSON Schema for tool input
    ///
    /// Should be a valid JSON Schema describing the expected input structure.
    fn schema(&self) -> serde_json::Value;

    /// Get the full tool definition
    ///
    /// Default implementation combines name, description, and schema.
    #[must_use]
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.schema(),
        }
    }

    /// Whether this tool requires access to the cross-thread persistent store
    ///
    /// Default implementation returns `false`. Tools that need store access
    /// should override this method to return `true`. When `true`, `ToolNode`
    /// will provide the store via [`ToolRuntime`] if available.
    ///
    /// # Note
    ///
    /// For tools that need full runtime access (state, store, config), consider
    /// implementing [`StatefulTool`] instead which receives a complete
    /// [`ToolRuntime`] context.
    #[must_use]
    fn requires_store(&self) -> bool {
        false
    }

    /// Execute the tool
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - Input validation fails
    /// - Tool execution encounters an error
    /// - Execution times out
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError>;
}

/// Blanket implementation for Arc-wrapped tools
#[async_trait]
impl<T: Tool + ?Sized> Tool for Arc<T> {
    fn name(&self) -> &str {
        T::name(self)
    }

    fn description(&self) -> &str {
        T::description(self)
    }

    fn schema(&self) -> serde_json::Value {
        T::schema(self)
    }

    fn requires_store(&self) -> bool {
        T::requires_store(self)
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        T::invoke(self, input).await
    }
}

/// Trait for stateful tools that need access to runtime context
///
/// Unlike [`Tool`] which executes in isolation, `StatefulTool` receives a
/// [`ToolRuntime`] providing access to the current graph state, execution
/// configuration, and an optional cross-thread persistent store.
///
/// This enables tools that:
/// - Read from or write to graph state during execution
/// - Store long-term knowledge across threads via the store
/// - Emit streaming output deltas during execution
///
/// # Type Parameters
///
/// * `S` - The graph state type (must implement [`State`])
///
/// # Example
///
/// ```ignore
/// use juncture::tools::{StatefulTool, ToolRuntime, ToolError};
/// use juncture_core::State;
/// use async_trait::async_trait;
/// use serde_json::json;
///
/// struct ContextAwareSearchTool;
///
/// #[async_trait]
/// impl<S: State + 'static> StatefulTool<S> for ContextAwareSearchTool {
///     async fn invoke_with_runtime(
///         &self,
///         input: serde_json::Value,
///         runtime: &ToolRuntime<S>,
///     ) -> Result<String, ToolError> {
///         // Access the persistent store for cross-thread memory
///         if let Some(store) = runtime.store() {
///             let _item = store.get("namespace", "key").await
///                 .map_err(|e| ToolError::execution_failed(e.to_string()))?;
///         }
///
///         Ok(format!("Searched with config: {:?}", runtime.config()))
///     }
///
///     fn name(&self) -> &'static str {
///         "context_aware_search"
///     }
///
///     fn description(&self) -> &'static str {
///         "Searches using graph state context"
///     }
///
///     fn schema(&self) -> serde_json::Value {
///         json!({"type": "object"})
///     }
/// }
/// ```
#[async_trait]
pub trait StatefulTool<S: State>: Send + Sync + 'static {
    /// Invoke the tool with full runtime context access
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - Input validation fails
    /// - Tool execution encounters an error
    /// - Store operations fail
    async fn invoke_with_runtime(
        &self,
        input: serde_json::Value,
        runtime: &ToolRuntime<S>,
    ) -> Result<String, ToolError>;

    /// Tool name (must be unique within a tool registry)
    fn name(&self) -> &'static str;

    /// Tool description (shown to the LLM)
    fn description(&self) -> &'static str;

    /// JSON Schema for tool input parameters
    fn schema(&self) -> serde_json::Value;

    /// Get the full tool definition
    ///
    /// Default implementation combines name, description, and schema.
    #[must_use]
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.schema(),
        }
    }
}

/// Blanket implementation for Arc-wrapped stateful tools
#[async_trait]
impl<T, S> StatefulTool<S> for Arc<T>
where
    T: StatefulTool<S> + ?Sized,
    S: State,
{
    fn name(&self) -> &'static str {
        T::name(self)
    }

    fn description(&self) -> &'static str {
        T::description(self)
    }

    fn schema(&self) -> serde_json::Value {
        T::schema(self)
    }

    async fn invoke_with_runtime(
        &self,
        input: serde_json::Value,
        runtime: &ToolRuntime<S>,
    ) -> Result<String, ToolError> {
        T::invoke_with_runtime(self, input, runtime).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test tool for unit tests
    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            "test_tool"
        }

        fn description(&self) -> &'static str {
            "A test tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            })
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            Ok(format!("Processed: {input}"))
        }
    }

    #[test]
    fn test_tool_definition_new() {
        let def = ToolDefinition::new("my_tool", "My description", json!({"type": "object"}));

        assert_eq!(def.name, "my_tool");
        assert_eq!(def.description, "My description");
        assert_eq!(def.parameters, json!({"type": "object"}));
    }

    #[test]
    fn test_tool_definition_to_openai_format() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        );

        let openai = def.to_openai_format();
        assert_eq!(openai["type"], "function");
        assert_eq!(openai["function"]["name"], "search");
        assert_eq!(openai["function"]["description"], "Search the web");
        assert_eq!(openai["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn test_tool_definition_to_anthropic_format() {
        let def = ToolDefinition::new("search", "Search the web", json!({"type": "object"}));

        let anthropic = def.to_anthropic_format();
        assert_eq!(anthropic["name"], "search");
        assert_eq!(anthropic["description"], "Search the web");
        assert_eq!(anthropic["input_schema"]["type"], "object");
    }

    #[tokio::test]
    async fn test_tool_trait() {
        let tool = TestTool;

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        let def = tool.definition();
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");

        tool.invoke(json!({"test": "value"})).await.unwrap();
    }

    #[tokio::test]
    async fn test_tool_arc_wrapper() {
        let tool = Arc::new(TestTool);

        assert_eq!(tool.name(), "test_tool");
        assert_eq!(tool.description(), "A test tool");

        tool.invoke(json!({})).await.unwrap();
    }

    #[test]
    fn test_tool_requires_store_default() {
        let tool = TestTool;
        assert!(
            !tool.requires_store(),
            "default requires_store should return false"
        );
    }

    #[test]
    fn test_tool_requires_store_arc_wrapper() {
        let tool = Arc::new(TestTool);
        assert!(
            !tool.requires_store(),
            "Arc-wrapped tool should use default requires_store"
        );
    }

    // --- StatefulTool tests ---

    /// Dummy State for stateful tool tests
    #[derive(Clone, Debug)]
    struct TestState {
        context: String,
    }

    impl juncture_core::State for TestState {
        type Update = TestStateUpdate;
        type FieldVersions = juncture_core::state::FieldVersions;

        fn apply(&mut self, _update: Self::Update) -> juncture_core::state::FieldsChanged {
            juncture_core::state::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct TestStateUpdate;

    /// Test stateful tool that reads runtime context
    struct TestStatefulTool;

    #[async_trait]
    impl StatefulTool<TestState> for TestStatefulTool {
        async fn invoke_with_runtime(
            &self,
            input: serde_json::Value,
            runtime: &ToolRuntime<TestState>,
        ) -> Result<String, ToolError> {
            let context = &runtime.state.context;
            let call_id = runtime.tool_call_id();
            Ok(format!(
                "input={input}, context={context}, call_id={call_id}"
            ))
        }

        fn name(&self) -> &'static str {
            "test_stateful_tool"
        }

        fn description(&self) -> &'static str {
            "A test stateful tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            })
        }
    }

    #[test]
    fn test_stateful_tool_definition() {
        let tool = TestStatefulTool;

        assert_eq!(tool.name(), "test_stateful_tool");
        assert_eq!(tool.description(), "A test stateful tool");

        let def = tool.definition();
        assert_eq!(def.name, "test_stateful_tool");
        assert_eq!(def.description, "A test stateful tool");
        assert_eq!(def.parameters["type"], "object");
    }

    #[tokio::test]
    async fn test_stateful_tool_invoke_with_runtime() {
        let tool = TestStatefulTool;
        let state = TestState {
            context: "search_context".to_string(),
        };
        let config = juncture_core::config::RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_abc".to_string(), config);

        let result = tool
            .invoke_with_runtime(json!({"query": "hello"}), &runtime)
            .await
            .expect("invoke_with_runtime should succeed");

        assert!(result.contains("hello"));
        assert!(result.contains("context=search_context"));
        assert!(result.contains("call_id=call_abc"));
    }

    #[tokio::test]
    async fn test_stateful_tool_arc_wrapper() {
        let tool = Arc::new(TestStatefulTool);
        let state = TestState {
            context: "arc_test".to_string(),
        };
        let config = juncture_core::config::RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_arc".to_string(), config);

        assert_eq!(tool.name(), "test_stateful_tool");
        assert_eq!(tool.description(), "A test stateful tool");

        let result = tool
            .invoke_with_runtime(json!({}), &runtime)
            .await
            .expect("Arc-wrapped invoke_with_runtime should succeed");

        assert!(result.contains("context=arc_test"));
    }

    #[tokio::test]
    async fn test_stateful_tool_error_propagation() {
        struct FailStatefulTool;

        #[async_trait]
        impl StatefulTool<TestState> for FailStatefulTool {
            async fn invoke_with_runtime(
                &self,
                _input: serde_json::Value,
                _runtime: &ToolRuntime<TestState>,
            ) -> Result<String, ToolError> {
                Err(ToolError::execution_failed(
                    "intentional failure".to_string(),
                ))
            }

            fn name(&self) -> &'static str {
                "fail_stateful"
            }

            fn description(&self) -> &'static str {
                "Always fails"
            }

            fn schema(&self) -> serde_json::Value {
                json!({"type": "object"})
            }
        }

        let tool = FailStatefulTool;
        let state = TestState {
            context: String::new(),
        };
        let config = juncture_core::config::RunnableConfig::default();
        let runtime = ToolRuntime::new_without_store(state, "call_err".to_string(), config);

        let result = tool.invoke_with_runtime(json!({}), &runtime).await;
        assert!(result.is_err());
        let err = result.expect_err("should have failed");
        assert!(
            matches!(err, ToolError::ExecutionFailed(ref msg) if msg.contains("intentional failure"))
        );
    }

    #[test]
    fn test_stateful_tool_definition_matches_tool() {
        let stateful = TestStatefulTool;
        let stateless = TestTool;

        let stateful_def = stateful.definition();
        let stateless_def = stateless.definition();

        // Both should produce valid ToolDefinitions with the same structure
        assert_eq!(stateful_def.name, "test_stateful_tool");
        assert_eq!(stateless_def.name, "test_tool");
        assert_eq!(stateful_def.parameters["type"], "object");
        assert_eq!(stateless_def.parameters["type"], "object");
    }
}

// Rust guideline compliant 2026-05-19
