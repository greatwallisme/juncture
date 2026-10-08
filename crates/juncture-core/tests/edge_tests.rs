//! Integration tests for Edge system

use juncture_core::edge::{END, Edge, PathMap, RouteResult, Router, START};
use juncture_core::graph::CompiledGraph;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Test state
#[derive(Debug, Clone, Default, State)]
struct TestState {
    value: u32,
}

// Test router
const fn simple_router(state: &TestState) -> &str {
    if state.value > 10 { "high" } else { "low" }
}

#[test]
fn test_start_end_constants() {
    assert_eq!(START, "__start__");
    assert_eq!(END, "__end__");
}

#[test]
fn test_edge_fixed() {
    let edge = Edge::<TestState>::Fixed {
        from: "node_a".to_string(),
        to: "node_b".to_string(),
    };

    assert!(matches!(edge, Edge::Fixed { .. }));
}

#[test]
fn test_edge_conditional() {
    let router = Arc::new(simple_router) as Arc<dyn Router<TestState>>;
    let path_map = PathMap::new();

    let edge = Edge::<TestState>::Conditional {
        from: "router".to_string(),
        router,
        path_map: path_map.clone(),
    };

    assert!(matches!(edge, Edge::Conditional { .. }));
    assert_eq!(path_map.len(), 0);
}

#[test]
fn test_path_map_new() {
    let map = PathMap::new();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}

#[test]
fn test_path_map_insert() {
    let mut map = PathMap::new();
    map.insert("approve", "publish");
    map.insert("reject", "archive");

    assert_eq!(map.len(), 2);
    assert!(map.contains_key("approve"));
    assert!(map.contains_key("reject"));
}

#[test]
fn test_path_map_get() {
    let mut map = PathMap::new();
    map.insert("key1", "value1");

    assert_eq!(map.get("key1"), Some(&"value1".to_string()));
    assert_eq!(map.get("key2"), None);
}

#[test]
fn test_path_map_from_hashmap() {
    let mut hm = std::collections::HashMap::new();
    hm.insert("a".to_string(), "node_a".to_string());
    hm.insert("b".to_string(), "node_b".to_string());

    let map = PathMap::from(hm);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("a"), Some(&"node_a".to_string()));
    assert_eq!(map.get("b"), Some(&"node_b".to_string()));
}

#[test]
fn test_path_map_from_slice() {
    let pairs = &[("approve", "publish"), ("reject", "archive")][..];
    let map = PathMap::from(pairs);

    assert_eq!(map.len(), 2);
    assert_eq!(map.get("approve"), Some(&"publish".to_string()));
    assert_eq!(map.get("reject"), Some(&"archive".to_string()));
}

#[test]
fn test_path_map_from_array() {
    let pairs = [("approve", "publish"), ("reject", "archive")];
    let map = PathMap::from(&pairs);

    assert_eq!(map.len(), 2);
    assert_eq!(map.get("approve"), Some(&"publish".to_string()));
    assert_eq!(map.get("reject"), Some(&"archive".to_string()));
}

#[tokio::test]
async fn test_router_sync_closure() {
    let router = simple_router;

    let state_high = TestState { value: 20 };
    let state_low = TestState { value: 5 };

    let result_high = router.route(&state_high).await.unwrap();
    let result_low = router.route(&state_low).await.unwrap();

    assert_eq!(result_high, RouteResult::One("high".to_string()));
    assert_eq!(result_low, RouteResult::One("low".to_string()));
}

#[test]
fn test_route_result_equality() {
    let result1 = RouteResult::One("target".to_string());
    let result2 = RouteResult::One("target".to_string());
    let result3 = RouteResult::One("other".to_string());

    assert_eq!(result1, result2);
    assert_ne!(result1, result3);
}

#[test]
fn test_path_map_iterator() {
    let mut map = PathMap::new();
    map.insert("a", "node_a");
    map.insert("b", "node_b");

    let pairs: Vec<_> = map.iter().collect();
    assert_eq!(pairs.len(), 2);

    let keys: Vec<_> = pairs.iter().map(|(k, _)| *k).collect();
    assert!(keys.contains(&&"a".to_string()));
    assert!(keys.contains(&&"b".to_string()));
}

// --- Issue #15 regression: conditional edges must translate the router's
// --- branch label through `path_map` at runtime, invoke the router exactly
// --- once per superstep, and fail loudly on a label that is neither a
// --- `path_map` key nor a registered node.

/// Loop state for the issue #15 regression graphs.
#[derive(Debug, Clone, Default, State, serde::Serialize, serde::Deserialize)]
struct LoopState {
    count: i32,
    audited: bool,
}

/// Router emitting branch labels ("continue"/"done") that the `path_map`
/// must translate into node names.
const fn loop_router(state: &LoopState) -> &str {
    if state.count >= 3 { "done" } else { "continue" }
}

/// Router returning node names directly (issue #15 case A: identity map).
const fn identity_router(state: &LoopState) -> &str {
    if state.count >= 3 { END } else { "plan" }
}

/// Router returning a label that is neither a `path_map` key nor a known node.
struct BogusRouter;

impl Router<LoopState> for BogusRouter {
    fn route(
        &self,
        _state: &LoopState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        Box::pin(async { Ok(RouteResult::One("bogus".to_string())) })
    }
}

/// Counting router for the once-per-superstep invocation contract.
struct CountingRouter {
    calls: Arc<AtomicUsize>,
}

impl Router<LoopState> for CountingRouter {
    fn route(
        &self,
        state: &LoopState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let label = if state.count >= 3 { "done" } else { "continue" };
        Box::pin(async move { Ok(RouteResult::One(label.to_string())) })
    }
}

/// Router fanning out to two declared branches mid-loop.
struct MultiRouter;

impl Router<LoopState> for MultiRouter {
    fn route(
        &self,
        state: &LoopState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        let result = if state.count >= 3 {
            RouteResult::One("done".to_string())
        } else if state.count == 1 {
            RouteResult::Multiple(vec!["continue".to_string(), "review".to_string()])
        } else {
            RouteResult::One("continue".to_string())
        };
        Box::pin(async move { Ok(result) })
    }
}

/// Router returning node names directly with an *empty* `path_map`
/// (`LangGraph`'s `path_map=None` mode).
struct NameRouter;

impl Router<LoopState> for NameRouter {
    fn route(
        &self,
        state: &LoopState,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult, JunctureError>> + Send + '_>> {
        let label = if state.count >= 3 { END } else { "plan" };
        Box::pin(async move { Ok(RouteResult::One(label.to_string())) })
    }
}

/// Build the issue #15 loop graph: `plan` self-loops through a conditional
/// edge whose `path_map` closes the cycle.
fn build_loop_graph(
    router: Arc<dyn Router<LoopState>>,
    path_map: PathMap,
) -> CompiledGraph<LoopState> {
    let mut graph: StateGraph<LoopState> = StateGraph::new();
    graph
        .add_node_simple(
            "plan",
            NodeFnUpdate(|state: &LoopState| {
                let next = state.count + 1;
                Box::pin(async move {
                    Ok::<_, JunctureError>(LoopStateUpdate {
                        count: Some(next),
                        audited: None,
                    })
                })
            }),
        )
        .expect("add_node_simple should succeed");
    graph.set_entry_point("plan");
    graph.add_edge(START, "plan");
    graph.add_conditional_edges("plan", router, path_map);
    graph.compile().expect("compile should succeed")
}

/// Issue #15 case A: an identity `path_map` (router returns the key, the key
/// maps to itself) must keep routing exactly as before the fix.
#[tokio::test]
async fn test_conditional_edge_identity_path_map_still_routes() {
    let mut path_map = PathMap::new();
    path_map.insert("plan", "plan");
    path_map.insert(END, END);

    let app = build_loop_graph(Arc::new(identity_router), path_map);
    let output = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await
        .expect("invoke should succeed");
    assert_eq!(output.value.count, 3);
}

/// Issue #15 case B: a conditional edge closing a cycle through a
/// *non-identity* `path_map` ({"continue" -> "plan", "done" -> END}). The
/// pre-fix engine used the raw router label as a node name, matched nothing,
/// and silently stopped after one superstep returning Ok(count=1).
#[tokio::test]
async fn test_conditional_edge_non_identity_path_map_closes_cycle() {
    let mut path_map = PathMap::new();
    path_map.insert("continue", "plan");
    path_map.insert("done", END);

    let app = build_loop_graph(Arc::new(loop_router), path_map);
    let output = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await
        .expect("invoke should succeed");
    assert_eq!(output.value.count, 3, "loop must run three supersteps");
}

/// `LangGraph` invokes the branch function exactly once per superstep: three
/// plan executions means three router calls, not the six the pre-fix engine
/// produced (one in the gate check, one in edge processing).
#[tokio::test]
async fn test_conditional_edge_router_invoked_once_per_superstep() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut path_map = PathMap::new();
    path_map.insert("continue", "plan");
    path_map.insert("done", END);

    let app = build_loop_graph(
        Arc::new(CountingRouter {
            calls: Arc::clone(&calls),
        }),
        path_map,
    );
    let output = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await
        .expect("invoke should succeed");
    assert_eq!(output.value.count, 3);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "router must be invoked exactly once per superstep"
    );
}

/// A label that is neither a `path_map` key nor a registered node must
/// surface as an execution error naming the label, not a silent stop.
#[tokio::test]
async fn test_conditional_edge_unknown_branch_label_is_error() {
    let mut path_map = PathMap::new();
    path_map.insert("continue", "plan");
    path_map.insert("done", END);

    let app = build_loop_graph(Arc::new(BogusRouter), path_map);
    let result = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await;

    let error = result.expect_err("unknown label must error, not stop silently");
    assert!(
        error.to_string().contains("bogus"),
        "error must name the unresolvable label, got: {error}"
    );
}

/// `RouteResult::Multiple`: every branch label is translated independently
/// and all referenced nodes actually execute.
#[tokio::test]
async fn test_conditional_edge_multiple_targets_all_resolve() {
    let mut path_map = PathMap::new();
    path_map.insert("continue", "plan");
    path_map.insert("review", "audit");
    path_map.insert("done", END);

    let mut graph: StateGraph<LoopState> = StateGraph::new();
    graph
        .add_node_simple(
            "plan",
            NodeFnUpdate(|state: &LoopState| {
                let next = state.count + 1;
                Box::pin(async move {
                    Ok::<_, JunctureError>(LoopStateUpdate {
                        count: Some(next),
                        audited: None,
                    })
                })
            }),
        )
        .expect("add_node_simple should succeed");
    graph
        .add_node_simple(
            "audit",
            NodeFnUpdate(|_state: &LoopState| {
                Box::pin(async {
                    Ok::<_, JunctureError>(LoopStateUpdate {
                        count: None,
                        audited: Some(true),
                    })
                })
            }),
        )
        .expect("add_node_simple should succeed");
    graph.set_entry_point("plan");
    graph.add_edge(START, "plan");
    graph.add_conditional_edges("plan", Arc::new(MultiRouter), path_map);

    let app = graph.compile().expect("compile should succeed");
    let output = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await
        .expect("invoke should succeed");
    assert_eq!(output.value.count, 3);
    assert!(output.value.audited, "fan-out branch must have executed");
}

/// An empty `path_map` with a router returning node names directly (`LangGraph`'s
/// `path_map=None` mode): the registered-node fallback must schedule the named
/// node and `END` must terminate.
#[tokio::test]
async fn test_conditional_edge_empty_path_map_routes_by_node_name() {
    let app = build_loop_graph(Arc::new(NameRouter), PathMap::new());
    let output = app
        .invoke_async(LoopState::default(), &RunnableConfig::new())
        .await
        .expect("invoke should succeed");
    assert_eq!(output.value.count, 3);
}

// Rust guideline compliant 2026-05-18
