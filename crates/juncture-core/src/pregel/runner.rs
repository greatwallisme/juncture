//! Parallel execution for Pregel engine
//!
//! This module provides parallel task execution using tokio for concurrent
//! node execution with bounded concurrency and cancellation support.

use crate::{
    JunctureError, Node, State,
    config::RunnableConfig,
    pregel::types::{PendingTask, SuperstepResult, TaskOutput},
};
use std::{sync::Arc, time::Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Execute a single superstep in parallel
///
/// This function spawns all pending tasks concurrently, respecting the
/// `max_parallel_tasks` limit from the config, and collects their results
/// as they complete.
///
/// # Arguments
///
/// * `pending_tasks` - Tasks to execute in this superstep
/// * `state` - Current state (cloned for each task unless override provided)
/// * `nodes` - Graph nodes
/// * `config` - Execution configuration
/// * `cancellation_token` - Token for cooperative cancellation
///
/// # Returns
///
/// A `SuperstepResult` containing outputs from all completed tasks.
///
/// # Errors
///
/// Returns an error if:
/// - A node execution fails
/// - Cancellation is requested
/// - A task's node is not found in the graph
///
/// # Panics
///
/// Panics if the semaphore permit acquisition fails (should never happen
/// in normal operation as the semaphore is created with the correct permit count).
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::runner::execute_superstep;
/// use tokio_util::sync::CancellationToken;
///
/// # let pending_tasks = vec![];
/// # let state = MyState;
/// # let nodes = IndexMap::new();
/// # let config = RunnableConfig::new();
/// # let token = CancellationToken::new();
/// let result = execute_superstep(&pending_tasks, &state, &nodes, &config, &token).await?;
/// ```
pub async fn execute_superstep<S: State>(
    pending_tasks: &[PendingTask<S>],
    state: &S,
    nodes: &indexmap::IndexMap<String, Arc<dyn Node<S>>>,
    config: &RunnableConfig,
    cancellation_token: &CancellationToken,
) -> Result<SuperstepResult<S>, JunctureError> {
    if pending_tasks.is_empty() {
        return Ok(SuperstepResult::empty());
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_parallel_tasks));
    let mut join_set = JoinSet::new();

    // Spawn all tasks
    for task in pending_tasks {
        let node = Arc::clone(nodes.get(&task.node_name).ok_or_else(|| {
            JunctureError::execution(format!("Node '{}' not found", task.node_name))
        })?);

        let task_state = task.state_override.clone().unwrap_or_else(|| state.clone());

        let task_config = config.clone();
        let task_id = task.id.clone();
        let node_name = task.node_name.clone();
        let task_trigger = task.trigger.clone();
        let permit = Arc::clone(&semaphore);
        let token = cancellation_token.clone();

        join_set.spawn(async move {
            // Acquire semaphore permit
            let _permit = permit.acquire_owned().await.unwrap();

            let start = Instant::now();

            // Execute task with cancellation support
            let result = tokio::select! {
                biased;
                () = token.cancelled() => {
                    return Err(JunctureError::execution("Task cancelled"));
                }
                result = node.call(task_state, &task_config) => result,
            };

            let duration = start.elapsed();

            result.map(|command| TaskOutput {
                task_id,
                node_name,
                command,
                duration,
                trigger: task_trigger,
            })
        });
    }

    // Collect results as they complete
    let mut task_outputs = Vec::new();

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(output)) => {
                task_outputs.push(output);
            }
            Ok(Err(error)) => {
                // Task failed, cancel remaining tasks
                cancellation_token.cancel();
                join_set.shutdown().await;

                return Err(error);
            }
            Err(join_error) => {
                // Task panicked
                cancellation_token.cancel();
                join_set.shutdown().await;

                return Err(JunctureError::execution(format!(
                    "Task panicked: {join_error}"
                )));
            }
        }
    }

    Ok(SuperstepResult { task_outputs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{IntoNode, NodeFnCommand};

    #[tokio::test]
    async fn test_execute_superstep_empty() {
        let state = TestState;
        let nodes = indexmap::IndexMap::new();
        let config = RunnableConfig::new();
        let token = CancellationToken::new();

        let result = execute_superstep(&[], &state, &nodes, &config, &token)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_execute_superstep_single_task() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "test_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(crate::Command::end()) }).into_node("test_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];

        let result = execute_superstep(&tasks, &state, &nodes, &config, &token)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result.task_outputs[0].node_name, "test_node");
    }

    #[tokio::test]
    async fn test_execute_superstep_parallel_tasks() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        for i in 0..3 {
            nodes.insert(
                format!("node_{i}"),
                NodeFnCommand(move |_s| async move { Ok(crate::Command::end()) })
                    .into_node(format!("node_{i}").as_str()),
            );
        }

        let config = RunnableConfig::new();
        let token = CancellationToken::new();

        let tasks: Vec<PendingTask<TestState>> = (0..3)
            .map(|i| PendingTask::pull(uuid::Uuid::new_v4().to_string(), format!("node_{i}")))
            .collect();

        let result = execute_superstep(&tasks, &state, &nodes, &config, &token)
            .await
            .unwrap();

        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_execute_superstep_cancellation() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "slow_node".to_string(),
            NodeFnCommand(|_s| async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                Ok(crate::Command::end())
            })
            .into_node("slow_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "slow_node".to_string(),
        )];

        // Cancel immediately
        token.cancel();

        let result = execute_superstep(&tasks, &state, &nodes, &config, &token).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
    }

    #[tokio::test]
    async fn test_execute_superstep_node_not_found() {
        let state = TestState;
        let nodes = indexmap::IndexMap::new();
        let config = RunnableConfig::new();
        let token = CancellationToken::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "nonexistent".to_string(),
        )];

        let result = execute_superstep(&tasks, &state, &nodes, &config, &token).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
    }

    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        type FieldVersions = ();

        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
    struct TestUpdate;
}

// Rust guideline compliant 2026-05-19
