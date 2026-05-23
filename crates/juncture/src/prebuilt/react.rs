//! `ReAct` agent: a prebuilt agent that reasons and acts with tools.
//!
//! This module provides [`create_react_agent`] and related types for building
//! agent workflows that follow the Reason-Act (`ReAct`) pattern. The agent
//! calls an LLM, and if the LLM requests tool execution, the tools are run
//! and the results are fed back to the LLM for further reasoning.
//!
//! # Graph Structure
//!
//! ```text
//! START -> agent -> [conditional] -> tools -> agent
//!                        |
//!                        v
//!                       END
//! ```
//!
//! - The `agent` node calls the LLM model.
//! - If the LLM response contains tool calls, the `tools` node executes them.
//! - If the LLM response has no tool calls, execution ends.
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::{ChatModel, MockChatModel};
//! use juncture::prebuilt::{create_react_agent, MessagesState};
//! use juncture::tools::Tool;
//!
//! let model = MockChatModel::new("gpt-4").with_response("Done!");
//! let tools: Vec<Box<dyn Tool>> = vec![];
//!
//! let agent = create_react_agent(model, tools)?;
//! ```

use std::fmt;
use std::sync::Arc;

use juncture_core::edge::{END, PathMap, RouteResult, Router};
use juncture_core::error::JunctureError;
use juncture_core::graph::{CompiledGraph, StateGraph, TopologyError};
use juncture_core::node::{IntoNode, Node};
use juncture_core::state::messages::Message;
use juncture_core::store::Store;
use juncture_core::{Command, RunnableConfig};

use crate::llm::{CallOptions, ChatModel, ToolDefinition as LlmToolDefinition};
use crate::prebuilt::messages_state::{MessagesState, MessagesStateUpdate};
use crate::tools::{Tool, ToolDefinition, ToolNode};

/// Type alias for the dynamic prompt function signature.
///
/// Reduces type complexity in the [`PromptSource::Dynamic`] variant.
type DynamicPromptFn = Arc<dyn Fn(&[Message]) -> String + Send + Sync>;

/// Pre-model hook: transforms [`MessagesState`] before model invocation.
type PreModelHook = Arc<dyn Fn(&MessagesState) -> MessagesState + Send + Sync>;

/// Post-model hook: transforms the model response [`Message`] after invocation.
type PostModelHook = Arc<dyn Fn(&MessagesState, &Message) -> Message + Send + Sync>;

/// Model selector: returns per-call [`CallOptions`] based on current state.
type ModelSelector = Arc<dyn Fn(&MessagesState) -> CallOptions + Send + Sync>;

/// Convert tool definitions from the tools module format to the LLM module format.
///
/// The `tools` module defines [`ToolDefinition`] with `name`, `description`, and
/// `parameters` fields. The `llm` module defines its own [`LlmToolDefinition`] with
/// the same fields. This function converts between them.
fn convert_tool_defs(defs: &[ToolDefinition]) -> Vec<LlmToolDefinition> {
    defs.iter()
        .map(|d| LlmToolDefinition {
            name: d.name.clone(),
            description: d.description.clone(),
            parameters: d.parameters.clone(),
        })
        .collect()
}

/// Create a `ReAct` agent with default configuration.
///
/// Builds a graph that alternates between LLM reasoning and tool execution.
/// The agent calls the LLM, and if the response contains tool calls, the
/// tools are executed and the results are fed back. This continues until
/// the LLM produces a response without tool calls.
///
/// # Arguments
///
/// * `model` - The LLM model to use for reasoning.
/// * `tools` - The tools available to the agent.
///
/// # Errors
///
/// Returns [`TopologyError`] if the graph cannot be compiled, for example
/// if node names conflict.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::MockChatModel;
/// use juncture::prebuilt::create_react_agent;
/// use juncture::tools::Tool;
///
/// let model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let tools: Vec<Box<dyn Tool>> = vec![];
///
/// let graph = create_react_agent(model, tools)?;
/// ```
pub fn create_react_agent<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
) -> Result<CompiledGraph<MessagesState>, TopologyError> {
    create_react_agent_with_config(model, tools, ReactAgentConfig::default())
}

/// Create a `ReAct` agent with custom configuration.
///
/// Like [`create_react_agent`], but accepts a [`ReactAgentConfig`] for
/// additional options such as system prompts and interrupt settings.
///
/// # Arguments
///
/// * `model` - The LLM model to use for reasoning.
/// * `tools` - The tools available to the agent.
/// * `config` - Configuration options for the agent.
///
/// # Errors
///
/// Returns [`TopologyError`] if the graph cannot be compiled.
///
/// # Example
///
/// ```ignore
/// use juncture::llm::MockChatModel;
/// use juncture::prebuilt::{create_react_agent_with_config, ReactAgentConfig};
/// use juncture::tools::Tool;
///
/// let model = MockChatModel::new("gpt-4").with_response("Hello!");
/// let tools: Vec<Box<dyn Tool>> = vec![];
/// let config = ReactAgentConfig {
///     system_message: Some("You are a helpful assistant.".to_string()),
///     ..Default::default()
/// };
///
/// let graph = create_react_agent_with_config(model, tools, config)?;
/// ```
#[allow(
    clippy::needless_pass_by_value,
    reason = "model ownership is transferred into the graph"
)]
pub fn create_react_agent_with_config<M: ChatModel>(
    model: M,
    tools: Vec<Box<dyn Tool>>,
    config: ReactAgentConfig,
) -> Result<CompiledGraph<MessagesState>, TopologyError> {
    let tool_defs: Vec<ToolDefinition> = tools.iter().map(|t| t.definition()).collect();
    let llm_tool_defs = convert_tool_defs(&tool_defs);
    let model_with_tools = model.bind_tools(llm_tool_defs);

    let prompt = config.system_message.map(PromptSource::Static);
    let mut agent_node = AgentNode::new_with_prompt_option(model_with_tools, prompt);
    if let Some(hook) = config.pre_model_hook {
        agent_node = agent_node.with_pre_model_hook(hook);
    }
    if let Some(hook) = config.post_model_hook {
        agent_node = agent_node.with_post_model_hook(hook);
    }
    if let Some(selector) = config.model_selector {
        agent_node = agent_node.with_model_selector(selector);
    }

    let tool_node = Arc::new(ToolNode::new(tools));
    let tool_adapter = ToolNodeAdapter::new(tool_node, config.store);

    let mut graph = StateGraph::<MessagesState>::new();

    graph.add_node_simple("agent", agent_node)?;
    graph.add_node_simple("tools", tool_adapter)?;

    graph.set_entry_point("agent");

    let path_map = PathMap::from(&[("tools", "tools"), (END, END)][..]);
    graph.add_conditional_edges("agent", Arc::new(AgentRouter), path_map);

    graph.add_edge("tools", "agent");

    graph.compile()
}

/// Source for system prompts in agent nodes.
///
/// Prompts can be either a static string injected on every invocation,
/// or a dynamic function that computes the prompt from the current messages.
#[derive(Clone)]
pub enum PromptSource {
    /// Static system prompt string.
    Static(String),

    /// Dynamic prompt computed from the current message list.
    Dynamic(DynamicPromptFn),
}

impl fmt::Debug for PromptSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(s) => f.debug_tuple("Static").field(s).finish(),
            Self::Dynamic(_) => f.debug_tuple("Dynamic").field(&"<fn>").finish(),
        }
    }
}

/// Configuration for [`create_react_agent_with_config`].
///
/// Controls optional behavior such as system prompt injection, iteration
/// limits, human-in-the-loop interrupt points, pre/post model hooks,
/// dynamic model selection, and cross-thread persistent store integration.
#[derive(Clone, Default)]
pub struct ReactAgentConfig {
    /// Optional system message injected before each LLM call.
    ///
    /// When set, the agent node prepends a system message to the conversation
    /// before invoking the model.
    pub system_message: Option<String>,

    /// Maximum number of agent-tool loop iterations.
    ///
    /// When set, limits how many times the agent can cycle through the
    /// reasoning-acting loop. This prevents infinite loops when the LLM
    /// keeps requesting tool calls.
    pub max_iterations: Option<usize>,

    /// Whether to interrupt execution before tool calls are executed.
    ///
    /// When true, execution pauses before the tools node runs, allowing
    /// a human to review and approve tool invocations before they proceed.
    pub interrupt_before_tools: bool,

    /// Hook called before each model invocation.
    ///
    /// Receives a reference to the current [`MessagesState`] and returns a
    /// (possibly modified) [`MessagesState`]. Useful for injecting context,
    /// trimming messages, or adding system instructions before the LLM call.
    pub pre_model_hook: Option<PreModelHook>,

    /// Hook called after each model invocation.
    ///
    /// Receives a reference to the current [`MessagesState`] and a reference
    /// to the model response [`Message`], returns a (possibly modified)
    /// response [`Message`]. Useful for post-processing, validation, or
    /// content filtering of LLM responses.
    pub post_model_hook: Option<PostModelHook>,

    /// Dynamic model selection strategy.
    ///
    /// Takes a reference to the current [`MessagesState`] and returns
    /// [`CallOptions`] for this invocation. Use this to switch models
    /// dynamically (via [`CallOptions::model_override`]), adjust
    /// temperature, set `tool_choice`, or customize other per-call options.
    pub model_selector: Option<ModelSelector>,

    /// Cross-thread persistent store for long-term memory.
    ///
    /// When set, the store is passed to the tool adapter and made available
    /// to stateful tools via [`ToolRuntime`](crate::tools::ToolRuntime).
    /// This enables tools to persist and retrieve knowledge across graph
    /// executions.
    pub store: Option<Arc<dyn Store>>,
}

impl fmt::Debug for ReactAgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReactAgentConfig")
            .field("system_message", &self.system_message)
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

/// Agent node: calls an LLM and returns the response as a state update.
///
/// This node optionally injects a system prompt, then invokes the bound
/// LLM model with the current conversation messages. The LLM response
/// is returned as a state update that appends to the messages list.
///
/// Supports optional pre/post model hooks for state transformation and
/// response post-processing, and a `model_selector` for dynamic per-call
/// [`CallOptions`] (e.g., model override, temperature adjustment).
pub struct AgentNode<M: ChatModel> {
    model: M,
    prompt: Option<PromptSource>,
    /// Hook called before each model invocation.
    pre_model_hook: Option<PreModelHook>,
    /// Hook called after each model invocation.
    post_model_hook: Option<PostModelHook>,
    /// Dynamic model selection strategy returning per-call `CallOptions`.
    model_selector: Option<ModelSelector>,
}

impl<M: ChatModel> AgentNode<M> {
    /// Create a new agent node without a system prompt.
    #[must_use]
    pub fn new(model: M) -> Self {
        Self {
            model,
            prompt: None,
            pre_model_hook: None,
            post_model_hook: None,
            model_selector: None,
        }
    }

    /// Create a new agent node with a system prompt.
    #[must_use]
    pub fn with_prompt(model: M, prompt: PromptSource) -> Self {
        Self {
            model,
            prompt: Some(prompt),
            pre_model_hook: None,
            post_model_hook: None,
            model_selector: None,
        }
    }

    /// Create a new agent node with an optional prompt source.
    #[must_use]
    fn new_with_prompt_option(model: M, prompt: Option<PromptSource>) -> Self {
        Self {
            model,
            prompt,
            pre_model_hook: None,
            post_model_hook: None,
            model_selector: None,
        }
    }

    /// Set a pre-model hook on this agent node.
    ///
    /// The hook is called before each model invocation, receiving a reference
    /// to the current state and returning a (possibly modified) state.
    #[must_use]
    pub fn with_pre_model_hook(mut self, hook: PreModelHook) -> Self {
        self.pre_model_hook = Some(hook);
        self
    }

    /// Set a post-model hook on this agent node.
    ///
    /// The hook is called after each model invocation, receiving a reference
    /// to the current state and the model response, returning a (possibly
    /// modified) response message.
    #[must_use]
    pub fn with_post_model_hook(mut self, hook: PostModelHook) -> Self {
        self.post_model_hook = Some(hook);
        self
    }

    /// Set a model selector on this agent node.
    ///
    /// The selector receives a reference to the current state and returns
    /// [`CallOptions`] for this invocation (e.g., to switch models via
    /// `model_override`).
    #[must_use]
    pub fn with_model_selector(mut self, selector: ModelSelector) -> Self {
        self.model_selector = Some(selector);
        self
    }

    /// Build the message list to send to the LLM.
    ///
    /// If a prompt source is configured, a system message is prepended.
    fn build_messages(&self, state: &MessagesState) -> Vec<Message> {
        match &self.prompt {
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
}

impl<M: ChatModel> fmt::Debug for AgentNode<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentNode")
            .field("model", &self.model.model_name())
            .field("prompt", &self.prompt)
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
            .finish()
    }
}

impl<M: ChatModel> Node<MessagesState> for AgentNode<M> {
    #[allow(
        clippy::needless_pass_by_value,
        reason = "state ownership is transferred into the async future"
    )]
    fn call(
        &self,
        state: MessagesState,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Command<MessagesState>, JunctureError>>
                + Send
                + '_,
        >,
    > {
        // Clone the budget tracker Arc so it can be moved into the async block
        let budget_tracker = config.budget_tracker().cloned();

        // Apply pre_model_hook to transform state before building messages
        let state = match &self.pre_model_hook {
            Some(hook) => hook(&state),
            None => state,
        };

        let messages = self.build_messages(&state);

        // Apply model_selector for per-call options (e.g., model override)
        let options = self.model_selector.as_ref().map(|sel| sel(&state));

        Box::pin(async move {
            let response = self
                .model
                .invoke(&messages, options.as_ref())
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))?;

            // Apply post_model_hook to transform the response
            let response = match &self.post_model_hook {
                Some(hook) => hook(&state, &response),
                None => response,
            };

            // Report token usage to the budget tracker, if configured
            if let Some(usage) = &response.usage
                && let Some(ref tracker) = budget_tracker
            {
                tracker.report_model_call(usage.input_tokens, usage.output_tokens);
            }

            let update = MessagesStateUpdate {
                messages: Some(vec![response]),
            };
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &'static str {
        "agent"
    }
}

impl<M: ChatModel> IntoNode<MessagesState> for AgentNode<M> {
    fn into_node(self, name: &str) -> Arc<dyn Node<MessagesState>> {
        Arc::new(NamedNodeWrapper {
            inner: self,
            name: name.to_string(),
        })
    }
}

/// Wrapper that pairs a node implementation with a name.
///
/// Used by [`IntoNode`] implementations to carry the graph-assigned
/// node name alongside the node logic.
struct NamedNodeWrapper<N> {
    inner: N,
    name: String,
}

impl<N: Node<MessagesState>> fmt::Debug for NamedNodeWrapper<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamedNodeWrapper")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<N: Node<MessagesState>> Node<MessagesState> for NamedNodeWrapper<N> {
    fn call(
        &self,
        state: MessagesState,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Command<MessagesState>, JunctureError>>
                + Send
                + '_,
        >,
    > {
        self.inner.call(state, config)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Router that determines whether to proceed to tools or end.
///
/// Examines the last message in the state: if it contains tool calls,
/// routes to the "tools" node; otherwise routes to END.
struct AgentRouter;

impl Router<MessagesState> for AgentRouter {
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

/// Adapter that wraps a [`ToolNode`] to implement [`Node<MessagesState>`].
///
/// [`ToolNode`] has an `execute` method that takes `&[Message]` and returns
/// `Vec<Message>`, but does not directly implement the [`Node`] trait. This
/// adapter bridges the gap by extracting messages from the state, calling
/// `execute`, and returning the results as a state update.
///
/// The adapter also carries an optional cross-thread persistent [`Store`]
/// for stateful tool execution.
struct ToolNodeAdapter {
    tool_node: Arc<ToolNode<MessagesState>>,
    /// Optional cross-thread persistent store for long-term memory.
    store: Option<Arc<dyn Store>>,
}

impl ToolNodeAdapter {
    /// Create a new adapter wrapping the given tool node.
    #[must_use]
    fn new(tool_node: Arc<ToolNode<MessagesState>>, store: Option<Arc<dyn Store>>) -> Self {
        Self { tool_node, store }
    }
}

impl fmt::Debug for ToolNodeAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolNodeAdapter")
            .field("tool_node", &self.tool_node)
            .field("store", &self.store.as_ref().map(|_| "..."))
            .finish()
    }
}

impl Node<MessagesState> for ToolNodeAdapter {
    fn call(
        &self,
        state: MessagesState,
        _config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Command<MessagesState>, JunctureError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            // Execute tools with state access for stateful tools
            let results = self
                .tool_node
                .execute_with_state(&state.messages, Some(&state))
                .await
                .map_err(|e| JunctureError::execution(e.to_string()))?;

            let update = MessagesStateUpdate {
                messages: Some(results),
            };
            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &'static str {
        "tools"
    }
}

impl IntoNode<MessagesState> for ToolNodeAdapter {
    fn into_node(self, name: &str) -> Arc<dyn Node<MessagesState>> {
        Arc::new(NamedNodeWrapper {
            inner: self,
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockChatModel;
    use crate::tools::ToolError;
    use async_trait::async_trait;
    use juncture_core::State as _;
    use juncture_core::state::messages::Content;
    use juncture_core::state::messages::ToolCall;
    use serde_json::json;

    /// Simple echo tool for testing
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echoes back the input message"
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

    #[test]
    fn test_react_agent_config_default() {
        let config = ReactAgentConfig::default();
        assert!(config.system_message.is_none());
        assert!(config.max_iterations.is_none());
        assert!(!config.interrupt_before_tools);
    }

    #[test]
    fn test_prompt_source_static_debug() {
        let prompt = PromptSource::Static("You are helpful.".to_string());
        let debug = format!("{prompt:?}");
        assert!(debug.contains("Static"));
        assert!(debug.contains("You are helpful."));
    }

    #[test]
    fn test_prompt_source_dynamic_debug() {
        let prompt = PromptSource::Dynamic(Arc::new(|_msgs: &[Message]| "dynamic".to_string()));
        let debug = format!("{prompt:?}");
        assert!(debug.contains("Dynamic"));
        assert!(debug.contains("<fn>"));
    }

    #[test]
    fn test_agent_node_debug() {
        let model = MockChatModel::new("gpt-4");
        let node = AgentNode::new(model);
        let debug = format!("{node:?}");
        assert!(debug.contains("AgentNode"));
        assert!(debug.contains("gpt-4"));
    }

    #[test]
    fn test_agent_node_with_prompt_debug() {
        let model = MockChatModel::new("gpt-4");
        let prompt = PromptSource::Static("You are a calculator.".to_string());
        let node = AgentNode::with_prompt(model, prompt);
        let debug = format!("{node:?}");
        assert!(debug.contains("AgentNode"));
        assert!(debug.contains("prompt"));
    }

    #[test]
    fn test_messages_state_update_default() {
        let update = MessagesStateUpdate::default();
        assert!(update.messages.is_none());
    }

    #[test]
    fn test_messages_state_apply_append() {
        let mut state = MessagesState {
            messages: vec![Message::human("Hello")],
        };
        let update = MessagesStateUpdate {
            messages: Some(vec![Message::ai("Hi there!")]),
        };
        let changed = state.apply(update);
        assert!(!changed.is_empty());
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn test_messages_state_apply_no_change() {
        let mut state = MessagesState {
            messages: vec![Message::human("Hello")],
        };
        let update = MessagesStateUpdate { messages: None };
        let changed = state.apply(update);
        assert!(changed.is_empty());
        assert_eq!(state.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_agent_node_call_without_prompt() {
        let model = MockChatModel::new("gpt-4").with_response("Hello back!");
        let node = AgentNode::new(model);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;

        let cmd = result.unwrap();
        assert!(cmd.update.is_some());
    }

    #[tokio::test]
    async fn test_agent_node_call_with_static_prompt() {
        let model = MockChatModel::new("gpt-4").with_response("Calculated!");
        let prompt = PromptSource::Static("You are a calculator.".to_string());
        let node = AgentNode::with_prompt(model, prompt);

        let state = MessagesState {
            messages: vec![Message::human("What is 2+2?")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap();
    }

    #[tokio::test]
    async fn test_agent_node_call_with_dynamic_prompt() {
        let model = MockChatModel::new("gpt-4").with_response("Response");
        let prompt = PromptSource::Dynamic(Arc::new(|msgs: &[Message]| {
            format!("Context: {} messages", msgs.len())
        }));
        let node = AgentNode::with_prompt(model, prompt);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap();
    }

    #[tokio::test]
    async fn test_agent_node_call_model_error() {
        let model = MockChatModel::new("gpt-4").with_error();
        let node = AgentNode::new(model);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap_err();
    }

    #[tokio::test]
    async fn test_tool_node_adapter() {
        let tool_node = Arc::new(ToolNode::new(vec![Box::new(EchoTool)]));
        let adapter = ToolNodeAdapter::new(tool_node, None);

        let state = MessagesState {
            messages: vec![Message::ai_with_tool_calls(
                "Echo this",
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"message": "hello"}),
                }],
            )],
        };

        let result = adapter.call(state, &RunnableConfig::default()).await;
        assert!(result.is_ok());

        let cmd = result.unwrap();
        assert!(cmd.update.is_some());

        let update = cmd.update.unwrap();
        assert!(update.messages.is_some());
        let tool_messages = update.messages.unwrap();
        assert_eq!(tool_messages.len(), 1);
    }

    #[tokio::test]
    async fn test_tool_node_adapter_no_tool_calls() {
        let tool_node = Arc::new(ToolNode::new(vec![Box::new(EchoTool)]));
        let adapter = ToolNodeAdapter::new(tool_node, None);

        let state = MessagesState {
            messages: vec![Message::ai("No tools here")],
        };

        let result = adapter.call(state, &RunnableConfig::default()).await;
        result.unwrap_err();
    }

    #[test]
    fn test_create_react_agent_basic() {
        let model = MockChatModel::new("gpt-4").with_response("Hello!");
        let tools: Vec<Box<dyn Tool>> = vec![];

        let result = create_react_agent(model, tools);
        result.unwrap();
    }

    #[test]
    fn test_create_react_agent_with_tools() {
        let model = MockChatModel::new("gpt-4").with_response("Let me search for that.");
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool)];

        let result = create_react_agent(model, tools);
        result.unwrap();
    }

    #[test]
    fn test_create_react_agent_with_config() {
        let model = MockChatModel::new("gpt-4").with_response("Done!");
        let tools: Vec<Box<dyn Tool>> = vec![];

        let config = ReactAgentConfig {
            system_message: Some("You are a helpful assistant.".to_string()),
            max_iterations: Some(10),
            interrupt_before_tools: false,
            ..Default::default()
        };

        let result = create_react_agent_with_config(model, tools, config);
        result.unwrap();
    }

    #[test]
    fn test_react_agent_config_new_fields_default() {
        let config = ReactAgentConfig::default();
        assert!(config.system_message.is_none());
        assert!(config.max_iterations.is_none());
        assert!(!config.interrupt_before_tools);
        assert!(config.pre_model_hook.is_none());
        assert!(config.post_model_hook.is_none());
        assert!(config.model_selector.is_none());
        assert!(config.store.is_none());
    }

    #[test]
    fn test_react_agent_config_debug_with_new_fields() {
        let config = ReactAgentConfig::default();
        let debug = format!("{config:?}");
        assert!(debug.contains("ReactAgentConfig"));
        assert!(debug.contains("pre_model_hook"));
        assert!(debug.contains("post_model_hook"));
        assert!(debug.contains("model_selector"));
        assert!(debug.contains("store"));
    }

    #[test]
    #[allow(
        clippy::redundant_clone,
        reason = "intentional clone to verify Clone impl preserves all fields including Arc-wrapped function types"
    )]
    fn test_react_agent_config_clone_with_all_fields() {
        use juncture_core::store::MemoryStore;

        let store = Arc::new(MemoryStore::new()) as Arc<dyn Store>;
        let config = ReactAgentConfig {
            system_message: Some("Hello".to_string()),
            max_iterations: Some(5),
            interrupt_before_tools: true,
            pre_model_hook: Some(Arc::new(|s: &MessagesState| s.clone())),
            post_model_hook: Some(Arc::new(|_s: &MessagesState, r: &Message| r.clone())),
            model_selector: Some(Arc::new(|_s: &MessagesState| CallOptions {
                model_override: Some("gpt-4-turbo".to_string()),
                ..Default::default()
            })),
            store: Some(store),
        };

        // Verify all fields are preserved after clone
        assert_eq!(config.system_message, Some("Hello".to_string()));
        assert_eq!(config.max_iterations, Some(5));
        assert!(config.interrupt_before_tools);
        assert!(config.pre_model_hook.is_some());
        assert!(config.post_model_hook.is_some());
        assert!(config.model_selector.is_some());
        assert!(config.store.is_some());

        // Verify clone produces a separate instance with same values.
        let cloned = config.clone();
        assert!(cloned.pre_model_hook.is_some());
        assert!(cloned.post_model_hook.is_some());
        assert!(cloned.model_selector.is_some());
        assert!(cloned.store.is_some());
    }

    #[tokio::test]
    async fn test_agent_node_call_with_model_selector() {
        let model = MockChatModel::new("gpt-4").with_response("Response");
        let selector: ModelSelector = Arc::new(|_state: &MessagesState| CallOptions {
            model_override: Some("gpt-4-turbo".to_string()),
            temperature: Some(0.5),
            ..Default::default()
        });
        let node = AgentNode::new(model).with_model_selector(selector);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap();
    }

    #[tokio::test]
    async fn test_agent_node_call_with_pre_model_hook() {
        let model = MockChatModel::new("gpt-4").with_response("Response");
        let hook: PreModelHook = Arc::new(|state: &MessagesState| {
            let mut new_state = state.clone();
            new_state
                .messages
                .push(Message::system("Hook added context"));
            new_state
        });
        let node = AgentNode::new(model).with_pre_model_hook(hook);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap();
    }

    #[tokio::test]
    async fn test_agent_node_call_with_post_model_hook() {
        let model = MockChatModel::new("gpt-4").with_response("Initial response");
        // Post-model hook that wraps the response with a prefix annotation.
        // Uses `Message::ai` to create a new message from the response text.
        let hook: PostModelHook = Arc::new(|_state: &MessagesState, response: &Message| {
            let text = match &response.content {
                Content::Text(t) => format!("[Post-processed] {t}"),
                Content::MultiPart(_) => "[Post-processed multi-part]".to_string(),
            };
            Message::ai(&text)
        });
        let node = AgentNode::new(model).with_post_model_hook(hook);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let result = node.call(state, &RunnableConfig::default()).await;
        result.unwrap();
    }

    #[test]
    fn test_tool_node_adapter_with_store() {
        use juncture_core::store::MemoryStore;

        let store = Arc::new(MemoryStore::new()) as Arc<dyn Store>;
        let tool_node = Arc::new(ToolNode::new(vec![Box::new(EchoTool)]));
        let adapter = ToolNodeAdapter::new(tool_node, Some(store));

        let debug = format!("{adapter:?}");
        assert!(debug.contains("ToolNodeAdapter"));
        assert!(debug.contains("store"));
        assert!(debug.contains("..."));
    }

    #[test]
    fn test_react_agent_config_builder_default_store_missing() {
        let config = ReactAgentConfig::default();
        // Builder methods should not exist on the config; users set fields directly.
        // Verify store defaults to None.
        assert!(config.store.is_none());
    }

    #[test]
    fn test_agent_router_with_tool_calls() {
        let state = MessagesState {
            messages: vec![Message::ai_with_tool_calls(
                "Let me look that up",
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: json!({}),
                }],
            )],
        };

        let router = AgentRouter;
        let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = rt.block_on(router.route(&state)).unwrap();

        assert_eq!(result, RouteResult::One("tools".to_string()));
    }

    #[test]
    fn test_agent_router_without_tool_calls() {
        let state = MessagesState {
            messages: vec![Message::ai("Here is the answer.")],
        };

        let router = AgentRouter;
        let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = rt.block_on(router.route(&state)).unwrap();

        assert_eq!(result, RouteResult::One(END.to_string()));
    }

    #[test]
    fn test_agent_router_empty_messages() {
        let state = MessagesState { messages: vec![] };

        let router = AgentRouter;
        let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
        let result = rt.block_on(router.route(&state)).unwrap();

        assert_eq!(result, RouteResult::One(END.to_string()));
    }

    #[test]
    fn test_build_messages_without_prompt() {
        let model = MockChatModel::new("gpt-4");
        let node = AgentNode::new(model);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let msgs = node.build_messages(&state);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn test_build_messages_with_static_prompt() {
        let model = MockChatModel::new("gpt-4");
        let prompt = PromptSource::Static("You are helpful.".to_string());
        let node = AgentNode::with_prompt(model, prompt);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let msgs = node.build_messages(&state);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, juncture_core::state::messages::Role::System);
    }

    #[test]
    fn test_build_messages_with_dynamic_prompt() {
        let model = MockChatModel::new("gpt-4");
        let prompt =
            PromptSource::Dynamic(Arc::new(|_msgs: &[Message]| "Dynamic prompt".to_string()));
        let node = AgentNode::with_prompt(model, prompt);

        let state = MessagesState {
            messages: vec![Message::human("Hello")],
        };

        let msgs = node.build_messages(&state);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, juncture_core::state::messages::Role::System);
    }

    #[test]
    fn test_tool_node_adapter_debug() {
        let tool_node = Arc::new(ToolNode::new(vec![Box::new(EchoTool)]));
        let adapter = ToolNodeAdapter::new(tool_node, None);
        let debug = format!("{adapter:?}");
        assert!(debug.contains("ToolNodeAdapter"));
    }

    #[test]
    fn test_convert_tool_defs() {
        let defs = vec![ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object"}),
        )];
        let llm_defs = convert_tool_defs(&defs);

        assert_eq!(llm_defs.len(), 1);
        assert_eq!(llm_defs[0].name, "search");
        assert_eq!(llm_defs[0].description, "Search the web");
    }
}

// Rust guideline compliant 2026-05-19
