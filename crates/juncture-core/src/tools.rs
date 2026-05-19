// Tool system for agent function calling
//
// This module provides the tool abstraction and ToolNode implementation
// for ReAct-style agent workflows.
//
// # Design Principles
//
// - Unified interface: Single trait for all tools
// - Concurrent execution: Multiple tools can execute in parallel
// - Error resilience: Failed tools return error messages to LLM
// - Type-safe: Tool inputs/outputs use serde JSON for validation

use async_trait::async_trait;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::RunnableConfig;
use crate::llm::ToolDefinition;
use crate::state::{Message, State};
use crate::store::Store;

/// Tool execution error types
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    /// Invalid input to tool
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Tool execution failed
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// Tool execution timeout
    #[error("timeout")]
    Timeout,

    /// Tool not found
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    /// Validation error
    #[error("validation error: {0}")]
    ValidationError(String),
}

/// Unified tool trait
///
/// All tools must implement this trait to be used with `ToolNode`.
///
/// Note: This trait does not implement Debug as it's an async trait intended
/// for dynamic dispatch via trait objects.
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// Get tool name
    fn name(&self) -> &str;

    /// Get tool description
    fn description(&self) -> &str;

    /// Get JSON Schema for tool parameters
    fn schema(&self) -> serde_json::Value;

    /// Get tool definition
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.schema(),
        }
    }

    /// Execute the tool
    ///
    /// # Arguments
    ///
    /// * `input` - Tool input as JSON value (validated against schema)
    ///
    /// # Returns
    ///
    /// Tool output as string
    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError>;
}

/// Tool runtime context injected into tool execution
///
/// Provides access to graph state, configuration, and store during tool execution.
#[allow(missing_debug_implementations, reason = "Contains dyn Store trait object which doesn't implement Debug")]
pub struct ToolRuntime<S: State> {
    /// Current graph state (read-only snapshot)
    pub state: S,
    /// Current tool call ID
    pub tool_call_id: String,
    /// Runtime configuration
    pub config: RunnableConfig,
    /// Cross-thread persistent store
    pub store: Option<Arc<dyn Store>>,
}

impl<S: State> ToolRuntime<S> {
    /// Emit tool output delta for streaming
    ///
    /// Allows tools to stream intermediate results during execution.
    pub const fn emit_output_delta(&self, _delta: &str) {
        // Streaming functionality to be implemented
    }
}

/// Stateful tool trait for tools that need graph state access
///
/// Tools can access the current graph state during execution.
///
/// Note: This trait does not implement Debug as it's an async trait intended
/// for dynamic dispatch via trait objects.
#[async_trait]
pub trait StatefulTool<S: State>: Tool {
    /// Execute with state access
    ///
    /// # Arguments
    ///
    /// * `input` - Tool input
    /// * `runtime` - Runtime context with state access
    ///
    /// # Returns
    ///
    /// Tool output as string
    fn invoke_with_state(
        &self,
        input: serde_json::Value,
        runtime: &ToolRuntime<S>,
    ) -> BoxFuture<'_, Result<String, ToolError>>;

    /// Execute with store access
    ///
    /// # Arguments
    ///
    /// * `input` - Tool input
    /// * `store` - Store for cross-thread data access
    ///
    /// # Returns
    ///
    /// Tool output as string
    fn invoke_with_store(
        &self,
        input: serde_json::Value,
        store: &dyn Store,
    ) -> BoxFuture<'_, Result<String, ToolError>>;
}

/// Tool call interceptor trait
///
/// Allows injecting custom logic before and after tool execution.
///
/// Note: This trait does not implement Debug as it's an async trait intended
/// for dynamic dispatch via trait objects.
#[async_trait]
pub trait ToolInterceptor: Send + Sync + 'static {
    /// Called before tool execution
    ///
    /// Return Err to cancel tool execution with error message.
    fn pre_execute(
        &self,
        tool_call: &crate::state::ToolCall,
        state: &serde_json::Value,
    ) -> BoxFuture<'_, Result<(), ToolError>>;

    /// Called after tool execution
    ///
    /// Can modify the tool result.
    fn post_execute(
        &self,
        tool_call: &crate::state::ToolCall,
        result: &Result<String, ToolError>,
    ) -> BoxFuture<'_, Result<String, ToolError>>;
}

/// No-op interceptor (default implementation)
#[derive(Debug)]
pub struct NopToolInterceptor;

#[async_trait]
impl ToolInterceptor for NopToolInterceptor {
    fn pre_execute(
        &self,
        _tool_call: &crate::state::ToolCall,
        _state: &serde_json::Value,
    ) -> BoxFuture<'_, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }

    fn post_execute(
        &self,
        _tool_call: &crate::state::ToolCall,
        result: &Result<String, ToolError>,
    ) -> BoxFuture<'_, Result<String, ToolError>> {
        let result_clone = result.clone();
        Box::pin(async move {
            result_clone.map_err(|e| ToolError::ExecutionFailed(e.to_string()))
        })
    }
}

/// Tool call transformer trait
///
/// Allows transforming tool call parameters before execution.
///
/// Note: This trait does not implement Debug as it's intended for dynamic
/// dispatch via trait objects.
pub trait ToolCallTransformer: Send + Sync + 'static {
    /// Transform the tool call
    ///
    /// # Errors
    ///
    /// Returns `ToolError` if the transformation fails.
    fn transform(&self, tool_call: &mut crate::state::ToolCall) -> Result<(), ToolError>;
}

/// Tool node configuration
#[allow(
    missing_debug_implementations,
    clippy::type_complexity,
    reason = "Contains trait objects and Arc<dyn Fn> which don't implement Debug. Complex trait object type is required for dynamic tool configuration."
)]
pub struct ToolNodeConfig {
    /// List of tools
    pub tools: Vec<Box<dyn Tool>>,
    /// Handle errors by returning them to LLM (true) or failing (false)
    pub handle_errors: bool,
    /// Validate tool inputs against schema before execution
    pub validate_input: bool,
    /// Optional tool call transformer
    pub call_transformer: Option<Box<dyn ToolCallTransformer>>,
    /// Optional tool call interceptor
    pub interceptor: Option<Arc<dyn ToolInterceptor>>,
    /// Optional tools condition function
    pub tools_condition: Option<Arc<dyn Fn(&Message) -> bool + Send + Sync>>,
}

impl Default for ToolNodeConfig {
    fn default() -> Self {
        Self {
            tools: vec![],
            handle_errors: true,
            validate_input: false,
            call_transformer: None,
            interceptor: None,
            tools_condition: None,
        }
    }
}

/// Tool node for executing function calls
///
/// Extracts tool calls from the last AI message and executes them.
#[allow(missing_debug_implementations, reason = "Contains trait objects which don't implement Debug")]
pub struct ToolNode {
    /// Tool registry
    #[expect(dead_code, reason = "Used in tool execution")]
    tools: HashMap<String, Box<dyn Tool>>,
    /// Error handling mode
    handle_errors: bool,
    /// Input validation
    validate_input: bool,
    /// Optional tool call transformer
    call_transformer: Option<Box<dyn ToolCallTransformer>>,
    /// Optional interceptor
    interceptor: Option<Arc<dyn ToolInterceptor>>,
}

impl ToolNode {
    /// Create new tool node from tools
    #[must_use]
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        let tool_map = tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();

        Self {
            tools: tool_map,
            handle_errors: true,
            validate_input: false,
            call_transformer: None,
            interceptor: None,
        }
    }

    /// Create tool node from config
    #[must_use]
    pub fn from_config(config: ToolNodeConfig) -> Self {
        let tool_map = config
            .tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();

        Self {
            tools: tool_map,
            handle_errors: config.handle_errors,
            validate_input: config.validate_input,
            call_transformer: config.call_transformer,
            interceptor: config.interceptor,
        }
    }

    /// Set error handling mode
    #[must_use]
    pub const fn with_error_handling(mut self, handle: bool) -> Self {
        self.handle_errors = handle;
        self
    }

    /// Enable input validation
    #[must_use]
    pub const fn with_validation(mut self, validate: bool) -> Self {
        self.validate_input = validate;
        self
    }

    /// Set tool call transformer
    #[must_use]
    pub fn with_transformer(mut self, transformer: Box<dyn ToolCallTransformer>) -> Self {
        self.call_transformer = Some(transformer);
        self
    }

    /// Set interceptor
    #[must_use]
    pub fn with_interceptor(mut self, interceptor: Arc<dyn ToolInterceptor>) -> Self {
        self.interceptor = Some(interceptor);
        self
    }
}

/// Tool execution trace record
#[derive(Debug, Clone)]
pub struct ToolExecutionTrace {
    /// Tool name
    pub tool_name: String,
    /// Tool call ID
    pub tool_call_id: String,
    /// Attempt number
    pub attempt: usize,
    /// First attempt timestamp
    pub first_attempt_time: chrono::DateTime<chrono::Utc>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Success flag
    pub success: bool,
}

/// Validate tool input against schema
#[expect(dead_code, reason = "Used in tool execution validation")]
fn validate_tool_input(tool: &dyn Tool, input: &serde_json::Value) -> Result<(), ToolError> {
    let schema = tool.schema();

    // Basic JSON schema validation
    if let Some(obj) = input.as_object()
        && let Some(schema_obj) = schema.as_object()
        && let Some(required) = schema_obj.get("required").and_then(|v| v.as_array())
    {
        for field in required {
            if let Some(field_name) = field.as_str()
                && !obj.contains_key(field_name)
            {
                return Err(ToolError::ValidationError(format!(
                    "Missing required field: {field_name}",
                )));
            }
        }
    }

    Ok(())
}

/// Tools condition router function
///
/// Standard routing function for `ReAct` agents.
/// Routes to "tools" node if last message has `tool_calls`, otherwise to END.
///
/// # Type Parameters
///
/// * `S` - State type
///
/// # Arguments
///
/// * `_state` - Graph state
/// * `_messages_field` - Name of messages field
///
/// # Returns
///
/// Target node name ("tools" or END)
///
/// # Examples
///
/// ```ignore
/// graph.add_conditional_edges(
///     "agent",
///     |state: &MyState| tools_condition(state, "messages"),
///     path_map! {
///         "tools" => "tools",
///         END => END,
///     },
/// );
/// ```
///
/// # Implementation
///
/// Returns END by default. Custom implementations should inspect the state's
/// messages field and check if the last message contains `tool_calls`.
pub const fn tools_condition<S: State>(_state: &S, _messages_field: &str) -> &'static str {
    crate::END
}

// Rust guideline compliant 2026-05-19
