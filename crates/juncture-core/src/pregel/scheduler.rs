//! Version tracking and task computation for Pregel engine
//!
//! This module provides field version tracking, task scheduling logic,
//! state write application, and trigger-to-node mapping for the Pregel
//! execution engine.

use crate::{
    JunctureError, State,
    edge::{CompiledEdge, TriggerSource, TriggerTable},
    pregel::types::PendingTask,
    state::FieldsChanged,
};
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
    seen: HashMap<String, Vec<u64>>,
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
    state: &S,
) -> Result<Vec<PendingTask<S>>, JunctureError> {
    let mut next_tasks = Vec::new();
    let mut seen_nodes = HashSet::new();

    // First, check if any task returned a Command with explicit routing
    for task_output in completed_tasks {
        let command = &task_output.command;

        match &command.goto {
            crate::Goto::None => {
                // No explicit routing, use trigger table
                if let Some(edges) = trigger_table.outgoing.get(&task_output.node_name) {
                    for edge in edges {
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
            crate::Goto::Send(_send_targets) => {
                // Dynamic fan-out with state overrides
                // Note: Send operations require State to implement Deserialize
                // which is not a general requirement. This will be implemented
                // in a future phase by adding a separate trait bound for
                // deserializable states or using a different approach.
            }
            crate::Goto::End => {
                // Termination, no next tasks
            }
        }
    }

    Ok(next_tasks)
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
            if !seen_nodes.contains(target) {
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

            if !seen_nodes.contains(target) {
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
/// # Examples
///
/// ```ignore
/// use juncture_core::pregel::scheduler::{apply_writes, FieldVersionTracker};
///
/// let mut state = MyState::default();
/// let mut tracker = FieldVersionTracker::new(3);
/// let changed = apply_writes(&mut state, &task_outputs, &mut tracker);
/// ```
pub fn apply_writes<S: State>(
    state: &mut S,
    task_outputs: &[crate::pregel::types::TaskOutput<S>],
    field_versions: &mut FieldVersionTracker,
) -> FieldsChanged {
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
            let changed = state.apply(update.clone());
            total_changed.merge(&changed);
        }
    }

    // Bump field versions for all changed fields
    field_versions.bump_all(&total_changed);

    total_changed
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

impl std::fmt::Debug for TriggerToNodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriggerToNodes")
            .field("mapping_len", &self.mapping.len())
            .finish()
    }
}

/// Output from a single completed task (re-export for scheduler use)
pub use crate::pregel::types::TaskOutput;

#[cfg(test)]
mod scheduler_tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct TestState;

    impl State for TestState {
        type Update = TestUpdate;
        type FieldVersions = ();

        fn apply(&mut self, _: Self::Update) -> FieldsChanged {
            FieldsChanged(0)
        }

        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default)]
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

        let changed = apply_writes(&mut state, &outputs, &mut tracker);
        assert_eq!(changed.0, 0);
    }
}

// Rust guideline compliant 2026-05-19
