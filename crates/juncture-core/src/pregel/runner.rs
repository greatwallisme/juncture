//! Parallel execution for Pregel engine
//!
//! This module provides parallel task execution using tokio for concurrent
//! node execution with bounded concurrency and cancellation support.

use crate::{
    JunctureError, Node, State,
    config::RunnableConfig,
    graph::{RetryPolicy, execute_with_retry},
    interrupt::{InterruptContext, InterruptSignal, ResumeValue, Scratchpad},
    pregel::context::TimeoutPolicy,
    pregel::types::{PendingTask, SuperstepResult, TaskOutput},
    runtime::Heartbeat,
};
use std::collections::HashMap;
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

#[cfg(feature = "otel")]
use tracing::{Level, event};

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
/// * `checkpointer` - Optional checkpointer for immediate write persistence
/// * `pending_interrupts` - Interrupt signals from prior supersteps (for multi-interrupt matching)
/// * `scratchpad` - Scratchpad tracking processed interrupts (for null-resume detection)
/// * `error_handler_map` - Maps node names to their error handler node names
/// * `retry_policies` - Per-node retry policies; nodes with entries are wrapped
///   with [`execute_with_retry`] using exponential backoff and jitter
/// * `timeout_policies` - Per-node timeout policies; nodes with entries are wrapped
///   with `tokio::time::timeout` using the configured `run_timeout`. The timeout
///   wraps the entire execution including retry attempts.
/// * `step` - Current superstep number (for observability span attribute `juncture.step`)
///
/// # Returns
///
/// A tuple containing:
/// - A `SuperstepResult` with outputs from all completed tasks
/// - The interrupt signal receiver channel (for draining interrupt signals)
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
/// use juncture_core::interrupt::Scratchpad;
/// use tokio_util::sync::CancellationToken;
///
/// # let pending_tasks = vec![];
/// # let state = MyState;
/// # let nodes = IndexMap::new();
/// # let config = RunnableConfig::new();
/// # let token = CancellationToken::new();
/// # let pending_interrupts = vec![];
/// # let scratchpad = Scratchpad::new();
/// let retry_policies = std::collections::HashMap::new();
/// let timeout_policies = std::collections::HashMap::new();
/// let (result, _interrupt_rx) = execute_superstep(
///     &pending_tasks,
///     &state,
///     &nodes,
///     &config,
///     &token,
///     None::<Arc<dyn CheckpointSaver>>,
///     &pending_interrupts,
///     &scratchpad,
///     &std::collections::HashMap::new(),
///     &retry_policies,
///     &timeout_policies,
///     0, // step
/// ).await?;
/// ```
#[expect(
    clippy::too_many_lines,
    reason = "execute_superstep requires: early return, semaphore creation, interrupt context setup, task spawning with span creation, timeout/retry wrapping, and result collection. The length is justified by the complexity of parallel execution with proper error handling and observability."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "execute_superstep requires: tasks, state, nodes, config, cancellation token, checkpointer, pending interrupts, scratchpad, error handler map, retry policies, timeout policies, and step. All are necessary for the multi-interrupt matching algorithm, error recovery, retry execution, and timeout enforcement."
)]
#[expect(
    clippy::implicit_hasher,
    reason = "error_handler_map, retry_policies, and timeout_policies use std::collections::HashMap as the canonical type matching the builder metadata extraction; no alternative hasher is needed."
)]
pub async fn execute_superstep<S: State>(
    pending_tasks: &[PendingTask<S>],
    state: &S,
    nodes: &indexmap::IndexMap<String, Arc<dyn Node<S>>>,
    config: &RunnableConfig,
    cancellation_token: &CancellationToken,
    checkpointer: Option<&Arc<dyn crate::checkpoint::CheckpointSaver>>,
    pending_interrupts: &[InterruptSignal],
    scratchpad: &Scratchpad,
    error_handler_map: &HashMap<String, String>,
    retry_policies: &HashMap<String, RetryPolicy>,
    timeout_policies: &HashMap<String, TimeoutPolicy>,
    step: usize,
) -> Result<
    (
        SuperstepResult<S>,
        mpsc::UnboundedReceiver<crate::interrupt::InterruptSignal>,
    ),
    JunctureError,
>
where
    S::Update: serde::Serialize,
{
    if pending_tasks.is_empty() {
        // Return empty result and a dummy channel
        let (_interrupt_tx, interrupt_rx) = mpsc::unbounded_channel();
        return Ok((SuperstepResult::empty(), interrupt_rx));
    }

    // Create interrupt context for HITL support
    // Use multi-interrupt matching algorithm that consults the scratchpad
    // for processed interrupts when resolving resume values
    let resume_values =
        match_resume_to_interrupts(&config.resume_value, pending_interrupts, scratchpad);

    let (interrupt_tx, interrupt_rx) = mpsc::unbounded_channel();
    let interrupt_context = Arc::new(InterruptContext::new(resume_values, interrupt_tx));

    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_parallel_tasks));
    let mut join_set = JoinSet::new();

    // Spawn all tasks
    for task in pending_tasks {
        let node = Arc::clone(nodes.get(&task.node_name).ok_or_else(|| {
            JunctureError::execution(format!("Node '{}' not found", task.node_name))
        })?);

        let task_state = task.state_override.clone().unwrap_or_else(|| state.clone());

        let mut task_config = config.clone();
        let task_id = task.id.clone();
        let node_name = task.node_name.clone();
        let task_trigger = task.trigger.clone();
        let permit = Arc::clone(&semaphore);
        let token = cancellation_token.clone();
        let ctx = Arc::clone(&interrupt_context);
        let has_error_handler = error_handler_map.contains_key(&node_name);

        // Extract per-node retry policy (if any) before moving into the async block
        let retry_policy = retry_policies.get(&task.node_name).cloned();

        // Extract per-node timeout policy (if any) before moving into the async block
        let timeout_policy = timeout_policies.get(&task.node_name).cloned();

        // Create a per-task heartbeat watcher when idle_timeout is configured.
        // The heartbeat sender is stored in the task_config so nodes with config
        // access can call `config.heartbeat.as_ref().map(|h| h.ping())` to signal
        // liveness. The watcher is moved into the spawned future for idle timeout
        // monitoring alongside the node execution.
        let idle_watcher = timeout_policy.as_ref().and_then(|tp| {
            tp.idle_timeout.map(|_| {
                let (heartbeat, watcher) = Heartbeat::new_pair();
                task_config.heartbeat = Some(heartbeat);
                watcher
            })
        });

        // Extract callback handler separately so it can be used after task_config
        // is moved into the async block for node.call()
        let callback_handler = task_config.callback_handler.clone();

        // Create span before moving values into the async block
        let span = tracing::info_span!(
            "juncture.node.execute",
            node_name = %node_name,
            task_id = %task_id,
            "juncture.step" = step,
            "juncture.thread.id" = %config.thread_id.as_deref().unwrap_or(""),
            "juncture.node.output_type" = tracing::field::Empty,
            "juncture.node.duration_ms" = tracing::field::Empty,
            "juncture.node.error" = tracing::field::Empty,
        );

        join_set.spawn(
            async move {
                // Acquire semaphore permit
                // The semaphore is created with max_parallel_tasks permits and
                // we never close it, so acquisition should always succeed
                let _permit = permit.acquire_owned().await.expect(
                    "Semaphore acquisition failed: semaphore should never be closed \
                     as it is owned by the PregelLoop and never dropped during execution",
                );

                // Notify callback handler: node starting
                if let Some(ref handler) = callback_handler {
                    handler.on_node_start(&node_name, &task_id);
                }

                let start = Instant::now();

                // Clone node_name for use inside the async execution block.
                // The outer node_name is retained for error reporting paths
                // and the final TaskOutput construction below.
                let exec_node_name = node_name.clone();

                // Execute task with cancellation support, interrupt context,
                // optional retry wrapping, and optional timeout enforcement.
                // The INTERRUPT_CONTEXT.scope() makes the context available to
                // the interrupt!() macro within node execution.
                //
                // Layering order (outermost to innermost):
                //   1. cancellation (tokio::select!)
                //   2. timeout (tokio::time::timeout, when configured)
                //   3. idle timeout (heartbeat-based, when configured)
                //   4. retry (execute_with_retry, when configured)
                //   5. interrupt context (INTERRUPT_CONTEXT.scope)
                //   6. node.call()
                //
                // The timeout wraps the entire retry sequence so that cumulative
                // retry time is bounded by the run_timeout. The idle timeout
                // runs concurrently with the node execution and fires when no
                // heartbeat is received within the configured idle timeout.
                let result = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        tracing::Span::current().record("juncture.node.error", "cancelled");
                        tracing::Span::current().record("otel.status_code", "ERROR");
                        let err = JunctureError::execution("Task cancelled");
                        if let Some(ref handler) = callback_handler {
                            handler.on_node_error(&node_name, &err);
                        }
                        return Err((node_name.clone(), err));
                    }
                    result = async {
                        // Clone node name for the timeout error path before
                        // exec_node_name is moved into the inner_future closure.
                        let timeout_node_name = exec_node_name.clone();

                        let inner_future = async {
                            if let Some(ref policy) = retry_policy {
                                // Retry-enabled execution: each attempt runs inside
                                // the interrupt context so interrupt!() works across
                                // retries. State is cloned per-attempt by execute_with_retry.
                                let ctx_ref = Arc::clone(&ctx);
                                crate::interrupt::INTERRUPT_CONTEXT.scope(ctx_ref, async move {
                                    execute_with_retry(
                                        &exec_node_name,
                                        policy,
                                        |s, cfg| node.call(s, cfg),
                                        task_state,
                                        &task_config,
                                    )
                                    .await
                                }).await
                            } else {
                                // Standard execution (no retry)
                                crate::interrupt::INTERRUPT_CONTEXT.scope(ctx, async move {
                                    node.call(task_state, &task_config).await
                                }).await
                            }
                        };

                        // Wrap with timeout when a timeout policy is configured.
                        // When idle_timeout is also configured, the task wraps
                        // with idle timeout monitoring that checks heartbeats.
                        if let Some(ref tp) = timeout_policy {

                            let timeout_result = if let (Some(idle_to), Some(mut watcher)) = (
                                tp.idle_timeout,
                                idle_watcher,
                            ) {
                                // With both run_timeout and idle_timeout.
                                // The idle timeout wraps the execution: if no
                                // heartbeat is received within idle_to, the
                                // task is considered stale.
                                let to_name = timeout_node_name.clone();
                                tokio::time::timeout(tp.run_timeout, async move {
                                    tokio::pin!(inner_future);
                                    loop {
                                        tokio::select! {
                                            result = &mut inner_future => return result,
                                            () = tokio::time::sleep(idle_to) => {
                                                if !watcher.is_alive(idle_to) {
                                                    return Err(
                                                        crate::JunctureError::node_timeout(
                                                            crate::error::NodeTimeoutError::IdleTimeout {
                                                                node: to_name,
                                                                timeout: u64::try_from(
                                                                    idle_to.as_millis(),
                                                                ).unwrap_or(u64::MAX),
                                                            },
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                })
                                .await
                            } else {
                                // Standard run_timeout without idle monitoring
                                tokio::time::timeout(tp.run_timeout, inner_future)
                                    .await
                            };

                            timeout_result.map_or_else(
                                |_| {
                                    Err(crate::JunctureError::node_timeout(
                                        crate::error::NodeTimeoutError::RunTimeout {
                                            node: timeout_node_name,
                                            timeout: u64::try_from(tp.run_timeout.as_millis())
                                                .unwrap_or(u64::MAX),
                                        },
                                    ))
                                },
                                std::convert::identity,
                            )
                        } else {
                            inner_future.await
                        }
                    } => result,
                };

                let duration = start.elapsed();

                // Determine and record output type
                // Priority: error_handler > interrupt > send > end > goto > update
                let output_type = result.as_ref().map_or(
                    if has_error_handler {
                        "error_handler"
                    } else {
                        "error"
                    },
                    |command| {
                        if command.resume.is_some() {
                            "interrupt"
                        } else if matches!(command.goto, crate::command::Goto::Send(_)) {
                            "send"
                        } else if matches!(command.goto, crate::command::Goto::End) {
                            "end"
                        } else if !matches!(command.goto, crate::command::Goto::None) {
                            "goto"
                        } else if command.update.is_some() {
                            "update"
                        } else {
                            "none"
                        }
                    },
                );

                // Record output type in span
                tracing::Span::current().record("juncture.node.output_type", output_type);

                // Record duration in span
                let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
                tracing::Span::current().record("juncture.node.duration_ms", duration_ms);

                // Record error attributes when task execution failed
                if let Err(ref e) = result {
                    tracing::Span::current()
                        .record("juncture.node.error", tracing::field::display(e));
                    tracing::Span::current().record("otel.status_code", "ERROR");
                }

                #[cfg(feature = "otel")]
                {
                    // Emit metrics for node execution
                    event!(
                        name: "juncture.node.execute.metrics",
                        Level::DEBUG,
                        node_name = %node_name,
                        duration_ms = duration.as_millis(),
                        success = result.is_ok(),
                        output_type = %output_type,
                    );
                };

                // Notify callback handler of node result
                if let Some(ref handler) = callback_handler {
                    match &result {
                        Ok(_) => {
                            handler.on_node_end(&node_name, &task_id, duration_ms);
                        }
                        Err(err) => {
                            handler.on_node_error(&node_name, err);
                        }
                    }
                }

                result
                    .map(|command| TaskOutput {
                        task_id,
                        node_name: node_name.clone(),
                        command,
                        duration,
                        trigger: task_trigger,
                        error: None,
                    })
                    .map_err(|e| (node_name, e))
            }
            .instrument(span),
        );
    }

    // Collect results as they complete. Errors carry the node_name so the
    // error handler map can be consulted without relying on the error message.
    let mut task_outputs = Vec::new();

    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(output)) => {
                // Persist writes immediately after each task completes.
                // This ensures crash recovery can resume from the last completed task.
                // Each PendingWrite records one field (channel) that was modified,
                // enabling fine-grained replay of partially completed supersteps.
                if let Some(cp) = checkpointer
                    && let Some(ref update) = output.command.update
                {
                    let writes = serialize_pending_writes(&output.task_id, update);
                    if !writes.is_empty() {
                        let _ = cp.put_writes(config, writes, &output.task_id).await;
                    }
                }

                task_outputs.push(output);
            }
            Ok(Err((failed_node_name, error))) => {
                // Task failed. Check if the node has a registered error handler.
                // If so, record the error in the output and continue executing
                // remaining tasks. If not, cancel all remaining tasks.
                if let Some(handler_name) = error_handler_map.get(&failed_node_name) {
                    tracing::warn!(
                        name: "juncture.node.error.handler_scheduled",
                        node_name = %failed_node_name,
                        handler = %handler_name,
                        error = %error,
                        "Node failed with error handler registered, scheduling recovery"
                    );

                    task_outputs.push(TaskOutput {
                        task_id: uuid::Uuid::new_v4().to_string(),
                        node_name: failed_node_name,
                        command: crate::Command::default(),
                        duration: std::time::Duration::ZERO,
                        trigger: crate::pregel::types::TaskTrigger::Pull,
                        error: Some(error),
                    });
                } else {
                    // No error handler, cancel remaining tasks
                    cancellation_token.cancel();
                    join_set.shutdown().await;

                    return Err(error);
                }
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

    Ok((
        SuperstepResult {
            task_outputs,
            bubble_ups: Vec::new(),
        },
        interrupt_rx,
    ))
}

/// Match resume values against pending interrupts using the design-specified algorithm.
///
/// The matching follows three strategies depending on the `ResumeValue` variant:
///
/// 1. **Single (global matching)**: A single value is applied to all pending interrupts
///    that are not already processed in the scratchpad. Processed interrupts receive
///    a `Null` resume value instead. When no pending interrupts exist, returns a
///    single-element vector for the first interrupt position.
///
/// 2. **`ById` (ID-based matching)**: Maps values by interrupt ID. If an interrupt has
///    been processed (tracked in the scratchpad), it receives a `Null` resume value
///    so the node skips re-execution of that interrupt point.
///
/// 3. **`ByNamespace` (index-based matching)**: Parses numeric keys as positional indices.
///    Processed interrupts in the scratchpad that have no explicit mapping receive
///    a `Null` resume value.
///
/// # Arguments
///
/// * `resume_value` - Optional resume value from config
/// * `pending_interrupts` - Interrupt signals from prior supersteps
/// * `scratchpad` - Scratchpad tracking processed interrupts
///
/// # Returns
///
/// A vector of resume values indexed by interrupt position
#[allow(
    clippy::ref_option,
    reason = "config stores Option<ResumeValue> and we need to pass it by reference"
)]
#[must_use]
fn match_resume_to_interrupts(
    resume_value: &Option<ResumeValue>,
    pending_interrupts: &[InterruptSignal],
    scratchpad: &Scratchpad,
) -> Vec<Option<serde_json::Value>> {
    let Some(rv) = resume_value else {
        return Vec::new();
    };

    match rv {
        ResumeValue::Single(value) => {
            // Global matching: single value for all pending interrupts,
            // but processed interrupts (tracked in scratchpad) receive
            // null-resume so they are silently acknowledged without
            // re-triggering.
            if pending_interrupts.is_empty() {
                vec![Some(value.clone())]
            } else {
                pending_interrupts
                    .iter()
                    .map(|signal| {
                        if let Some(ref id) = signal.id
                            && scratchpad.get_null_resume(id)
                        {
                            return Some(serde_json::Value::Null);
                        }
                        Some(value.clone())
                    })
                    .collect()
            }
        }
        ResumeValue::ById(map) => {
            // ID-based matching: check scratchpad for processed -> null-resume
            pending_interrupts
                .iter()
                .map(|signal| {
                    if let Some(ref id) = signal.id {
                        if let Some(value) = map.get(id) {
                            return Some(value.clone());
                        }
                        if scratchpad.get_null_resume(id) {
                            return Some(serde_json::Value::Null);
                        }
                    }
                    None
                })
                .collect()
        }
        ResumeValue::ByNamespace(map) => {
            // Index-based matching: parse numeric keys, skip processed
            let max_index = map
                .keys()
                .filter_map(|k| k.parse::<usize>().ok())
                .max()
                .unwrap_or(0);

            let size = pending_interrupts.len().max(max_index + 1);
            let mut values = vec![None; size];

            for (key, value) in map {
                if let Ok(index) = key.parse::<usize>()
                    && index < values.len()
                {
                    values[index] = Some(value.clone());
                }
            }

            // Fill processed interrupts with null-resume
            for (i, signal) in pending_interrupts.iter().enumerate() {
                if values[i].is_none()
                    && let Some(ref id) = signal.id
                    && scratchpad.get_null_resume(id)
                {
                    values[i] = Some(serde_json::Value::Null);
                }
            }

            values
        }
    }
}

/// Serialize an Update into per-field `PendingWrite` entries.
///
/// The `#[derive(State)]` macro generates Update structs where each field is
/// `Option<T>`. When serialized to JSON, `None` fields become `null` and
/// `Some(v)` fields become the actual value. This function filters out null
/// entries and creates one `PendingWrite` per non-null field, enabling
/// fine-grained crash recovery at the channel level.
///
/// Returns an empty vector if serialization fails (graceful degradation).
fn serialize_pending_writes<U>(task_id: &str, update: &U) -> Vec<crate::checkpoint::PendingWrite>
where
    U: serde::Serialize,
{
    let Ok(value) = serde_json::to_value(update) else {
        return Vec::new();
    };
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(channel, value)| crate::checkpoint::PendingWrite {
            task_id: task_id.to_string(),
            channel: channel.clone(),
            value: value.clone(),
        })
        .collect()
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
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let (result, _rx) = execute_superstep(
            &[],
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
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
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "test_node".to_string(),
        )];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
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
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks: Vec<PendingTask<TestState>> = (0..3)
            .map(|i| PendingTask::pull(uuid::Uuid::new_v4().to_string(), format!("node_{i}")))
            .collect();

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
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
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "slow_node".to_string(),
        )];

        // Cancel immediately
        token.cancel();

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
    }

    #[tokio::test]
    async fn test_execute_superstep_node_not_found() {
        let state = TestState;
        let nodes = indexmap::IndexMap::new();
        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "nonexistent".to_string(),
        )];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
    }

    // --- match_resume_to_interrupts tests ---

    #[test]
    fn test_match_resume_none_returns_empty() {
        let scratchpad = Scratchpad::new();
        let result = match_resume_to_interrupts(&None, &[], &scratchpad);
        assert!(result.is_empty());
    }

    #[test]
    fn test_match_single_value_no_pending_interrupts() {
        let scratchpad = Scratchpad::new();
        let resume = Some(ResumeValue::Single(serde_json::json!("yes")));
        let result = match_resume_to_interrupts(&resume, &[], &scratchpad);
        assert_eq!(result, vec![Some(serde_json::json!("yes"))]);
    }

    #[test]
    fn test_match_single_value_with_pending_interrupts() {
        let scratchpad = Scratchpad::new();
        let resume = Some(ResumeValue::Single(serde_json::json!("approve")));
        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::json!("approve")),
                Some(serde_json::json!("approve")),
            ]
        );
    }

    #[test]
    fn test_match_single_value_with_scratchpad_null_resume() {
        // When an interrupt is already processed in the scratchpad, it should
        // receive Null even under Single (global) matching. This enables the
        // "click to continue" pattern where the user acknowledges all remaining
        // interrupts without re-triggering already-handled ones.
        let mut scratchpad = Scratchpad::new();
        scratchpad.mark_interrupt_processed("id-0");

        let resume = Some(ResumeValue::Single(serde_json::json!("approve")));
        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::Value::Null),      // processed -> null-resume
                Some(serde_json::json!("approve")), // unprocessed -> single value
            ]
        );
    }

    #[test]
    fn test_match_single_null_value_with_scratchpad() {
        // Single(Value::Null) with processed interrupts: all get Null,
        // but the scratchpad-processed ones are still recognized as null-resume.
        let mut scratchpad = Scratchpad::new();
        scratchpad.mark_interrupt_processed("id-1");

        let resume = Some(ResumeValue::Single(serde_json::Value::Null));
        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![Some(serde_json::Value::Null), Some(serde_json::Value::Null),]
        );
    }

    #[test]
    fn test_match_by_id_with_matching_ids() {
        let scratchpad = Scratchpad::new();
        let mut map = std::collections::HashMap::new();
        map.insert("id-0".to_string(), serde_json::json!("value-0"));
        map.insert("id-1".to_string(), serde_json::json!("value-1"));
        let resume = Some(ResumeValue::ById(map));

        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::json!("value-0")),
                Some(serde_json::json!("value-1")),
            ]
        );
    }

    #[test]
    fn test_match_by_id_with_scratchpad_null_resume() {
        let mut scratchpad = Scratchpad::new();
        scratchpad.mark_interrupt_processed("id-0");

        let mut map = std::collections::HashMap::new();
        map.insert("id-1".to_string(), serde_json::json!("value-1"));
        let resume = Some(ResumeValue::ById(map));

        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 2,
                id: Some("id-2".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::Value::Null),      // processed -> null-resume
                Some(serde_json::json!("value-1")), // explicit match
                None,                               // no match, not processed
            ]
        );
    }

    #[test]
    fn test_match_by_id_no_match_returns_none() {
        let scratchpad = Scratchpad::new();
        let mut map = std::collections::HashMap::new();
        map.insert("other-id".to_string(), serde_json::json!("value"));
        let resume = Some(ResumeValue::ById(map));

        let interrupts = vec![InterruptSignal {
            index: 0,
            id: Some("id-0".to_string()),
            payload: serde_json::Value::Null,
        }];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(result, vec![None]);
    }

    #[test]
    fn test_match_by_namespace_index_mapping() {
        let scratchpad = Scratchpad::new();
        let mut map = std::collections::HashMap::new();
        map.insert("0".to_string(), serde_json::json!("first"));
        map.insert("2".to_string(), serde_json::json!("third"));
        let resume = Some(ResumeValue::ByNamespace(map));

        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 2,
                id: Some("id-2".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::json!("first")),
                None,
                Some(serde_json::json!("third")),
            ]
        );
    }

    #[test]
    fn test_match_by_namespace_with_scratchpad_fill() {
        let mut scratchpad = Scratchpad::new();
        scratchpad.mark_interrupt_processed("id-1");

        let mut map = std::collections::HashMap::new();
        map.insert("0".to_string(), serde_json::json!("first"));
        let resume = Some(ResumeValue::ByNamespace(map));

        let interrupts = vec![
            InterruptSignal {
                index: 0,
                id: Some("id-0".to_string()),
                payload: serde_json::Value::Null,
            },
            InterruptSignal {
                index: 1,
                id: Some("id-1".to_string()),
                payload: serde_json::Value::Null,
            },
        ];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::json!("first")),
                Some(serde_json::Value::Null), // processed -> null-resume
            ]
        );
    }

    #[test]
    fn test_match_by_namespace_no_pending_interrupts() {
        let scratchpad = Scratchpad::new();
        let mut map = std::collections::HashMap::new();
        map.insert("0".to_string(), serde_json::json!("first"));
        map.insert("2".to_string(), serde_json::json!("third"));
        let resume = Some(ResumeValue::ByNamespace(map));

        let result = match_resume_to_interrupts(&resume, &[], &scratchpad);
        assert_eq!(
            result,
            vec![
                Some(serde_json::json!("first")),
                None,
                Some(serde_json::json!("third")),
            ]
        );
    }

    #[test]
    fn test_match_by_id_signal_without_id() {
        let scratchpad = Scratchpad::new();
        let mut map = std::collections::HashMap::new();
        map.insert("id-0".to_string(), serde_json::json!("value"));
        let resume = Some(ResumeValue::ById(map));

        let interrupts = vec![InterruptSignal {
            index: 0,
            id: None,
            payload: serde_json::Value::Null,
        }];
        let result = match_resume_to_interrupts(&resume, &interrupts, &scratchpad);
        assert_eq!(result, vec![None]);
    }

    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;

        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct TestUpdate;

    // --- serialize_pending_writes tests ---

    #[test]
    fn test_serialize_pending_writes_unit_update() {
        let update = TestUpdate;
        let writes = serialize_pending_writes("task-1", &update);
        // Unit struct serializes to null, not an object
        assert!(writes.is_empty());
    }

    #[test]
    fn test_serialize_pending_writes_with_fields() {
        #[derive(serde::Serialize)]
        struct SampleUpdate {
            messages: Option<Vec<String>>,
            count: Option<u64>,
            untouched: Option<String>,
        }

        let update = SampleUpdate {
            messages: Some(vec!["hello".to_string()]),
            count: Some(42),
            untouched: None,
        };

        let writes = serialize_pending_writes("task-99", &update);
        assert_eq!(writes.len(), 2);

        let channels: std::collections::HashSet<&str> =
            writes.iter().map(|w| w.channel.as_str()).collect();
        assert!(channels.contains("messages"));
        assert!(channels.contains("count"));
        assert!(!channels.contains("untouched"));

        for w in &writes {
            assert_eq!(w.task_id, "task-99");
        }

        let msg_write = writes
            .iter()
            .find(|w| w.channel == "messages")
            .expect("messages write");
        assert_eq!(msg_write.value, serde_json::json!(["hello"]));

        let count_write = writes
            .iter()
            .find(|w| w.channel == "count")
            .expect("count write");
        assert_eq!(count_write.value, serde_json::json!(42));
    }

    #[test]
    fn test_serialize_pending_writes_all_none() {
        #[derive(serde::Serialize)]
        struct EmptyUpdate {
            a: Option<String>,
            b: Option<u64>,
        }

        let update = EmptyUpdate { a: None, b: None };
        let writes = serialize_pending_writes("task-x", &update);
        assert!(writes.is_empty());
    }

    // --- Retry integration tests in execute_superstep ---

    #[tokio::test]
    async fn test_execute_superstep_with_retry_succeeds_after_failure() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let state = TestState;
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "flaky_node".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    let n = counter.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        Err(crate::JunctureError::execution("transient failure"))
                    } else {
                        Ok(crate::Command::end())
                    }
                }
            })
            .into_node("flaky_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let retry_policies = {
            let mut map = HashMap::new();
            map.insert(
                "flaky_node".to_string(),
                RetryPolicy {
                    max_attempts: 3,
                    initial_interval: std::time::Duration::from_millis(1),
                    backoff_factor: 2.0,
                    max_interval: std::time::Duration::from_secs(1),
                    jitter: false,
                    retry_on: None,
                },
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "flaky_node".to_string(),
        )];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &retry_policies,
            &HashMap::new(),
            0,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.task_outputs[0].error.is_none());
        assert_eq!(
            attempt_count.load(Ordering::Relaxed),
            2,
            "should succeed on second attempt"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_with_retry_exhausts_attempts() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let state = TestState;
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "always_fail".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Err(crate::JunctureError::execution("persistent failure"))
                }
            })
            .into_node("always_fail"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let retry_policies = {
            let mut map = HashMap::new();
            map.insert(
                "always_fail".to_string(),
                RetryPolicy {
                    max_attempts: 3,
                    initial_interval: std::time::Duration::from_millis(1),
                    backoff_factor: 2.0,
                    max_interval: std::time::Duration::from_secs(1),
                    jitter: false,
                    retry_on: None,
                },
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "always_fail".to_string(),
        )];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &retry_policies,
            &HashMap::new(),
            0,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().is_execution());
        assert_eq!(
            attempt_count.load(Ordering::Relaxed),
            3,
            "should attempt exactly max_attempts times"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_retry_does_not_retry_cancelled() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let state = TestState;
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "cancel_node".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Err(crate::JunctureError::cancelled())
                }
            })
            .into_node("cancel_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let retry_policies = {
            let mut map = HashMap::new();
            map.insert(
                "cancel_node".to_string(),
                RetryPolicy {
                    max_attempts: 3,
                    initial_interval: std::time::Duration::from_millis(1),
                    backoff_factor: 2.0,
                    max_interval: std::time::Duration::from_secs(1),
                    jitter: false,
                    retry_on: None,
                },
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "cancel_node".to_string(),
        )];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &retry_policies,
            &HashMap::new(),
            0,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result.unwrap_err().is_cancelled(),
            "cancelled errors should not be retried"
        );
        assert_eq!(
            attempt_count.load(Ordering::Relaxed),
            1,
            "cancelled errors should not be retried"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_retry_only_applies_to_configured_node() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let state = TestState;
        let attempt_count_a = Arc::new(AtomicU32::new(0));
        let attempt_count_b = Arc::new(AtomicU32::new(0));
        let clone_a = Arc::clone(&attempt_count_a);
        let clone_b = Arc::clone(&attempt_count_b);

        let mut nodes = indexmap::IndexMap::new();
        // node_a has a retry policy
        nodes.insert(
            "node_a".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&clone_a);
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Err(crate::JunctureError::execution("node_a fails"))
                }
            })
            .into_node("node_a"),
        );
        // node_b has NO retry policy
        nodes.insert(
            "node_b".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&clone_b);
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Err(crate::JunctureError::execution("node_b fails"))
                }
            })
            .into_node("node_b"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        // Only node_a has a retry policy
        let retry_policies = {
            let mut map = HashMap::new();
            map.insert(
                "node_a".to_string(),
                RetryPolicy {
                    max_attempts: 3,
                    initial_interval: std::time::Duration::from_millis(1),
                    backoff_factor: 2.0,
                    max_interval: std::time::Duration::from_secs(1),
                    jitter: false,
                    retry_on: None,
                },
            );
            map
        };

        // node_b has an error handler so the superstep doesn't abort
        let error_handlers = {
            let mut map = HashMap::new();
            map.insert("node_b".to_string(), "handler".to_string());
            map
        };

        let tasks = vec![
            PendingTask::pull(uuid::Uuid::new_v4().to_string(), "node_a".to_string()),
            PendingTask::pull(uuid::Uuid::new_v4().to_string(), "node_b".to_string()),
        ];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &error_handlers,
            &retry_policies,
            &HashMap::new(),
            0,
        )
        .await;

        // node_a exhausted retries (3 attempts), node_b failed once with error handler
        // Since node_a has no error handler and exhausted retries, the superstep fails
        let err = result.unwrap_err();
        assert!(err.is_execution(), "expected execution error, got: {err}");
        assert_eq!(
            attempt_count_a.load(Ordering::Relaxed),
            3,
            "node_a should retry max_attempts times"
        );
        assert_eq!(
            attempt_count_b.load(Ordering::Relaxed),
            1,
            "node_b should execute only once (no retry policy)"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_no_retry_policy_same_behavior() {
        // Verify that without retry policies, behavior is identical to before
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "simple_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(crate::Command::end()) }).into_node("simple_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "simple_node".to_string(),
        )];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.task_outputs[0].error.is_none());
    }

    // --- Timeout integration tests in execute_superstep ---

    #[tokio::test]
    async fn test_execute_superstep_with_timeout_succeeds_within_limit() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "fast_node".to_string(),
            NodeFnCommand(|_s| async move {
                // Completes quickly
                Ok(crate::Command::end())
            })
            .into_node("fast_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let timeout_policies = {
            let mut map = HashMap::new();
            map.insert(
                "fast_node".to_string(),
                crate::pregel::context::TimeoutPolicy::new()
                    .with_run_timeout(std::time::Duration::from_secs(10)),
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "fast_node".to_string(),
        )];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &timeout_policies,
            0,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.task_outputs[0].error.is_none());
    }

    #[tokio::test]
    async fn test_execute_superstep_with_timeout_exceeds_limit() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "slow_node".to_string(),
            NodeFnCommand(|_s| async move {
                // Sleep longer than the timeout
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                Ok(crate::Command::end())
            })
            .into_node("slow_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let timeout_policies = {
            let mut map = HashMap::new();
            map.insert(
                "slow_node".to_string(),
                crate::pregel::context::TimeoutPolicy::new()
                    .with_run_timeout(std::time::Duration::from_millis(50)),
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "slow_node".to_string(),
        )];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &timeout_policies,
            0,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.is_node_timeout(),
            "expected node timeout error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_timeout_wraps_retry_entire_sequence() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let state = TestState;
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "slow_retry_node".to_string(),
            NodeFnCommand(move |_s: TestState| {
                let counter = Arc::clone(&attempt_clone);
                async move {
                    let _n = counter.fetch_add(1, Ordering::Relaxed);
                    // Each attempt sleeps long enough that cumulative retries
                    // will exceed the timeout
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    Err(crate::JunctureError::execution("transient failure"))
                }
            })
            .into_node("slow_retry_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let retry_policies = {
            let mut map = HashMap::new();
            map.insert(
                "slow_retry_node".to_string(),
                RetryPolicy {
                    max_attempts: 10,
                    initial_interval: std::time::Duration::from_millis(1),
                    backoff_factor: 1.0,
                    max_interval: std::time::Duration::from_millis(1),
                    jitter: false,
                    retry_on: None,
                },
            );
            map
        };

        let timeout_policies = {
            let mut map = HashMap::new();
            map.insert(
                "slow_retry_node".to_string(),
                crate::pregel::context::TimeoutPolicy::new()
                    .with_run_timeout(std::time::Duration::from_millis(200)),
            );
            map
        };

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "slow_retry_node".to_string(),
        )];

        let result = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &retry_policies,
            &timeout_policies,
            0,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.is_node_timeout(),
            "timeout should fire before retries exhaust, got: {err}"
        );
        // Verify that retries were attempted (at least one attempt completed)
        let attempts = attempt_count.load(Ordering::Relaxed);
        assert!(
            attempts >= 1,
            "should have attempted at least once before timeout, got {attempts}"
        );
        // Verify retries did NOT exhaust (timeout should interrupt)
        assert!(
            attempts < 10,
            "timeout should have prevented all 10 retry attempts, got {attempts}"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_timeout_only_applies_to_configured_node() {
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        // fast_node has no timeout -- should succeed even though slow_node times out
        nodes.insert(
            "fast_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(crate::Command::end()) }).into_node("fast_node"),
        );
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
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        // Only slow_node has a timeout; fast_node runs without timeout
        let timeout_policies = {
            let mut map = HashMap::new();
            map.insert(
                "slow_node".to_string(),
                crate::pregel::context::TimeoutPolicy::new()
                    .with_run_timeout(std::time::Duration::from_millis(50)),
            );
            map
        };

        // Give slow_node an error handler so the superstep doesn't abort
        let error_handlers = {
            let mut map = HashMap::new();
            map.insert("slow_node".to_string(), "handler".to_string());
            map
        };

        let tasks = vec![
            PendingTask::pull(uuid::Uuid::new_v4().to_string(), "fast_node".to_string()),
            PendingTask::pull(uuid::Uuid::new_v4().to_string(), "slow_node".to_string()),
        ];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &error_handlers,
            &HashMap::new(),
            &timeout_policies,
            0,
        )
        .await
        .unwrap();

        // fast_node should succeed, slow_node should have an error recorded
        assert_eq!(result.len(), 2);
        let fast_output = result
            .task_outputs
            .iter()
            .find(|o| o.node_name == "fast_node")
            .expect("fast_node output should exist");
        assert!(fast_output.error.is_none());

        let slow_output = result
            .task_outputs
            .iter()
            .find(|o| o.node_name == "slow_node")
            .expect("slow_node output should exist");
        assert!(
            slow_output.error.is_some(),
            "slow_node should have timed out with error handler"
        );
    }

    #[tokio::test]
    async fn test_execute_superstep_no_timeout_policy_same_behavior() {
        // Verify that without timeout policies, behavior is identical to before
        let state = TestState;

        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            "simple_node".to_string(),
            NodeFnCommand(|_s| async move { Ok(crate::Command::end()) }).into_node("simple_node"),
        );

        let config = RunnableConfig::new();
        let token = CancellationToken::new();
        let pending_interrupts = vec![];
        let scratchpad = Scratchpad::new();

        let tasks = vec![PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            "simple_node".to_string(),
        )];

        let (result, _rx) = execute_superstep(
            &tasks,
            &state,
            &nodes,
            &config,
            &token,
            None,
            &pending_interrupts,
            &scratchpad,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            0,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.task_outputs[0].error.is_none());
    }
}

// Rust guideline compliant 2026-05-22
