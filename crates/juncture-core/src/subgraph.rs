//! Subgraph support for nested graph execution
//!
//! Provides types and utilities for embedding graphs as nodes within other graphs.
//! Subgraphs enable modular composition and reusable graph components.

use crate::{State, command::Command, config::RunnableConfig, error::JunctureError, node::Node};
use std::sync::Arc;

/// Compile-time constraint for shared-state subgraph mode
///
/// This trait defines the relationship between a parent graph's state and
/// a subgraph's state when they share state fields. It enables type-safe
/// state transformation between parent and child graphs.
///
/// # Type Parameters
///
/// * `Parent` - The parent graph's state type
///
/// # Examples
///
/// ```ignore
/// use juncture_core::State;
///
/// struct ParentState {
///     name: String,
///     age: u32,
/// }
///
/// struct ChildState {
///     name: String,
/// }
///
/// impl StateSubset<ParentState> for ChildState {
///     fn extract(parent: &ParentState) -> Self {
///         Self { name: parent.name.clone() }
///     }
///
///     fn map_update(update: Self::Update) -> ParentState::Update {
///         // Map child update to parent update
///         ParentStateUpdate { name: update.name }
///     }
/// }
/// ```
pub trait StateSubset<Parent: State>: State {
    /// Extract subgraph state from parent state
    ///
    /// This method transforms the parent graph's state into the subgraph's
    /// state type, typically by copying or projecting relevant fields.
    ///
    /// # Arguments
    ///
    /// * `parent` - Reference to the parent state
    ///
    /// # Returns
    ///
    /// The subgraph state
    fn extract(parent: &Parent) -> Self;

    /// Map subgraph update to parent update
    ///
    /// This method transforms a subgraph state update into a parent state
    /// update, allowing changes made in the subgraph to be applied to the
    /// parent graph's state.
    ///
    /// # Arguments
    ///
    /// * `update` - The subgraph's state update
    ///
    /// # Returns
    ///
    /// The parent graph's state update
    fn map_update(update: Self::Update) -> Parent::Update;
}

/// Configuration for subgraph execution
///
/// Defines how a subgraph interacts with checkpointing and state management.
#[derive(Clone, Debug, Default)]
pub struct SubgraphConfig {
    /// Checkpoint persistence mode
    pub persistence: SubgraphPersistence,
}

/// Checkpoint persistence mode for subgraphs
///
/// Determines how subgraph state is persisted and isolated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SubgraphPersistence {
    /// Inherit persistence from parent graph
    #[default]
    Inherit,

    /// Use per-thread checkpoint isolation
    PerThread,

    /// Disable checkpointing for this subgraph
    Stateless,
}

/// A mounted subgraph ready for execution as a node
///
/// Contains the compiled subgraph and configuration needed to execute it
/// as a node within a parent graph.
pub struct SubgraphMount<S: State> {
    /// Name of the subgraph mount point
    pub name: String,

    /// Subgraph configuration
    pub config: SubgraphConfig,

    /// Type-erased subgraph node implementation
    pub node: Arc<dyn Node<S>>,
}

impl<S: State> std::fmt::Debug for SubgraphMount<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubgraphMount")
            .field("name", &self.name)
            .field("config", &self.config)
            .field("node", &"<node>")
            .finish()
    }
}

impl<S: State> SubgraphMount<S> {
    /// Create a new subgraph mount
    #[must_use]
    pub fn new(name: impl Into<String>, config: SubgraphConfig, node: Arc<dyn Node<S>>) -> Self {
        Self {
            name: name.into(),
            config,
            node,
        }
    }
}

/// Subgraph node wrapper for type erasure
///
/// When a subgraph has a different state type than its parent graph,
/// this wrapper handles the state transformation via `input_map` and `output_map`.
#[allow(
    dead_code,
    reason = "will be used when subgraph support is fully implemented"
)]
pub struct SubgraphNode<S: State, Sub: State> {
    /// Compiled subgraph to execute
    pub subgraph: Arc<crate::graph::CompiledGraph<Sub>>,

    /// Subgraph name for logging
    pub name: String,

    /// Transform parent state to subgraph input
    #[allow(
        clippy::type_complexity,
        reason = "requires type erasure for trait object"
    )]
    pub input_map: Arc<dyn Fn(&S) -> Sub + Send + Sync>,

    /// Transform subgraph output to parent state update
    #[allow(
        clippy::type_complexity,
        reason = "requires type erasure for trait object"
    )]
    pub output_map: Arc<dyn Fn(&Sub) -> S::Update + Send + Sync>,

    /// Subgraph configuration
    pub config: SubgraphConfig,
}

impl<S: State, Sub: State> std::fmt::Debug for SubgraphNode<S, Sub> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubgraphNode")
            .field("subgraph", &"<graph>")
            .field("name", &self.name)
            .field("input_map", &"<fn>")
            .field("output_map", &"<fn>")
            .field("config", &self.config)
            .finish()
    }
}

impl<S: State, Sub: State> SubgraphNode<S, Sub> {
    /// Create a new subgraph node
    #[must_use]
    #[allow(
        dead_code,
        reason = "will be used when subgraph support is fully implemented"
    )]
    #[allow(
        clippy::type_complexity,
        reason = "requires type erasure for trait object"
    )]
    pub fn new(
        subgraph: Arc<crate::graph::CompiledGraph<Sub>>,
        name: String,
        #[allow(
            clippy::type_complexity,
            reason = "requires type erasure for trait object"
        )]
        input_map: Arc<dyn Fn(&S) -> Sub + Send + Sync>,
        #[allow(
            clippy::type_complexity,
            reason = "requires type erasure for trait object"
        )]
        output_map: Arc<dyn Fn(&Sub) -> S::Update + Send + Sync>,
        config: SubgraphConfig,
    ) -> Self {
        Self {
            subgraph,
            name,
            input_map,
            output_map,
            config,
        }
    }
}

impl<S: State, Sub: State> Node<S> for SubgraphNode<S, Sub> {
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let config = config.clone();
        Box::pin(async move {
            // Transform parent state to subgraph input
            let sub_input = (self.input_map)(&state);

            // Execute subgraph
            let sub_output = self
                .subgraph
                .invoke(sub_input, &config)
                .map_err(|e| JunctureError::subgraph(format!("{}: {}", self.name, e)))?;

            // Transform subgraph output back to parent state update
            let update = (self.output_map)(&sub_output.value);

            Ok(Command::update(update))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::IntoNode, node::NodeFnUpdate};

    #[test]
    fn test_subgraph_config_default() {
        let config = SubgraphConfig::default();
        assert_eq!(config.persistence, SubgraphPersistence::Inherit);
    }

    #[test]
    fn test_subgraph_persistence_variants() {
        let inherit = SubgraphPersistence::Inherit;
        let per_thread = SubgraphPersistence::PerThread;
        let stateless = SubgraphPersistence::Stateless;

        assert_ne!(inherit, per_thread);
        assert_ne!(inherit, stateless);
        assert_ne!(per_thread, stateless);
    }

    #[test]
    fn test_subgraph_mount_creation() {
        let node = mock_node("test");
        let mount = SubgraphMount::new("subgraph_test", SubgraphConfig::default(), node);

        assert_eq!(mount.name, "subgraph_test");
        assert_eq!(mount.config.persistence, SubgraphPersistence::Inherit);
    }

    fn mock_node(name: &str) -> Arc<dyn crate::Node<StateDummy>> {
        NodeFnUpdate(|_s: StateDummy| async move { Ok(StateDummyUpdate) }).into_node(name)
    }

    #[derive(Clone, Debug)]
    struct StateDummy;

    impl crate::State for StateDummy {
        type Update = StateDummyUpdate;
        type FieldVersions = ();

        fn apply(&mut self, _update: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct StateDummyUpdate;
}

/// Subgraph stream event transformer with namespace and filter
///
/// Transforms stream events from subgraph execution by adding namespace
/// prefixes and filtering events based on configuration.
#[derive(Clone, Debug)]
pub struct SubgraphTransformer {
    /// Subgraph name for namespace prefix
    pub subgraph_name: String,

    /// Current namespace stack
    pub ns: Vec<String>,

    /// Optional filter for event types
    pub filter: Option<Vec<String>>,

    /// Whether to include internal events
    pub include_internal: bool,
}

impl SubgraphTransformer {
    /// Create a new subgraph transformer
    ///
    /// # Arguments
    ///
    /// * `subgraph_name` - Name of the subgraph for namespace prefix
    #[must_use]
    pub const fn new(subgraph_name: String) -> Self {
        Self {
            subgraph_name,
            ns: Vec::new(),
            filter: None,
            include_internal: false,
        }
    }

    /// Set event filter
    ///
    /// # Arguments
    ///
    /// * `filter` - List of event types to include (None means all events)
    #[must_use]
    pub fn with_filter(mut self, filter: Vec<String>) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Set whether to include internal events
    ///
    /// # Arguments
    ///
    /// * `include` - Whether to include internal events
    #[must_use]
    pub const fn with_internal(mut self, include: bool) -> Self {
        self.include_internal = include;
        self
    }

    /// Transform a stream event by adding namespace
    ///
    /// This method adds namespace prefixes to events and filters based
    /// on the configured filter.
    ///
    /// # Arguments
    ///
    /// * `event` - The stream event to transform
    ///
    /// # Returns
    ///
    /// `Some(event)` if the event passes the filter, `None` otherwise
    #[must_use]
    #[allow(
        dead_code,
        reason = "will be used when stream transformation is fully implemented"
    )]
    pub fn transform<S: State>(
        &self,
        event: &crate::stream::StreamEvent<S>,
    ) -> Option<crate::stream::StreamEvent<S>> {
        // Apply filter if configured
        if let Some(ref filter) = self.filter {
            let event_type = Self::get_event_type(event);
            if !filter.contains(&event_type) {
                return None;
            }
        }

        // Return cloned event with namespace applied
        // Full namespace transformation will be implemented with stream processing
        Some(event.clone())
    }

    /// Add a namespace segment
    ///
    /// # Arguments
    ///
    /// * `segment` - The namespace segment to add
    pub fn add_namespace(&mut self, segment: String) {
        self.ns.push(segment);
    }

    /// Get the event type for filtering
    ///
    /// # Arguments
    ///
    /// * `event` - The stream event
    ///
    /// # Returns
    ///
    /// The event type as a string
    #[must_use]
    fn get_event_type<S: State>(event: &crate::stream::StreamEvent<S>) -> String {
        match event {
            crate::stream::StreamEvent::Values { .. } => "values".to_string(),
            crate::stream::StreamEvent::Updates { .. } => "updates".to_string(),
            crate::stream::StreamEvent::Messages { .. } => "messages".to_string(),
            crate::stream::StreamEvent::Custom { .. } => "custom".to_string(),
            crate::stream::StreamEvent::TaskStart { .. } => "task_start".to_string(),
            crate::stream::StreamEvent::TaskEnd { .. } => "task_end".to_string(),
            crate::stream::StreamEvent::Interrupt { .. } => "interrupt".to_string(),
            crate::stream::StreamEvent::BudgetExceeded { .. } => "budget_exceeded".to_string(),
            crate::stream::StreamEvent::End { .. } => "end".to_string(),
            crate::stream::StreamEvent::Debug(_) => "debug".to_string(),
            crate::stream::StreamEvent::Tools(_) => "tools".to_string(),
            crate::stream::StreamEvent::CheckpointSaved { .. } => "checkpoint_saved".to_string(),
            crate::stream::StreamEvent::TaskDetail { .. } => "task_detail".to_string(),
        }
    }
}

// Rust guideline compliant 2026-05-19
