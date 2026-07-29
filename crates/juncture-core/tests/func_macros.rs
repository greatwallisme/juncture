//! Integration tests for the `#[task]` and `#[entrypoint]` attribute macros
//! (design `03-pregel-engine` §13.3).
//!
//! These live in a separate integration-test crate (not `juncture-core`'s unit
//! tests) because the macros emit `::juncture_core::` absolute paths, which
//! only resolve in a crate that has `juncture_core` as an extern dependency.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use juncture_core::JunctureError;
use juncture_core::state::MessagesState;

type TestState = MessagesState;
type TestStateUpdate = <TestState as juncture_core::state::State>::Update;

// ============================ #[task] =======================================

/// Bare `#[task]`: returns a `SyncAsyncFuture` whose `.result().await` yields
/// the computed value.
#[juncture_derive::task]
async fn double_task(x: u32) -> Result<u32, JunctureError> {
    Ok(x * 2)
}

#[tokio::test]
async fn task_basic_returns_value() {
    let value = double_task(21).result().await.expect("task should succeed");
    assert_eq!(value, 42);
}

/// `#[task(retry = ...)]` retries a transiently-failing task and succeeds on
/// the 2nd attempt (proves the retry loop + per-attempt arg clone).
static FLAKY_CALLS: AtomicU32 = AtomicU32::new(0);
#[juncture_derive::task(
    retry = juncture_core::graph::RetryPolicy {
        max_attempts: 3,
        initial_interval: Duration::from_millis(1),
        jitter: false,
        ..std::default::Default::default()
    }
)]
async fn flaky_task(x: u32) -> Result<u32, JunctureError> {
    let n = FLAKY_CALLS.fetch_add(1, Ordering::SeqCst);
    if n == 0 {
        Err(JunctureError::execution("transient failure"))
    } else {
        Ok(x)
    }
}

#[tokio::test]
async fn task_retry_succeeds_on_second_attempt() {
    FLAKY_CALLS.store(0, Ordering::SeqCst);
    let value = flaky_task(7)
        .result()
        .await
        .expect("retry should eventually succeed");
    assert_eq!(value, 7);
    assert!(
        FLAKY_CALLS.load(Ordering::SeqCst) >= 2,
        "task must have been retried at least once"
    );
}

/// `#[task(timeout = ...)]` surfaces a sleeping task as an error.
#[juncture_derive::task(timeout = Duration::from_millis(10))]
async fn slow_task(x: u32) -> Result<u32, JunctureError> {
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(x)
}

#[tokio::test]
async fn task_timeout_errors() {
    let result = slow_task(5).result().await;
    assert!(result.is_err(), "slow task must time out");
}

/// `#[task(cache = ...)]`: the 2nd identical call is served from the per-task
/// cache without re-executing the body.
static CACHED_CALLS: AtomicU32 = AtomicU32::new(0);
#[juncture_derive::task(cache = juncture_core::config::CachePolicy::default_policy())]
async fn cached_task(x: u32) -> Result<u32, JunctureError> {
    CACHED_CALLS.fetch_add(1, Ordering::SeqCst);
    Ok(x)
}

#[tokio::test]
async fn task_cache_hits_on_second_call() {
    CACHED_CALLS.store(0, Ordering::SeqCst);
    let v1 = cached_task(9).result().await.expect("first call succeeds");
    let v2 = cached_task(9).result().await.expect("cached call succeeds");
    assert_eq!(v1, 9);
    assert_eq!(v2, 9);
    assert_eq!(
        CACHED_CALLS.load(Ordering::SeqCst),
        1,
        "second call must be served from the per-task cache"
    );
}

// ========================== #[entrypoint] ===================================

/// `#[entrypoint]` on a node-compatible fn generates a `compile()` accessor
/// that builds a `CompiledGraph`. The fn receives `(&S, &Runtime<()>)` so it
/// can access `runtime.previous` (design 03 §14) and store/context.
#[juncture_derive::entrypoint]
async fn sample_workflow(
    _state: &TestState,
    _runtime: &juncture_core::runtime::Runtime<()>,
) -> Result<TestStateUpdate, JunctureError> {
    Ok(TestStateUpdate::default())
}

#[tokio::test]
async fn entrypoint_compiles_and_invokes() {
    let graph = compile(None).expect("entrypoint should compile into a graph");
    let input = TestState::default();
    let result = graph
        .invoke_async(input, &juncture_core::config::RunnableConfig::new())
        .await;
    assert!(result.is_ok(), "compiled entrypoint should invoke cleanly");
}

// ====================== design 03 §14: previous access ======================

mod previous_access {
    use super::*;
    use juncture_core::config::RunnableConfig;
    use std::sync::atomic::AtomicBool;

    static PREVIOUS_OBSERVED: AtomicBool = AtomicBool::new(false);

    /// `#[entrypoint]` must receive the engine-loaded `previous` value via
    /// `runtime.previous` (design 03-pregel-engine §14).
    #[juncture_derive::entrypoint]
    async fn observe_previous(
        _state: &TestState,
        runtime: &juncture_core::runtime::Runtime<()>,
    ) -> Result<TestStateUpdate, JunctureError> {
        if runtime.previous.is_some() {
            PREVIOUS_OBSERVED.store(true, Ordering::SeqCst);
        }
        Ok(TestStateUpdate::default())
    }

    #[tokio::test]
    async fn entrypoint_accesses_previous_return_value() {
        PREVIOUS_OBSERVED.store(false, Ordering::SeqCst);
        let graph = compile(None).expect("entrypoint should compile into a graph");
        let mut config = RunnableConfig::new();
        config.previous = Some(serde_json::json!({"return": 42}));
        let result = graph.invoke_async(TestState::default(), &config).await;
        assert!(result.is_ok(), "invoke should succeed");
        assert!(
            PREVIOUS_OBSERVED.load(Ordering::SeqCst),
            "entrypoint fn must see runtime.previous (design 03 §14)"
        );
    }
}

// ==================== runtime context (store/context) access ====================

mod runtime_access {
    use super::*;
    use juncture_core::config::RunnableConfig;
    use std::sync::atomic::AtomicBool;

    static RUNTIME_TOUCHED: AtomicBool = AtomicBool::new(false);

    /// `#[entrypoint]` must receive an injected `Runtime` so it can read
    /// context, store, and other runtime fields.
    #[juncture_derive::entrypoint]
    async fn touch_runtime(
        _state: &TestState,
        runtime: &juncture_core::runtime::Runtime<()>,
    ) -> Result<TestStateUpdate, JunctureError> {
        // Touch several Runtime fields to prove the whole Runtime is injected.
        let _ = runtime.previous.is_some();
        let _ = runtime.store.is_none();
        let _: () = runtime.context;
        RUNTIME_TOUCHED.store(true, Ordering::SeqCst);
        Ok(TestStateUpdate::default())
    }

    #[tokio::test]
    async fn entrypoint_accesses_runtime_context() {
        RUNTIME_TOUCHED.store(false, Ordering::SeqCst);
        let graph = compile(None).expect("entrypoint should compile into a graph");
        let result = graph
            .invoke_async(TestState::default(), &RunnableConfig::new())
            .await;
        assert!(result.is_ok(), "invoke should succeed");
        assert!(
            RUNTIME_TOUCHED.load(Ordering::SeqCst),
            "Runtime must be injected into the entrypoint fn"
        );
    }
}

// ==================== design 03 §13.3: graph-level #[task] cache ====================

mod graph_level_cache {
    use super::*;
    use juncture_core::config::CachePolicy;
    use juncture_core::pregel::TASK_CACHE_POLICY;

    static GRAPH_CACHE_CALLS: AtomicU32 = AtomicU32::new(0);

    /// `#[task]` without a per-task policy still consults the graph-level
    /// cache scoped by the runner from `RunnableConfig::task_cache_policy`
    /// (design 03-pregel-engine §13.3).
    #[juncture_derive::task]
    async fn graph_cached_task(x: u32) -> Result<u32, JunctureError> {
        GRAPH_CACHE_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(x)
    }

    #[tokio::test]
    async fn task_hits_graph_level_cache_via_task_local() {
        GRAPH_CACHE_CALLS.store(0, Ordering::SeqCst);
        let policy = CachePolicy::default_policy();
        TASK_CACHE_POLICY
            .scope(Some(policy), async {
                let v1 = graph_cached_task(7)
                    .result()
                    .await
                    .expect("first call succeeds");
                let v2 = graph_cached_task(7)
                    .result()
                    .await
                    .expect("second call succeeds");
                assert_eq!(v1, 7);
                assert_eq!(v2, 7);
                assert_eq!(
                    GRAPH_CACHE_CALLS.load(Ordering::SeqCst),
                    1,
                    "second identical call must hit the graph-level cache"
                );
            })
            .await;
    }
}
