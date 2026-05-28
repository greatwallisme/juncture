//! Prebuilt agent patterns for common LLM workflows.
//!
//! This module provides ready-to-use agent implementations that follow
//! established patterns like `ReAct` (Reason-Act). These agents handle the
//! boilerplate of graph construction, node wiring, and conditional routing,
//! allowing you to focus on configuring models and tools.
//!
//! # Available Agents
//!
//! - **`ReAct` Agent**: [`create_react_agent`] builds an agent that alternates
//!   between LLM reasoning and tool execution. Use it when you want a
//!   straightforward agent that can call tools and iterate.
//!
//! # State Type
//!
//! All prebuilt agents use [`MessagesState`], a simple state type with a
//! single `messages` field using reducer-based merge semantics.
//!
//! # Example
//!
//! ```ignore
//! use juncture::llm::{ChatModel, MockChatModel};
//! use juncture::prebuilt::{create_react_agent, MessagesState, ReactAgentConfig};
//! use juncture::tools::Tool;
//!
//! let model = MockChatModel::new("gpt-4").with_response("Done!");
//! let tools: Vec<Box<dyn Tool>> = vec![];
//!
//! // Basic usage
//! let agent = create_react_agent(model, tools)?;
//!
//! // With configuration
//! let model = MockChatModel::new("gpt-4").with_response("Done!");
//! let config = ReactAgentConfig {
//!     system_message: Some("You are a helpful assistant.".to_string()),
//!     ..Default::default()
//! };
//! let agent = create_react_agent_with_config(model, tools, config)?;
//! ```

mod agent_factory;
pub mod agent_middleware;
mod messages_state;
mod react;
mod subagent;

pub use agent_factory::{AgentConfig, create_agent_with_middleware};
pub use agent_middleware::{
    AgentMiddleware, AgentMiddlewareChain, LoopDetectionMiddleware, MiddlewareAction,
    NopMiddleware, ToolErrorHandlingMiddleware,
};
pub use messages_state::MessagesState;
pub use react::{
    AgentNode, PromptSource, ReactAgentConfig, create_agent, create_agent_with_config,
    create_react_agent, create_react_agent_with_config,
};
pub use subagent::{
    AgentEntry, AgentRegistry, InMemoryAgentRegistry, IntoAgentEntry, SubagentError, SubagentTool,
};

// Rust guideline compliant 2026-05-19
