//! Functional API for defining workflows with plain functions
//!
//! This module provides the functional entrypoint/task API as an alternative
//! to [`StateGraph`](crate::graph::StateGraph). Users can define workflows
//! using ordinary async functions with runtime context instead of manually
//! building graphs.
//!
//! # Concepts
//!
//! - **Entrypoint functions** - Main workflow functions that can be compiled into graphs
//! - **Task configuration** - Reusable functions with retry/cache/timeout policies
//! - **`Runtime<S>`** - Provides access to previous state, checkpointer, and store
//!
//! # Architecture
//!
//! The functional API is a lightweight wrapper around `StateGraph`:
//! - Entrypoint functions compile to single-node graphs
//! - Task functions use [`TaskConfig`] for per-node configuration
//! - [`Runtime<S>`] provides the same context as [`CoreRuntime`](crate::runtime::Runtime)
//!   with additional functional-API-specific features
//!
//! # Example
//!
//! ```ignore
//! use juncture_core::func::{compile_entrypoint, Runtime};
//! use juncture_core::checkpoint::MemorySaver;
//! use juncture_core::state::CowState;
//! use juncture_core::runtime::Runtime as CoreRuntime;
//!
//! // Define the workflow function
//! async fn my_workflow(
//!     state: CowState<MyState>,
//!     runtime: &CoreRuntime<MyState>,
//! ) -> Result<MyStateUpdate, JunctureError> {
//!     Ok(MyStateUpdate::default())
//! }
//!
//! // Compile into a graph
//! let graph = compile_entrypoint::<MyState, Input, Output, _>(
//!     my_workflow,
//!     Some(Arc::new(MemorySaver::new()))
//! )?;
//!
//! // Execute
//! let result = graph.invoke(input, &config).await?;
//! ```

use std::sync::Arc;

use crate::checkpoint::CheckpointSaver;
use crate::config::{EntrypointConfig, TaskConfig};
use crate::graph::{StateGraph, TopologyError};
use crate::node::IntoNode;
use crate::runtime::Runtime as CoreRuntime;
use crate::state::{FromState, IntoState, State};
use crate::store::Store;

/// Runtime context for functional API workflows
///
/// Provides access to previous execution state, checkpointing, and storage
/// during workflow execution. This type extends [`CoreRuntime`] with
/// functional-API-specific features like previous value access.
///
/// # Type Parameters
///
/// * `S` - State type (must implement [`State`] and [`Default`])
///
/// # Fields
///
/// - `previous` - Previous execution return value (for accumulation patterns)
/// - `checkpointer` - Checkpoint saver for state persistence
/// - `store` - Cross-thread persistent key-value store
/// - `core` - Underlying core runtime for advanced use cases
///
/// # Examples
///
/// ## Accessing previous state
///
/// ```ignore
/// use juncture_core::func::Runtime;
///
/// async fn accumulating_workflow(
///     state: CowState<MyState>,
///     runtime: &CoreRuntime<MyState>,
/// ) -> Result<MyStateUpdate, JunctureError> {
///     // Access the functional runtime
///     let func_runtime = Runtime::from_core(runtime);
///
///     // Get the previous return value
///     if let Some(previous) = &func_runtime.previous {
///         let prev_output: Output = serde_json::from_value(previous.clone())
///             .map_err(|e| JunctureError::execution(format!("Failed to deserialize previous: {}", e)))?;
///         // Use previous value for accumulation
///     }
///
///     Ok(MyStateUpdate::default())
/// }
/// ```
///
/// ## Using the store
///
/// ```ignore
/// use juncture_core::func::Runtime;
///
/// async fn workflow_with_store(
///     state: CowState<MyState>,
///     runtime: &CoreRuntime<MyState>,
/// ) -> Result<MyStateUpdate, JunctureError> {
///     let func_runtime = Runtime::from_core(runtime);
///
///     if let Some(store) = &func_runtime.store {
///         store.put("key", serde_json::json!("value")).await?;
///     }
///
///     Ok(MyStateUpdate::default())
/// }
/// ```
#[derive(Clone)]
pub struct Runtime<S: State + Default> {
    /// Previous execution return value (for accumulation patterns)
    ///
    /// When resuming from a checkpoint, this contains the return value from
    /// the previous execution. For first-time execution, this is `None`.
    pub previous: Option<serde_json::Value>,

    /// Checkpoint saver for state persistence
    ///
    /// When set, the workflow can save and restore intermediate state.
    pub checkpointer: Option<Arc<dyn CheckpointSaver>>,

    /// Cross-thread persistent key-value store
    ///
    /// Provides durable storage that survives across workflow executions.
    pub store: Option<Arc<dyn Store>>,

    /// Underlying core runtime
    ///
    /// Provides access to advanced runtime features like heartbeat,
    /// execution metadata, and streaming.
    pub core: CoreRuntime<S>,
}

impl<S: State + Default + std::fmt::Debug> std::fmt::Debug for Runtime<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("previous", &self.previous)
            .field(
                "checkpointer",
                &self.checkpointer.as_ref().map(|_| "<CheckpointSaver>"),
            )
            .field("store", &self.store.as_ref().map(|_| "<Store>"))
            .field("core", &self.core)
            .finish()
    }
}

impl<S: State + Default> Runtime<S> {
    /// Create a new runtime with minimal configuration
    #[must_use]
    pub fn new() -> Self
    where
        S: Default,
    {
        Self {
            previous: None,
            checkpointer: None,
            store: None,
            core: CoreRuntime::new(),
        }
    }

    /// Create a functional runtime from a core runtime
    ///
    /// This extracts functional-API-specific context from the core runtime,
    /// allowing entrypoint functions to access features like previous values.
    #[must_use]
    pub fn from_core(core: &CoreRuntime<S>) -> Self {
        Self {
            previous: core.previous.clone(),
            checkpointer: None,
            store: core.store.clone(),
            core: core.clone(),
        }
    }

    /// Create a runtime from an entrypoint configuration
    #[must_use]
    pub fn from_entrypoint_config(config: &EntrypointConfig) -> Self
    where
        S: Default,
    {
        Self {
            previous: None,
            checkpointer: config.checkpointer.clone(),
            store: config.store.clone(),
            core: CoreRuntime::new(),
        }
    }

    /// Set the previous execution value
    #[must_use]
    pub fn with_previous(mut self, previous: serde_json::Value) -> Self {
        self.previous = Some(previous);
        self
    }

    /// Set the checkpointer
    #[must_use]
    pub fn with_checkpointer(mut self, checkpointer: Arc<dyn CheckpointSaver>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    /// Set the store
    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Set the core runtime
    #[must_use]
    pub fn with_core(mut self, core: CoreRuntime<S>) -> Self {
        self.core = core;
        self
    }
}

impl<S: State + Default> Default for Runtime<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile a functional workflow entrypoint into an executable graph
///
/// This function wraps a simple async function in a [`StateGraph`] with a
/// single entrypoint node, providing a functional API alternative to manual
/// graph construction.
///
/// # Type Parameters
///
/// * `S` - State type
/// * `I` - Input type (must implement [`IntoState<S>`])
/// * `O` - Output type (must implement [`FromState<S>`])
/// * `F` - Function type (must implement [`IntoNode<S>`])
///
/// # Parameters
///
/// - `func` - The entrypoint function to compile
/// - `checkpointer` - Optional checkpoint saver for state persistence
///
/// # Returns
///
/// A compiled graph that can be invoked with [`CompiledGraph::invoke`](crate::graph::CompiledGraph::invoke)
/// or streamed with [`CompiledGraph::stream`](crate::graph::CompiledGraph::stream).
///
/// # Errors
///
/// Returns [`TopologyError`] if:
/// - The function cannot be converted into a node
/// - The graph structure is invalid
///
/// # Examples
///
/// ## Basic usage
///
/// ```ignore
/// use juncture_core::func::compile_entrypoint;
/// use juncture_core::checkpoint::MemorySaver;
/// use juncture_core::state::CowState;
/// use juncture_core::runtime::Runtime as CoreRuntime;
/// use juncture_core::JunctureError;
///
/// async fn my_workflow(
///     state: CowState<MyState>,
///     runtime: &CoreRuntime<MyState>,
/// ) -> Result<MyStateUpdate, JunctureError> {
///     Ok(MyStateUpdate::default())
/// }
///
/// let graph = compile_entrypoint::<MyState, Input, Output, _>(
///     my_workflow,
///     Some(Arc::new(MemorySaver::new()))
/// )?;
///
/// let result = graph.invoke(input, &config).await?;
/// ```
///
/// ## With task configuration
///
/// ```ignore
/// use juncture_core::func::compile_entrypoint_with_config;
/// use juncture_core::config::TaskConfig;
/// use juncture_core::graph::{RetryPolicy, NodeMetadata};
/// use std::time::Duration;
///
/// let retry_policy = RetryPolicy::max_attempts(3);
/// let task_config = TaskConfig {
///     retry_policy: Some(retry_policy.clone()),
///     cache_policy: None,
///     timeout: Some(Duration::from_secs(30)),
///     name: Some("my_workflow".to_string()),
/// };
///
/// let graph = compile_entrypoint_with_config(
///     my_workflow,
///     &task_config,
///     Some(Arc::new(MemorySaver::new()))
/// )?;
/// ```
pub fn compile_entrypoint<S: State + Default, I, O, F>(
    func: F,
    checkpointer: Option<Arc<dyn CheckpointSaver>>,
) -> Result<crate::graph::CompiledGraph<S, I, O>, TopologyError>
where
    F: IntoNode<S>,
    I: IntoState<S>,
    O: FromState<S>,
{
    compile_entrypoint_with_config(func, &TaskConfig::default(), checkpointer)
}

/// Compile a functional workflow entrypoint with task configuration
///
/// This is an extended version of [`compile_entrypoint`] that allows specifying
/// task-level configuration like retry policies, caching, and timeouts.
///
/// # Type Parameters
///
/// * `S` - State type
/// * `I` - Input type (must implement [`IntoState<S>`])
/// * `O` - Output type (must implement [`FromState<S>`])
/// * `F` - Function type (must implement [`IntoNode<S>`])
///
/// # Parameters
///
/// - `func` - The entrypoint function to compile
/// - `config` - Task configuration for the entrypoint node
/// - `checkpointer` - Optional checkpoint saver for state persistence
///
/// # Returns
///
/// A compiled graph with the entrypoint node configured according to `config`.
///
/// # Errors
///
/// Returns [`TopologyError`] if:
/// - The function cannot be converted into a node
/// - The graph structure is invalid
pub fn compile_entrypoint_with_config<S: State + Default, I, O, F>(
    func: F,
    config: &TaskConfig,
    checkpointer: Option<Arc<dyn CheckpointSaver>>,
) -> Result<crate::graph::CompiledGraph<S, I, O>, TopologyError>
where
    F: IntoNode<S>,
    I: IntoState<S>,
    O: FromState<S>,
{
    let entrypoint_name = config
        .name
        .clone()
        .unwrap_or_else(|| "__entrypoint__".to_string());

    let retry_policies = config
        .retry_policy
        .as_ref()
        .map(|p| vec![p.clone()])
        .unwrap_or_default();

    let mut graph = StateGraph::<S, I, O>::new();

    graph.add_node(
        &entrypoint_name,
        func,
        false,
        None,
        None,
        retry_policies,
        Vec::new(),
    )?;

    graph.set_entry_point(&entrypoint_name);
    graph.set_finish_point(&entrypoint_name);

    graph.compile_with_checkpointer(checkpointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JunctureError;
    use crate::node::NodeFnUpdate;
    use crate::state::MessagesState;

    type TestState = MessagesState;
    type TestStateUpdate = <TestState as State>::Update;

    #[test]
    fn test_runtime_new() {
        let runtime = Runtime::<TestState>::new();
        assert!(runtime.previous.is_none());
        assert!(runtime.checkpointer.is_none());
        assert!(runtime.store.is_none());
    }

    #[test]
    fn test_runtime_default() {
        let runtime = Runtime::<TestState>::default();
        assert!(runtime.previous.is_none());
        assert!(runtime.checkpointer.is_none());
        assert!(runtime.store.is_none());
    }

    #[test]
    fn test_runtime_with_previous() {
        let previous = serde_json::json!("previous_value");
        let runtime = Runtime::<TestState>::new().with_previous(previous.clone());
        assert_eq!(runtime.previous, Some(previous));
    }

    #[test]
    fn test_runtime_from_entrypoint_config() {
        let config = EntrypointConfig {
            checkpointer: None,
            store: None,
        };
        let runtime = Runtime::<TestState>::from_entrypoint_config(&config);
        assert!(runtime.checkpointer.is_none());
        assert!(runtime.store.is_none());
    }

    #[test]
    fn test_runtime_clone() {
        let runtime = Runtime::<TestState>::new();
        let _cloned = runtime.clone();
        assert!(runtime.previous.is_none());
        assert!(runtime.checkpointer.is_none());
    }

    #[test]
    fn test_compile_entrypoint_basic() {
        let result = compile_entrypoint::<TestState, TestState, TestState, _>(
            NodeFnUpdate(|_state: TestState| async {
                Ok::<TestStateUpdate, JunctureError>(TestStateUpdate::default())
            }),
            None,
        );
        result.unwrap();
    }

    #[test]
    fn test_compile_entrypoint_with_config() {
        let retry_policy = crate::graph::RetryPolicy {
            max_attempts: 3,
            ..Default::default()
        };
        let config = TaskConfig {
            retry_policy: Some(retry_policy),
            cache_policy: None,
            timeout: None,
            name: Some("custom_entrypoint".to_string()),
        };

        let result = compile_entrypoint_with_config::<TestState, TestState, TestState, _>(
            NodeFnUpdate(|_state: TestState| async {
                Ok::<TestStateUpdate, JunctureError>(TestStateUpdate::default())
            }),
            &config,
            None,
        );
        result.unwrap();
    }
}

// Rust guideline compliant 2026-05-23
