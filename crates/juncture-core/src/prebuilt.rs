// Pre-built agent configurations
//
// This module provides pre-built agent configurations like `ReactAgentConfig`
// for common agent patterns.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::llm::{ChatModel, ResponseFormat};
use crate::node::Node;
use crate::state::State;
use crate::store::Store;

/// `ReAct` agent configuration
///
/// Configuration for creating `ReAct` (Reasoning + Acting) agents with tools.
#[allow(
    missing_debug_implementations,
    reason = "Contains Arc<dyn Node> and Arc<dyn Fn> which don't implement Debug"
)]
pub struct ReactAgentConfig<S: State, M: ChatModel> {
    /// LLM model
    pub model: M,
    /// List of tools
    pub tools: Vec<Box<dyn crate::Tool>>,
    /// System prompt
    pub prompt: Option<PromptSource<S>>,
    /// Response format for structured output
    pub response_format: Option<ResponseFormat>,
    /// Pre-model hook node
    pub pre_model_hook: Option<Arc<dyn Node<S>>>,
    /// Post-model hook node
    pub post_model_hook: Option<Arc<dyn Node<S>>>,
    /// State schema marker
    pub state_schema: PhantomData<S>,
    /// Store for cross-thread data
    pub store: Option<Arc<dyn Store>>,
    /// Interrupt before these nodes
    pub interrupt_before: Vec<String>,
    /// Interrupt after these nodes
    pub interrupt_after: Vec<String>,
    /// Dynamic model selector
    pub model_selector: Option<ModelSelector<S, M>>,
}

/// Type alias for model selector function
pub type ModelSelector<S, M> = Arc<dyn Fn(&S) -> M + Send + Sync>;

impl<S: State, M: ChatModel> ReactAgentConfig<S, M> {
    /// Create new `ReAct` agent configuration
    ///
    /// # Arguments
    ///
    /// * `model` - LLM model
    /// * `tools` - List of tools
    pub fn new(model: M, tools: Vec<Box<dyn crate::Tool>>) -> Self {
        Self {
            model,
            tools,
            prompt: None,
            response_format: None,
            pre_model_hook: None,
            post_model_hook: None,
            state_schema: PhantomData,
            store: None,
            interrupt_before: vec![],
            interrupt_after: vec![],
            model_selector: None,
        }
    }

    /// Set system prompt
    ///
    /// # Arguments
    ///
    /// * `prompt` - Prompt source
    #[must_use]
    pub fn with_prompt(mut self, prompt: PromptSource<S>) -> Self {
        self.prompt = Some(prompt);
        self
    }

    /// Set response format
    ///
    /// # Arguments
    ///
    /// * `format` - Response format
    #[must_use]
    pub fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    /// Set pre-model hook
    ///
    /// # Arguments
    ///
    /// * `hook` - Hook node
    #[must_use]
    pub fn with_pre_model_hook(mut self, hook: Arc<dyn Node<S>>) -> Self {
        self.pre_model_hook = Some(hook);
        self
    }

    /// Set post-model hook
    ///
    /// # Arguments
    ///
    /// * `hook` - Hook node
    #[must_use]
    pub fn with_post_model_hook(mut self, hook: Arc<dyn Node<S>>) -> Self {
        self.post_model_hook = Some(hook);
        self
    }

    /// Set store
    ///
    /// # Arguments
    ///
    /// * `store` - Store instance
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Set interrupt before
    ///
    /// # Arguments
    ///
    /// * `nodes` - List of node names
    #[must_use]
    pub fn with_interrupt_before(mut self, nodes: Vec<String>) -> Self {
        self.interrupt_before = nodes;
        self
    }

    /// Set interrupt after
    ///
    /// # Arguments
    ///
    /// * `nodes` - List of node names
    #[must_use]
    pub fn with_interrupt_after(mut self, nodes: Vec<String>) -> Self {
        self.interrupt_after = nodes;
        self
    }

    /// Set model selector
    ///
    /// # Arguments
    ///
    /// * `selector` - Function to select model based on state
    #[must_use]
    pub fn with_model_selector(mut self, selector: ModelSelector<S, M>) -> Self {
        self.model_selector = Some(selector);
        self
    }
}

/// Prompt source for agents
///
/// Can be a static string or a dynamic function.
#[allow(
    missing_debug_implementations,
    reason = "Contains Arc<dyn Fn> which doesn't implement Debug"
)]
pub enum PromptSource<S: State> {
    /// Static prompt string
    Static(String),
    /// Dynamic prompt function
    Dynamic(Arc<dyn Fn(&S) -> String + Send + Sync>),
}

// Rust guideline compliant 2026-05-19
