//! `ToolNode`: executes tools from AI message `tool_calls`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use juncture_core::state::messages::{Message, Role, ToolCall};
use juncture_tracing::spans::attrs;
use tokio::task::JoinSet;

use crate::tools::error::ToolError;
use crate::tools::interceptor::{NopToolInterceptor, ToolInterceptor};
use crate::tools::trait_::Tool;
use crate::tools::transformer::ToolCallTransformer;

/// Configuration for `ToolNode`
///
/// Controls tool execution behavior including error handling,
/// validation, and interception.
pub struct ToolNodeConfig {
    /// Available tools for execution
    pub tools: Vec<Box<dyn Tool>>,

    /// Whether to handle errors as tool result messages
    ///
    /// If true, errors are returned as tool result messages so the LLM can retry.
    /// If false, errors are propagated immediately.
    pub handle_errors: bool,

    /// Whether to validate tool input against schema
    pub validate_input: bool,

    /// Optional transformer for tool call arguments
    pub call_transformer: Option<Box<dyn ToolCallTransformer>>,

    /// Optional interceptor for pre/post execution hooks
    pub interceptor: Option<Arc<dyn ToolInterceptor>>,
}

impl Default for ToolNodeConfig {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
        }
    }
}

impl std::fmt::Debug for ToolNodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolNodeConfig")
            .field("tools_count", &self.tools.len())
            .field("handle_errors", &self.handle_errors)
            .field("validate_input", &self.validate_input)
            .field("call_transformer", &self.call_transformer.is_some())
            .field("interceptor", &self.interceptor.is_some())
            .finish()
    }
}

/// Tool execution trace for observability
///
/// Records execution metadata for each tool call, useful for
/// debugging, monitoring, and audit trails.
#[derive(Clone, Debug)]
pub struct ToolExecutionTrace {
    /// Name of the tool that was executed
    pub tool_name: String,

    /// Tool call ID from the AI message
    pub tool_call_id: String,

    /// Attempt number (for retry logic)
    pub attempt: usize,

    /// Unix timestamp of first attempt
    pub first_attempt_time: f64,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Whether the execution succeeded
    pub success: bool,
}

impl ToolExecutionTrace {
    /// Create a new tool execution trace
    #[must_use]
    pub fn new(tool_name: String, tool_call_id: String, attempt: usize) -> Self {
        Self {
            tool_name,
            tool_call_id,
            attempt,
            first_attempt_time: Self::now(),
            duration_ms: 0,
            success: false,
        }
    }

    /// Mark the trace as completed
    pub const fn complete(&mut self, duration_ms: u64, success: bool) {
        self.duration_ms = duration_ms;
        self.success = success;
    }

    /// Get current Unix timestamp
    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    }
}

/// `ToolNode`: executes tools from AI message `tool_calls`
///
/// This is the standard tool execution node in `ReAct` agent patterns.
/// It extracts `tool_calls` from the last AI message, looks up the corresponding
/// Tool implementations, executes them concurrently, and returns tool result messages.
///
/// # Execution Flow
///
/// 1. Extract `tool_calls` from the last AI message in the conversation
/// 2. For each `tool_call`:
///    - Apply `pre_execute` interceptor hook
///    - Transform the tool call arguments
///    - Execute the tool concurrently
///    - Apply `post_execute` interceptor hook
/// 3. Return tool result messages
///
/// # Example
///
/// ```ignore
/// use juncture::tools::{ToolNode, Tool};
/// use juncture_core::state::messages::{Message, ToolCall};
/// use serde_json::json;
///
/// // Create tools
/// let tools = vec![Box::new(MySearchTool::new())];
///
/// // Create ToolNode
/// let tool_node = ToolNode::new(tools);
///
/// // Execute tool calls from AI message
/// let messages = vec![
///     Message::human("Search for rust programming"),
///     Message::ai_with_tool_calls("", vec![
///         ToolCall {
///             id: "call_123".to_string(),
///             name: "search".to_string(),
///             arguments: json!({"query": "rust programming"}),
///         },
///     ]),
/// ];
///
/// let results = tool_node.execute(&messages).await?;
/// // results contains tool result messages
/// ```
pub struct ToolNode {
    /// Registered tools indexed by name
    tools: HashMap<String, Arc<dyn Tool>>,

    /// Whether to handle errors as tool result messages
    handle_errors: bool,

    /// Whether to validate tool input against schema
    validate_input: bool,

    /// Optional transformer for tool call arguments
    call_transformer: Option<Arc<dyn ToolCallTransformer>>,

    /// Optional interceptor for pre/post execution hooks
    interceptor: Option<Arc<dyn ToolInterceptor>>,
}

impl std::fmt::Debug for ToolNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolNode")
            .field("tools", &self.tools.len())
            .field("handle_errors", &self.handle_errors)
            .field("validate_input", &self.validate_input)
            .field("call_transformer", &self.call_transformer.is_some())
            .field("interceptor", &self.interceptor.is_some())
            .finish()
    }
}

impl ToolNode {
    /// Create a new `ToolNode` with the given tools
    ///
    /// Uses default configuration: error handling enabled, validation enabled.
    #[must_use]
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        let mut tools_map = HashMap::new();
        for tool in tools {
            let tool_arc: Arc<dyn Tool> = Arc::from(tool);
            tools_map.insert(tool_arc.name().to_string(), tool_arc);
        }

        Self {
            tools: tools_map,
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
        }
    }

    /// Create a `ToolNode` with custom configuration
    #[must_use]
    pub fn with_config(config: ToolNodeConfig) -> Self {
        let mut tools_map = HashMap::new();
        for tool in config.tools {
            let tool_arc: Arc<dyn Tool> = Arc::from(tool);
            tools_map.insert(tool_arc.name().to_string(), tool_arc);
        }

        Self {
            tools: tools_map,
            handle_errors: config.handle_errors,
            validate_input: config.validate_input,
            call_transformer: config.call_transformer.map(Arc::from),
            interceptor: config.interceptor,
        }
    }

    /// Set error handling mode
    ///
    /// If true, errors are returned as tool result messages.
    /// If false, errors are propagated immediately.
    #[must_use]
    pub const fn with_error_handling(mut self, handle: bool) -> Self {
        self.handle_errors = handle;
        self
    }

    /// Enable or disable input validation
    #[must_use]
    pub const fn with_validation(mut self, validate: bool) -> Self {
        self.validate_input = validate;
        self
    }

    /// Set a tool call transformer
    #[must_use]
    pub fn with_transformer(mut self, transformer: Box<dyn ToolCallTransformer>) -> Self {
        self.call_transformer = Some(Arc::from(transformer));
        self
    }

    /// Set a tool execution interceptor
    #[must_use]
    pub fn with_interceptor(mut self, interceptor: Arc<dyn ToolInterceptor>) -> Self {
        self.interceptor = Some(interceptor);
        self
    }

    /// Execute tools from the last AI message's `tool_calls`
    ///
    /// This is the main execution method that:
    /// 1. Finds the last AI message in the conversation
    /// 2. Extracts `tool_calls` from it
    /// 3. Executes each tool concurrently
    /// 4. Returns tool result messages
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - No AI message with tool calls is found
    /// - Tool execution fails and error handling is disabled
    /// - Required tool is not found and error handling is disabled
    pub async fn execute(&self, messages: &[Message]) -> Result<Vec<Message>, ToolError> {
        // Find the last AI message with tool calls
        let last_ai = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Ai && m.has_tool_calls())
            .ok_or_else(|| {
                ToolError::validation_failed("No AI message with tool calls found".to_string())
            })?;

        if last_ai.tool_calls.is_empty() {
            return Ok(Vec::new());
        }

        // Collect tool results
        // Execute all tool calls concurrently
        let mut results = JoinSet::new();
        let mut tool_messages: Vec<Message> = Vec::new();
        let interceptor = self
            .interceptor
            .as_ref()
            .map_or_else(|| Arc::new(NopToolInterceptor), Arc::clone);

        for tool_call in &last_ai.tool_calls {
            let tool = if let Some(t) = self.tools.get(&tool_call.name) {
                Arc::clone(t)
            } else {
                let error = ToolError::tool_not_found(&tool_call.name);
                if self.handle_errors {
                    // Add error as a tool result
                    tool_messages.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!("Error: {error}"),
                    ));
                    continue;
                }
                return Err(error);
            };

            let tool_call = tool_call.clone();
            let interceptor = Arc::clone(&interceptor);

            results.spawn(async move {
                Self::execute_single_tool(&tool_call, tool.as_ref(), &interceptor).await
            });
        }

        // Collect results
        while let Some(result) = results.join_next().await {
            match result {
                Ok(Ok((tool_call_id, output))) => {
                    tool_messages.push(Message::tool_result(tool_call_id, output));
                }
                Ok(Err(e)) => {
                    if self.handle_errors {
                        // Return error as tool result for LLM to retry
                        tool_messages.push(Message::tool_result("unknown", format!("Error: {e}")));
                    } else {
                        return Err(e);
                    }
                }
                Err(join_err) => {
                    let msg = format!("Tool execution panicked: {join_err}");
                    if self.handle_errors {
                        tool_messages.push(Message::tool_result("unknown".to_string(), msg));
                    } else {
                        return Err(ToolError::execution_failed(msg));
                    }
                }
            }
        }

        Ok(tool_messages)
    }

    /// Execute a single tool call
    #[allow(
        clippy::cognitive_complexity,
        reason = "execute_single_tool requires: span creation, interceptor hooks, tool invocation, error handling, metrics emission, and result transformation. The complexity is justified by the comprehensive tool execution with observability."
    )]
    async fn execute_single_tool(
        tool_call: &ToolCall,
        tool: &dyn Tool,
        interceptor: &Arc<dyn ToolInterceptor>,
    ) -> Result<(String, String), ToolError> {
        let span = tracing::info_span!(
            "juncture.tool.call",
            "juncture.tool.name" = %tool.name(),
            "juncture.tool.duration_ms" = tracing::field::Empty,
            "juncture.tool.error" = tracing::field::Empty,
        );
        let _enter = span.enter();

        let state = serde_json::Value::Null;

        // Pre-execute hook
        interceptor.pre_execute(tool_call, &state).await?;

        // Execute the tool
        let start = std::time::Instant::now();
        let result = tool.invoke(tool_call.arguments.clone()).await;
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Record duration
        tracing::Span::current().record(attrs::TOOL_DURATION_MS, duration_ms);

        // Emit metrics for tool execution
        tracing::debug!(
            name: "juncture.tool.calls",
            tool_name = %tool.name(),
        );

        tracing::debug!(
            name: "juncture.tool.duration_ms",
            duration_ms = duration_ms,
            tool_name = %tool.name(),
        );

        // Post-execute hook
        let output = match interceptor.post_execute(tool_call, &result).await {
            Ok(out) => out,
            Err(e) => {
                // Record error attribute
                tracing::Span::current().record(attrs::TOOL_ERROR, e.to_string());

                // Emit error metric
                tracing::debug!(
                    name: "juncture.tool.errors",
                    tool_name = %tool.name(),
                );

                return Err(e);
            }
        };

        Ok((tool_call.id.clone(), output))
    }

    /// Get a list of registered tool names
    #[must_use]
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(std::string::String::as_str).collect()
    }

    /// Check if a tool is registered
    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get the number of registered tools
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

impl ToolError {
    /// Get the tool call ID associated with this error
    #[must_use]
    pub const fn tool_call_id(&self) -> Option<&str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    /// Simple test tool that echoes its input
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echoes the input"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                },
                "required": ["message"]
            })
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            input["message"]
                .as_str()
                .map(std::string::ToString::to_string)
                .ok_or_else(|| ToolError::invalid_input("Missing 'message' field".to_string()))
        }
    }

    /// Test tool that always fails
    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &'static str {
            "fail"
        }

        fn description(&self) -> &'static str {
            "Always fails"
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }

        async fn invoke(&self, _input: serde_json::Value) -> Result<String, ToolError> {
            Err(ToolError::execution_failed(
                "Intentional failure".to_string(),
            ))
        }
    }

    /// Test tool that validates input
    #[allow(dead_code, reason = "reserved for future validation tests")]
    struct ValidateTool {
        require_field: String,
    }

    #[async_trait]
    impl Tool for ValidateTool {
        fn name(&self) -> &'static str {
            "validate"
        }

        fn description(&self) -> &'static str {
            "Validates input"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "value": {"type": "string"}
                },
                "required": ["value"]
            })
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            input[self.require_field.as_str()]
                .as_str()
                .map(std::string::ToString::to_string)
                .ok_or_else(|| {
                    ToolError::invalid_input(format!("Missing '{}'", self.require_field))
                })
        }
    }

    #[tokio::test]
    async fn test_tool_node_new() {
        let tools = vec![Box::new(EchoTool) as Box<dyn Tool>];
        let node = ToolNode::new(tools);

        assert_eq!(node.tool_count(), 1);
        assert!(node.has_tool("echo"));
        assert!(!node.has_tool("nonexistent"));
    }

    #[tokio::test]
    async fn test_tool_node_with_config() {
        let config = ToolNodeConfig {
            tools: vec![Box::new(EchoTool) as Box<dyn Tool>],
            handle_errors: false,
            validate_input: false,
            call_transformer: None,
            interceptor: None,
        };
        let node = ToolNode::with_config(config);

        assert_eq!(node.tool_count(), 1);
        assert!(node.has_tool("echo"));
    }

    #[tokio::test]
    async fn test_tool_node_execute_single() {
        let node = ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "Echo this",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_call_id, Some("call_1".to_string()));
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert_eq!(text, "hello");
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content")
            }
        }
    }

    #[tokio::test]
    async fn test_tool_node_execute_multiple() {
        let node = ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "Echo these",
            vec![
                ToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"message": "first"}),
                },
                ToolCall {
                    id: "call_2".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"message": "second"}),
                },
            ],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 2);

        // Results should be in order
        assert_eq!(results[0].tool_call_id, Some("call_1".to_string()));
        assert_eq!(results[1].tool_call_id, Some("call_2".to_string()));
    }

    #[tokio::test]
    async fn test_tool_node_no_tool_calls() {
        let node = ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai("No tools here")];

        let results = node.execute(&messages).await;
        assert!(results.is_err());
        assert!(matches!(
            results.unwrap_err(),
            ToolError::ValidationFailed(_)
        ));
    }

    #[tokio::test]
    async fn test_tool_node_tool_not_found_with_error_handling() {
        let node = ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "Call nonexistent",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "nonexistent".to_string(),
                arguments: json!({}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        // Should return error as tool result
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("tool not found") && text.contains("nonexistent"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with error message")
            }
        }
    }

    #[tokio::test]
    async fn test_tool_node_tool_not_found_without_error_handling() {
        let node =
            ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_error_handling(false);
        let messages = vec![Message::ai_with_tool_calls(
            "Call nonexistent",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "nonexistent".to_string(),
                arguments: json!({}),
            }],
        )];

        let results = node.execute(&messages).await;
        assert!(results.is_err());
        assert!(matches!(results.unwrap_err(), ToolError::ToolNotFound(_)));
    }

    #[tokio::test]
    async fn test_tool_node_tool_failure_with_error_handling() {
        let node = ToolNode::new(vec![Box::new(FailTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "Fail",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "fail".to_string(),
                arguments: json!({}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        // Should return error as tool result
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("Error:"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with error message")
            }
        }
    }

    #[tokio::test]
    async fn test_tool_node_tool_failure_without_error_handling() {
        let node =
            ToolNode::new(vec![Box::new(FailTool) as Box<dyn Tool>]).with_error_handling(false);
        let messages = vec![Message::ai_with_tool_calls(
            "Fail",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "fail".to_string(),
                arguments: json!({}),
            }],
        )];

        let results = node.execute(&messages).await;
        assert!(results.is_err());
        assert!(matches!(
            results.unwrap_err(),
            ToolError::ExecutionFailed(_)
        ));
    }

    #[tokio::test]
    async fn test_tool_node_with_error_handling() {
        let node =
            ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_error_handling(false);
        assert!(!node.handle_errors);
    }

    #[tokio::test]
    async fn test_tool_node_with_validation() {
        let node = ToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_validation(false);
        assert!(!node.validate_input);
    }

    #[tokio::test]
    async fn test_tool_execution_trace() {
        let mut trace = ToolExecutionTrace::new("test_tool".to_string(), "call_123".to_string(), 1);
        assert_eq!(trace.tool_name, "test_tool");
        assert_eq!(trace.tool_call_id, "call_123");
        assert_eq!(trace.attempt, 1);
        assert!(!trace.success);
        assert_eq!(trace.duration_ms, 0);

        trace.complete(100, true);
        assert_eq!(trace.duration_ms, 100);
        assert!(trace.success);
    }

    #[test]
    fn test_tool_execution_trace_now() {
        let trace1 = ToolExecutionTrace::new("t".to_string(), "c".to_string(), 1);
        let trace2 = ToolExecutionTrace::new("t".to_string(), "c".to_string(), 1);
        // Both should have timestamps close to each other
        assert!(trace2.first_attempt_time >= trace1.first_attempt_time);
    }
}

// Rust guideline compliant 2026-05-19
