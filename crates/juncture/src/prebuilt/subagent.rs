//! Subagent delegation system for multi-agent workflows.
//!
//! This module provides [`AgentRegistry`] and [`SubagentTool`] for building
//! multi-agent systems where an orchestrator agent can dispatch tasks to
//! specialized sub-agents. Each sub-agent is a compiled graph registered
//! with a unique name, and the orchestrator invokes them via a tool call.
//!
//! # Architecture
//!
//! ```text
//! Orchestrator Agent (LLM)
//!     |
//!     v tool call: {"subagent_type": "researcher", "task": "..."}
//! SubagentTool
//!     |
//!     v lookup in registry
//! Registered Sub-Agent Graphs
//!     - researcher: CompiledGraph
//!     - coder: CompiledGraph
//!     - analyst: CompiledGraph
//! ```
//!
//! # Example
//!
//! ```ignore
//! use juncture::prebuilt::{
//!     create_react_agent, InMemoryAgentRegistry, MessagesState,
//! };
//! use juncture::llm::MockChatModel;
//! use juncture::tools::Tool;
//!
//! // Create sub-agent graphs
//! let researcher = create_react_agent(
//!     MockChatModel::new("gpt-4").with_response("Research complete."),
//!     vec![],
//! )?;
//!
//! // Register sub-agents
//! let mut registry = InMemoryAgentRegistry::new();
//! registry.register("researcher".to_string(), researcher.into());
//!
//! // Create orchestrator with subagent tool
//! let subagent_tool = SubagentTool::new(registry);
//! let orchestrator = create_react_agent(
//!     MockChatModel::new("gpt-4").with_response("Task delegated."),
//!     vec![Box::new(subagent_tool)],
//! )?;
//! ```

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use juncture_core::config::RunnableConfig;
use juncture_core::graph::CompiledGraph;
use juncture_core::state::messages::{Content, Message, Role};
use serde_json::json;
use tokio::sync::RwLock;

use crate::prebuilt::messages_state::MessagesState;
use crate::tools::{Tool, ToolError};

/// Boxed future for async agent invocation.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type alias for the agent invocation function.
type AgentInvokeFn = dyn Fn(MessagesState, &RunnableConfig) -> BoxFuture<'static, Result<MessagesState, SubagentError>>
    + Send
    + Sync;

/// Error type for subagent operations.
///
/// Represents failures that can occur when delegating tasks to sub-agents.
#[derive(Debug, thiserror::Error)]
pub enum SubagentError {
    /// Agent not found in registry.
    ///
    /// Occurs when the orchestrator requests a sub-agent type that
    /// has not been registered.
    #[error("agent not found: {0}")]
    NotFound(String),

    /// Agent execution failed.
    ///
    /// Occurs when the sub-agent graph execution encounters an error.
    #[error("agent invocation failed: {0}")]
    InvocationFailed(String),

    /// No agents registered.
    ///
    /// Occurs when attempting to list agents from an empty registry.
    #[error("no agents registered")]
    Empty,
}

/// Trait for types that can be converted into an [`AgentEntry`].
///
/// This trait enables [`CompiledGraph`] instances to be stored in the
/// agent registry without requiring the registry to be generic over
/// the graph's internal types.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::{IntoAgentEntry, AgentEntry};
/// use juncture::prebuilt::MessagesState;
///
/// let graph: CompiledGraph<MessagesState> = create_react_agent(...)?;
/// let entry: AgentEntry = graph.into_agent_entry();
/// ```
pub trait IntoAgentEntry: Send + Sync + 'static {
    /// Get the agent description.
    ///
    /// Returns a human-readable description of what this agent does.
    fn description(&self) -> String;

    /// Invoke the agent with the given state and config.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError`] if the agent execution fails.
    fn invoke_boxed<'a>(
        &'a self,
        state: MessagesState,
        config: &'a RunnableConfig,
    ) -> BoxFuture<'a, Result<MessagesState, SubagentError>>;
}

/// Wrapper for a compiled graph that can be stored in an agent registry.
///
/// `AgentEntry` wraps a compiled graph with a type-erased invocation function.
/// This allows different graph types to be stored together in the same
/// registry without requiring the registry to be generic.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::{AgentEntry, IntoAgentEntry};
/// use std::sync::Arc;
///
/// let graph: CompiledGraph<MessagesState> = create_react_agent(...)?;
/// let entry = AgentEntry::from_graph(graph);
/// ```
pub struct AgentEntry {
    /// Human-readable description of what this agent does.
    pub description: String,

    /// Type-erased invocation function.
    ///
    /// This closure captures the compiled graph and invokes it when called.
    invoke_fn: Arc<AgentInvokeFn>,
}

impl AgentEntry {
    /// Create a new agent entry from an invocation function.
    ///
    /// # Arguments
    ///
    /// * `description` - Human-readable description of the agent.
    /// * `invoke_fn` - Async function that invokes the agent graph.
    #[must_use]
    pub fn new<F>(description: String, invoke_fn: Arc<F>) -> Self
    where
        F: Fn(
                MessagesState,
                &RunnableConfig,
            ) -> BoxFuture<'static, Result<MessagesState, SubagentError>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            description,
            invoke_fn,
        }
    }

    /// Create an agent entry from a graph implementing [`IntoAgentEntry`].
    ///
    /// # Arguments
    ///
    /// * `graph` - A compiled graph or type implementing [`IntoAgentEntry`].
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::prebuilt::{AgentEntry, create_react_agent};
    ///
    /// let graph = create_react_agent(model, tools)?;
    /// let entry = AgentEntry::from_graph(graph);
    /// ```
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "Complex type is required for type-erased async function storage"
    )]
    pub fn from_graph<T: IntoAgentEntry + Clone + 'static>(graph: T) -> Self {
        let description = graph.description();

        let invoke_fn: Arc<
            dyn Fn(
                    MessagesState,
                    &RunnableConfig,
                )
                    -> Pin<Box<dyn Future<Output = Result<MessagesState, SubagentError>> + Send>>
                + Send
                + Sync,
        > = Arc::new(move |state: MessagesState, config: &RunnableConfig| {
            let graph = graph.clone();
            let config = config.clone();
            Box::pin(async move {
                graph
                    .invoke_boxed(state, &config)
                    .await
                    .map_err(|e| SubagentError::InvocationFailed(e.to_string()))
            })
        });

        Self {
            description,
            invoke_fn,
        }
    }

    /// Get the agent description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Invoke the agent graph with the given state and configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::InvocationFailed`] if the graph execution fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::prebuilt::{AgentEntry, MessagesState};
    /// use juncture_core::RunnableConfig;
    ///
    /// # async fn example(entry: AgentEntry) -> Result<(), Box<dyn std::error::Error>> {
    /// let state = MessagesState::default();
    /// let config = RunnableConfig::default();
    /// let result_state = entry.invoke(state, &config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn invoke(
        &self,
        state: MessagesState,
        config: &RunnableConfig,
    ) -> Result<MessagesState, SubagentError> {
        (self.invoke_fn)(state, config).await
    }
}

impl fmt::Debug for AgentEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentEntry")
            .field("description", &self.description)
            .field("invoke_fn", &"<fn>")
            .finish()
    }
}

/// Trait for managing registered sub-agent graphs.
///
/// An agent registry maintains a collection of named sub-agent graphs
/// that can be dispatched to by an orchestrator agent. Implementations
/// can use in-memory storage, persistent storage, or remote registries.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::{AgentRegistry, InMemoryAgentRegistry, AgentEntry};
///
/// let mut registry = InMemoryAgentRegistry::new();
/// registry.register("researcher".to_string(), agent_entry);
///
/// if let Some(entry) = registry.get("researcher") {
///     let description = entry.description();
///     assert_eq!(description, "Researches topics");
/// }
///
/// let agents = registry.list();
/// assert!(agents.contains(&"researcher".to_string()));
/// ```
pub trait AgentRegistry: Send + Sync {
    /// Get an agent entry by name.
    ///
    /// Returns `None` if no agent with the given name is registered.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the agent to retrieve.
    ///
    /// # Returns
    ///
    /// `Some(AgentEntry)` if found, `None` otherwise.
    fn get(&self, name: &str) -> Option<AgentEntry>;

    /// List all registered agent names.
    ///
    /// Returns a vector of agent names in the registry. The order is
    /// implementation-dependent.
    ///
    /// # Returns
    ///
    /// A vector of registered agent names.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Empty`] if no agents are registered.
    fn list(&self) -> Vec<String>;

    /// Register a new agent.
    ///
    /// # Arguments
    ///
    /// * `name` - Unique name for this agent.
    /// * `entry` - The agent entry to register.
    ///
    /// # Note
    ///
    /// If an agent with the same name already exists, it should be
    /// replaced with the new entry.
    fn register(&mut self, name: String, entry: AgentEntry);
}

/// In-memory implementation of [`AgentRegistry`].
///
/// Stores agent entries in a `HashMap`. This is the default registry
/// implementation for single-process applications.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::{InMemoryAgentRegistry, AgentEntry};
///
/// let mut registry = InMemoryAgentRegistry::new();
/// registry.register("researcher".to_string(), agent_entry);
/// ```
#[derive(Debug, Default)]
pub struct InMemoryAgentRegistry {
    /// Map of agent name to agent entry.
    agents: HashMap<String, AgentEntry>,
}

impl InMemoryAgentRegistry {
    /// Create a new empty registry.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::prebuilt::InMemoryAgentRegistry;
    ///
    /// let registry = InMemoryAgentRegistry::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }
}

impl AgentRegistry for InMemoryAgentRegistry {
    fn get(&self, name: &str) -> Option<AgentEntry> {
        // Clone the Arc inside AgentEntry for thread safety
        self.agents.get(name).map(|entry| AgentEntry {
            description: entry.description.clone(),
            invoke_fn: Arc::clone(&entry.invoke_fn),
        })
    }

    fn list(&self) -> Vec<String> {
        let names: Vec<String> = self.agents.keys().cloned().collect();

        if names.is_empty() {
            // Return empty vector instead of error for list() operation
            // This is more ergonomic for callers
        }

        names
    }

    fn register(&mut self, name: String, entry: AgentEntry) {
        self.agents.insert(name, entry);
    }
}

/// Tool that delegates tasks to registered sub-agents.
///
/// When invoked by an LLM, this tool looks up the requested sub-agent
/// type in the registry, creates a fresh [`MessagesState`] with the
/// task as a human message, invokes the sub-agent graph, and returns
/// the last AI message content as the result.
///
/// The tool name is "task" following the deer-flow/deepagents convention.
///
/// # Example
///
/// ```ignore
/// use juncture::prebuilt::{InMemoryAgentRegistry, SubagentTool};
/// use juncture::tools::Tool;
///
/// let registry = InMemoryAgentRegistry::new();
/// // Register agents...
///
/// let tool = SubagentTool::new(registry);
/// let definition = tool.definition();
/// assert_eq!(definition.name, "task");
/// ```
pub struct SubagentTool {
    /// Registry of available sub-agents.
    registry: Arc<RwLock<dyn AgentRegistry>>,
}

impl SubagentTool {
    /// Create a new subagent tool.
    ///
    /// # Arguments
    ///
    /// * `registry` - The registry containing available sub-agents.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use juncture::prebuilt::{InMemoryAgentRegistry, SubagentTool};
    ///
    /// let registry = InMemoryAgentRegistry::new();
    /// let tool = SubagentTool::new(registry);
    /// ```
    #[must_use]
    pub fn new<R: AgentRegistry + 'static>(registry: R) -> Self {
        Self {
            registry: Arc::new(RwLock::new(registry)),
        }
    }
}

impl fmt::Debug for SubagentTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubagentTool")
            .field("registry", &"<registry>")
            .finish()
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Delegate a task to a specialized sub-agent. Available agents are listed in the registry."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "subagent_type": {
                    "type": "string",
                    "description": "The type/name of the sub-agent to invoke"
                },
                "task": {
                    "type": "string",
                    "description": "The task description to delegate to the sub-agent"
                }
            },
            "required": ["subagent_type", "task"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        // Parse input
        let subagent_type = input["subagent_type"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'subagent_type' field".to_string()))?;

        let task = input["task"]
            .as_str()
            .ok_or_else(|| ToolError::invalid_input("Missing 'task' field".to_string()))?;

        // Get the agent from registry
        let entry = {
            let registry = self.registry.read().await;
            registry.get(subagent_type).ok_or_else(|| {
                ToolError::execution_failed(format!("Sub-agent not found: {subagent_type}"))
            })?
        };

        // Create initial state with task as human message
        let initial_state = MessagesState {
            messages: vec![Message::human(task)],
        };

        // Create config for sub-agent execution
        let config = RunnableConfig::default();

        // Invoke the sub-agent
        let result_state = entry
            .invoke(initial_state, &config)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;

        // Extract the last AI message content
        #[expect(
            clippy::map_unwrap_or,
            reason = "unwrap_or_else is needed because the default value constructs a new String"
        )]
        let result_text = result_state
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::Ai))
            .map(|m| match &m.content {
                Content::Text(t) => t.clone(),
                Content::MultiPart(parts) => {
                    // For multi-part content, join all text parts
                    parts
                        .iter()
                        .filter_map(|p| match p {
                            juncture_core::state::messages::ContentPart::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                }
            })
            .unwrap_or_else(|| "Sub-agent completed with no output".to_string());

        Ok(result_text)
    }
}

// Implement IntoAgentEntry for CompiledGraph<MessagesState>
impl IntoAgentEntry for CompiledGraph<MessagesState> {
    fn description(&self) -> String {
        "Compiled agent graph".to_string()
    }

    fn invoke_boxed<'a>(
        &'a self,
        state: MessagesState,
        config: &'a RunnableConfig,
    ) -> BoxFuture<'a, Result<MessagesState, SubagentError>> {
        Box::pin(async move {
            let output = self
                .invoke_async(state, config)
                .await
                .map_err(|e| SubagentError::InvocationFailed(e.to_string()))?;
            Ok(output.value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a mock agent entry for testing.
    fn mock_agent_entry(response_text: &str) -> AgentEntry {
        let response = response_text.to_string();

        let invoke_fn = Arc::new(
            move |_state: MessagesState,
                  _config: &RunnableConfig|
                  -> BoxFuture<'static, Result<MessagesState, SubagentError>> {
                let response = response.clone();
                Box::pin(async move {
                    let mut state = MessagesState::default();
                    state.messages.push(Message::ai(&response));
                    Ok(state)
                })
            },
        );

        AgentEntry::new("Mock agent for testing".to_string(), invoke_fn)
    }

    #[test]
    fn test_in_memory_registry_new() {
        let registry = InMemoryAgentRegistry::new();
        assert!(registry.agents.is_empty());
    }

    #[test]
    fn test_in_memory_registry_register_and_get() {
        let mut registry = InMemoryAgentRegistry::new();
        let entry = mock_agent_entry("Test response");

        registry.register("test_agent".to_string(), entry);

        let retrieved = registry.get("test_agent");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().description, "Mock agent for testing");
    }

    #[test]
    fn test_in_memory_registry_list() {
        let mut registry = InMemoryAgentRegistry::new();
        let entry1 = mock_agent_entry("Response 1");
        let entry2 = mock_agent_entry("Response 2");

        registry.register("agent1".to_string(), entry1);
        registry.register("agent2".to_string(), entry2);

        let names = registry.list();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"agent1".to_string()));
        assert!(names.contains(&"agent2".to_string()));
    }

    #[test]
    fn test_in_memory_registry_not_found() {
        let registry = InMemoryAgentRegistry::new();
        let result = registry.get("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_agent_entry_description() {
        let entry = mock_agent_entry("response");
        assert_eq!(entry.description(), "Mock agent for testing");
    }

    #[tokio::test]
    async fn test_agent_entry_invoke() {
        let entry = mock_agent_entry("Hello from agent");
        let state = MessagesState::default();
        let config = RunnableConfig::default();

        let result = entry.invoke(state, &config).await.unwrap();
        assert_eq!(result.messages.len(), 1);
        assert!(matches!(result.messages[0].role, Role::Ai));
    }

    #[test]
    fn test_subagent_tool_definition() {
        let registry = InMemoryAgentRegistry::new();
        let tool = SubagentTool::new(registry);

        let def = tool.definition();
        assert_eq!(def.name, "task");
        assert!(def.description.contains("Delegate"));

        // Verify schema structure
        assert_eq!(def.parameters["type"], "object");
        assert!(def.parameters["properties"]["subagent_type"]["type"] == "string");
        assert!(def.parameters["properties"]["task"]["type"] == "string");
    }

    #[tokio::test]
    async fn test_subagent_tool_invoke_success() {
        let mut registry = InMemoryAgentRegistry::new();
        registry.register(
            "test_agent".to_string(),
            mock_agent_entry("Agent completed task"),
        );

        let tool = SubagentTool::new(registry);
        let input = json!({
            "subagent_type": "test_agent",
            "task": "Do something"
        });

        let result = tool.invoke(input).await.unwrap();
        assert_eq!(result, "Agent completed task");
    }

    #[tokio::test]
    async fn test_subagent_tool_agent_not_found() {
        let registry = InMemoryAgentRegistry::new();
        let tool = SubagentTool::new(registry);

        let input = json!({
            "subagent_type": "nonexistent",
            "task": "Test"
        });

        let result = tool.invoke(input).await;
        let _ = result.unwrap_err();
    }

    #[tokio::test]
    async fn test_subagent_tool_missing_subagent_type() {
        let registry = InMemoryAgentRegistry::new();
        let tool = SubagentTool::new(registry);

        let input = json!({
            "task": "Test"
        });

        let result = tool.invoke(input).await;
        let _ = result.unwrap_err();
    }

    #[tokio::test]
    async fn test_subagent_tool_missing_task() {
        let registry = InMemoryAgentRegistry::new();
        let tool = SubagentTool::new(registry);

        let input = json!({
            "subagent_type": "test"
        });

        let result = tool.invoke(input).await;
        let _ = result.unwrap_err();
    }

    #[test]
    fn test_subagent_error_display() {
        let err = SubagentError::NotFound("test_agent".to_string());
        assert!(err.to_string().contains("agent not found"));
        assert!(err.to_string().contains("test_agent"));

        let err = SubagentError::InvocationFailed("execution error".to_string());
        assert!(err.to_string().contains("invocation failed"));

        let err = SubagentError::Empty;
        assert!(err.to_string().contains("no agents registered"));
    }

    #[test]
    fn test_agent_entry_from_graph_closure() {
        // Test that the closure capture works correctly
        let description = "Test agent".to_string();
        let response = "Test response".to_string();

        let invoke_fn = Arc::new(
            move |_state: MessagesState,
                  _config: &RunnableConfig|
                  -> BoxFuture<'static, Result<MessagesState, SubagentError>> {
                let response = response.clone();
                Box::pin(async move {
                    let mut state = MessagesState::default();
                    state.messages.push(Message::ai(&response));
                    Ok(state)
                })
            },
        );

        let entry = AgentEntry::new(description, invoke_fn);
        assert_eq!(entry.description(), "Test agent");
    }

    #[test]
    fn test_agent_entry_debug() {
        let entry = mock_agent_entry("response");
        let debug_str = format!("{entry:?}");
        assert!(debug_str.contains("AgentEntry"));
        assert!(debug_str.contains("description"));
        assert!(debug_str.contains("Mock agent for testing"));
    }

    #[test]
    fn test_subagent_tool_debug() {
        let registry = InMemoryAgentRegistry::new();
        let tool = SubagentTool::new(registry);
        let debug_str = format!("{tool:?}");
        assert!(debug_str.contains("SubagentTool"));
    }

    #[tokio::test]
    async fn test_subagent_tool_result_extraction_multi_part() {
        let mut registry = InMemoryAgentRegistry::new();

        // Create an agent that returns multi-part content
        let invoke_fn = Arc::new(
            |_state: MessagesState,
             _config: &RunnableConfig|
             -> BoxFuture<'static, Result<MessagesState, SubagentError>> {
                Box::pin(async move {
                    use juncture_core::state::messages::ContentPart;
                    let mut state = MessagesState::default();
                    state.messages.push(Message {
                        id: uuid::Uuid::new_v4().to_string(),
                        role: Role::Ai,
                        content: Content::MultiPart(vec![
                            ContentPart::Text {
                                text: "Part 1".to_string(),
                            },
                            ContentPart::Text {
                                text: "Part 2".to_string(),
                            },
                        ]),
                        tool_calls: vec![],
                        tool_call_id: None,
                        name: None,
                        usage: None,
                    });
                    Ok(state)
                })
            },
        );
        let entry = AgentEntry::new("Multi-part agent".to_string(), invoke_fn);
        registry.register("multi_agent".to_string(), entry);

        let tool = SubagentTool::new(registry);
        let input = json!({
            "subagent_type": "multi_agent",
            "task": "Test"
        });

        let result = tool.invoke(input).await.unwrap();
        assert!(result.contains("Part 1"));
        assert!(result.contains("Part 2"));
    }

    #[tokio::test]
    async fn test_subagent_tool_result_extraction_no_ai_message() {
        let mut registry = InMemoryAgentRegistry::new();

        // Create an agent that returns no AI messages
        let invoke_fn = Arc::new(
            |_state: MessagesState,
             _config: &RunnableConfig|
             -> BoxFuture<'static, Result<MessagesState, SubagentError>> {
                Box::pin(async move { Ok(MessagesState::default()) })
            },
        );
        let entry = AgentEntry::new("Empty agent".to_string(), invoke_fn);
        registry.register("empty_agent".to_string(), entry);

        let tool = SubagentTool::new(registry);
        let input = json!({
            "subagent_type": "empty_agent",
            "task": "Test"
        });

        let result = tool.invoke(input).await.unwrap();
        assert_eq!(result, "Sub-agent completed with no output");
    }
}

// Rust guideline compliant 2026-05-26
