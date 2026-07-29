//! Functional API for defining workflows with plain functions
//!
//! This module provides the functional entrypoint/task API as an alternative
//! to [`StateGraph`]. Users can define workflows
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
    ///
    /// `previous` falls back to the `PREVIOUS` task-local scoped by the Pregel
    /// runner from `RunnableConfig::previous` (loaded from the checkpoint's
    /// `__return__` field, design `03-pregel-engine` §14) when the core runtime
    /// did not carry one -- which is the common case, since the engine injects
    /// `previous` via the task-local rather than threading a `Runtime` into
    /// `Node::call`.
    #[must_use]
    pub fn from_core(core: &CoreRuntime<S>) -> Self {
        let previous = core.previous.clone().or_else(|| {
            crate::pregel::PREVIOUS
                .try_with(std::clone::Clone::clone)
                .ok()
                .flatten()
        });
        Self {
            previous,
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

    // Wire TaskConfig::timeout into the node's TimeoutPolicy (was silently
    // dropped -- audit B-012). `run_timeout` carries the duration; idle/refresh
    // stay at their defaults.
    let timeout_policies = config.timeout.map_or_else(Vec::new, |run_timeout| {
        vec![crate::TimeoutPolicy {
            run_timeout,
            ..Default::default()
        }]
    });

    let mut graph = StateGraph::<S, I, O>::new();

    graph.add_node(
        &entrypoint_name,
        func,
        false,
        None,
        None,
        retry_policies,
        timeout_policies,
    )?;

    graph.set_entry_point(&entrypoint_name);
    graph.set_finish_point(&entrypoint_name);

    // Wire TaskConfig::cache_policy into the compiled graph's node-result cache
    // policy (was silently dropped -- audit B-012). `invoke`/`invoke_async` will
    // check this cache before executing the entrypoint and store the fresh result
    // after (design 03-pregel-engine §13.3 `#[task(cache = ...)]`).
    graph.compile_with_config_and_checkpointer(
        crate::graph::CompileConfig {
            cache_policy: config.cache_policy.clone(),
            ..Default::default()
        },
        checkpointer,
    )
}

/// Run a task body with optional retry and timeout (used by the `#[task]` macro).
///
/// `args` are cloned for each attempt so the task body `f` can be re-invoked
/// on transient failures (`A: Clone` is required when `retry` is set). When
/// `timeout` is set, each attempt is bounded; an elapsed deadline becomes
/// a `JunctureError::execution`. When `retry` is set, failed attempts are
/// retried with exponential backoff (capped by `max_interval`, +/- 25% jitter
/// when `jitter` is set) up to `max_attempts`.
///
/// # Errors
///
/// Returns the last error (mapped via `E: Into<JunctureError>`) once retries
/// are exhausted, or a timeout error if an attempt exceeds `timeout`.
#[allow(
    clippy::option_if_let_else,
    reason = "the inner future `fut` is moved in both the timeout and non-timeout arms, so `map_or_else` would require cloning it; the `match` is the only sound form"
)]
pub async fn run_task<A, O, E, Fut, F>(
    retry: Option<crate::graph::RetryPolicy>,
    timeout: Option<std::time::Duration>,
    args: A,
    f: F,
) -> Result<O, crate::JunctureError>
where
    A: Clone,
    E: Into<crate::JunctureError>,
    F: Fn(A) -> Fut,
    Fut: std::future::Future<Output = Result<O, E>>,
{
    let max_attempts = retry.as_ref().map_or(1, |r| r.max_attempts.max(1));
    let mut last_err: Option<crate::JunctureError> = None;
    let mut delay = retry
        .as_ref()
        .map(|r| r.initial_interval)
        .unwrap_or_default();

    for attempt in 0..max_attempts {
        let fut = f(args.clone());
        let result: Result<O, crate::JunctureError> = match timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(r) => r.map_err(E::into),
                Err(_) => Err(crate::JunctureError::execution(format!(
                    "task exceeded {d:?} timeout"
                ))),
            },
            None => fut.await.map_err(E::into),
        };
        match result {
            Ok(value) => return Ok(value),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 >= max_attempts {
                    break;
                }
                if let Some(ref r) = retry {
                    let actual = task_compute_delay(delay, r.jitter, r.max_interval);
                    tokio::time::sleep(actual).await;
                    delay = delay.mul_f64(r.backoff_factor).min(r.max_interval);
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| crate::JunctureError::execution("task produced no result")))
}

/// Compute a backoff sleep with optional +/- 25% jitter, capped at `max_interval`
/// (mirrors `graph::builder::compute_delay` for the task retry path).
fn task_compute_delay(
    base: std::time::Duration,
    jitter: bool,
    max_interval: std::time::Duration,
) -> std::time::Duration {
    let capped = base.min(max_interval);
    if !jitter {
        return capped;
    }
    let jitter_fraction: f64 = rand::random_range(0.75..=1.25);
    capped.mul_f64(jitter_fraction).min(max_interval)
}

/// Build a stable cache key for a `#[task]` invocation.
///
/// Combines a namespace (the task name) with the serialized args. Used by the
/// `#[task]` macro's per-task `OnceLock<CachePolicy>` cache to distinguish
/// arg-sets.
#[must_use]
pub fn task_cache_key(namespace: &str, args: &serde_json::Value) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(namespace, &mut hasher);
    std::hash::Hash::hash(args, &mut hasher);
    format!(
        "task:{}:{:016x}",
        namespace,
        std::hash::Hasher::finish(&hasher)
    )
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

    /// `Runtime::from_core` falls back to the `PREVIOUS` task-local (scoped by
    /// the Pregel runner from `RunnableConfig::previous`) when the core runtime
    /// carries no `previous` -- the path that makes the engine-injected
    /// `__return__` visible to entrypoint nodes (design `03-pregel-engine` §14).
    #[tokio::test]
    async fn test_runtime_from_core_falls_back_to_previous_task_local() {
        let core = CoreRuntime::<TestState>::new();
        let prev = serde_json::json!({"accumulated": 42});
        crate::pregel::PREVIOUS
            .scope(Some(prev.clone()), async {
                let runtime = Runtime::<TestState>::from_core(&core);
                assert_eq!(runtime.previous, Some(prev));
            })
            .await;
    }

    /// A `previous` set directly on the core runtime takes precedence over the
    /// task-local fallback (explicit override wins).
    #[tokio::test]
    async fn test_runtime_from_core_prefers_core_previous_over_task_local() {
        let explicit = serde_json::json!("explicit");
        let from_task_local = serde_json::json!("task_local");
        let mut core = CoreRuntime::<TestState>::new();
        core.previous = Some(explicit.clone());
        crate::pregel::PREVIOUS
            .scope(Some(from_task_local), async {
                let runtime = Runtime::<TestState>::from_core(&core);
                assert_eq!(runtime.previous, Some(explicit));
            })
            .await;
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
            NodeFnUpdate(|_state: &TestState| async {
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
            NodeFnUpdate(|_state: &TestState| async {
                Ok::<TestStateUpdate, JunctureError>(TestStateUpdate::default())
            }),
            &config,
            None,
        );
        result.unwrap();
    }

    /// Node-result cache (design `03-pregel-engine` §13.3, audit B-012): an
    /// entrypoint compiled with a `TaskConfig::cache_policy` runs once for a
    /// given input; the second identical `invoke` returns the cached result
    /// without re-executing the entrypoint function.
    #[test]
    fn test_compile_entrypoint_node_result_cache_hit_on_second_invoke() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let exec_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&exec_count);
        let func = move |_state: &TestState| {
            let c = Arc::clone(&counter);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok::<TestStateUpdate, JunctureError>(TestStateUpdate::default())
            }
        };

        let config = TaskConfig {
            cache_policy: Some(crate::config::CachePolicy::default_policy()),
            ..Default::default()
        };
        let graph = compile_entrypoint_with_config::<TestState, TestState, TestState, _>(
            crate::node::NodeFnUpdate(func),
            &config,
            None,
        )
        .expect("compile should succeed");

        let input = TestState::default();
        let runnable = crate::config::RunnableConfig::new().with_thread_id("cache-test");
        // First invoke: executes the entrypoint (count = 1) and caches the result.
        let first = graph.invoke(input.clone(), &runnable);
        assert!(first.is_ok(), "first invoke should succeed");
        assert_eq!(exec_count.load(Ordering::SeqCst), 1, "entrypoint ran once");
        // Second identical invoke: cache hit — entrypoint NOT re-run (still 1).
        let second = graph.invoke(input, &runnable);
        assert!(second.is_ok(), "second invoke should succeed (cache hit)");
        assert_eq!(
            exec_count.load(Ordering::SeqCst),
            1,
            "second invoke must be served from the node-result cache, not re-executed"
        );
    }
}

// Rust guideline compliant 2026-05-23
