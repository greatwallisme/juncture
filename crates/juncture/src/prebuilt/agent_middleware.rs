//! Agent-level middleware system for prebuilt agents.
//!
//! This module provides [`AgentMiddleware`] for intercepting agent execution
//! at the tool-call level, distinct from [`LlmMiddleware`](crate::llm::middleware::LlmMiddleware)
//! which wraps `ChatModel::invoke()`. Agent middleware can:
//!
//! - Transform state before/after model invocation
//! - Intercept tool calls before execution
//! - Handle errors across the entire agent loop
//! - Implement loop detection, guardrails, and observability
//!
//! # Lifecycle
//!
//! For each agent loop iteration:
//!
//! 1. [`AgentMiddleware::before_model()`] — transform state before LLM call
//! 2. LLM call executes
//! 3. [`AgentMiddleware::after_model()`] — transform model response
//! 4. If tool calls present:
//!    a. [`AgentMiddleware::before_tool()`] — intercept before tool execution
//!    b. Tool executes
//!    c. [`AgentMiddleware::after_tool()`] — intercept after tool execution
//! 5. [`AgentMiddleware::on_error()`] — handle errors at any stage

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use juncture_core::state::messages::Message;

use crate::prebuilt::messages_state::MessagesState;

/// Result of a middleware hook invocation.
///
/// Determines whether the agent loop should continue or short-circuit.
#[derive(Debug)]
pub enum MiddlewareAction {
    /// Continue with the normal agent loop.
    Continue,
    /// Short-circuit the current phase with a replacement message.
    /// For example, `before_model` can return a synthetic response to skip the LLM call.
    ShortCircuit(Message),
}

/// Agent-level middleware for intercepting agent execution.
///
/// Unlike [`LlmMiddleware`](crate::llm::middleware::LlmMiddleware) which wraps
/// individual `ChatModel::invoke()` calls, `AgentMiddleware` operates at the
/// agent loop level — intercepting tool calls, transforming state, and handling
/// errors across the entire execution cycle.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::middleware::{AgentMiddleware, MiddlewareAction};
/// use juncture::prebuilt::MessagesState;
/// use juncture::llm::Message;
///
/// struct LoggingMiddleware;
///
/// #[async_trait]
/// impl AgentMiddleware for LoggingMiddleware {
///     async fn before_model(&self, state: &MessagesState) -> MiddlewareAction {
///         tracing::info!("Agent has {} messages", state.messages.len());
///         MiddlewareAction::Continue
///     }
/// }
/// ```
#[async_trait]
pub trait AgentMiddleware: Send + Sync + fmt::Debug {
    /// Called before the model is invoked.
    ///
    /// Return [`MiddlewareAction::ShortCircuit`] with a synthetic message to skip
    /// the LLM call (e.g., for caching or guardrails).
    async fn before_model(&self, _state: &MessagesState) -> MiddlewareAction {
        MiddlewareAction::Continue
    }

    /// Called after the model returns a response.
    ///
    /// Return a modified message to replace the model's response.
    /// The default implementation returns the original message unchanged.
    async fn after_model(&self, _state: &MessagesState, response: &Message) -> Message {
        response.clone()
    }

    /// Called before a tool is executed.
    ///
    /// Return [`MiddlewareAction::ShortCircuit`] with a synthetic tool-result
    /// message to skip the actual tool execution (e.g., for permission checks).
    async fn before_tool(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> MiddlewareAction {
        MiddlewareAction::Continue
    }

    /// Called after a tool returns a result.
    ///
    /// Return a modified message to replace the tool's result.
    async fn after_tool(&self, _tool_name: &str, result: &Message) -> Message {
        result.clone()
    }

    /// Called when an error occurs at any stage of the agent loop.
    ///
    /// Return `Some(recovery_message)` to recover from the error and continue,
    /// or `None` to propagate the error.
    async fn on_error(&self, _error: &str) -> Option<Message> {
        None
    }
}

/// A no-op middleware that passes through all calls unchanged.
///
/// Useful as a pass-through default in middleware chains.
#[derive(Debug)]
pub struct NopMiddleware;

#[async_trait]
impl AgentMiddleware for NopMiddleware {}

/// Middleware that detects and prevents infinite tool loops.
///
/// Tracks consecutive identical tool calls and stops the agent when
/// the same tool is called with the same arguments more than `max_repetitions` times.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::middleware::LoopDetectionMiddleware;
///
/// let middleware = LoopDetectionMiddleware::new(3);
/// ```
#[derive(Debug)]
pub struct LoopDetectionMiddleware {
    max_repetitions: usize,
}

impl LoopDetectionMiddleware {
    /// Create a new loop detection middleware.
    ///
    /// # Arguments
    ///
    /// * `max_repetitions` - Maximum number of identical consecutive tool calls allowed.
    #[must_use]
    pub const fn new(max_repetitions: usize) -> Self {
        Self { max_repetitions }
    }
}

#[async_trait]
impl AgentMiddleware for LoopDetectionMiddleware {
    async fn before_model(&self, state: &MessagesState) -> MiddlewareAction {
        if state.messages.len() < self.max_repetitions * 2 {
            return MiddlewareAction::Continue;
        }

        // Check if the last N tool-call/result pairs are identical
        let recent: Vec<&Message> = state
            .messages
            .iter()
            .rev()
            .take(self.max_repetitions * 2)
            .collect();

        if recent.len() < self.max_repetitions * 2 {
            return MiddlewareAction::Continue;
        }

        // Check if all recent tool calls are identical
        let tool_calls: Vec<(&str, &serde_json::Value)> = recent
            .iter()
            .filter_map(|m| {
                m.tool_calls
                    .first()
                    .map(|tc| (tc.name.as_str(), &tc.arguments))
            })
            .collect();

        if tool_calls.len() >= self.max_repetitions {
            let first = &tool_calls[0];
            let all_same = tool_calls
                .iter()
                .all(|tc| tc.0 == first.0 && tc.1 == first.1);
            if all_same {
                return MiddlewareAction::ShortCircuit(Message::ai(format!(
                    "Loop detected: tool '{}' called {} times with identical arguments. Stopping.",
                    first.0, self.max_repetitions
                )));
            }
        }

        MiddlewareAction::Continue
    }
}

/// Middleware that handles tool execution errors gracefully.
///
/// Catches tool errors and returns a synthetic error message instead of
/// propagating the error, allowing the agent to recover and try alternative
/// approaches.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::middleware::ToolErrorHandlingMiddleware;
///
/// let middleware = ToolErrorHandlingMiddleware::new();
/// ```
#[derive(Debug)]
pub struct ToolErrorHandlingMiddleware;

impl ToolErrorHandlingMiddleware {
    /// Create a new tool error handling middleware.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ToolErrorHandlingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentMiddleware for ToolErrorHandlingMiddleware {
    async fn after_tool(&self, tool_name: &str, result: &Message) -> Message {
        // Check if the result indicates an error
        if let crate::llm::Content::Text(text) = &result.content
            && (text.starts_with("Error:") || text.starts_with("error:"))
        {
            return Message::tool_result(
                result.tool_call_id.clone().unwrap_or_default(),
                format!(
                    "Tool '{tool_name}' failed: {text}\nPlease try a different approach or tool."
                ),
            );
        }
        result.clone()
    }

    async fn on_error(&self, error: &str) -> Option<Message> {
        Some(Message::ai(format!(
            "An error occurred: {error}\nLet me try a different approach."
        )))
    }
}

/// Container for an ordered chain of agent middleware.
///
/// Middleware are executed in the order they were added for `before_*` hooks,
/// and in reverse order for `after_*` hooks (last added runs first).
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::middleware::{
///     AgentMiddlewareChain, LoopDetectionMiddleware, ToolErrorHandlingMiddleware,
/// };
///
/// let chain = AgentMiddlewareChain::new()
///     .with(LoopDetectionMiddleware::new(3))
///     .with(ToolErrorHandlingMiddleware::new());
/// ```
#[derive(Clone)]
pub struct AgentMiddlewareChain {
    middlewares: Vec<Arc<dyn AgentMiddleware>>,
}

impl AgentMiddlewareChain {
    /// Create an empty middleware chain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the chain.
    ///
    /// Middleware are executed in insertion order for `before_*` hooks,
    /// and in reverse order for `after_*` hooks.
    #[must_use]
    pub fn with<M: AgentMiddleware + 'static>(mut self, middleware: M) -> Self {
        self.middlewares.push(Arc::new(middleware));
        self
    }

    /// Get the number of middleware in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    /// Check if the chain is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }

    /// Run all `before_model` hooks in forward order.
    ///
    /// Returns the first `ShortCircuit` encountered, or `Continue` if all pass.
    pub async fn run_before_model(&self, state: &MessagesState) -> MiddlewareAction {
        for mw in &self.middlewares {
            match mw.before_model(state).await {
                MiddlewareAction::Continue => {}
                MiddlewareAction::ShortCircuit(msg) => return MiddlewareAction::ShortCircuit(msg),
            }
        }
        MiddlewareAction::Continue
    }

    /// Run all `after_model` hooks in reverse order.
    pub async fn run_after_model(&self, state: &MessagesState, response: &Message) -> Message {
        let mut result = response.clone();
        for mw in self.middlewares.iter().rev() {
            result = mw.after_model(state, &result).await;
        }
        result
    }

    /// Run all `before_tool` hooks in forward order.
    pub async fn run_before_tool(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> MiddlewareAction {
        for mw in &self.middlewares {
            match mw.before_tool(tool_name, arguments).await {
                MiddlewareAction::Continue => {}
                MiddlewareAction::ShortCircuit(msg) => return MiddlewareAction::ShortCircuit(msg),
            }
        }
        MiddlewareAction::Continue
    }

    /// Run all `after_tool` hooks in reverse order.
    pub async fn run_after_tool(&self, tool_name: &str, result: &Message) -> Message {
        let mut result = result.clone();
        for mw in self.middlewares.iter().rev() {
            result = mw.after_tool(tool_name, &result).await;
        }
        result
    }

    /// Run all `on_error` hooks until one returns a recovery message.
    pub async fn run_on_error(&self, error: &str) -> Option<Message> {
        for mw in &self.middlewares {
            if let Some(recovery) = mw.on_error(error).await {
                return Some(recovery);
            }
        }
        None
    }
}

impl Default for AgentMiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AgentMiddlewareChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentMiddlewareChain")
            .field("count", &self.middlewares.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nop_middleware_debug() {
        let mw = NopMiddleware;
        let debug = format!("{mw:?}");
        assert_eq!(debug, "NopMiddleware");
    }

    #[test]
    fn test_loop_detection_middleware_new() {
        let mw = LoopDetectionMiddleware::new(3);
        assert_eq!(mw.max_repetitions, 3);
    }

    #[test]
    fn test_tool_error_handling_middleware_default() {
        let mw = ToolErrorHandlingMiddleware;
        let _ = format!("{mw:?}");
    }

    #[test]
    fn test_middleware_chain_new() {
        let chain = AgentMiddlewareChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn test_middleware_chain_with() {
        let chain = AgentMiddlewareChain::new()
            .with(NopMiddleware)
            .with(LoopDetectionMiddleware::new(3));
        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_middleware_chain_default() {
        let chain = AgentMiddlewareChain::default();
        assert!(chain.is_empty());
    }

    #[test]
    fn test_middleware_chain_clone() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let cloned = chain.clone();
        drop(chain);
        assert_eq!(cloned.len(), 1);
    }

    #[test]
    fn test_middleware_chain_debug() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let debug = format!("{chain:?}");
        assert!(debug.contains("AgentMiddlewareChain"));
        assert!(debug.contains("count: 1"));
    }

    #[tokio::test]
    async fn test_middleware_action_debug() {
        let cont = MiddlewareAction::Continue;
        assert_eq!(format!("{cont:?}"), "Continue");

        let sc = MiddlewareAction::ShortCircuit(Message::ai("test"));
        let debug = format!("{sc:?}");
        assert!(debug.contains("ShortCircuit"));
    }

    #[tokio::test]
    async fn test_chain_run_before_model_continue() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let state = MessagesState::default();
        let result = chain.run_before_model(&state).await;
        assert!(matches!(result, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_chain_run_after_model_passthrough() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let state = MessagesState::default();
        let response = Message::ai("hello");
        let result = chain.run_after_model(&state, &response).await;
        assert_eq!(result.content_text(), "hello");
    }

    #[tokio::test]
    async fn test_chain_run_before_tool_continue() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let args = serde_json::json!({});
        let result = chain.run_before_tool("test_tool", &args).await;
        assert!(matches!(result, MiddlewareAction::Continue));
    }

    #[tokio::test]
    async fn test_chain_run_after_tool_passthrough() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let result_msg = Message::tool_result("call_1", "result");
        let result = chain.run_after_tool("test_tool", &result_msg).await;
        assert_eq!(result.content_text(), "result");
    }

    #[tokio::test]
    async fn test_chain_run_on_error_none() {
        let chain = AgentMiddlewareChain::new().with(NopMiddleware);
        let result = chain.run_on_error("test error").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_tool_error_handling_recovery() {
        let chain = AgentMiddlewareChain::new().with(ToolErrorHandlingMiddleware::new());
        let result = chain.run_on_error("something broke").await;
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.content_text().contains("something broke"));
    }

    #[tokio::test]
    async fn test_tool_error_handling_normal_result() {
        let chain = AgentMiddlewareChain::new().with(ToolErrorHandlingMiddleware::new());
        let result_msg = Message::tool_result("call_1", "success");
        let result = chain.run_after_tool("test_tool", &result_msg).await;
        assert_eq!(result.content_text(), "success");
    }

    #[tokio::test]
    async fn test_tool_error_handling_error_result() {
        let chain = AgentMiddlewareChain::new().with(ToolErrorHandlingMiddleware::new());
        let result_msg = Message::tool_result("call_1", "Error: something failed");
        let result = chain.run_after_tool("test_tool", &result_msg).await;
        assert!(result.content_text().contains("test_tool"));
        assert!(result.content_text().contains("Error: something failed"));
    }

    #[tokio::test]
    async fn test_loop_detection_no_loop() {
        let chain = AgentMiddlewareChain::new().with(LoopDetectionMiddleware::new(3));
        let state = MessagesState {
            messages: vec![Message::human("hello")],
        };
        let result = chain.run_before_model(&state).await;
        assert!(matches!(result, MiddlewareAction::Continue));
    }
}

// Rust guideline compliant 2026-05-27
