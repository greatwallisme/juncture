//! Integration tests for Node system

use juncture_core::node::{
    NodeFnCommand, NodeFnCommandWithConfig, NodeFnUpdate, NodeFnUpdateWithConfig,
};
use juncture_core::{IntoNode, JunctureError, RunnableConfig};
use juncture_derive::State;
use std::sync::Arc;

// Test state types
#[derive(Debug, Clone, State)]
struct TestState {
    value: u32,
}

// Test nodes
async fn simple_node(state: TestState) -> Result<TestStateUpdate, JunctureError> {
    Ok(TestStateUpdate {
        value: Some(state.value + 1),
    })
}

async fn config_node(
    state: TestState,
    _config: RunnableConfig,
) -> Result<TestStateUpdate, JunctureError> {
    Ok(TestStateUpdate {
        value: Some(state.value + 2),
    })
}

async fn command_node(
    state: TestState,
) -> Result<juncture_core::Command<TestState>, JunctureError> {
    Ok(juncture_core::Command::update(TestStateUpdate {
        value: Some(state.value + 3),
    }))
}

async fn full_node(
    state: TestState,
    _config: RunnableConfig,
) -> Result<juncture_core::Command<TestState>, JunctureError> {
    Ok(juncture_core::Command::update(TestStateUpdate {
        value: Some(state.value + 4),
    }))
}

#[tokio::test]
async fn test_into_node_from_simple_function() {
    let node = NodeFnUpdate(simple_node).into_node("simple");
    assert_eq!(node.name(), "simple");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_config_function() {
    let node = NodeFnUpdateWithConfig(config_node).into_node("config");
    assert_eq!(node.name(), "config");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_command_function() {
    let node = NodeFnCommand(command_node).into_node("command");
    assert_eq!(node.name(), "command");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_full_function() {
    let node = NodeFnCommandWithConfig(full_node).into_node("full");
    assert_eq!(node.name(), "full");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_node_arc_cloning() {
    let node: Arc<dyn juncture_core::Node<TestState>> =
        NodeFnUpdate(simple_node).into_node("clone_test");

    let node1 = Arc::clone(&node);
    let node2 = Arc::clone(&node);

    assert_eq!(node.name(), "clone_test");
    assert_eq!(node1.name(), "clone_test");
    assert_eq!(node2.name(), "clone_test");

    let state = TestState { value: 5 };
    let config = RunnableConfig::default();

    let result1 = node1.call(state.clone(), &config).await.unwrap();
    let result2 = node2.call(state, &config).await.unwrap();

    assert!(result1.update.is_some());
    assert!(result2.update.is_some());
}

// Rust guideline compliant 2026-05-18
