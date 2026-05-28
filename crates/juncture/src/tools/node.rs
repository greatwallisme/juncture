//! `ToolNode`: executes tools from AI message `tool_calls`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use juncture_core::state::State;
use juncture_core::state::messages::{Message, Role, ToolCall};
use juncture_core::stream::ToolsEvent;
use juncture_tracing::spans::attrs;
use tokio::task::JoinSet;

use crate::tools::error::ToolError;
use crate::tools::interceptor::{NopToolInterceptor, ToolInterceptor};
use crate::tools::runtime::ToolRuntime;
use crate::tools::trait_::{StatefulTool, Tool, ToolDefinition};
use crate::tools::transformer::ToolCallTransformer;

/// Type alias for the tools condition function to reduce type complexity
type ToolsConditionFn = Arc<dyn Fn(&Message) -> bool + Send + Sync>;

/// Configuration for `ToolNode`
///
/// Controls tool execution behavior including error handling,
/// validation, and interception.
pub struct ToolNodeConfig<S: State> {
    /// Available tools for execution
    pub tools: Vec<ToolEntry<S>>,

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

    /// Optional condition function to determine if tools should be executed
    ///
    /// If set, this function is called with the AI message containing tool calls.
    /// Returns true to execute tools, false to skip tool execution.
    /// Used for implementing `tools_condition` routing pattern.
    pub tools_condition: Option<ToolsConditionFn>,
}

/// A wrapper that can hold either a stateless or stateful tool.
///
/// This enum enables `ToolNode` to store and dispatch to both types of tools:
/// - Stateless tools implement only `Tool` trait
/// - Stateful tools implement `StatefulTool<S>` and can access graph state
#[derive(Clone)]
pub enum ToolEntry<S: State> {
    /// A stateless tool that implements only `Tool`
    Stateless(Arc<dyn Tool>),

    /// A stateful tool that implements `StatefulTool<S>` with runtime access
    Stateful(Arc<dyn StatefulTool<S>>),
}

impl<S: State> std::fmt::Debug for ToolEntry<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stateless(_) => f.debug_tuple("Stateless").field(&self.name()).finish(),
            Self::Stateful(_) => f.debug_tuple("Stateful").field(&self.name()).finish(),
        }
    }
}

impl<S: State> ToolEntry<S> {
    /// Get the tool name
    pub fn name(&self) -> &str {
        match self {
            Self::Stateless(tool) => tool.name(),
            Self::Stateful(tool) => tool.name(),
        }
    }

    /// Get the tool description
    pub fn description(&self) -> &str {
        match self {
            Self::Stateless(tool) => tool.description(),
            Self::Stateful(tool) => tool.description(),
        }
    }

    /// Get the tool's JSON schema
    pub fn schema(&self) -> serde_json::Value {
        match self {
            Self::Stateless(tool) => tool.schema(),
            Self::Stateful(tool) => tool.schema(),
        }
    }

    /// Get the tool definition
    pub fn definition(&self) -> ToolDefinition {
        match self {
            Self::Stateless(tool) => tool.definition(),
            Self::Stateful(tool) => tool.definition(),
        }
    }

    /// Create a stateless tool entry from a boxed Tool
    #[must_use]
    pub fn from_stateless(tool: Box<dyn Tool>) -> Self {
        Self::Stateless(Arc::from(tool))
    }

    /// Create a stateful tool entry from an Arc<StatefulTool>
    #[must_use]
    pub fn from_stateful(tool: Arc<dyn StatefulTool<S>>) -> Self {
        Self::Stateful(tool)
    }
}

impl<S: State> Default for ToolNodeConfig<S> {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
            tools_condition: None,
        }
    }
}

impl<S: State> std::fmt::Debug for ToolNodeConfig<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolNodeConfig")
            .field("tools_count", &self.tools.len())
            .field("handle_errors", &self.handle_errors)
            .field("validate_input", &self.validate_input)
            .field("call_transformer", &self.call_transformer.is_some())
            .field("interceptor", &self.interceptor.is_some())
            .field("tools_condition", &self.tools_condition.is_some())
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

    /// Tool input arguments at time of execution
    pub input: serde_json::Value,

    /// Tool output on success
    pub output: Option<String>,

    /// Error message on failure
    pub error: Option<String>,
}

impl ToolExecutionTrace {
    /// Create a new tool execution trace
    #[must_use]
    pub fn new(
        tool_name: String,
        tool_call_id: String,
        attempt: usize,
        input: serde_json::Value,
    ) -> Self {
        Self {
            tool_name,
            tool_call_id,
            attempt,
            first_attempt_time: Self::now(),
            duration_ms: 0,
            success: false,
            input,
            output: None,
            error: None,
        }
    }

    /// Mark the trace as completed with duration, success status, and optional output/error
    pub fn complete(
        &mut self,
        duration_ms: u64,
        success: bool,
        output: Option<String>,
        error: Option<String>,
    ) {
        self.duration_ms = duration_ms;
        self.success = success;
        self.output = output;
        self.error = error;
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
/// # Type Parameters
///
/// * `S` - The state type (must implement [`State`])
///
/// # Execution Flow
///
/// 1. Extract `tool_calls` from the last AI message in the conversation
/// 2. For each `tool_call`:
///    - Apply `pre_execute` interceptor hook
///    - Transform the tool call arguments
///    - Execute the tool concurrently (with state access for stateful tools)
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
pub struct ToolNode<S: State> {
    /// Registered tools indexed by name
    tools: HashMap<String, ToolEntry<S>>,

    /// Whether to handle errors as tool result messages
    handle_errors: bool,

    /// Whether to validate tool input against schema
    validate_input: bool,

    /// Optional transformer for tool call arguments
    call_transformer: Option<Arc<dyn ToolCallTransformer>>,

    /// Optional interceptor for pre/post execution hooks
    interceptor: Option<Arc<dyn ToolInterceptor>>,

    /// Optional condition function to determine if tools should be executed
    ///
    /// If set, this function is called with the AI message containing tool calls.
    /// Returns true to execute tools, false to skip tool execution.
    tools_condition: Option<ToolsConditionFn>,

    /// Optional sender for tool lifecycle streaming events.
    ///
    /// When set, [`ToolStarted`](ToolsEvent::ToolStarted) and
    /// [`ToolFinished`](ToolsEvent::ToolFinished) events are emitted
    /// during tool execution.
    tools_event_tx: Option<tokio::sync::mpsc::UnboundedSender<ToolsEvent>>,
}

impl<S: State> std::fmt::Debug for ToolNode<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolNode")
            .field("tools", &self.tools.len())
            .field("handle_errors", &self.handle_errors)
            .field("validate_input", &self.validate_input)
            .field("call_transformer", &self.call_transformer.is_some())
            .field("interceptor", &self.interceptor.is_some())
            .field("tools_condition", &self.tools_condition.is_some())
            .field("tools_event_tx", &self.tools_event_tx.is_some())
            .finish()
    }
}

// Generic implementation for any State type
impl<S: State> ToolNode<S> {
    /// Create a new `ToolNode` with stateless tools (backward compatible)
    ///
    /// Uses default configuration: error handling enabled, validation enabled.
    ///
    /// This method accepts stateless tools for backward compatibility.
    /// For stateful tools, use [`ToolNode::with_stateful_tools`] instead.
    #[must_use]
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        let mut tools_map = HashMap::new();
        for tool in tools {
            let tool_arc: Arc<dyn Tool> = Arc::from(tool);
            tools_map.insert(tool_arc.name().to_string(), ToolEntry::Stateless(tool_arc));
        }

        Self {
            tools: tools_map,
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
            tools_condition: None,
            tools_event_tx: None,
        }
    }

    /// Create a new `ToolNode` with stateful tools
    ///
    /// Uses default configuration: error handling enabled, validation enabled.
    ///
    /// This method accepts both stateless and stateful tools.
    #[must_use]
    pub fn with_stateful_tools(tools: Vec<ToolEntry<S>>) -> Self {
        let mut tools_map = HashMap::new();
        for tool in tools {
            tools_map.insert(tool.name().to_string(), tool);
        }

        Self {
            tools: tools_map,
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
            tools_condition: None,
            tools_event_tx: None,
        }
    }

    /// Create a `ToolNode` with custom configuration
    #[must_use]
    pub fn with_config(config: ToolNodeConfig<S>) -> Self {
        let mut tools_map = HashMap::new();
        for tool in config.tools {
            tools_map.insert(tool.name().to_string(), tool);
        }

        Self {
            tools: tools_map,
            handle_errors: config.handle_errors,
            validate_input: config.validate_input,
            call_transformer: config.call_transformer.map(Arc::from),
            interceptor: config.interceptor,
            tools_condition: config.tools_condition,
            tools_event_tx: None,
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

    /// Set a tools condition function for conditional tool execution
    ///
    /// If set, this function is called with the AI message containing tool calls.
    /// When the function returns false, tool execution is skipped and an empty
    /// result is returned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::tools::{ToolNode, Tool};
    /// use juncture_core::state::messages::Message;
    /// use std::sync::Arc;
    ///
    /// let tools = vec![Box::new(MySearchTool::new())];
    /// let tool_node = ToolNode::new(tools)
    ///     .with_tools_condition(Arc::new(|msg| {
    ///         // Only execute tools if message contains specific keyword
    ///         msg.content_text().contains("search")
    ///     }));
    /// ```
    #[must_use]
    pub fn with_tools_condition(mut self, condition: ToolsConditionFn) -> Self {
        self.tools_condition = Some(condition);
        self
    }

    /// Attach a tool event sender for streaming lifecycle events.
    ///
    /// When set, [`ToolStarted`](ToolsEvent::ToolStarted) and
    /// [`ToolFinished`](ToolsEvent::ToolFinished) events are emitted
    /// during tool execution.
    #[must_use]
    pub fn with_tools_event_tx(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<ToolsEvent>,
    ) -> Self {
        self.tools_event_tx = Some(tx);
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
        self.execute_with_state(messages, None).await
    }

    /// Execute tools with state access for stateful tools
    ///
    /// This method provides the current state to stateful tools via `ToolRuntime`.
    /// For stateless tools, the state parameter is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] if:
    /// - No AI message with tool calls is found
    /// - Tool execution fails and error handling is disabled
    /// - Required tool is not found and error handling is disabled
    #[allow(
        clippy::too_many_lines,
        reason = "execute_with_state requires: message validation, tools_condition check, tool iteration, concurrent spawning, transformer application, validation, interceptor hooks, and result collection. The complexity is necessary for comprehensive tool execution with state support and conditional execution."
    )]
    pub async fn execute_with_state(
        &self,
        messages: &[Message],
        state: Option<&S>,
    ) -> Result<Vec<Message>, ToolError> {
        // Find the last AI message with tool calls
        let last_ai = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Ai && m.has_tool_calls())
            .ok_or_else(|| {
                ToolError::validation_failed(vec![
                    "No AI message with tool calls found".to_string(),
                ])
            })?;

        if last_ai.tool_calls.is_empty() {
            return Ok(Vec::new());
        }

        // Check tools_condition if set
        if let Some(ref condition) = self.tools_condition
            && !condition(last_ai)
        {
            // Condition returned false, skip tool execution
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
                t.clone()
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

            let mut tool_call = tool_call.clone();

            // Apply transformer if configured
            if let Some(ref transformer) = self.call_transformer
                && let Err(e) = transformer.transform(&mut tool_call)
            {
                if self.handle_errors {
                    tool_messages.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!("Error: {e}"),
                    ));
                    continue;
                }
                return Err(e);
            }

            // Validate input against the tool's JSON schema if enabled
            if self.validate_input
                && let Err(e) = self.validate_tool_call(&tool_call)
            {
                if self.handle_errors {
                    tool_messages.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!("Error: {e}"),
                    ));
                    continue;
                }
                return Err(e);
            }

            let interceptor = Arc::clone(&interceptor);
            let tools_event_tx = self.tools_event_tx.clone();

            // Clone state for stateful tool execution
            let state_clone = state.cloned();

            results.spawn(async move {
                Self::execute_single_tool(
                    &tool_call,
                    &tool,
                    &interceptor,
                    tools_event_tx,
                    state_clone,
                )
                .await
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
        clippy::too_many_lines,
        reason = "execute_single_tool requires: span creation, interceptor hooks, tool invocation (stateless or stateful), error handling, metrics emission, result transformation, and streaming event emission. The complexity is justified by the comprehensive tool execution with observability."
    )]
    async fn execute_single_tool(
        tool_call: &ToolCall,
        tool: &ToolEntry<S>,
        interceptor: &Arc<dyn ToolInterceptor>,
        tools_event_tx: Option<tokio::sync::mpsc::UnboundedSender<ToolsEvent>>,
        state: Option<S>,
    ) -> Result<(String, String), ToolError> {
        let span = tracing::info_span!(
            "juncture.tool.call",
            "juncture.tool.name" = %tool.name(),
            "juncture.tool.duration_ms" = tracing::field::Empty,
            "juncture.tool.error" = tracing::field::Empty,
        );
        let _enter = span.enter();

        // Emit ToolStarted event before execution
        if let Some(ref tx) = tools_event_tx {
            let input = tool_call.arguments.clone();
            let event = ToolsEvent::ToolStarted {
                tool_name: tool.name().to_string(),
                tool_call_id: tool_call.id.clone(),
                node: "tools".to_string(),
                input,
                timestamp: chrono::Utc::now(),
            };
            let _ = tx.send(event);
        }

        let state_json = serde_json::Value::Null;

        // Pre-execute hook
        interceptor.pre_execute(tool_call, &state_json).await?;

        // Create execution trace capturing the tool input at time of execution
        let mut trace = ToolExecutionTrace::new(
            tool.name().to_string(),
            tool_call.id.clone(),
            1,
            tool_call.arguments.clone(),
        );

        // Execute the tool - dispatch based on tool type
        #[cfg(not(target_family = "wasm"))]
        let start = std::time::Instant::now();
        let result = match tool {
            ToolEntry::Stateless(stateless_tool) => {
                // Stateless tool execution
                stateless_tool.invoke(tool_call.arguments.clone()).await
            }
            ToolEntry::Stateful(stateful_tool) => {
                // Stateful tool execution with ToolRuntime
                if let Some(ref state_data) = state {
                    let runtime = ToolRuntime::new(
                        state_data.clone(),
                        tool_call.id.clone(),
                        juncture_core::config::RunnableConfig::default(),
                        None, // Store is not available in this context
                    );
                    stateful_tool
                        .invoke_with_runtime(tool_call.arguments.clone(), &runtime)
                        .await
                } else {
                    // Stateful tool called without state - this is an error
                    return Err(ToolError::execution_failed(format!(
                        "Stateful tool '{}' called without state context",
                        tool.name()
                    )));
                }
            }
        };
        #[cfg(not(target_family = "wasm"))]
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        #[cfg(target_family = "wasm")]
        let duration_ms: u64 = 0;

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

        // Report tool call and duration metrics
        let _ = juncture_core::pregel::try_report_tool_call();
        let _ = juncture_core::pregel::try_report_tool_duration(duration_ms);

        // Post-execute hook
        let output = match interceptor.post_execute(tool_call, &result).await {
            Ok(out) => {
                // Complete trace with success details
                trace.complete(duration_ms, true, Some(out.clone()), None);

                // Emit ToolFinished event with output
                if let Some(ref tx) = tools_event_tx {
                    let output_json = serde_json::json!({"result": out});
                    let event = ToolsEvent::ToolFinished {
                        tool_call_id: tool_call.id.clone(),
                        output: output_json,
                        duration_ms,
                        success: true,
                    };
                    let _ = tx.send(event);
                }

                out
            }
            Err(e) => {
                // Complete trace with failure details
                trace.complete(duration_ms, false, None, Some(e.to_string()));

                // Emit ToolFinished event with error
                if let Some(ref tx) = tools_event_tx {
                    let event = ToolsEvent::ToolFinished {
                        tool_call_id: tool_call.id.clone(),
                        output: serde_json::json!({"error": e.to_string()}),
                        duration_ms,
                        success: false,
                    };
                    let _ = tx.send(event);
                }

                // Log execution trace before returning error
                tracing::debug!(
                    name: "juncture.tool.trace",
                    tool_name = %trace.tool_name,
                    tool_call_id = %trace.tool_call_id,
                    attempt = trace.attempt,
                    duration_ms = trace.duration_ms,
                    success = trace.success,
                );

                // Record error attribute
                tracing::Span::current().record(attrs::TOOL_ERROR, e.to_string());

                // Emit error metric
                tracing::debug!(
                    name: "juncture.tool.errors",
                    tool_name = %tool.name(),
                );

                // Report tool error metric
                let _ = juncture_core::pregel::try_report_tool_error();

                return Err(e);
            }
        };

        // Log execution trace for successful execution
        tracing::debug!(
            name: "juncture.tool.trace",
            tool_name = %trace.tool_name,
            tool_call_id = %trace.tool_call_id,
            attempt = trace.attempt,
            duration_ms = trace.duration_ms,
            success = trace.success,
        );

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

    /// Validate tool call arguments against the registered tool's JSON schema
    ///
    /// Checks that the tool exists and that the arguments conform to its schema.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::ValidationFailed`] if:
    /// - The tool is not registered
    /// - The arguments do not match the tool's JSON schema
    fn validate_tool_call(&self, tool_call: &ToolCall) -> Result<(), ToolError> {
        let tool = self.tools.get(&tool_call.name).ok_or_else(|| {
            ToolError::validation_failed(vec![format!(
                "Tool '{}' not found in registered tools",
                tool_call.name
            )])
        })?;
        Self::validate_arguments_against_schema(&tool_call.arguments, &tool.schema())
    }

    /// Validate JSON arguments against a JSON Schema
    ///
    /// Performs basic structural validation:
    /// - Type checking (object, array, string, number, boolean)
    /// - Required property verification for object schemas
    /// - Property type matching
    fn validate_arguments_against_schema(
        arguments: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> Result<(), ToolError> {
        let Some(schema_obj) = schema.as_object() else {
            return Ok(());
        };

        if schema_obj.is_empty() {
            return Ok(());
        }

        // Check the schema-level type
        let Some(schema_type) = schema.get("type").and_then(serde_json::Value::as_str) else {
            return Ok(());
        };

        match schema_type {
            "object" => Self::validate_object_arguments(arguments, schema)?,
            "array" => {
                if !arguments.is_array() {
                    return Err(ToolError::validation_failed(vec![format!(
                        "Expected array arguments, got '{}'",
                        Self::value_type_name(arguments)
                    )]));
                }
            }
            "string" => {
                if !arguments.is_string() {
                    return Err(ToolError::validation_failed(vec![format!(
                        "Expected string arguments, got '{}'",
                        Self::value_type_name(arguments)
                    )]));
                }
            }
            "number" | "integer" => {
                if !arguments.is_number() {
                    return Err(ToolError::validation_failed(vec![format!(
                        "Expected number arguments, got '{}'",
                        Self::value_type_name(arguments)
                    )]));
                }
            }
            "boolean" => {
                if !arguments.is_boolean() {
                    return Err(ToolError::validation_failed(vec![format!(
                        "Expected boolean arguments, got '{}'",
                        Self::value_type_name(arguments)
                    )]));
                }
            }
            _ => {} // Unknown type, skip validation
        }

        Ok(())
    }

    /// Validate object-type arguments against an object schema
    fn validate_object_arguments(
        arguments: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> Result<(), ToolError> {
        if !arguments.is_object() {
            return Err(ToolError::validation_failed(vec![format!(
                "Expected object arguments, got '{}'",
                Self::value_type_name(arguments)
            )]));
        }

        // Check required fields exist
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            let obj = arguments.as_object().expect("already checked is_object");
            for field in required {
                let field_name = field.as_str().ok_or_else(|| {
                    ToolError::validation_failed(vec![
                        "Invalid schema: required field name is not a string".to_string(),
                    ])
                })?;
                if !obj.contains_key(field_name) {
                    return Err(ToolError::validation_failed(vec![format!(
                        "Missing required field: '{field_name}'"
                    )]));
                }
            }
        }

        // Validate property types if schema defines them
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            let obj = arguments.as_object().expect("already checked is_object");
            for (prop_name, prop_schema) in properties {
                if let Some(arg_val) = obj.get(prop_name) {
                    Self::validate_property_type(arg_val, prop_schema, prop_name)?;
                }
            }
        }

        Ok(())
    }

    /// Validate a single property value against its JSON Schema type definition
    fn validate_property_type(
        value: &serde_json::Value,
        prop_schema: &serde_json::Value,
        prop_name: &str,
    ) -> Result<(), ToolError> {
        let Some(expected_type) = prop_schema.get("type").and_then(serde_json::Value::as_str)
        else {
            return Ok(());
        };

        let matches = match expected_type {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.is_i64() || value.is_u64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true, // Unknown schema type, accept
        };

        if !matches {
            return Err(ToolError::validation_failed(vec![format!(
                "Field '{prop_name}' expected type '{expected_type}', got '{}'",
                Self::value_type_name(value)
            )]));
        }

        Ok(())
    }

    /// Get a human-readable name for a JSON value type
    const fn value_type_name(value: &serde_json::Value) -> &'static str {
        match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
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

    // Type alias for tests using a simple state type
    type TestToolNode = ToolNode<juncture_core::state::messages::MessagesState>;

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

    #[tokio::test]
    async fn test_tool_node_new() {
        let tools = vec![Box::new(EchoTool) as Box<dyn Tool>];
        let node = TestToolNode::new(tools);

        assert_eq!(node.tool_count(), 1);
        assert!(node.has_tool("echo"));
        assert!(!node.has_tool("nonexistent"));
    }

    #[tokio::test]
    async fn test_tool_node_with_config() {
        let config = ToolNodeConfig::<juncture_core::state::messages::MessagesState> {
            tools: vec![ToolEntry::from_stateless(Box::new(EchoTool))],
            handle_errors: false,
            validate_input: false,
            call_transformer: None,
            interceptor: None,
            tools_condition: None,
        };
        let node = TestToolNode::with_config(config);

        assert_eq!(node.tool_count(), 1);
        assert!(node.has_tool("echo"));
    }

    #[tokio::test]
    async fn test_tool_node_execute_single() {
        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
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
        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
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
        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
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
        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]);
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
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_error_handling(false);
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
        let node = TestToolNode::new(vec![Box::new(FailTool) as Box<dyn Tool>]);
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
            TestToolNode::new(vec![Box::new(FailTool) as Box<dyn Tool>]).with_error_handling(false);
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
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_error_handling(false);
        assert!(!node.handle_errors);
    }

    #[tokio::test]
    async fn test_tool_node_with_validation() {
        let node =
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_validation(false);
        assert!(!node.validate_input);
    }

    #[tokio::test]
    async fn test_tool_execution_trace() {
        let mut trace = ToolExecutionTrace::new(
            "test_tool".to_string(),
            "call_123".to_string(),
            1,
            json!({"key": "value"}),
        );
        assert_eq!(trace.tool_name, "test_tool");
        assert_eq!(trace.tool_call_id, "call_123");
        assert_eq!(trace.attempt, 1);
        assert!(!trace.success);
        assert_eq!(trace.duration_ms, 0);
        assert_eq!(trace.input["key"], "value");
        assert!(trace.output.is_none());
        assert!(trace.error.is_none());

        trace.complete(100, true, Some("ok".to_string()), None);
        assert_eq!(trace.duration_ms, 100);
        assert!(trace.success);
        assert_eq!(trace.output, Some("ok".to_string()));
        assert!(trace.error.is_none());
    }

    #[test]
    fn test_tool_execution_trace_now() {
        let trace1 = ToolExecutionTrace::new("t".to_string(), "c".to_string(), 1, json!(null));
        let trace2 = ToolExecutionTrace::new("t".to_string(), "c".to_string(), 1, json!(null));
        // Both should have timestamps close to each other
        assert!(trace2.first_attempt_time >= trace1.first_attempt_time);
    }

    // --- StatefulTool integration tests ---

    /// Test stateful tool that accesses runtime state
    struct StatefulTestTool;

    #[async_trait]
    impl StatefulTool<juncture_core::state::messages::MessagesState> for StatefulTestTool {
        async fn invoke_with_runtime(
            &self,
            _input: serde_json::Value,
            runtime: &ToolRuntime<juncture_core::state::messages::MessagesState>,
        ) -> Result<String, ToolError> {
            let message_count = runtime.state.messages.len();
            Ok(format!("Processed with {message_count} messages in state"))
        }

        fn name(&self) -> &'static str {
            "stateful_test_tool"
        }

        fn description(&self) -> &'static str {
            "A test stateful tool"
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
    }

    #[tokio::test]
    async fn test_stateful_tool_execution() {
        use juncture_core::state::messages::MessagesState;

        // Create a tool node with a stateful tool
        let stateful_entry = ToolEntry::from_stateful(Arc::new(StatefulTestTool));
        let node = ToolNode::<MessagesState>::with_stateful_tools(vec![stateful_entry]);

        // Create test state with messages
        let state = MessagesState {
            messages: vec![Message::human("Hello"), Message::ai("Hi there")],
        };

        // Create tool calls
        let messages = vec![Message::ai_with_tool_calls(
            "Execute stateful tool",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "stateful_test_tool".to_string(),
                arguments: json!({}),
            }],
        )];

        // Execute tools with state
        let results = node
            .execute_with_state(&messages, Some(&state))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        // Verify the stateful tool accessed the state correctly
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("2 messages in state"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content")
            }
        }
    }

    #[tokio::test]
    async fn test_mixed_stateless_and_stateful_tools() {
        use juncture_core::state::messages::MessagesState;

        // Create a tool node with both stateless and stateful tools
        let stateless_entry = ToolEntry::from_stateless(Box::new(EchoTool));
        let stateful_entry = ToolEntry::from_stateful(Arc::new(StatefulTestTool));
        let node =
            ToolNode::<MessagesState>::with_stateful_tools(vec![stateless_entry, stateful_entry]);

        // Create test state
        let state = MessagesState {
            messages: vec![Message::human("Test")],
        };

        // Create tool calls for both tools
        let messages = vec![Message::ai_with_tool_calls(
            "Execute both tools",
            vec![
                ToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"message": "test message"}),
                },
                ToolCall {
                    id: "call_2".to_string(),
                    name: "stateful_test_tool".to_string(),
                    arguments: json!({}),
                },
            ],
        )];

        // Execute tools with state
        let results = node
            .execute_with_state(&messages, Some(&state))
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // Verify both tools executed
        let echo_result = &results[0];
        let stateful_result = &results[1];

        match &echo_result.content {
            juncture_core::state::messages::Content::Text(text) => {
                assert_eq!(text, "test message");
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content")
            }
        }

        match &stateful_result.content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("1 messages in state"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content")
            }
        }
    }

    /// Test tool that returns its input as a JSON string for verification
    struct JsonDumpTool;

    #[async_trait]
    impl Tool for JsonDumpTool {
        fn name(&self) -> &'static str {
            "json_dump"
        }

        fn description(&self) -> &'static str {
            "Returns the input as a JSON string"
        }

        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            Ok(input.to_string())
        }
    }

    /// Transformer that injects a default limit parameter into tool calls
    struct AddDefaultLimit;

    impl ToolCallTransformer for AddDefaultLimit {
        fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
            if let Some(obj) = tool_call.arguments.as_object_mut() {
                obj.entry("limit").or_insert(json!(10));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_tool_node_with_transformer() {
        let node = TestToolNode::new(vec![Box::new(JsonDumpTool) as Box<dyn Tool>])
            .with_transformer(Box::new(AddDefaultLimit));

        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "json_dump".to_string(),
                arguments: json!({"query": "test"}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);

        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                let output: serde_json::Value =
                    serde_json::from_str(text).expect("output should be valid JSON");
                assert_eq!(output["limit"], 10);
                assert_eq!(output["query"], "test");
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with transformed arguments")
            }
        }
    }

    #[tokio::test]
    async fn test_tool_node_with_transformer_error_handling() {
        struct BlockingTransformer;

        impl ToolCallTransformer for BlockingTransformer {
            fn transform(&self, tool_call: &mut ToolCall) -> Result<(), ToolError> {
                Err(ToolError::intercepted(format!(
                    "Transformer blocked '{}'",
                    tool_call.name
                )))
            }
        }

        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>])
            .with_transformer(Box::new(BlockingTransformer));

        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("Error:"));
                assert!(text.contains("Transformer blocked"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with error message")
            }
        }
    }

    #[tokio::test]
    async fn test_tool_node_with_transformer_no_error_handling() {
        struct FatalBlockingTransformer;

        impl ToolCallTransformer for FatalBlockingTransformer {
            fn transform(&self, _tool_call: &mut ToolCall) -> Result<(), ToolError> {
                Err(ToolError::Intercepted("fatal block".to_string()))
            }
        }

        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>])
            .with_transformer(Box::new(FatalBlockingTransformer))
            .with_error_handling(false);

        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let result = node.execute(&messages).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::Intercepted(_)));
    }

    #[tokio::test]
    async fn test_tool_execution_trace_with_fields() {
        let mut trace = ToolExecutionTrace::new(
            "tracker".to_string(),
            "call_42".to_string(),
            1,
            json!({"cmd": "deploy"}),
        );
        assert_eq!(trace.input["cmd"], "deploy");

        // Simulate a successful execution
        trace.complete(250, true, Some("deployed".to_string()), None);
        assert_eq!(trace.duration_ms, 250);
        assert!(trace.success);
        assert_eq!(trace.output, Some("deployed".to_string()));
        assert!(trace.error.is_none());

        // Simulate a failed execution
        let mut err_trace = ToolExecutionTrace::new(
            "tracker".to_string(),
            "call_99".to_string(),
            2,
            json!({"cmd": "fail"}),
        );
        err_trace.complete(50, false, None, Some("timeout".to_string()));
        assert!(!err_trace.success);
        assert!(err_trace.output.is_none());
        assert_eq!(err_trace.error, Some("timeout".to_string()));
    }

    // --- Validation tests ---

    /// Test tool with a defined JSON schema for validation testing
    struct SchemaTool;

    #[async_trait]
    impl Tool for SchemaTool {
        fn name(&self) -> &'static str {
            "schema_tool"
        }

        fn description(&self) -> &'static str {
            "Tool with defined schema for validation"
        }

        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "count": {"type": "integer"},
                    "active": {"type": "boolean"}
                },
                "required": ["name", "count"]
            })
        }

        async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
            Ok(format!("Processed: {input}"))
        }
    }

    #[tokio::test]
    async fn test_validation_valid_input_passes() {
        let node = TestToolNode::new(vec![Box::new(SchemaTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "schema_tool".to_string(),
                arguments: json!({"name": "test", "count": 42, "active": true}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("Processed"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content");
            }
        }
    }

    #[tokio::test]
    async fn test_validation_missing_required_field_rejected() {
        let node = TestToolNode::new(vec![Box::new(SchemaTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "schema_tool".to_string(),
                arguments: json!({"name": "test"}), // missing required "count" field
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("Missing required field"));
                assert!(text.contains("count"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with error message");
            }
        }
    }

    #[tokio::test]
    async fn test_validation_wrong_type_rejected() {
        let node = TestToolNode::new(vec![Box::new(SchemaTool) as Box<dyn Tool>]);
        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "schema_tool".to_string(),
                arguments: json!({"name": "test", "count": "not_a_number"}), // count should be integer
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("expected type"));
                assert!(text.contains("count"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content with error message");
            }
        }
    }

    #[tokio::test]
    async fn test_validation_disabled_skips_checks() {
        let node =
            TestToolNode::new(vec![Box::new(SchemaTool) as Box<dyn Tool>]).with_validation(false);
        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "schema_tool".to_string(),
                arguments: json!({"name": "test"}), // missing required "count" but validation is off
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        // Tool processes the input even though it's missing required fields (validation bypassed)
        match &results[0].content {
            juncture_core::state::messages::Content::Text(text) => {
                assert!(text.contains("Processed"));
            }
            juncture_core::state::messages::Content::MultiPart(_) => {
                panic!("Expected Text content");
            }
        }
    }

    #[tokio::test]
    async fn test_validation_propagates_error_when_not_handled() {
        let node = TestToolNode::new(vec![Box::new(SchemaTool) as Box<dyn Tool>])
            .with_error_handling(false);
        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "schema_tool".to_string(),
                arguments: json!({"name": "test"}), // missing required "count"
            }],
        )];

        let result = node.execute(&messages).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ToolError::ValidationFailed(_)
        ));
    }

    // --- Tool lifecycle streaming event tests ---

    #[tokio::test]
    async fn test_tool_node_emits_started_and_finished_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        #[allow(clippy::redundant_clone, reason = "clarity in test setup")]
        let node =
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_tools_event_tx(tx);

        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let _results = node.execute(&messages).await.unwrap();

        // Collect the events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have ToolStarted and ToolFinished
        assert!(
            events.iter().any(|e| matches!(
                e,
                juncture_core::stream::ToolsEvent::ToolStarted {
                    tool_call_id,
                    ..
                } if tool_call_id == "call_1"
            )),
            "expected ToolStarted event"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                juncture_core::stream::ToolsEvent::ToolFinished {
                    tool_call_id,
                    ..
                } if tool_call_id == "call_1"
            )),
            "expected ToolFinished event"
        );
    }

    #[tokio::test]
    async fn test_tool_node_emits_events_in_order() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        #[allow(clippy::redundant_clone, reason = "clarity in test setup")]
        let node =
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_tools_event_tx(tx);

        let messages = vec![Message::ai_with_tool_calls(
            "test",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let _results = node.execute(&messages).await.unwrap();

        // Collect the events in order
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // First event should be ToolStarted, last should be ToolFinished
        if !events.is_empty() {
            assert!(
                matches!(
                    events[0],
                    juncture_core::stream::ToolsEvent::ToolStarted { .. }
                ),
                "first event should be ToolStarted"
            );
            assert!(
                matches!(
                    events[events.len() - 1],
                    juncture_core::stream::ToolsEvent::ToolFinished { .. }
                ),
                "last event should be ToolFinished"
            );
        }
    }

    #[tokio::test]
    async fn test_tool_node_multiple_tools_emit_multiple_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        #[allow(clippy::redundant_clone, reason = "clarity in test setup")]
        let node =
            TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>]).with_tools_event_tx(tx);

        let messages = vec![Message::ai_with_tool_calls(
            "test",
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

        let _results = node.execute(&messages).await.unwrap();

        // Count events
        let mut started_count = 0;
        let mut finished_count = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                juncture_core::stream::ToolsEvent::ToolStarted { .. } => started_count += 1,
                juncture_core::stream::ToolsEvent::ToolFinished { .. } => finished_count += 1,
                _ => {}
            }
        }

        assert_eq!(started_count, 2, "should have 2 ToolStarted events");
        assert_eq!(finished_count, 2, "should have 2 ToolFinished events");
    }

    // --- tools_condition tests ---

    #[tokio::test]
    async fn test_tool_node_with_tools_condition_allows_execution() {
        use std::sync::Arc;

        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>])
            .with_tools_condition(Arc::new(|msg| {
                // Allow execution if message contains "execute"
                msg.content_text().contains("execute")
            }));

        let messages = vec![Message::ai_with_tool_calls(
            "execute this tool",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 1);
        // Tool should have executed
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
    async fn test_tool_node_with_tools_condition_blocks_execution() {
        use std::sync::Arc;

        let node = TestToolNode::new(vec![Box::new(EchoTool) as Box<dyn Tool>])
            .with_tools_condition(Arc::new(|msg| {
                // Block execution if message contains "block"
                !msg.content_text().contains("block")
            }));

        let messages = vec![Message::ai_with_tool_calls(
            "block this tool",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "hello"}),
            }],
        )];

        let results = node.execute(&messages).await.unwrap();
        assert_eq!(results.len(), 0);
        // Tool should not have executed
    }

    #[tokio::test]
    async fn test_tool_node_tools_condition_with_config() {
        use std::sync::Arc;

        let config = ToolNodeConfig::<juncture_core::state::messages::MessagesState> {
            tools: vec![ToolEntry::from_stateless(Box::new(EchoTool))],
            handle_errors: true,
            validate_input: true,
            call_transformer: None,
            interceptor: None,
            tools_condition: Some(Arc::new(|msg| msg.content_text().contains("allow"))),
        };
        let node = TestToolNode::with_config(config);

        // Test with allowed message
        let messages_allowed = vec![Message::ai_with_tool_calls(
            "allow execution",
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "test"}),
            }],
        )];

        let results = node.execute(&messages_allowed).await.unwrap();
        assert_eq!(results.len(), 1);

        // Test with blocked message
        let messages_blocked = vec![Message::ai_with_tool_calls(
            "deny execution",
            vec![ToolCall {
                id: "call_2".to_string(),
                name: "echo".to_string(),
                arguments: json!({"message": "test"}),
            }],
        )];

        let results = node.execute(&messages_blocked).await.unwrap();
        assert_eq!(results.len(), 0);
    }
}

// Rust guideline compliant 2026-05-22
