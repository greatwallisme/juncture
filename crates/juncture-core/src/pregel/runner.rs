//! Parallel execution for Pregel engine
//!
//! This module provides parallel task execution using tokio for concurrent
//! node execution with bounded concurrency and cancellation support.

use crate::{
    JunctureError, Node, State,
    config::RunnableConfig,
    interrupt::{InterruptContext, InterruptSignal, ResumeValue, Scratchpad},
    pregel::types::{PendingTask, SuperstepResult, TaskOutput},
};
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
/// let (result, _interrupt_rx) = execute_superstep(
///     &pending_tasks,
///     &state,
///     &nodes,
///     &config,
///     &token,
///     None::<Arc<dyn CheckpointSaver>>,
///     &pending_interrupts,
///     &scratchpad,
/// ).await?;
/// ```
#[expect(
    clippy::too_many_lines,
    reason = "execute_superstep requires: early return, semaphore creation, interrupt context setup, task spawning with span creation, and result collection. The length is justified by the complexity of parallel execution with proper error handling and observability."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "execute_superstep requires: tasks, state, nodes, config, cancellation token, checkpointer, pending interrupts, and scratchpad. All are necessary for the multi-interrupt matching algorithm."
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

        let task_config = config.clone();
        let task_id = task.id.clone();
        let node_name = task.node_name.clone();
        let task_trigger = task.trigger.clone();
        let permit = Arc::clone(&semaphore);
        let token = cancellation_token.clone();
        let ctx = Arc::clone(&interrupt_context);

        // Create span before moving values into the async block
        let span = tracing::info_span!(
            "juncture.node.execute",
            node_name = %node_name,
            task_id = %task_id,
            "juncture.node.output_type" = tracing::field::Empty,
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

                let start = Instant::now();

                // Execute task with cancellation support and interrupt context
                // The INTERRUPT_CONTEXT.scope() makes the context available to
                // the interrupt!() macro within node execution
                let result = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        return Err(JunctureError::execution("Task cancelled"));
                    }
                    result = crate::interrupt::INTERRUPT_CONTEXT.scope(ctx, async move {
                        node.call(task_state, &task_config).await
                    }) => result,
                };

                let duration = start.elapsed();

                // Determine and record output type
                // Priority: interrupt > send > end > goto > update
                let output_type = result.as_ref().map_or("error", |command| {
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
                });

                // Record output type in span
                tracing::Span::current().record("juncture.node.output_type", output_type);

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

                result.map(|command| TaskOutput {
                    task_id,
                    node_name,
                    command,
                    duration,
                    trigger: task_trigger,
                })
            }
            .instrument(span),
        );
    }

    // Collect results as they complete
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
/// 1. **Single (global matching)**: A single value is applied to all pending interrupts.
///    When no pending interrupts exist, returns a single-element vector for the
///    first interrupt position.
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
            // Global matching: single value for all pending interrupts
            if pending_interrupts.is_empty() {
                vec![Some(value.clone())]
            } else {
                vec![Some(value.clone()); pending_interrupts.len()]
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
        type FieldVersions = ();

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
}

// Rust guideline compliant 2026-05-21
