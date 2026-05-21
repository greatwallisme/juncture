//! Subgraph support for nested graph execution
//!
//! Provides types and utilities for embedding graphs as nodes within other graphs.
//! Subgraphs enable modular composition and reusable graph components.

use crate::{
    State, checkpoint::CHECKPOINT_NS_SEPARATOR, command::Command, config::RunnableConfig,
    error::JunctureError, node::Node,
};
use std::sync::Arc;

/// Compute the child checkpoint namespace based on persistence mode.
///
/// Returns `None` for [`SubgraphPersistence::Stateless`] (no checkpointing),
/// a stable `|name:thread_id` namespace for [`SubgraphPersistence::PerThread`],
/// and a fresh UUID-based namespace for [`SubgraphPersistence::Inherit`].
fn compute_child_namespace(
    persistence: SubgraphPersistence,
    name: &str,
    parent_ns: Option<&str>,
    thread_id: Option<&str>,
) -> Option<String> {
    match persistence {
        SubgraphPersistence::Stateless => None,
        SubgraphPersistence::PerThread => {
            let thread_key = thread_id.unwrap_or("default");
            Some(parent_ns.map_or_else(
                || format!("{CHECKPOINT_NS_SEPARATOR}{name}:{thread_key}"),
                |ns| format!("{ns}{CHECKPOINT_NS_SEPARATOR}{name}:{thread_key}"),
            ))
        }
        SubgraphPersistence::Inherit => {
            let invocation_id = uuid::Uuid::new_v4().to_string();
            Some(parent_ns.map_or_else(
                || format!("{CHECKPOINT_NS_SEPARATOR}{name}:{invocation_id}"),
                |ns| format!("{ns}{CHECKPOINT_NS_SEPARATOR}{name}:{invocation_id}"),
            ))
        }
    }
}

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

    /// Set or change the subgraph mount name.
    ///
    /// Consumes and returns `Self` for fluent chaining.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Replace the subgraph configuration.
    ///
    /// Consumes and returns `Self` for fluent chaining.
    #[must_use]
    pub const fn with_config(mut self, config: SubgraphConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the checkpoint persistence mode on the config.
    ///
    /// Convenience builder that updates `self.config.persistence`
    /// without replacing the entire `SubgraphConfig`.
    /// Consumes and returns `Self` for fluent chaining.
    #[must_use]
    pub const fn with_persistence(mut self, persistence: SubgraphPersistence) -> Self {
        self.config.persistence = persistence;
        self
    }
}

/// Subgraph node wrapper for type erasure
///
/// When a subgraph has a different state type than its parent graph,
/// this wrapper handles the state transformation via `input_map` and `output_map`.
///
/// # Send API Compatibility
///
/// The Send API (dynamic fan-out) works correctly with subgraph nodes.
/// When multiple `Send` operations target the same subgraph node,
/// each invocation receives a unique checkpoint namespace (`|name:uuid`)
/// ensuring proper state isolation between concurrent subgraph executions.
/// This uniqueness is guaranteed by [`SubgraphPersistence::Inherit`] mode,
/// which generates a fresh UUID on every call to [`compute_child_namespace`].
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

impl<S: State, Sub> Node<S> for SubgraphNode<S, Sub>
where
    Sub: State + serde::Serialize + for<'de> serde::Deserialize<'de>,
    Sub::Update: serde::Serialize,
{
    fn call(
        &self,
        state: S,
        config: &RunnableConfig,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Command<S>, JunctureError>> + Send + '_>,
    > {
        let config = config.clone();
        let subgraph = Arc::clone(&self.subgraph);
        let input_map = Arc::clone(&self.input_map);
        let output_map = Arc::clone(&self.output_map);
        let name = self.name.clone();
        let persistence = self.config.persistence;

        Box::pin(async move {
            // Build child checkpoint namespace based on persistence mode.
            let child_ns = compute_child_namespace(
                persistence,
                &name,
                config.checkpoint_ns.as_deref(),
                config.thread_id.as_deref(),
            );

            // Create child config with updated namespace and resume values from parent
            let mut child_config = config.clone();
            child_config.checkpoint_ns = child_ns;

            // Stateless mode: clear resume value since there is no interrupt support
            if matches!(persistence, SubgraphPersistence::Stateless) {
                child_config.resume_value = None;
            }
            // For Inherit and PerThread modes, child_config already carries
            // resume_value from the parent config (via clone above), so resume
            // values flow automatically from parent to child.

            // Check if subgraph has an interrupted checkpoint we should resume from.
            // When a parent graph resumes from its own interrupt, it re-executes
            // the subgraph node. But the subgraph may have saved its own interrupt
            // checkpoint. Detect this and resume the subgraph instead of re-invoking.
            let should_resume = if let Some(checkpointer) = subgraph.checkpointer() {
                checkpointer
                    .get_tuple(&child_config)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|tuple| {
                        matches!(
                            tuple.metadata.source,
                            crate::checkpoint::CheckpointSource::Interrupt { .. }
                        )
                    })
            } else {
                false
            };

            let sub_output = if should_resume {
                // Resume from interrupt checkpoint using the resume value from parent config.
                // Use Null as fallback when no resume value is provided.
                let resume_val = child_config.resume_value.clone().unwrap_or(
                    crate::interrupt::ResumeValue::Single(serde_json::Value::Null),
                );
                subgraph.resume(&child_config, resume_val).await
            } else {
                // Normal invocation: transform parent state to subgraph input, then execute
                let sub_input = (input_map)(&state);
                subgraph.invoke_async(sub_input, &child_config).await
            };

            // Handle subgraph output, propagating interrupts and parent commands
            let sub_output = match sub_output {
                Ok(output) => output,
                Err(e) if e.is_parent_command() => {
                    // Subgraph node requested routing to a parent node via exception mechanism.
                    // Convert the target node name to a Command::goto on the parent graph.
                    let target = e.parent_command_target().unwrap_or("END");
                    return Ok(Command::goto(target));
                }
                Err(e) if e.is_interrupt() => {
                    // Child subgraph was interrupted - propagate as interrupt signal to parent
                    // instead of as a generic subgraph error
                    return Err(e);
                }
                Err(e) => {
                    // Other errors - wrap as subgraph error with context
                    return Err(JunctureError::subgraph(format!("{name}: {e}")));
                }
            };

            // Transform subgraph output back to parent state update
            let update = (output_map)(&sub_output.value);

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

    #[test]
    fn test_with_name_changes_name() {
        let node = mock_node("test");
        let mount =
            SubgraphMount::new("original", SubgraphConfig::default(), node).with_name("renamed");

        assert_eq!(mount.name, "renamed");
    }

    #[test]
    fn test_with_config_replaces_config() {
        let node = mock_node("test");
        let custom_config = SubgraphConfig {
            persistence: SubgraphPersistence::Stateless,
        };
        let mount =
            SubgraphMount::new("sg", SubgraphConfig::default(), node).with_config(custom_config);

        assert_eq!(mount.config.persistence, SubgraphPersistence::Stateless);
    }

    #[test]
    fn test_with_persistence_sets_mode() {
        let node = mock_node("test");
        let mount = SubgraphMount::new("sg", SubgraphConfig::default(), node)
            .with_persistence(SubgraphPersistence::PerThread);

        assert_eq!(mount.config.persistence, SubgraphPersistence::PerThread);
    }

    #[test]
    fn test_builder_chaining() {
        let node = mock_node("test");
        let mount = SubgraphMount::new("initial", SubgraphConfig::default(), node)
            .with_name("chained")
            .with_persistence(SubgraphPersistence::Stateless);

        assert_eq!(mount.name, "chained");
        assert_eq!(mount.config.persistence, SubgraphPersistence::Stateless);
    }

    #[test]
    fn test_with_name_accepts_non_string_types() {
        let node = mock_node("test");
        let mount = SubgraphMount::new("x", SubgraphConfig::default(), node)
            .with_name(String::from("from_string"));

        assert_eq!(mount.name, "from_string");
    }

    #[test]
    fn test_checkpoint_namespace_separator() {
        // Test that namespace uses |name:id format per design spec 07-subgraph.md section 3
        let ns = crate::checkpoint::CheckpointNamespace::root();
        let child = ns.child("node1", "id1");
        let grandchild = child.child("node2", "id2");

        assert_eq!(child.as_str(), "|node1:id1");
        assert_eq!(grandchild.as_str(), "|node1:id1|node2:id2");

        // Test parsing round-trip
        let parsed = crate::checkpoint::CheckpointNamespace::parse("|node1:id1|node2:id2");
        assert_eq!(parsed.as_str(), "|node1:id1|node2:id2");

        // Test root is empty
        assert_eq!(ns.as_str(), "");
        assert!(ns.is_root());
    }

    // --- compute_child_namespace tests ---

    #[test]
    fn test_stateless_namespace_is_none() {
        let ns = compute_child_namespace(
            SubgraphPersistence::Stateless,
            "my_sub",
            None,
            Some("thread-42"),
        );
        assert_eq!(ns, None);
    }

    #[test]
    fn test_stateless_namespace_is_none_even_with_parent_ns() {
        let ns = compute_child_namespace(
            SubgraphPersistence::Stateless,
            "my_sub",
            Some("|parent:abc"),
            Some("thread-42"),
        );
        assert_eq!(ns, None);
    }

    #[test]
    fn test_perthread_namespace_uses_thread_id() {
        let ns = compute_child_namespace(
            SubgraphPersistence::PerThread,
            "my_sub",
            None,
            Some("thread-42"),
        );
        let ns = ns.expect("PerThread should produce a namespace");
        assert_eq!(ns, "|my_sub:thread-42");
    }

    #[test]
    fn test_perthread_namespace_appends_to_parent_ns() {
        let ns = compute_child_namespace(
            SubgraphPersistence::PerThread,
            "my_sub",
            Some("|parent:abc"),
            Some("thread-42"),
        );
        let ns = ns.expect("PerThread should produce a namespace");
        assert_eq!(ns, "|parent:abc|my_sub:thread-42");
    }

    #[test]
    fn test_perthread_namespace_falls_back_to_default() {
        let ns = compute_child_namespace(SubgraphPersistence::PerThread, "my_sub", None, None);
        let ns = ns.expect("PerThread should produce a namespace");
        assert_eq!(ns, "|my_sub:default");
    }

    #[test]
    fn test_perthread_namespace_is_stable() {
        // Same inputs must always produce the same namespace
        let a = compute_child_namespace(SubgraphPersistence::PerThread, "sub", None, Some("t1"));
        let b = compute_child_namespace(SubgraphPersistence::PerThread, "sub", None, Some("t1"));
        assert_eq!(a, b);
    }

    #[test]
    fn test_inherit_namespace_is_uuid_based() {
        let ns = compute_child_namespace(
            SubgraphPersistence::Inherit,
            "my_sub",
            None,
            Some("thread-42"),
        );
        let ns = ns.expect("Inherit should produce a namespace");
        assert!(ns.starts_with("|my_sub:"));
        // The suffix after "|my_sub:" must be a valid UUID
        let uuid_part = ns.strip_prefix("|my_sub:").expect("prefix present");
        assert!(
            uuid::Uuid::parse_str(uuid_part).is_ok(),
            "suffix should be a valid UUID, got: {uuid_part}"
        );
    }

    #[test]
    fn test_inherit_namespace_appends_to_parent_ns() {
        let ns = compute_child_namespace(
            SubgraphPersistence::Inherit,
            "my_sub",
            Some("|parent:abc"),
            Some("thread-42"),
        );
        let ns = ns.expect("Inherit should produce a namespace");
        assert!(ns.starts_with("|parent:abc|my_sub:"));
        let uuid_part = ns
            .strip_prefix("|parent:abc|my_sub:")
            .expect("prefix present");
        assert!(
            uuid::Uuid::parse_str(uuid_part).is_ok(),
            "suffix should be a valid UUID, got: {uuid_part}"
        );
    }

    #[test]
    fn test_inherit_namespace_differs_between_invocations() {
        // Two calls with the same inputs should produce different namespaces
        // because Inherit mode uses a fresh UUID each time.
        let a = compute_child_namespace(SubgraphPersistence::Inherit, "sub", None, Some("t1"));
        let b = compute_child_namespace(SubgraphPersistence::Inherit, "sub", None, Some("t1"));
        assert_ne!(a, b, "Inherit mode should produce unique namespaces");
    }

    #[test]
    fn send_fan_out_produces_unique_namespaces() {
        // Simulates the Send API scenario: multiple fan-out invocations targeting
        // the same subgraph node. Each must get a distinct checkpoint namespace
        // so that concurrent subgraph executions do not collide.
        let count = 10;
        let namespaces: Vec<Option<String>> = (0..count)
            .map(|_| {
                compute_child_namespace(SubgraphPersistence::Inherit, "worker", None, Some("t1"))
            })
            .collect();

        // All namespaces must be Some (Inherit never returns None)
        assert!(
            namespaces.iter().all(Option::is_some),
            "all Inherit invocations should produce a namespace"
        );

        // All namespaces must be unique (UUID guarantees this)
        let unique: std::collections::HashSet<&str> = namespaces
            .iter()
            .map(|ns| ns.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(
            unique.len(),
            count,
            "Send fan-out to subgraph must produce {count} distinct namespaces"
        );

        // Each namespace must follow the |name:uuid format
        for ns in &namespaces {
            let ns = ns.as_deref().unwrap_or("");
            assert!(
                ns.starts_with("|worker:"),
                "namespace should start with '|worker:', got: {ns}"
            );
            let uuid_part = ns.strip_prefix("|worker:").unwrap_or("");
            assert!(
                uuid::Uuid::parse_str(uuid_part).is_ok(),
                "suffix must be a valid UUID, got: {uuid_part}"
            );
        }
    }

    // --- SubgraphTransformer::transform namespace tests ---

    fn make_transformer(name: &str) -> SubgraphTransformer {
        SubgraphTransformer::new(name.to_string())
    }

    fn make_nested_transformer(name: &str, parent_ns: &[&str]) -> SubgraphTransformer {
        let mut t = SubgraphTransformer::new(name.to_string());
        for segment in parent_ns {
            t.add_namespace((*segment).to_string());
        }
        t
    }

    #[test]
    fn transform_updates_prefixes_node_name() {
        let t = make_transformer("review");
        let event = crate::stream::StreamEvent::<StateDummy>::Updates {
            node: "agent".to_string(),
            update: StateDummyUpdate,
            step: 1,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Updates { node, .. } => {
                assert_eq!(node, "review/agent");
            }
            other => panic!("expected Updates, got {other:?}"),
        }
    }

    #[test]
    fn transform_filtered_updates_prefixes_node_name() {
        let t = make_transformer("review");
        let event = crate::stream::StreamEvent::<StateDummy>::FilteredUpdates {
            node: "agent".to_string(),
            data: serde_json::json!({"key": "val"}),
            step: 2,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::FilteredUpdates { node, .. } => {
                assert_eq!(node, "review/agent");
            }
            other => panic!("expected FilteredUpdates, got {other:?}"),
        }
    }

    #[test]
    fn transform_task_start_prefixes_node_name() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::TaskStart {
            node: "worker".to_string(),
            task_id: "t1".to_string(),
            step: 3,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::TaskStart {
                node,
                task_id,
                step,
            } => {
                assert_eq!(node, "sub/worker");
                assert_eq!(task_id, "t1");
                assert_eq!(step, 3);
            }
            other => panic!("expected TaskStart, got {other:?}"),
        }
    }

    #[test]
    fn transform_task_end_prefixes_node_name() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::TaskEnd {
            node: "worker".to_string(),
            task_id: "t1".to_string(),
            step: 3,
            duration_ms: 150,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::TaskEnd {
                node, duration_ms, ..
            } => {
                assert_eq!(node, "sub/worker");
                assert_eq!(duration_ms, 150);
            }
            other => panic!("expected TaskEnd, got {other:?}"),
        }
    }

    #[test]
    fn transform_task_detail_prefixes_node_name() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::TaskDetail {
            task_id: "t2".to_string(),
            node: "inner".to_string(),
            step: 4,
            attempt: 1,
            event: crate::stream::TaskEventType::Started,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::TaskDetail { task_id, node, .. } => {
                assert_eq!(task_id, "t2");
                assert_eq!(node, "sub/inner");
            }
            other => panic!("expected TaskDetail, got {other:?}"),
        }
    }

    #[test]
    fn transform_custom_prefixes_node_and_ns() {
        let t = make_transformer("review");
        let event = crate::stream::StreamEvent::<StateDummy>::Custom {
            node: "agent".to_string(),
            data: serde_json::json!({"action": "thinking"}),
            ns: vec!["old_ns".to_string()],
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Custom { node, ns, .. } => {
                assert_eq!(node, "review/agent");
                // ns should be replaced with the transformer's full namespace
                assert_eq!(ns, vec!["review"]);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn transform_interrupt_prefixes_node_and_ns() {
        let t = make_transformer("review");
        let event = crate::stream::StreamEvent::<StateDummy>::Interrupt {
            node: "agent".to_string(),
            payload: serde_json::json!({"question": "approve?"}),
            resumable: true,
            ns: vec![],
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Interrupt {
                node,
                ns,
                resumable,
                ..
            } => {
                assert_eq!(node, "review/agent");
                assert_eq!(ns, vec!["review"]);
                assert!(resumable);
            }
            other => panic!("expected Interrupt, got {other:?}"),
        }
    }

    #[test]
    fn transform_messages_prefixes_node_in_metadata() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::Messages {
            chunk: crate::stream::MessageChunk {
                content: "hello".to_string(),
                tool_call_chunks: vec![],
                usage_delta: None,
            },
            metadata: crate::stream::MessageStreamMetadata {
                node: "llm".to_string(),
                model: "gpt-4".to_string(),
                tags: vec![],
                ns: vec![],
            },
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Messages { metadata, .. } => {
                assert_eq!(metadata.node, "sub/llm");
                assert_eq!(metadata.ns, vec!["sub"]);
                assert_eq!(metadata.model, "gpt-4");
            }
            other => panic!("expected Messages, got {other:?}"),
        }
    }

    // --- Pass-through variants (no node field) ---

    #[test]
    fn transform_values_passes_through() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::Values {
            state: StateDummy,
            step: 5,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Values { step, .. } => assert_eq!(step, 5),
            other => panic!("expected Values, got {other:?}"),
        }
    }

    #[test]
    fn transform_end_passes_through() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::End { output: StateDummy };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::End { .. } => {}
            other => panic!("expected End, got {other:?}"),
        }
    }

    #[test]
    fn transform_budget_exceeded_passes_through() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::BudgetExceeded {
            reason: crate::pregel::BudgetExceededReason::Steps {
                used: 25,
                limit: 25,
            },
            usage: crate::stream::BudgetUsage {
                tokens_used: 1000,
                cost_usd: 0.05,
                duration_ms: 200,
                steps_completed: 25,
            },
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::BudgetExceeded { .. } => {}
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn transform_checkpoint_saved_passes_through() {
        let t = make_transformer("sub");
        let event = crate::stream::StreamEvent::<StateDummy>::CheckpointSaved {
            checkpoint_id: "cp-1".to_string(),
            metadata: crate::checkpoint::CheckpointMetadata {
                source: crate::checkpoint::CheckpointSource::Loop,
                step: 1,
                writes: std::collections::HashMap::new(),
                parents: std::collections::HashMap::new(),
                run_id: "run-1".to_string(),
            },
            step: 1,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::CheckpointSaved { checkpoint_id, .. } => {
                assert_eq!(checkpoint_id, "cp-1");
            }
            other => panic!("expected CheckpointSaved, got {other:?}"),
        }
    }

    // --- Nested namespace (multiple parent segments) ---

    #[test]
    fn transform_nested_namespace_prefixes_correctly() {
        let t = make_nested_transformer("child", &["parent", "middle"]);
        let event = crate::stream::StreamEvent::<StateDummy>::Updates {
            node: "agent".to_string(),
            update: StateDummyUpdate,
            step: 1,
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Updates { node, .. } => {
                assert_eq!(node, "parent/middle/child/agent");
            }
            other => panic!("expected Updates, got {other:?}"),
        }
    }

    #[test]
    fn transform_nested_custom_sets_full_ns() {
        let t = make_nested_transformer("child", &["parent"]);
        let event = crate::stream::StreamEvent::<StateDummy>::Custom {
            node: "agent".to_string(),
            data: serde_json::json!({}),
            ns: vec![],
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Custom { node, ns, .. } => {
                assert_eq!(node, "parent/child/agent");
                assert_eq!(ns, vec!["parent", "child"]);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn transform_nested_interrupt_sets_full_ns() {
        let t = make_nested_transformer("grandchild", &["parent", "child"]);
        let event = crate::stream::StreamEvent::<StateDummy>::Interrupt {
            node: "agent".to_string(),
            payload: serde_json::Value::Null,
            resumable: false,
            ns: vec!["old".to_string()],
        };
        let result = t.transform(&event).expect("should pass filter");
        match result {
            crate::stream::StreamEvent::Interrupt { node, ns, .. } => {
                assert_eq!(node, "parent/child/grandchild/agent");
                assert_eq!(ns, vec!["parent", "child", "grandchild"]);
            }
            other => panic!("expected Interrupt, got {other:?}"),
        }
    }

    // --- Filter behavior ---

    #[test]
    fn transform_filter_rejects_non_matching_type() {
        let t = SubgraphTransformer::new("sub".to_string())
            .with_filter_types(vec!["updates".to_string()]);

        let event = crate::stream::StreamEvent::<StateDummy>::TaskStart {
            node: "worker".to_string(),
            task_id: "t1".to_string(),
            step: 1,
        };
        assert!(
            t.transform(&event).is_none(),
            "task_start should be filtered"
        );
    }

    #[test]
    fn transform_filter_allows_matching_type() {
        let t = SubgraphTransformer::new("sub".to_string())
            .with_filter_types(vec!["updates".to_string()]);

        let event = crate::stream::StreamEvent::<StateDummy>::Updates {
            node: "agent".to_string(),
            update: StateDummyUpdate,
            step: 1,
        };
        let result = t.transform(&event).expect("updates should pass filter");
        match result {
            crate::stream::StreamEvent::Updates { node, .. } => {
                assert_eq!(node, "sub/agent");
            }
            other => panic!("expected Updates, got {other:?}"),
        }
    }

    #[test]
    fn transform_filter_empty_types_allows_all() {
        let t = SubgraphTransformer::new("sub".to_string()).with_filter_types(vec![]);
        let event = crate::stream::StreamEvent::<StateDummy>::End { output: StateDummy };
        assert!(
            t.transform(&event).is_some(),
            "empty filter should allow all"
        );
    }

    // --- Nested namespace depth tests (3+ levels) ---

    #[test]
    fn nested_namespace_three_levels_deep() {
        let ns = crate::checkpoint::CheckpointNamespace::root();
        let level1 = ns.child("review", "uuid-1");
        let level2 = level1.child("detail", "uuid-2");
        let level3 = level2.child("sub", "uuid-3");

        assert_eq!(level1.as_str(), "|review:uuid-1");
        assert_eq!(level2.as_str(), "|review:uuid-1|detail:uuid-2");
        assert_eq!(level3.as_str(), "|review:uuid-1|detail:uuid-2|sub:uuid-3");
        assert!(ns.is_root());
        assert!(!level1.is_root());
        assert!(!level3.is_root());
    }

    #[test]
    fn nested_namespace_parse_roundtrip_three_levels() {
        let original = "|alpha:aaa|beta:bbb|gamma:ccc";
        let parsed = crate::checkpoint::CheckpointNamespace::parse(original);
        assert_eq!(parsed.as_str(), original);

        // Verify each segment is intact
        assert_eq!(parsed.segments.len(), 3);
        assert_eq!(parsed.segments[0].node_name, "alpha");
        assert_eq!(parsed.segments[0].invocation_id, "aaa");
        assert_eq!(parsed.segments[1].node_name, "beta");
        assert_eq!(parsed.segments[1].invocation_id, "bbb");
        assert_eq!(parsed.segments[2].node_name, "gamma");
        assert_eq!(parsed.segments[2].invocation_id, "ccc");

        // Constructing from parsed segments and calling child should extend correctly
        let level4 = parsed.child("delta", "ddd");
        assert_eq!(level4.as_str(), "|alpha:aaa|beta:bbb|gamma:ccc|delta:ddd");
    }

    #[test]
    fn nested_compute_child_namespace_chains_correctly() {
        // Start with a parent namespace that already has two segments
        let parent = "|review:uuid-1|detail:uuid-2";

        // Inherit mode: appends a fresh UUID-based child segment
        let child_inherit = compute_child_namespace(
            SubgraphPersistence::Inherit,
            "sub",
            Some(parent),
            Some("thread-1"),
        );
        let child_inherit = child_inherit.expect("Inherit should produce a namespace");
        assert!(child_inherit.starts_with("|review:uuid-1|detail:uuid-2|sub:"));
        let uuid_part = child_inherit
            .strip_prefix("|review:uuid-1|detail:uuid-2|sub:")
            .expect("prefix present");
        assert!(
            uuid::Uuid::parse_str(uuid_part).is_ok(),
            "suffix should be a valid UUID, got: {uuid_part}"
        );

        // PerThread mode: appends a thread-id-based child segment
        let child_perthread = compute_child_namespace(
            SubgraphPersistence::PerThread,
            "sub",
            Some(parent),
            Some("thread-42"),
        );
        let child_perthread = child_perthread.expect("PerThread should produce a namespace");
        assert_eq!(
            child_perthread,
            "|review:uuid-1|detail:uuid-2|sub:thread-42"
        );

        // Stateless mode: returns None regardless of parent depth
        let child_stateless = compute_child_namespace(
            SubgraphPersistence::Stateless,
            "sub",
            Some(parent),
            Some("thread-1"),
        );
        assert_eq!(child_stateless, None);
    }

    #[test]
    fn nested_namespace_different_uuids_at_each_level() {
        let ns = crate::checkpoint::CheckpointNamespace::root();
        let level1 = ns.child("review", "11111111-1111-1111-1111-111111111111");
        let level2 = level1.child("detail", "22222222-2222-2222-2222-222222222222");
        let level3 = level2.child("sub", "33333333-3333-3333-3333-333333333333");

        let rendered = level3.as_str();
        assert_eq!(
            rendered,
            "|review:11111111-1111-1111-1111-111111111111\
             |detail:22222222-2222-2222-2222-222222222222\
             |sub:33333333-3333-3333-3333-333333333333"
        );

        // Each level must produce a distinct namespace string
        assert_ne!(level1.as_str(), level2.as_str());
        assert_ne!(level2.as_str(), level3.as_str());
        assert_ne!(level1.as_str(), level3.as_str());

        // Each level's segment count must be correct
        assert_eq!(level1.segments.len(), 1);
        assert_eq!(level2.segments.len(), 2);
        assert_eq!(level3.segments.len(), 3);

        // Parent chain is consistent
        assert_eq!(
            level3
                .parent()
                .as_ref()
                .map(crate::checkpoint::CheckpointNamespace::as_str),
            Some(level2.as_str())
        );
        assert_eq!(
            level2
                .parent()
                .as_ref()
                .map(crate::checkpoint::CheckpointNamespace::as_str),
            Some(level1.as_str())
        );
        assert_eq!(
            level1
                .parent()
                .as_ref()
                .map(crate::checkpoint::CheckpointNamespace::as_str),
            Some(String::new())
        );
        assert_eq!(ns.parent(), None);
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
#[derive(Clone)]
pub struct SubgraphTransformer {
    /// Subgraph name for namespace prefix
    pub subgraph_name: String,

    /// Current namespace stack
    pub ns: Vec<String>,

    /// Optional filter for event types
    ///
    /// The closure receives the event as a `serde_json::Value` and returns
    /// true if the event should be included, false otherwise.
    #[allow(
        clippy::type_complexity,
        reason = "trait object requires full signature for filter closure"
    )]
    pub filter: Option<std::sync::Arc<dyn Fn(&serde_json::Value) -> bool + Send + Sync>>,

    /// Whether to include internal events
    pub include_internal: bool,
}

impl std::fmt::Debug for SubgraphTransformer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubgraphTransformer")
            .field("subgraph_name", &self.subgraph_name)
            .field("ns", &self.ns)
            .field("filter", &self.filter.as_ref().map(|_| "<fn>"))
            .field("include_internal", &self.include_internal)
            .finish()
    }
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

    /// Set event filter as a closure
    ///
    /// # Arguments
    ///
    /// * `filter` - Closure that receives the event as JSON and returns
    ///   true if the event should be included
    #[must_use]
    pub fn with_filter(
        mut self,
        filter: impl Fn(&serde_json::Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.filter = Some(std::sync::Arc::new(filter));
        self
    }

    /// Set event filter by event type names (backward compatibility)
    ///
    /// # Arguments
    ///
    /// * `types` - List of event types to include (empty means all events)
    #[must_use]
    pub fn with_filter_types(mut self, types: Vec<String>) -> Self {
        if types.is_empty() {
            self.filter = None;
        } else {
            let filter = move |value: &serde_json::Value| {
                value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|event_type| types.iter().any(|t| t == event_type))
            };
            self.filter = Some(std::sync::Arc::new(filter));
        }
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

    /// Transform a stream event by adding namespace prefixes.
    ///
    /// Applies two transformations to subgraph stream events:
    ///
    /// 1. **Filter**: if a filter closure is configured, events whose type
    ///    does not match are discarded (`None` is returned).
    /// 2. **Namespace**: the `node` field of matching events is prefixed with
    ///    the subgraph namespace (`ns/subgraph_name`), and the `ns` vector is
    ///    extended with the subgraph name to reflect the nesting path.
    ///
    /// # Arguments
    ///
    /// * `event` - The stream event to transform.
    ///
    /// # Returns
    ///
    /// `Some(transformed_event)` if the event passes the filter, `None`
    /// otherwise.  Variants that carry no `node` or `ns` fields are returned
    /// unchanged (aside from cloning).
    #[must_use]
    pub fn transform<S: State>(
        &self,
        event: &crate::stream::StreamEvent<S>,
    ) -> Option<crate::stream::StreamEvent<S>> {
        if !self.passes_filter(event) {
            return None;
        }
        Some(self.apply_namespace(event))
    }

    /// Check whether the event passes the configured type filter.
    fn passes_filter<S: State>(&self, event: &crate::stream::StreamEvent<S>) -> bool {
        use crate::stream::StreamEvent;

        let Some(ref filter) = self.filter else {
            return true;
        };
        let event_type = match event {
            StreamEvent::Values { .. } | StreamEvent::FilteredValues { .. } => "values",
            StreamEvent::Updates { .. } | StreamEvent::FilteredUpdates { .. } => "updates",
            StreamEvent::Messages { .. } => "messages",
            StreamEvent::Custom { .. } => "custom",
            StreamEvent::TaskStart { .. } => "task_start",
            StreamEvent::TaskEnd { .. } => "task_end",
            StreamEvent::Interrupt { .. } => "interrupt",
            StreamEvent::BudgetExceeded { .. } => "budget_exceeded",
            StreamEvent::End { .. } => "end",
            StreamEvent::Debug(_) => "debug",
            StreamEvent::Tools(_) => "tools",
            StreamEvent::CheckpointSaved { .. } => "checkpoint_saved",
            StreamEvent::TaskDetail { .. } => "task_detail",
        };
        let filter_value = serde_json::json!({ "type": event_type });
        filter(&filter_value)
    }

    /// Build namespace prefix string and full namespace vector from the
    /// current transformer state.
    fn build_ns(&self) -> (String, Vec<String>) {
        let ns_prefix = if self.ns.is_empty() {
            self.subgraph_name.clone()
        } else {
            format!("{}/{}", self.ns.join("/"), self.subgraph_name)
        };
        let full_ns = {
            let mut ns = self.ns.clone();
            ns.push(self.subgraph_name.clone());
            ns
        };
        (ns_prefix, full_ns)
    }

    /// Apply namespace prefix to node names and prepend to ns vectors.
    fn apply_namespace<S: State>(
        &self,
        event: &crate::stream::StreamEvent<S>,
    ) -> crate::stream::StreamEvent<S> {
        use crate::stream::StreamEvent;

        let (ns_prefix, full_ns) = self.build_ns();
        let namespaced = |node: &str| -> String { format!("{ns_prefix}/{node}") };

        match event.clone() {
            StreamEvent::Updates { node, update, step } => StreamEvent::Updates {
                node: namespaced(&node),
                update,
                step,
            },
            StreamEvent::FilteredUpdates { node, data, step } => StreamEvent::FilteredUpdates {
                node: namespaced(&node),
                data,
                step,
            },
            StreamEvent::TaskStart {
                node,
                task_id,
                step,
            } => StreamEvent::TaskStart {
                node: namespaced(&node),
                task_id,
                step,
            },
            StreamEvent::TaskEnd {
                node,
                task_id,
                step,
                duration_ms,
            } => StreamEvent::TaskEnd {
                node: namespaced(&node),
                task_id,
                step,
                duration_ms,
            },
            StreamEvent::TaskDetail {
                task_id,
                node,
                step,
                attempt,
                event: task_event,
            } => StreamEvent::TaskDetail {
                task_id,
                node: namespaced(&node),
                step,
                attempt,
                event: task_event,
            },

            // Variants with node AND ns
            StreamEvent::Custom { node, data, .. } => StreamEvent::Custom {
                node: namespaced(&node),
                data,
                ns: full_ns,
            },
            StreamEvent::Interrupt {
                node,
                payload,
                resumable,
                ..
            } => StreamEvent::Interrupt {
                node: namespaced(&node),
                payload,
                resumable,
                ns: full_ns,
            },

            // Metadata carries node and ns
            StreamEvent::Messages {
                chunk,
                mut metadata,
            } => {
                metadata.node = namespaced(&metadata.node);
                metadata.ns = full_ns;
                StreamEvent::Messages { chunk, metadata }
            }

            // Pass-through variants (no node / no ns)
            other => other,
        }
    }

    /// Add a namespace segment
    ///
    /// # Arguments
    ///
    /// * `segment` - The namespace segment to add
    pub fn add_namespace(&mut self, segment: String) {
        self.ns.push(segment);
    }
}

// Rust guideline compliant 2026-05-21
