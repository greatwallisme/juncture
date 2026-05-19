//! Integration tests for Edge system

use juncture_core::edge::{END, Edge, PathMap, RouteResult, Router, START};
use juncture_derive::State;
use std::sync::Arc;

// Test state
#[derive(Debug, Clone, State)]
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

// Rust guideline compliant 2026-05-18
