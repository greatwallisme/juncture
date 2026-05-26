//! Integration tests for Node system

use juncture_core::node::{
    NodeFnCommand, NodeFnCommandWithConfig, NodeFnUpdate, NodeFnUpdateWithConfig,
};
use juncture_core::{IntoNode, JunctureError, RunnableConfig};
use juncture_derive::State;
use std::pin::Pin;
use std::sync::Arc;

// Type alias for boxed futures to satisfy higher-ranked lifetime bounds
type BoxResult<T> = Pin<Box<dyn Future<Output = Result<T, JunctureError>> + Send>>;

// Test state types
#[derive(Debug, Clone, Default, State)]
struct TestState {
    value: u32,
}

#[tokio::test]
async fn test_into_node_from_simple_function() {
    let node = NodeFnUpdate(
        |state: &TestState| -> BoxResult<<TestState as juncture_core::State>::Update> {
            let value = state.value;
            Box::pin(async move {
                Ok(TestStateUpdate {
                    value: Some(value + 1),
                })
            })
        },
    )
    .into_node("simple");
    assert_eq!(node.name(), "simple");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(&state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_config_function() {
    let node = NodeFnUpdateWithConfig(
        |state: &TestState,
         _config: RunnableConfig|
         -> BoxResult<<TestState as juncture_core::State>::Update> {
            let value = state.value;
            Box::pin(async move {
                Ok(TestStateUpdate {
                    value: Some(value + 2),
                })
            })
        },
    )
    .into_node("config");
    assert_eq!(node.name(), "config");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(&state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_command_function() {
    let node = NodeFnCommand(
        |state: &TestState| -> BoxResult<juncture_core::Command<TestState>> {
            let value = state.value;
            Box::pin(async move {
                Ok(juncture_core::Command::update(TestStateUpdate {
                    value: Some(value + 3),
                }))
            })
        },
    )
    .into_node("command");
    assert_eq!(node.name(), "command");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(&state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_into_node_from_full_function() {
    let node = NodeFnCommandWithConfig(
        |state: &TestState,
         _config: RunnableConfig|
         -> BoxResult<juncture_core::Command<TestState>> {
            let value = state.value;
            Box::pin(async move {
                Ok(juncture_core::Command::update(TestStateUpdate {
                    value: Some(value + 4),
                }))
            })
        },
    )
    .into_node("full");
    assert_eq!(node.name(), "full");

    let state = TestState { value: 10 };
    let config = RunnableConfig::default();

    let result = node.call(&state, &config).await;
    assert!(result.is_ok());

    let command = result.unwrap();
    assert!(command.update.is_some());
}

#[tokio::test]
async fn test_node_arc_cloning() {
    let node: Arc<dyn juncture_core::Node<TestState>> = NodeFnUpdate(
        |state: &TestState| -> BoxResult<<TestState as juncture_core::State>::Update> {
            let value = state.value;
            Box::pin(async move {
                Ok(TestStateUpdate {
                    value: Some(value + 1),
                })
            })
        },
    )
    .into_node("clone_test");

    let node1 = Arc::clone(&node);
    let node2 = Arc::clone(&node);

    assert_eq!(node.name(), "clone_test");
    assert_eq!(node1.name(), "clone_test");
    assert_eq!(node2.name(), "clone_test");

    let state = TestState { value: 5 };
    let config = RunnableConfig::default();

    let result1 = node1.call(&state, &config).await.unwrap();
    let result2 = node2.call(&state, &config).await.unwrap();

    assert!(result1.update.is_some());
    assert!(result2.update.is_some());
}

// Rust guideline compliant 2026-05-18
