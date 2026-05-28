//! Full-featured agent factory with middleware support.
//!
//! This module provides [`create_agent_with_middleware`], a factory function
//! that builds agents with composable middleware chains. This is the juncture
//! equivalent of deer-flow's `create_deerflow_agent`.
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::MockChatModel;
//! use juncture::prebuilt::{
//!     create_agent_with_middleware, AgentConfig, AgentMiddlewareChain,
//!     LoopDetectionMiddleware, ToolErrorHandlingMiddleware,
//! };
//! use juncture::tools::Tool;
//!
//! let model = MockChatModel::new("gpt-4").with_response("Hello!");
//! let tools: Vec<Box<dyn Tool>> = vec![];
//!
//! let middleware = AgentMiddlewareChain::new()
//!     .with(LoopDetectionMiddleware::new(3))
//!     .with(ToolErrorHandlingMiddleware::new());
//!
//! let config = AgentConfig {
//!     system_message: Some("You are a helpful assistant.".to_string()),
//!     middleware,
//!     ..Default::default()
//! };
//!
//! let graph = create_agent_with_middleware(model, tools, config)?;
//! ```

use std::fmt;
use std::sync::Arc;

use futures::future::FutureExt;
use juncture_core::edge::{END, PathMap, RouteResult, Router};
use juncture_core::error::JunctureError;
use juncture_core::graph::{CompiledGraph, StateGraph, TopologyError};
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::Message;
use juncture_core::store::Store;

use crate::llm::{CallOptions, ChatModel};
use crate::prebuilt::agent_middleware::{AgentMiddlewareChain, MiddlewareAction};
use crate::prebuilt::messages_state::{MessagesState, MessagesStateUpdate};
use crate::prebuilt::react::{PromptSource, convert_tool_defs};
use crate::tools::{Tool, ToolDefinition, ToolNode};

/// Type alias for pre-model hook functions.
type PreModelHook = Arc<dyn Fn(&MessagesState) -> MessagesState + Send + Sync>;

/// Type alias for post-model hook functions.
type PostModelHook = Arc<dyn Fn(&MessagesState, &Message) -> Message + Send + Sync>;

/// Type alias for model selector functions.
type ModelSelector = Arc<dyn Fn(&MessagesState) -> CallOptions + Send + Sync>;

/// Configuration for [`create_agent_with_middleware`].
///
/// Controls agent behavior including system prompt, middleware chain,
/// iteration limits, model hooks, and persistent storage.
#[derive(Clone, Default)]
pub struct AgentConfig {
    /// Optional system message injected before each LLM call.
    pub system_message: Option<String>,

    /// Middleware chain for intercepting agent execution.
    pub middleware: AgentMiddlewareChain,

    /// Maximum number of agent-tool loop iterations.
    pub max_iterations: Option<usize>,

    /// Whether to interrupt execution before tool calls are executed.
    pub interrupt_before_tools: bool,

    /// Hook called before each model invocation.
    pub pre_model_hook: Option<PreModelHook>,

    /// Hook called after each model invocation.
    pub post_model_hook: Option<PostModelHook>,

    /// Dynamic model selection strategy returning per-call `CallOptions`.
    pub model_selector: Option<ModelSelector>,

    /// Cross-thread persistent store for long-term memory.
    pub store: Option<Arc<dyn Store>>,
}

impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConfig")
            .field("system_message", &self.system_message)
            .field("middleware", &self.middleware)
            .field("max_iterations", &self.max_iterations)
            .field("interrupt_before_tools", &self.interrupt_before_tools)
            .field(
                "pre_model_hook",
                &self.pre_model_hook.as_ref().map(|_| "..."),
            )
            .field(
                "post_model_hook",
                &self.post_model_hook.as_ref().map(|_| "..."),
            )
            .field(
                "model_selector",
                &self.model_selector.as_ref().map(|_| "..."),
            )
            .field("store", &self.store.as_ref().map(|_| "..."))
            .finish()
    }
}

/// Create an agent with middleware support.
///
/// Builds a graph that alternates between LLM reasoning and tool execution,
/// with middleware hooks intercepting at each stage. This is the juncture
/// equivalent of deer-flow's `create_deerflow_agent`.
///
/// # Arguments
///
/// * `model` - The LLM model to use for reasoning.
/// * `tools` - The tools available to the agent.
/// * `config` - Configuration including middleware chain, system prompt, and hooks.
///
/// # Errors
///
/// Returns [`TopologyError`] if the graph cannot be compiled.
#[allow(
    clippy::needless_pass_by_value,
    reason = "model ownership is transferred into the graph"
)]
pub fn create_agent_with_middleware<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
    config: AgentConfig,
) -> Result<CompiledGraph<MessagesState>, TopologyError> {
    let tool_defs: Vec<ToolDefinition> = tools.iter().map(|t| t.definition()).collect();
    let llm_tool_defs = convert_tool_defs(&tool_defs);
    let model_with_tools = model.bind_tools(llm_tool_defs);

    let prompt = config.system_message.map(PromptSource::Static);
    let pre_model_hook = config.pre_model_hook;
    let post_model_hook = config.post_model_hook;
    let model_selector = config.model_selector;
    let middleware_for_agent = config.middleware.clone();
    let middleware_for_tools = config.middleware;

    // Build the agent node as a closure with middleware
    let agent_model = Arc::new(model_with_tools);
    let agent_node = NodeFnUpdate(move |state: &MessagesState| {
        let model = Arc::clone(&agent_model);
        let state = state.clone();
        let prompt = prompt.clone();
        let pre_hook = pre_model_hook.clone();
        let post_hook = post_model_hook.clone();
        let selector = model_selector.clone();
        let middleware = middleware_for_agent.clone();

        async move {
            // Apply pre_model_hook
            let state = match pre_hook {
                Some(ref hook) => hook(&state),
                None => state,
            };

            // Run middleware before_model
            match middleware.run_before_model(&state).await {
                MiddlewareAction::ShortCircuit(msg) => {
                    return Ok(MessagesStateUpdate {
                        messages: Some(vec![msg]),
                    });
                }
                MiddlewareAction::Continue => {}
            }

            // Build messages
            let messages = build_messages(&state, prompt.as_ref());

            // Apply model_selector
            let options = selector.as_ref().map(|sel| sel(&state));

            // Invoke the model
            // On WASM, ChatModel::invoke() returns !Send future; wrap with force_send.
            let response =
                juncture_core::wasm_send::force_send(model.invoke(&messages, options.as_ref()))
                    .await
                    .map_err(|e| JunctureError::execution(e.to_string()))?;

            // Apply post_model_hook
            let response = match post_hook {
                Some(ref hook) => hook(&state, &response),
                None => response,
            };

            // Run middleware after_model
            let response = middleware.run_after_model(&state, &response).await;

            Ok(MessagesStateUpdate {
                messages: Some(vec![response]),
            })
        }
        .boxed()
    });

    // Build the tool node as a closure with middleware
    let tool_node = Arc::new(ToolNode::new(tools));
    let tool_node_fn = NodeFnUpdate(move |state: &MessagesState| {
        let tool_node = Arc::clone(&tool_node);
        let state = state.clone();
        let middleware = middleware_for_tools.clone();

        async move {
            // Run before_tool middleware for each tool call
            if let Some(last_msg) = state.messages.last() {
                for tc in &last_msg.tool_calls {
                    match middleware.run_before_tool(&tc.name, &tc.arguments).await {
                        MiddlewareAction::ShortCircuit(msg) => {
                            return Ok(MessagesStateUpdate {
                                messages: Some(vec![msg]),
                            });
                        }
                        MiddlewareAction::Continue => {}
                    }
                }
            }

            // Execute tools
            let results = tool_node
                .execute_with_state(&state.messages, Some(&state))
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))?;

            // Run after_tool middleware for each result
            let mut transformed = Vec::with_capacity(results.len());
            for msg in results {
                let tool_name = msg.tool_call_id.as_deref().unwrap_or("unknown");
                let new_msg = middleware.run_after_tool(tool_name, &msg).await;
                transformed.push(new_msg);
            }

            Ok(MessagesStateUpdate {
                messages: Some(transformed),
            })
        }
        .boxed()
    });

    // Build the graph
    let mut graph = StateGraph::<MessagesState>::new();

    graph.add_node_simple("agent", agent_node)?;
    graph.add_node_simple("tools", tool_node_fn)?;

    graph.set_entry_point("agent");

    let path_map = PathMap::from(&[("tools", "tools"), (END, END)][..]);
    graph.add_conditional_edges("agent", Arc::new(MiddlewareAgentRouter), path_map);

    graph.add_edge("tools", "agent");

    graph.compile()
}

/// Build the message list for the LLM, optionally prepending a system prompt.
fn build_messages(state: &MessagesState, prompt: Option<&PromptSource>) -> Vec<Message> {
    match prompt {
        Some(PromptSource::Static(text)) => {
            let mut msgs = vec![Message::system(text)];
            msgs.extend_from_slice(&state.messages);
            msgs
        }
        Some(PromptSource::Dynamic(func)) => {
            let text = func(&state.messages);
            let mut msgs = vec![Message::system(&text)];
            msgs.extend_from_slice(&state.messages);
            msgs
        }
        None => state.messages.clone(),
    }
}

/// Router that determines whether to proceed to tools or end.
struct MiddlewareAgentRouter;

impl Router<MessagesState> for MiddlewareAgentRouter {
    fn route(
        &self,
        state: &MessagesState,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<RouteResult, JunctureError>> + Send + '_>,
    > {
        let target = state
            .messages
            .last()
            .map_or(END, |m| if m.has_tool_calls() { "tools" } else { END });

        let result = RouteResult::One(target.to_string());
        Box::pin(async move { Ok(result) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockChatModel;
    use crate::prebuilt::{LoopDetectionMiddleware, NopMiddleware};

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert!(config.system_message.is_none());
        assert!(config.middleware.is_empty());
        assert!(config.max_iterations.is_none());
        assert!(!config.interrupt_before_tools);
        assert!(config.pre_model_hook.is_none());
        assert!(config.post_model_hook.is_none());
        assert!(config.model_selector.is_none());
        assert!(config.store.is_none());
    }

    #[test]
    fn test_agent_config_debug() {
        let config = AgentConfig::default();
        let debug = format!("{config:?}");
        assert!(debug.contains("AgentConfig"));
    }

    #[test]
    fn test_create_agent_with_middleware_no_middleware() {
        let model = MockChatModel::new("gpt-4").with_response("Hello!");
        let tools: Vec<Box<dyn Tool>> = vec![];
        let config = AgentConfig::default();

        create_agent_with_middleware(model, tools, config).unwrap();
    }

    #[test]
    fn test_create_agent_with_middleware_with_chain() {
        let model = MockChatModel::new("gpt-4").with_response("Hello!");
        let tools: Vec<Box<dyn Tool>> = vec![];
        let middleware = AgentMiddlewareChain::new()
            .with(NopMiddleware)
            .with(LoopDetectionMiddleware::new(3));
        let config = AgentConfig {
            system_message: Some("You are helpful.".to_string()),
            middleware,
            ..Default::default()
        };

        create_agent_with_middleware(model, tools, config).unwrap();
    }

    #[test]
    fn test_agent_config_clone() {
        let config = AgentConfig {
            system_message: Some("test".to_string()),
            middleware: AgentMiddlewareChain::new().with(NopMiddleware),
            max_iterations: Some(5),
            ..Default::default()
        };
        let cloned = config.clone();
        drop(config);
        assert_eq!(cloned.system_message, Some("test".to_string()));
        assert_eq!(cloned.max_iterations, Some(5));
    }
}

// Rust guideline compliant 2026-05-27
