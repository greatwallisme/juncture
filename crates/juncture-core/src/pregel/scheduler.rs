//! Version tracking and task computation for Pregel engine
//!
//! This module provides field version tracking, task scheduling logic,
//! state write application, and trigger-to-node mapping for the Pregel
//! execution engine.

use crate::{
    JunctureError, State,
    edge::{CompiledEdge, TriggerSource, TriggerTable},
    pregel::types::{PendingTask, SuperstepResult, TaskOutput},
    state::FieldsChanged,
};
use indexmap::IndexMap;
use std::{collections::HashMap, collections::HashSet};

/// Field version tracker for Pregel execution
///
/// Tracks version numbers for each field in the state to determine
/// when nodes should be activated based on their trigger fields.
#[derive(Clone, Debug)]
pub struct FieldVersionTracker {
    /// Version number for each field (index = field position)
    versions: Vec<u64>,

    /// Global maximum version across all fields
    global_max: u64,
}

impl FieldVersionTracker {
    /// Create a new version tracker for the given number of fields
    ///
    /// # Panics
    ///
    /// Panics if `num_fields` is greater than 64 (the maximum number of
    /// fields that can be tracked in a `FieldsChanged` bitmask).
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let tracker = FieldVersionTracker::new(5);
    /// assert_eq!(tracker.versions().len(), 5);
    /// ```
    #[must_use]
    pub fn new(num_fields: usize) -> Self {
        assert!(
            num_fields <= 64,
            "Cannot track more than 64 fields (got {num_fields})"
        );

        Self {
            versions: vec![0; num_fields],
            global_max: 0,
        }
    }

    /// Bump all field versions (used when state changes globally)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    /// use juncture_core::state::FieldsChanged;
    ///
    /// let mut tracker = FieldVersionTracker::new(3);
    /// let changed = FieldsChanged(0b101); // fields 0 and 2 changed
    /// tracker.bump_all(&changed);
    /// assert_eq!(tracker.get(0), 1);
    /// assert_eq!(tracker.get(1), 0);
    /// assert_eq!(tracker.get(2), 1);
    /// ```
    pub fn bump_all(&mut self, changed: &FieldsChanged) {
        for field_idx in 0..self.versions.len() {
            if changed.has_field(field_idx) {
                self.bump(field_idx);
            }
        }
    }

    /// Bump version for a specific field
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let mut tracker = FieldVersionTracker::new(3);
    /// tracker.bump(1);
    /// assert_eq!(tracker.get(1), 1);
    /// assert_eq!(tracker.get(0), 0);
    /// ```
    pub fn bump(&mut self, field_idx: usize) {
        self.global_max = self.global_max.saturating_add(1);
        self.versions[field_idx] = self.global_max;
    }

    /// Get the current version of a field
    ///
    /// # Panics
    ///
    /// Panics if `field_idx` is out of bounds.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let mut tracker = FieldVersionTracker::new(3);
    /// tracker.bump(0);
    /// assert_eq!(tracker.get(0), 1);
    /// ```
    #[must_use]
    pub fn get(&self, field_idx: usize) -> u64 {
        self.versions[field_idx]
    }

    /// Get all field versions as a slice
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let tracker = FieldVersionTracker::new(3);
    /// let versions = tracker.versions();
    /// assert_eq!(versions, &[0, 0, 0]);
    /// ```
    #[must_use]
    pub fn versions(&self) -> &[u64] {
        &self.versions
    }

    /// Get the number of fields being tracked
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let tracker = FieldVersionTracker::new(5);
    /// assert_eq!(tracker.len(), 5);
    /// ```
    #[must_use]
    pub const fn len(&self) -> usize {
        self.versions.len()
    }

    /// Check if no fields are being tracked
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// Get all field versions as a slice (alias for `versions()`)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let tracker = FieldVersionTracker::new(3);
    /// assert_eq!(tracker.as_slice(), &[0, 0, 0]);
    /// ```
    #[must_use]
    pub fn as_slice(&self) -> &[u64] {
        self.versions()
    }

    /// Get the global maximum version
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::FieldVersionTracker;
    ///
    /// let mut tracker = FieldVersionTracker::new(3);
    /// tracker.bump(0);
    /// tracker.bump(1);
    /// assert_eq!(tracker.global_max(), 2);
    /// ```
    #[must_use]
    pub const fn global_max(&self) -> u64 {
        self.global_max
    }
}

/// Version tracking for node activation
///
/// Tracks which versions each node has seen to determine when it should
/// be activated based on its trigger fields.
#[derive(Clone, Debug)]
pub struct VersionsSeen {
    /// Map of node name to the field versions it has seen
    ///
    /// Uses `IndexMap` for deterministic iteration order, matching `LangGraph` semantics.
    seen: IndexMap<String, Vec<u64>>,
}

impl VersionsSeen {
    /// Create a new version tracker for the given nodes and fields
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::VersionsSeen;
    ///
    /// let node_names = vec!["node_a".to_string(), "node_b".to_string()];
    /// let seen = VersionsSeen::new(&node_names, 3);
    /// assert_eq!(seen.get_seen("node_a"), &[0, 0, 0]);
    /// ```
    #[must_use]
    pub fn new(node_names: &[String], num_fields: usize) -> Self {
        let seen = node_names
            .iter()
            .map(|name| (name.clone(), vec![0; num_fields]))
            .collect();

        Self { seen }
    }

    /// Check if a node should be activated based on its trigger fields
    ///
    /// Returns `true` if any of the node's trigger fields have new versions
    /// that the node hasn't seen yet.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::VersionsSeen;
    ///
    /// let node_names = vec!["node_a".to_string()];
    /// let mut seen = VersionsSeen::new(&node_names, 3);
    ///
    /// // Node should activate if field 0 has version > what it has seen
    /// let trigger_fields = vec![0]; // triggers on field 0
    /// let current = vec![1, 0, 0]; // field 0 is at version 1
    /// assert!(seen.should_activate("node_a", &trigger_fields, &current));
    /// ```
    #[must_use]
    pub fn should_activate(
        &self,
        node_name: &str,
        trigger_fields: &[usize],
        current: &[u64],
    ) -> bool {
        let Some(seen_versions) = self.seen.get(node_name) else {
            return true; // Node not yet tracked, should activate
        };

        for &field_idx in trigger_fields {
            if current[field_idx] > seen_versions[field_idx] {
                return true;
            }
        }

        false
    }

    /// Mark that a node has consumed the current field versions
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::VersionsSeen;
    ///
    /// let node_names = vec!["node_a".to_string()];
    /// let mut seen = VersionsSeen::new(&node_names, 3);
    ///
    /// let current = vec![1, 0, 0];
    /// seen.mark_consumed("node_a", &current);
    ///
    /// // Now node shouldn't activate for same versions
    /// assert!(!seen.should_activate("node_a", &[0], &current));
    /// ```
    pub fn mark_consumed(&mut self, node_name: &str, current: &[u64]) {
        if let Some(seen_versions) = self.seen.get_mut(node_name) {
            seen_versions.copy_from_slice(current);
        }
    }

    /// Get the versions a node has seen
    ///
    /// Returns an empty slice if the node is not tracked.
    #[must_use]
    pub fn get_seen(&self, node_name: &str) -> &[u64] {
        self.seen.get(node_name).map_or(&[], Vec::as_slice)
    }

    /// Get the versions a node has seen (alias for `get_seen`)
    ///
    /// Returns an empty slice if the node is not tracked.
    #[must_use]
    pub fn get_versions(&self, node_name: &str) -> &[u64] {
        self.get_seen(node_name)
    }

    /// Compute which fields triggered a node to activate
    ///
    /// Compares the node's seen versions with current field versions to determine
    /// which specific fields had updates that caused the node to be scheduled.
    ///
    /// # Arguments
    ///
    /// * `node_name` - Name of the node to check
    /// * `trigger_fields` - Field indices that the node subscribes to
    /// * `current_versions` - Current field versions
    ///
    /// # Returns
    ///
    /// Vector of field indices that triggered this node (subset of `trigger_fields`)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use juncture_core::pregel::scheduler::VersionsSeen;
    ///
    /// let node_names = vec!["node_a".to_string()];
    /// let mut seen = VersionsSeen::new(&node_names, 3);
    ///
    /// let trigger_fields = vec![0, 2]; // node subscribes to fields 0 and 2
    /// let current = vec![1, 0, 1]; // fields 0 and 2 have new versions
    /// let triggered = seen.compute_triggered_fields("node_a", &trigger_fields, &current);
    /// assert_eq!(triggered, vec![0, 2]); // both fields triggered
    /// ```
    #[must_use]
    pub fn compute_triggered_fields(
        &self,
        node_name: &str,
        trigger_fields: &[usize],
        current_versions: &[u64],
    ) -> Vec<usize> {
        let Some(seen_versions) = self.seen.get(node_name) else {
            // Node not yet tracked, all trigger fields are new
            return trigger_fields.to_vec();
        };

        trigger_fields
            .iter()
            .filter(|&&field_idx| current_versions[field_idx] > seen_versions[field_idx])
            .copied()
            .collect()
    }
}

/// Compute the next set of tasks to execute
///
/// This function determines which nodes should be activated in the next
/// superstep based on:
/// 1. Commands returned by completed tasks (highest priority)
/// 2. Trigger table edges (Fixed and Conditional)
///
/// # Arguments
///
/// * `completed_tasks` - Tasks that completed in the previous superstep
/// * `trigger_table` - Graph's trigger table
/// * `state` - Current state
///
/// # Returns
///
/// A vector of pending tasks to execute in the next superstep.
///
/// # Errors
///
/// Returns an error if:
/// - A conditional edge router fails to execute
/// - A conditional edge returns no target
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::compute_next_tasks;
/// use juncture_core::pregel::types::{TaskOutput, SuperstepResult};
/// use std::time::Duration;
///
/// # let completed_tasks = vec![];
/// # let trigger_table = TriggerTable::<MyState>::new();
/// # let state = MyState;
/// let next_tasks = compute_next_tasks(&completed_tasks, &trigger_table, &state)?;
/// ```
pub async fn compute_next_tasks<S: State>(
    completed_tasks: &[TaskOutput<S>],
    trigger_table: &TriggerTable<S>,
    trigger_to_nodes: &TriggerToNodes,
    state: &S,
) -> Result<Vec<PendingTask<S>>, JunctureError> {
    let mut next_tasks = Vec::new();
    let mut seen_nodes = HashSet::new();

    // First, check if any task returned a Command with explicit routing
    for task_output in completed_tasks {
        let command = &task_output.command;

        match &command.goto {
            crate::Goto::None => {
                // No explicit routing, use trigger table with reverse mapping optimization
                // Use TriggerToNodes to efficiently find which nodes should be triggered
                let triggered =
                    trigger_to_nodes.triggered_nodes(std::slice::from_ref(&task_output.node_name));

                // Filter outgoing edges to only those leading to triggered nodes
                if let Some(edges) = trigger_table.outgoing.get(&task_output.node_name) {
                    for edge in edges {
                        // Only process edges that lead to triggered nodes
                        if should_process_edge(edge, state, &triggered).await? {
                            process_edge(
                                edge,
                                state,
                                &mut next_tasks,
                                &mut seen_nodes,
                                &task_output.node_name,
                            )
                            .await?;
                        }
                    }
                }
            }
            crate::Goto::Next(target) => {
                // Route to single target
                if !seen_nodes.contains(target) {
                    seen_nodes.insert(target.clone());
                    next_tasks.push(PendingTask::pull(
                        uuid::Uuid::new_v4().to_string(),
                        target.clone(),
                    ));
                }
            }
            crate::Goto::Multiple(targets) => {
                // Route to multiple targets
                for target in targets {
                    if !seen_nodes.contains(target) {
                        seen_nodes.insert(target.clone());
                        next_tasks.push(PendingTask::pull(
                            uuid::Uuid::new_v4().to_string(),
                            target.clone(),
                        ));
                    }
                }
            }
            crate::Goto::Send(send_targets) => {
                // Dynamic fan-out with state overrides.
                // Each Send target creates a separate task even if multiple targets
                // share the same node name, because each carries a distinct state override.
                for (idx, target) in send_targets.iter().enumerate() {
                    next_tasks.push(PendingTask::push(
                        uuid::Uuid::new_v4().to_string(),
                        target.node.clone(),
                        idx,
                        target.state.clone(),
                    ));
                }
            }
            crate::Goto::End => {
                // Termination, no next tasks
            }
        }
    }

    Ok(next_tasks)
}

/// Check if an edge should be processed based on triggered nodes
///
/// For fixed edges, checks if the target is in the triggered set.
/// For conditional edges, the router is executed to determine the actual target.
async fn should_process_edge<S: State>(
    edge: &CompiledEdge<S>,
    state: &S,
    triggered_nodes: &HashSet<String>,
) -> Result<bool, JunctureError> {
    match edge {
        CompiledEdge::Fixed { target } => Ok(triggered_nodes.contains(target)),
        CompiledEdge::Conditional { router, .. } => {
            let route_result = router.route(state).await?;
            Ok(route_result
                .as_target()
                .is_some_and(|t| triggered_nodes.contains(t)))
        }
    }
}

/// Process a single edge and add appropriate tasks
async fn process_edge<S: State>(
    edge: &CompiledEdge<S>,
    state: &S,
    next_tasks: &mut Vec<PendingTask<S>>,
    seen_nodes: &mut HashSet<String>,
    from_node: &str,
) -> Result<(), JunctureError> {
    match edge {
        CompiledEdge::Fixed { target } => {
            if target != crate::edge::END && !seen_nodes.contains(target) {
                seen_nodes.insert(target.clone());
                next_tasks.push(PendingTask::pull(
                    uuid::Uuid::new_v4().to_string(),
                    target.clone(),
                ));
            }
        }
        CompiledEdge::Conditional { router, .. } => {
            let route_result = router.route(state).await?;
            let target = route_result.as_target().ok_or_else(|| {
                JunctureError::execution(format!(
                    "Conditional edge from '{from_node}' returned no target: {route_result:?}"
                ))
            })?;

            if target != crate::edge::END && !seen_nodes.contains(target) {
                seen_nodes.insert(target.to_string());
                next_tasks.push(PendingTask::pull(
                    uuid::Uuid::new_v4().to_string(),
                    target.to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Apply writes from completed tasks to the state
///
/// Takes outputs from a superstep and applies all updates to the state.
/// Uses path-based sorting (PULL tasks sorted by node name, PUSH tasks
/// sorted by send index) for deterministic merge order, matching the
/// `LangGraph` merge semantics.
///
/// Returns [`FieldsChanged`] indicating which fields were modified.
///
/// # Arguments
///
/// * `state` - Mutable state to apply updates to
/// * `task_outputs` - Outputs from completed tasks in the superstep
/// * `field_versions` - Version tracker to bump for changed fields
///
/// # Errors
///
/// Returns `JunctureError` if a reducer constraint is violated, such as
/// multiple nodes writing to a replace channel in the same superstep.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::{apply_writes, FieldVersionTracker};
///
/// let mut state = MyState::default();
/// let mut tracker = FieldVersionTracker::new(3);
/// let changed = apply_writes(&mut state, &task_outputs, &mut tracker)?;
/// ```
pub fn apply_writes<S: State>(
    state: &mut S,
    task_outputs: &[crate::pregel::types::TaskOutput<S>],
    field_versions: &mut FieldVersionTracker,
) -> Result<FieldsChanged, JunctureError> {
    // Check for multiple-writer conflicts on replace fields before applying any writes.
    // This must happen first so that we reject the entire superstep rather than
    // silently applying partial writes with last-write-wins semantics.
    check_replace_conflicts_from_state::<S>(task_outputs)?;

    let mut total_changed = FieldsChanged(0);

    // Sort indices by path-based ordering for deterministic merge
    // PULL tasks: alphabetical by node name
    // PUSH tasks: by send index
    let mut sorted_indices: Vec<usize> = (0..task_outputs.len()).collect();
    sorted_indices.sort_by(|&a, &b| {
        let task_a = &task_outputs[a];
        let task_b = &task_outputs[b];
        match (&task_a.trigger, &task_b.trigger) {
            (crate::pregel::types::TaskTrigger::Pull, crate::pregel::types::TaskTrigger::Pull) => {
                task_a.node_name.cmp(&task_b.node_name)
            }
            (
                crate::pregel::types::TaskTrigger::Push { index: idx_a },
                crate::pregel::types::TaskTrigger::Push { index: idx_b },
            ) => idx_a.cmp(idx_b),
            (
                crate::pregel::types::TaskTrigger::Pull,
                crate::pregel::types::TaskTrigger::Push { .. },
            ) => std::cmp::Ordering::Less,
            (
                crate::pregel::types::TaskTrigger::Push { .. },
                crate::pregel::types::TaskTrigger::Pull,
            ) => std::cmp::Ordering::Greater,
        }
    });

    for idx in sorted_indices {
        let output = &task_outputs[idx];
        if let Some(ref update) = output.command.update {
            let changed = state
                .try_apply(update.clone())
                .map_err(|e| JunctureError::invalid_update(e.to_string()))?;
            total_changed.merge(&changed);
        }
    }

    // Bump field versions for all changed fields
    field_versions.bump_all(&total_changed);

    Ok(total_changed)
}

/// Channel-to-node reverse mapping for efficient scheduling
///
/// When a channel (field) is updated, only the subscribed nodes need
/// to be checked, reducing scheduling from `O(nodes)` to `O(triggered_nodes)`.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::TriggerToNodes;
///
/// let trigger_to_nodes = TriggerToNodes::from_trigger_table(&trigger_table);
/// let triggered = trigger_to_nodes.triggered_nodes(&["field_a".to_string()]);
/// assert!(triggered.contains("node_x"));
/// ```
pub struct TriggerToNodes {
    mapping: HashMap<String, HashSet<String>>,
}

impl TriggerToNodes {
    /// Build from the compiled [`TriggerTable`]
    ///
    /// Constructs a reverse mapping from trigger source names to the
    /// set of nodes that subscribe to each source.
    #[must_use]
    pub fn from_trigger_table<S: State>(table: &TriggerTable<S>) -> Self {
        let mut mapping: HashMap<String, HashSet<String>> = HashMap::new();
        for (node_name, sources) in &table.incoming {
            for source in sources {
                match source {
                    TriggerSource::Edge { from } | TriggerSource::Send { from } => {
                        mapping
                            .entry(from.clone())
                            .or_default()
                            .insert(node_name.clone());
                    }
                }
            }
        }
        Self { mapping }
    }

    /// Given updated channel names, return the nodes that should be checked
    ///
    /// Returns the union of all node sets subscribed to any of the
    /// given channels.
    #[must_use]
    pub fn triggered_nodes(&self, updated_channels: &[String]) -> HashSet<String> {
        updated_channels
            .iter()
            .filter_map(|ch| self.mapping.get(ch))
            .flatten()
            .cloned()
            .collect()
    }
}

// Rust guideline compliant 2026-05-19

impl std::fmt::Debug for TriggerToNodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriggerToNodes")
            .field("mapping_len", &self.mapping.len())
            .finish()
    }
}

/// Check for replace conflicts in superstep results
///
/// For fields using `ReplaceReducer`, only one node is allowed to write
/// to that field in a single superstep. This function detects violations
/// of that constraint.
///
/// # Arguments
///
/// * `superstep_result` - Results from the completed superstep
/// * `replace_fields` - Field indices that use `ReplaceReducer`
///
/// # Returns
///
/// - `Ok(())` if no conflicts
/// - `Err(JunctureError::Execution)` if conflicts exist
///
/// # Errors
///
/// Returns an error if multiple nodes wrote to the same replace field.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::check_replace_conflicts;
///
/// let replace_fields = vec![0, 2]; // fields 0 and 2 use ReplaceReducer
/// check_replace_conflicts(&superstep_result, &replace_fields)?;
/// ```
pub fn check_replace_conflicts<S: State>(
    superstep_result: &SuperstepResult<S>,
    replace_fields: &[usize],
) -> Result<(), JunctureError> {
    for &field_idx in replace_fields {
        let writers: Vec<&str> = superstep_result
            .task_outputs
            .iter()
            .filter(|o| {
                o.command
                    .update
                    .as_ref()
                    .is_some_and(|u| S::field_is_set(u, field_idx))
            })
            .map(|o| o.node_name.as_str())
            .collect();

        if writers.len() > 1 {
            return Err(JunctureError::execution(format!(
                "Multiple writers for replace field {field_idx}: {writers:?}"
            )));
        }
    }
    Ok(())
}

/// Check for replace conflicts using the state's built-in field indices
///
/// Uses `S::replace_field_indices()` and `S::field_is_set()` generated by
/// the proc-macro to detect multiple-writer violations. This is the preferred
/// entry point for `apply_writes()` since it avoids the caller needing to
/// track replace field indices separately.
///
/// # Errors
///
/// Returns an error if multiple nodes wrote to the same replace field.
fn check_replace_conflicts_from_state<S: State>(
    task_outputs: &[crate::pregel::types::TaskOutput<S>],
) -> Result<(), JunctureError> {
    let replace_fields = S::replace_field_indices();
    for &field_idx in replace_fields {
        let writers: Vec<&str> = task_outputs
            .iter()
            .filter(|o| {
                o.command
                    .update
                    .as_ref()
                    .is_some_and(|u| S::field_is_set(u, field_idx))
            })
            .map(|o| o.node_name.as_str())
            .collect();

        if writers.len() > 1 {
            return Err(JunctureError::multiple_writers(
                field_idx,
                writers.into_iter().map(String::from).collect(),
            ));
        }
    }
    Ok(())
}

/// Consume triggered channels after `apply_writes`
///
/// This function implements the `consume()` step that happens after
/// `apply_writes` merges all writes but before `reset_ephemeral()`.
///
/// For `ephemeral` fields, this marks the channel's consumed flag, indicating
/// that the value has been read by the framework. The consumed flag is reset
/// on the next `update()` call. For other field types, `consume_field()` is
/// a no-op, making it safe to call on any field index.
///
/// # Arguments
///
/// * `state` - Mutable state to consume channels on
/// * `triggered_channels` - Field indices of channels that were triggered
///   (changed) in the current superstep
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::consume_triggered_channels;
///
/// let triggered_channels = vec![0, 2]; // channels 0 and 2 were triggered
/// consume_triggered_channels(&mut state, &triggered_channels);
/// ```
pub fn consume_triggered_channels<S: State>(state: &mut S, triggered_channels: &[usize]) {
    for &field_idx in triggered_channels {
        state.consume_field(field_idx);
    }
}

/// Schedule error handler tasks for failed nodes
///
/// Scans task outputs for failures (indicated by a present `error` field) and
/// creates recovery [`PendingTask`]s targeting each failed node's registered
/// error handler. The error handler map is consulted to find the handler node
/// name for each failed node.
///
/// The recovery tasks use [`TaskTrigger::Pull`] and are appended to the next
/// superstep's pending task list by the caller (`PregelLoop::after_tick`).
///
/// # Arguments
///
/// * `task_outputs` - All task outputs from the completed superstep
/// * `nodes` - All nodes in the graph (used to verify handler existence)
/// * `error_handler_map` - Maps node names to their error handler node names
///
/// # Returns
///
/// Vector of pending tasks targeting error handler nodes.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::schedule_error_handlers;
///
/// let recovery_tasks = schedule_error_handlers(&task_outputs, &nodes, &error_handler_map);
/// for task in recovery_tasks {
///     // Execute error handler task in next superstep
/// }
/// ```
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts std HashMap; callers typically construct from builder metadata"
)]
pub fn schedule_error_handlers<S: State>(
    task_outputs: &[TaskOutput<S>],
    nodes: &indexmap::IndexMap<String, std::sync::Arc<dyn crate::Node<S>>>,
    error_handler_map: &std::collections::HashMap<String, String>,
) -> Vec<PendingTask<S>> {
    let mut recovery_tasks = Vec::new();

    for output in task_outputs {
        let Some(ref error) = output.error else {
            continue;
        };

        let Some(handler_name) = error_handler_map.get(&output.node_name) else {
            continue;
        };

        // Verify the handler node actually exists in the graph
        if !nodes.contains_key(handler_name) {
            tracing::warn!(
                name: "juncture.error_handler.missing_node",
                node_name = %output.node_name,
                handler_name = %handler_name,
                error = %error,
                "Error handler node not found in graph, skipping recovery"
            );
            continue;
        }

        recovery_tasks.push(PendingTask::pull(
            uuid::Uuid::new_v4().to_string(),
            handler_name.clone(),
        ));
    }

    recovery_tasks
}

/// Get the error handler node name for a given node
///
/// Looks up the registered error handler for a node from the provided map.
/// Returns the handler node name if one is registered, `None` otherwise.
///
/// # Arguments
///
/// * `node_name` - Name of the node that failed
/// * `error_handler_map` - Maps node names to error handler node names
///
/// # Returns
///
/// `Some(error_handler_name)` if an error handler is registered, `None` otherwise
#[must_use]
#[allow(
    dead_code,
    reason = "tested via unit tests; public API awaiting external consumers"
)]
pub fn get_error_handler_node(
    node_name: &str,
    error_handler_map: &std::collections::HashMap<String, String>,
) -> Option<String> {
    error_handler_map.get(node_name).cloned()
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use crate::node::IntoNode;
    use crate::state::FieldVersions;

    #[derive(Clone, Debug, Default)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        type FieldVersions = FieldVersions;

        fn apply(&mut self, _: Self::Update) -> FieldsChanged {
            FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct TestUpdate;

    #[test]
    fn test_trigger_to_nodes_from_empty_table() {
        let table: TriggerTable<TestState> = TriggerTable::default();
        let ttn = TriggerToNodes::from_trigger_table(&table);
        assert!(ttn.triggered_nodes(&["node_a".to_string()]).is_empty());
    }

    #[test]
    fn test_trigger_to_nodes_with_sources() {
        let mut table: TriggerTable<TestState> = TriggerTable::default();
        table.add_incoming(
            "node_b".to_string(),
            TriggerSource::Edge {
                from: "node_a".to_string(),
            },
        );
        table.add_incoming(
            "node_c".to_string(),
            TriggerSource::Edge {
                from: "node_a".to_string(),
            },
        );
        table.add_incoming(
            "node_c".to_string(),
            TriggerSource::Edge {
                from: "node_d".to_string(),
            },
        );

        let ttn = TriggerToNodes::from_trigger_table(&table);
        let triggered = ttn.triggered_nodes(&["node_a".to_string()]);
        assert!(triggered.contains("node_b"));
        assert!(triggered.contains("node_c"));
        assert!(!triggered.contains("node_d"));

        let triggered_d = ttn.triggered_nodes(&["node_d".to_string()]);
        assert!(triggered_d.contains("node_c"));
        assert!(!triggered_d.contains("node_b"));
    }

    #[test]
    fn test_trigger_to_nodes_debug() {
        let table: TriggerTable<TestState> = TriggerTable::default();
        let ttn = TriggerToNodes::from_trigger_table(&table);
        let debug = format!("{ttn:?}");
        assert!(debug.contains("TriggerToNodes"));
    }

    #[test]
    fn test_apply_writes_empty_outputs() {
        let mut state = TestState;
        let mut tracker = FieldVersionTracker::new(3);
        let outputs: Vec<crate::pregel::types::TaskOutput<TestState>> = Vec::new();

        let changed =
            apply_writes(&mut state, &outputs, &mut tracker).expect("empty outputs should succeed");
        assert_eq!(changed.0, 0);
    }

    #[test]
    fn test_check_replace_conflicts_empty() {
        let result: SuperstepResult<TestState> = SuperstepResult {
            task_outputs: Vec::new(),
            bubble_ups: Vec::new(),
        };
        let replace_fields = vec![0, 1];
        check_replace_conflicts(&result, &replace_fields).unwrap();
    }

    #[test]
    fn test_check_replace_conflicts_no_conflicts() {
        use crate::Command;

        let task_output_a: crate::pregel::types::TaskOutput<TestState> =
            crate::pregel::types::TaskOutput {
                triggered_fields: vec![],
                task_id: "task_1".to_string(),
                node_name: "node_a".to_string(),
                trigger: crate::pregel::types::TaskTrigger::Pull,
                command: Command::end(),
                duration: std::time::Duration::from_millis(10),
                error: None,
            };

        let result: SuperstepResult<TestState> = SuperstepResult {
            task_outputs: vec![task_output_a],
            bubble_ups: Vec::new(),
        };
        let replace_fields = vec![0, 1];
        check_replace_conflicts(&result, &replace_fields).unwrap();
    }

    #[test]
    fn test_consume_triggered_channels_empty() {
        let mut state = TestState;
        let triggered_channels = vec![0usize; 0];
        consume_triggered_channels(&mut state, &triggered_channels);
    }

    #[test]
    fn test_consume_triggered_channels_some() {
        let mut state = TestState;
        let triggered_channels = vec![0, 2];
        consume_triggered_channels(&mut state, &triggered_channels);
    }

    #[test]
    fn test_schedule_error_handlers_no_failures() {
        let nodes: indexmap::IndexMap<String, std::sync::Arc<dyn crate::Node<TestState>>> =
            indexmap::IndexMap::new();
        let task_outputs: Vec<TaskOutput<TestState>> = Vec::new();
        let error_handler_map = std::collections::HashMap::new();

        let recovery_tasks = schedule_error_handlers(&task_outputs, &nodes, &error_handler_map);
        assert!(recovery_tasks.is_empty());
    }

    #[test]
    fn test_schedule_error_handlers_with_failure() {
        use crate::Command;

        let mut nodes: indexmap::IndexMap<String, std::sync::Arc<dyn crate::Node<TestState>>> =
            indexmap::IndexMap::new();
        nodes.insert(
            "error_handler_a".to_string(),
            crate::node::NodeFnCommand(|_s: &TestState| async move { Ok(Command::end()) })
                .into_node("error_handler_a"),
        );

        let task_outputs = vec![TaskOutput {
            triggered_fields: vec![],
            task_id: "task-1".to_string(),
            node_name: "failing_node".to_string(),
            command: Command::default(),
            duration: std::time::Duration::ZERO,
            trigger: crate::pregel::types::TaskTrigger::Pull,
            error: Some(crate::JunctureError::execution("test failure")),
        }];

        let mut error_handler_map = std::collections::HashMap::new();
        error_handler_map.insert("failing_node".to_string(), "error_handler_a".to_string());

        let recovery_tasks = schedule_error_handlers(&task_outputs, &nodes, &error_handler_map);
        assert_eq!(recovery_tasks.len(), 1);
        assert_eq!(recovery_tasks[0].node_name, "error_handler_a");
    }

    #[test]
    fn test_schedule_error_handlers_missing_handler_node() {
        use crate::Command;

        let nodes: indexmap::IndexMap<String, std::sync::Arc<dyn crate::Node<TestState>>> =
            indexmap::IndexMap::new();

        let task_outputs = vec![TaskOutput {
            triggered_fields: vec![],
            task_id: "task-1".to_string(),
            node_name: "failing_node".to_string(),
            command: Command::default(),
            duration: std::time::Duration::ZERO,
            trigger: crate::pregel::types::TaskTrigger::Pull,
            error: Some(crate::JunctureError::execution("test failure")),
        }];

        let mut error_handler_map = std::collections::HashMap::new();
        error_handler_map.insert(
            "failing_node".to_string(),
            "nonexistent_handler".to_string(),
        );

        let recovery_tasks = schedule_error_handlers(&task_outputs, &nodes, &error_handler_map);
        assert!(
            recovery_tasks.is_empty(),
            "handler node not in graph, no recovery task"
        );
    }

    #[test]
    fn test_schedule_error_handlers_no_handler_registered() {
        use crate::Command;

        let nodes: indexmap::IndexMap<String, std::sync::Arc<dyn crate::Node<TestState>>> =
            indexmap::IndexMap::new();

        let task_outputs = vec![TaskOutput {
            triggered_fields: vec![],
            task_id: "task-1".to_string(),
            node_name: "failing_node".to_string(),
            command: Command::default(),
            duration: std::time::Duration::ZERO,
            trigger: crate::pregel::types::TaskTrigger::Pull,
            error: Some(crate::JunctureError::execution("test failure")),
        }];

        let error_handler_map = std::collections::HashMap::new();

        let recovery_tasks = schedule_error_handlers(&task_outputs, &nodes, &error_handler_map);
        assert!(recovery_tasks.is_empty());
    }

    #[test]
    fn test_get_error_handler_node_found() {
        let mut error_handler_map = std::collections::HashMap::new();
        error_handler_map.insert("node_a".to_string(), "handler_a".to_string());

        let handler = get_error_handler_node("node_a", &error_handler_map);
        assert_eq!(handler, Some("handler_a".to_string()));
    }

    #[test]
    fn test_get_error_handler_node_not_found() {
        let error_handler_map = std::collections::HashMap::new();

        let handler = get_error_handler_node("node_a", &error_handler_map);
        assert!(handler.is_none());
    }
}

// Rust guideline compliant 2026-05-20

// Rust guideline compliant 2026-05-19
// Rust guideline compliant 2026-05-20
