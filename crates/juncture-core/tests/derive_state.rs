use juncture_core::pregel::{
    FieldVersionTracker, SuperstepResult, TaskOutput, TaskTrigger, apply_writes,
    check_replace_conflicts, consume_triggered_channels,
};
use juncture_core::subgraph::StateSubset;
use juncture_core::{Command, FieldsChanged, State};
use juncture_derive::State;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- try_apply() tests ---

#[test]
fn derive_try_apply_succeeds_for_valid_update() {
    let mut state = BasicState {
        count: 0,
        label: String::new(),
    };
    let update = BasicStateUpdate {
        count: Some(42),
        label: Some("hello".to_string()),
    };
    let result = state.try_apply(update);
    assert!(result.is_ok(), "try_apply should succeed for valid update");
    let changed = result.expect("ok");
    assert!(changed.has_field(BasicState::FIELD_COUNT));
    assert!(changed.has_field(BasicState::FIELD_LABEL));
    assert_eq!(state.count, 42);
    assert_eq!(state.label, "hello");
}

#[test]
fn derive_try_apply_partial_update() {
    let mut state = BasicState {
        count: 5,
        label: "existing".to_string(),
    };
    let update = BasicStateUpdate {
        count: Some(99),
        label: None,
    };
    let changed = state
        .try_apply(update)
        .expect("partial update should succeed");
    assert_eq!(state.count, 99);
    assert_eq!(state.label, "existing");
    assert!(changed.has_field(BasicState::FIELD_COUNT));
    assert!(!changed.has_field(BasicState::FIELD_LABEL));
}

#[test]
fn derive_try_apply_no_changes() {
    let mut state = BasicState {
        count: 10,
        label: "unchanged".to_string(),
    };
    let update = BasicStateUpdate {
        count: None,
        label: None,
    };
    let changed = state
        .try_apply(update)
        .expect("empty update should succeed");
    assert!(changed.is_empty());
    assert_eq!(state.count, 10);
    assert_eq!(state.label, "unchanged");
}

#[test]
fn derive_try_apply_append_reducer() {
    let mut state = FullState {
        value: 0,
        items: vec!["a".to_string()],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    let update = FullStateUpdate {
        value: None,
        items: Some(vec!["b".to_string(), "c".to_string()]),
        scratch: None,
        scores: None,
        status: None,
        cache: None,
    };
    let changed = state
        .try_apply(update)
        .expect("append reducer should succeed");
    assert_eq!(state.items, vec!["a", "b", "c"]);
    assert!(changed.has_field(FullState::FIELD_ITEMS));
}

#[test]
fn derive_try_apply_custom_reducer() {
    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: None,
        scores: HashMap::from([("a".to_string(), 1.0)]),
        status: String::new(),
        cache: None,
    };
    let update = FullStateUpdate {
        value: None,
        items: None,
        scratch: None,
        scores: Some(HashMap::from([("b".to_string(), 2.0)])),
        status: None,
        cache: None,
    };
    let changed = state
        .try_apply(update)
        .expect("custom reducer should succeed");
    assert_eq!(state.scores.len(), 2);
    assert!(changed.has_field(FullState::FIELD_SCORES));
}

// --- field_is_set() tests ---

#[test]
fn derive_field_is_set_detects_some_values() {
    let update = BasicStateUpdate {
        count: Some(42),
        label: None,
    };
    assert!(BasicState::field_is_set(&update, BasicState::FIELD_COUNT));
    assert!(!BasicState::field_is_set(&update, BasicState::FIELD_LABEL));
}

#[test]
fn derive_field_is_set_returns_false_for_none() {
    let update = BasicStateUpdate {
        count: None,
        label: None,
    };
    assert!(!BasicState::field_is_set(&update, BasicState::FIELD_COUNT));
    assert!(!BasicState::field_is_set(&update, BasicState::FIELD_LABEL));
}

#[test]
fn derive_field_is_set_returns_false_for_invalid_index() {
    let update = BasicStateUpdate {
        count: Some(1),
        label: Some("x".to_string()),
    };
    assert!(!BasicState::field_is_set(&update, 99));
}

// --- REPLACE_FIELD_INDICES tests ---

#[test]
fn derive_replace_field_indices_default_reducer() {
    // BasicState has count (replace) and label (replace)
    assert_eq!(BasicState::REPLACE_FIELD_INDICES, &[0, 1]);
    // State trait method should match
    assert_eq!(<BasicState as State>::replace_field_indices(), &[0, 1]);
}

#[test]
fn derive_replace_field_indices_mixed_reducers() {
    // FullState has: value (replace), items (append), scratch (ephemeral),
    // scores (custom), status (last_write_wins), cache (untracked)
    // Only 'value' uses replace reducer
    assert_eq!(FullState::REPLACE_FIELD_INDICES, &[0]);
    assert_eq!(<FullState as State>::replace_field_indices(), &[0]);
}

#[test]
fn derive_replace_field_indices_child_state() {
    // ChildState has: name (replace), messages (append)
    assert_eq!(ChildState::REPLACE_FIELD_INDICES, &[0]);
}

/// Basic state with default (replace) reducer
#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct BasicState {
    count: u32,
    label: String,
}

#[test]
fn derive_generates_update_struct() {
    let update = BasicStateUpdate {
        count: Some(42),
        label: Some("test".to_string()),
    };
    assert!(update.count == Some(42));
    assert!(update.label == Some("test".to_string()));
}

#[test]
fn derive_apply_replace_reducer() {
    let mut state = BasicState {
        count: 0,
        label: String::new(),
    };
    let update = BasicStateUpdate {
        count: Some(10),
        label: Some("hello".to_string()),
    };
    let changed = state.apply(update);
    assert_eq!(state.count, 10);
    assert_eq!(state.label, "hello");
    assert!(changed.has_field(BasicState::FIELD_COUNT));
    assert!(changed.has_field(BasicState::FIELD_LABEL));
}

#[test]
fn derive_apply_partial_update() {
    let mut state = BasicState {
        count: 5,
        label: "existing".to_string(),
    };
    let update = BasicStateUpdate {
        count: Some(99),
        label: None,
    };
    let changed = state.apply(update);
    assert_eq!(state.count, 99);
    assert_eq!(state.label, "existing");
    assert!(changed.has_field(BasicState::FIELD_COUNT));
    assert!(!changed.has_field(BasicState::FIELD_LABEL));
}

#[test]
fn derive_field_constants() {
    assert_eq!(BasicState::FIELD_COUNT, 0);
    assert_eq!(BasicState::FIELD_LABEL, 1);
}

#[test]
fn derive_schema_version_default() {
    assert_eq!(BasicState::schema_version(), 1);
}

/// State with all reducer types
#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct FullState {
    /// Default: replace
    value: u32,
    /// Append reducer
    #[reducer(append)]
    items: Vec<String>,
    /// Ephemeral
    #[reducer(ephemeral)]
    scratch: Option<String>,
    /// Custom reducer
    #[reducer(custom = merge_maps)]
    scores: HashMap<String, f32>,
    /// Last write wins
    #[reducer(last_write_wins)]
    status: String,
    /// Untracked
    #[reducer(untracked)]
    cache: Option<String>,
}

fn merge_maps(current: &mut HashMap<String, f32>, incoming: HashMap<String, f32>) {
    for (k, v) in incoming {
        current.insert(k, v);
    }
}

#[test]
fn append_reducer_extends_vec() {
    let mut state = FullState {
        value: 0,
        items: vec!["a".to_string()],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    let update = FullStateUpdate {
        value: None,
        items: Some(vec!["b".to_string(), "c".to_string()]),
        scratch: None,
        scores: None,
        status: None,
        cache: None,
    };
    state.apply(update);
    assert_eq!(state.items, vec!["a", "b", "c"]);
}

#[test]
fn ephemeral_reducer_resets_on_call() {
    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    let update = FullStateUpdate {
        value: None,
        items: None,
        scratch: Some(Some("temp".to_string())),
        scores: None,
        status: None,
        cache: None,
    };
    state.apply(update);
    assert_eq!(state.scratch, Some("temp".to_string()));

    // Ephemeral fields reset after superstep
    state.reset_ephemeral();
    assert_eq!(state.scratch, None);
}

#[test]
fn custom_reducer_merges() {
    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: None,
        scores: HashMap::from([("a".to_string(), 1.0)]),
        status: String::new(),
        cache: None,
    };
    let update = FullStateUpdate {
        value: None,
        items: None,
        scratch: None,
        scores: Some(HashMap::from([("b".to_string(), 2.0)])),
        status: None,
        cache: None,
    };
    state.apply(update);
    assert_eq!(state.scores.len(), 2);
    assert!((state.scores["a"] - 1.0).abs() < f32::EPSILON);
    assert!((state.scores["b"] - 2.0).abs() < f32::EPSILON);
}

/// State with explicit schema version
#[derive(State, Clone, Debug, Serialize, Deserialize)]
#[state_version(3)]
struct VersionedState {
    data: String,
}

#[test]
fn derive_schema_version_explicit() {
    assert_eq!(VersionedState::schema_version(), 3);
}

#[test]
fn fields_changed_bitmask() {
    let mut changed = FieldsChanged::default();
    assert!(changed.is_empty());

    changed.set_field(0);
    assert!(!changed.is_empty());
    assert!(changed.has_field(0));
    assert!(!changed.has_field(1));

    changed.set_field(5);
    assert!(changed.has_field(5));

    let mut other = FieldsChanged::default();
    other.set_field(10);
    changed.merge(&other);
    assert!(changed.has_field(0));
    assert!(changed.has_field(5));
    assert!(changed.has_field(10));
}

// --- StateSubset tests: shared-state subgraph mode (Mode 1) ---

/// Parent state with name, age, and messages fields.
/// Messages use append reducer for accumulated message history.
#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct ParentState {
    name: String,
    age: u32,
    #[reducer(append)]
    messages: Vec<String>,
}

/// Child state that is a subset of `ParentState` (name + messages only).
/// The `#[subset_of(ParentState)]` attribute generates the `StateSubset` impl.
#[derive(State, Clone, Debug, Serialize, Deserialize)]
#[subset_of(ParentState)]
struct ChildState {
    name: String,
    #[reducer(append)]
    messages: Vec<String>,
}

#[test]
fn subset_extract_clones_shared_fields_from_parent() {
    let parent = ParentState {
        name: "Alice".to_string(),
        age: 30,
        messages: vec!["hello".to_string(), "world".to_string()],
    };

    let child = ChildState::extract(&parent);

    assert_eq!(child.name, "Alice");
    assert_eq!(child.messages, vec!["hello", "world"]);
}

#[test]
fn subset_extract_omits_parent_only_fields() {
    let parent = ParentState {
        name: "Bob".to_string(),
        age: 25,
        messages: vec![],
    };

    let child = ChildState::extract(&parent);

    // Child state has no age field -- only name and messages are visible
    assert_eq!(child.name, "Bob");
    assert!(child.messages.is_empty());
}

#[test]
fn subset_map_update_projects_child_fields_into_parent_update() {
    let child_update = ChildStateUpdate {
        name: Some("Charlie".to_string()),
        messages: Some(vec!["new message".to_string()]),
    };

    let parent_update = <ChildState as StateSubset<ParentState>>::map_update(child_update);

    assert_eq!(parent_update.name, Some("Charlie".to_string()));
    assert_eq!(
        parent_update.messages,
        Some(vec!["new message".to_string()])
    );
    // age is not part of the child state, so it maps to None
    assert_eq!(parent_update.age, None);
}

#[test]
fn subset_map_update_partial_child_update_only_messages() {
    let child_update = ChildStateUpdate {
        name: None,
        messages: Some(vec!["msg1".to_string(), "msg2".to_string()]),
    };

    let parent_update = <ChildState as StateSubset<ParentState>>::map_update(child_update);

    assert_eq!(parent_update.name, None);
    assert_eq!(
        parent_update.messages,
        Some(vec!["msg1".to_string(), "msg2".to_string()])
    );
    assert_eq!(parent_update.age, None);
}

#[test]
fn subset_roundtrip_extract_then_map_update() {
    let parent = ParentState {
        name: "Dana".to_string(),
        age: 40,
        messages: vec!["first".to_string()],
    };

    // Extract child state from parent
    let mut child = ChildState::extract(&parent);
    assert_eq!(child.name, "Dana");
    assert_eq!(child.messages, vec!["first"]);

    // Simulate subgraph modifying the child state via apply
    let child_update = ChildStateUpdate {
        name: Some("Dana Updated".to_string()),
        messages: Some(vec!["second".to_string()]),
    };
    child.apply(child_update);
    assert_eq!(child.name, "Dana Updated");
    // Append reducer: original + new
    assert_eq!(child.messages, vec!["first", "second"]);

    // Map a final child update back to parent update
    let final_child_update = ChildStateUpdate {
        name: None,
        messages: Some(vec!["third".to_string()]),
    };
    let parent_update = <ChildState as StateSubset<ParentState>>::map_update(final_child_update);

    assert_eq!(parent_update.name, None);
    assert_eq!(parent_update.messages, Some(vec!["third".to_string()]));
}

// --- Multi-writer detection tests ---

#[test]
fn check_replace_conflicts_detects_multiple_writers() {
    // Two tasks both write to the "count" field (replace reducer)
    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(1),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };
    let task_b = TaskOutput {
        triggered_fields: vec![],
        task_id: "t2".to_string(),
        node_name: "node_b".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(2),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let result = SuperstepResult {
        task_outputs: vec![task_a, task_b],
        bubble_ups: vec![],
    };

    // count is at index 0, label is at index 1 -- both are replace fields
    let err = check_replace_conflicts::<BasicState>(&result, &[0])
        .expect_err("should detect multiple writers on count");
    assert!(err.is_execution(), "expected execution error, got: {err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("node_a") && msg.contains("node_b"),
        "error should list both writers: {msg}"
    );
}

#[test]
fn check_replace_conflicts_allows_single_writer() {
    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(1),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let result = SuperstepResult {
        task_outputs: vec![task_a],
        bubble_ups: vec![],
    };

    check_replace_conflicts::<BasicState>(&result, &[0]).expect("single writer should be allowed");
}

#[test]
fn check_replace_conflicts_allows_different_fields() {
    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(1),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };
    let task_b = TaskOutput {
        triggered_fields: vec![],
        task_id: "t2".to_string(),
        node_name: "node_b".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: None,
            label: Some("hello".to_string()),
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let result = SuperstepResult {
        task_outputs: vec![task_a, task_b],
        bubble_ups: vec![],
    };

    // Different fields should not conflict
    check_replace_conflicts::<BasicState>(&result, &[0, 1])
        .expect("different fields should not conflict");
}

#[test]
fn apply_writes_rejects_multiple_writers_on_replace_field() {
    let mut state = BasicState {
        count: 0,
        label: String::new(),
    };
    let mut tracker = FieldVersionTracker::new(2);

    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(1),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };
    let task_b = TaskOutput {
        triggered_fields: vec![],
        task_id: "t2".to_string(),
        node_name: "node_b".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(2),
            label: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let result = apply_writes(&mut state, &[task_a, task_b], &mut tracker);
    assert!(
        result.is_err(),
        "apply_writes should reject multiple writers"
    );
    let err = result.expect_err("should error");
    assert!(
        err.is_multiple_writers(),
        "expected MultipleWriters error, got: {err:?}"
    );
}

#[test]
fn apply_writes_allows_single_writer_on_replace_field() {
    let mut state = BasicState {
        count: 0,
        label: String::new(),
    };
    let mut tracker = FieldVersionTracker::new(2);

    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(BasicStateUpdate {
            count: Some(42),
            label: Some("hello".to_string()),
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let changed =
        apply_writes(&mut state, &[task_a], &mut tracker).expect("single writer should succeed");
    assert_eq!(state.count, 42);
    assert_eq!(state.label, "hello");
    assert!(changed.has_field(BasicState::FIELD_COUNT));
    assert!(changed.has_field(BasicState::FIELD_LABEL));
}

#[test]
fn apply_writes_allows_append_field_multiple_writers() {
    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    let mut tracker = FieldVersionTracker::new(6);

    // Both tasks write to items (append reducer) -- this should be fine
    let task_a = TaskOutput {
        triggered_fields: vec![],
        task_id: "t1".to_string(),
        node_name: "node_a".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(FullStateUpdate {
            value: None,
            items: Some(vec!["a".to_string()]),
            scratch: None,
            scores: None,
            status: None,
            cache: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };
    let task_b = TaskOutput {
        triggered_fields: vec![],
        task_id: "t2".to_string(),
        node_name: "node_b".to_string(),
        trigger: TaskTrigger::Pull,
        command: Command::update(FullStateUpdate {
            value: None,
            items: Some(vec!["b".to_string()]),
            scratch: None,
            scores: None,
            status: None,
            cache: None,
        }),
        duration: std::time::Duration::from_millis(1),
        error: None,
    };

    let changed = apply_writes(&mut state, &[task_a, task_b], &mut tracker)
        .expect("append reducer allows multiple writers");
    assert_eq!(state.items, vec!["a", "b"]);
    assert!(changed.has_field(FullState::FIELD_ITEMS));
}

// --- replace_after_finish reducer tests ---

/// State with `replace_after_finish` field for finish semantics testing
#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct FinishState {
    value: u32,
    #[reducer(replace_after_finish)]
    result: String,
    #[reducer(append)]
    logs: Vec<String>,
}

#[test]
fn derive_replace_after_finish_field_indices() {
    // FinishState has: value (replace), result (replace_after_finish), logs (append)
    // Only 'result' at index 1 uses replace_after_finish
    assert_eq!(
        FinishState::REPLACE_AFTER_FINISH_FIELD_INDICES,
        &[1],
        "only the 'result' field at index 1 should be in replace_after_finish indices"
    );
    assert_eq!(
        <FinishState as State>::replace_after_finish_field_indices(),
        &[1],
        "trait method should match the inherent constant"
    );
}

#[test]
fn derive_replace_after_finish_not_in_replace_indices() {
    // replace_after_finish fields should NOT appear in REPLACE_FIELD_INDICES
    // because they use different conflict detection semantics
    assert_eq!(
        FinishState::REPLACE_FIELD_INDICES,
        &[0],
        "only 'value' (index 0) uses the replace reducer"
    );
}

#[test]
fn derive_replace_after_finish_apply_works_normally() {
    // apply() should assign the value like any replace-like reducer
    let mut state = FinishState {
        value: 0,
        result: String::new(),
        logs: vec![],
    };
    let update = FinishStateUpdate {
        value: Some(42),
        result: Some("computed".to_string()),
        logs: Some(vec!["step1".to_string()]),
    };
    let changed = state.apply(update);
    assert_eq!(state.value, 42);
    assert_eq!(state.result, "computed");
    assert_eq!(state.logs, vec!["step1"]);
    assert!(changed.has_field(FinishState::FIELD_VALUE));
    assert!(changed.has_field(FinishState::FIELD_RESULT));
    assert!(changed.has_field(FinishState::FIELD_LOGS));
}

#[test]
fn derive_finish_field_is_noop_for_non_finish_fields() {
    // Calling finish_field for a non-replace_after_finish field should be a no-op
    let mut state = FinishState {
        value: 99,
        result: "data".to_string(),
        logs: vec!["log".to_string()],
    };
    // Index 0 = value (replace reducer) -- finish_field should be no-op
    state.finish_field(0);
    assert_eq!(
        state.value, 99,
        "value should be unchanged after finish_field"
    );
    // Index 2 = logs (append reducer) -- finish_field should be no-op
    state.finish_field(2);
    assert_eq!(
        state.logs,
        vec!["log"],
        "logs should be unchanged after finish_field"
    );
}

#[test]
fn derive_finish_field_handles_finish_index() {
    // Calling finish_field for the replace_after_finish field should succeed
    let mut state = FinishState {
        value: 1,
        result: "final_value".to_string(),
        logs: vec![],
    };
    // Index 1 = result (replace_after_finish) -- this is the field that
    // finish_all_channels targets
    state.finish_field(1);
    // The field value should be unchanged -- finish is a lifecycle notification
    assert_eq!(state.result, "final_value");
}

#[test]
fn derive_finish_field_ignores_invalid_index() {
    let mut state = FinishState {
        value: 5,
        result: "unchanged".to_string(),
        logs: vec![],
    };
    // Index 99 is out of bounds -- should be silently ignored
    state.finish_field(99);
    assert_eq!(state.value, 5);
    assert_eq!(state.result, "unchanged");
}

#[test]
fn derive_no_replace_after_finish_fields_yields_empty_slice() {
    // States without replace_after_finish fields should return empty slice
    let empty: &[usize] = &[];
    assert_eq!(
        BasicState::REPLACE_AFTER_FINISH_FIELD_INDICES,
        empty,
        "BasicState has no replace_after_finish fields"
    );
    assert_eq!(
        FullState::REPLACE_AFTER_FINISH_FIELD_INDICES,
        empty,
        "FullState has no replace_after_finish fields"
    );
}

/// State with multiple `replace_after_finish` fields
#[derive(State, Clone, Debug, Serialize, Deserialize)]
struct MultiFinishState {
    #[reducer(replace_after_finish)]
    output_a: String,
    intermediate: u32,
    #[reducer(replace_after_finish)]
    output_b: String,
}

#[test]
fn derive_multiple_replace_after_finish_fields() {
    // MultiFinishState: output_a (index 0), intermediate (index 1), output_b (index 2)
    assert_eq!(
        MultiFinishState::REPLACE_AFTER_FINISH_FIELD_INDICES,
        &[0, 2],
        "both output_a and output_b should be in replace_after_finish indices"
    );
}

#[test]
fn derive_multiple_finish_field_calls() {
    let mut state = MultiFinishState {
        output_a: "result_a".to_string(),
        intermediate: 42,
        output_b: "result_b".to_string(),
    };
    // Finish both replace_after_finish fields
    state.finish_field(0);
    state.finish_field(2);
    assert_eq!(state.output_a, "result_a");
    assert_eq!(state.output_b, "result_b");
    assert_eq!(state.intermediate, 42);
}

// --- consume_field() tests ---

#[test]
fn derive_ephemeral_field_indices_identified() {
    // FullState has: value (replace), items (append), scratch (ephemeral),
    //                scores (custom), status (last_write_wins), cache (untracked)
    // Only 'scratch' at index 2 uses ephemeral reducer
    assert_eq!(
        FullState::CONSUME_FIELD_INDICES,
        &[2],
        "only the 'scratch' field at index 2 should be in consume field indices"
    );
    assert_eq!(
        <FullState as State>::consume_field_indices(),
        &[2],
        "trait method should match the inherent constant"
    );
}

#[test]
fn derive_consume_field_is_noop_for_non_ephemeral_fields() {
    // Calling consume_field for a non-ephemeral field should be a no-op
    let mut state = FullState {
        value: 42,
        items: vec!["log".to_string()],
        scratch: Some("data".to_string()),
        scores: HashMap::new(),
        status: "active".to_string(),
        cache: None,
    };
    // Index 0 = value (replace reducer) -- consume_field should be no-op
    state.consume_field(0);
    assert_eq!(
        state.value, 42,
        "value should be unchanged after consume_field"
    );
    // Index 1 = items (append reducer) -- consume_field should be no-op
    state.consume_field(1);
    assert_eq!(
        state.items,
        vec!["log"],
        "items should be unchanged after consume_field"
    );
    // Index 3 = scores (custom reducer) -- consume_field should be no-op
    state.consume_field(3);
    assert_eq!(
        state.scores.len(),
        0,
        "scores should be unchanged after consume_field"
    );
}

#[test]
fn derive_consume_field_on_ephemeral_field_is_harmless() {
    // Calling consume_field for an ephemeral field should succeed without error.
    // The field value remains intact; reset_ephemeral() handles clearing.
    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: Some("in_progress".to_string()),
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    // Index 2 = scratch (ephemeral) -- consume_field should be callable
    state.consume_field(2);
    assert_eq!(
        state.scratch,
        Some("in_progress".to_string()),
        "ephemeral field value should remain after consume_field"
    );

    // After reset_ephemeral, the value should be cleared
    state.reset_ephemeral();
    assert_eq!(
        state.scratch, None,
        "ephemeral field should be cleared after reset_ephemeral"
    );
}

#[test]
fn derive_consume_field_ignores_invalid_index() {
    let mut state = FullState {
        value: 5,
        items: vec![],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };
    // Index 99 is out of bounds -- should be silently ignored
    state.consume_field(99);
    assert_eq!(state.value, 5);
}

#[test]
fn derive_no_ephemeral_fields_yields_empty_slice() {
    // States without ephemeral fields should return empty slice
    let empty: &[usize] = &[];
    assert_eq!(
        BasicState::CONSUME_FIELD_INDICES,
        empty,
        "BasicState has no ephemeral fields"
    );
    assert_eq!(
        FinishState::CONSUME_FIELD_INDICES,
        empty,
        "FinishState has no ephemeral fields"
    );
}

#[test]
fn derive_consume_field_multiple_ephemeral_fields() {
    /// State with multiple ephemeral fields
    #[derive(State, Clone, Debug, Serialize, Deserialize)]
    struct MultiEphemeralState {
        #[reducer(ephemeral)]
        temp_a: String,
        persistent: u32,
        #[reducer(ephemeral)]
        temp_b: Option<String>,
    }

    // MultiEphemeralState: temp_a (index 0), persistent (index 1), temp_b (index 2)
    assert_eq!(
        MultiEphemeralState::CONSUME_FIELD_INDICES,
        &[0, 2],
        "both temp_a and temp_b should be in consume field indices"
    );

    let mut state = MultiEphemeralState {
        temp_a: "value_a".to_string(),
        persistent: 42,
        temp_b: Some("value_b".to_string()),
    };

    // Consume both ephemeral fields
    state.consume_field(0);
    state.consume_field(2);
    assert_eq!(
        state.temp_a, "value_a",
        "temp_a should remain after consume"
    );
    assert_eq!(
        state.temp_b,
        Some("value_b".to_string()),
        "temp_b should remain after consume"
    );
    assert_eq!(state.persistent, 42, "persistent should be unchanged");
}

#[test]
fn derive_consume_field_after_apply_writes_round_trip() {
    // Simulate the full cycle: apply_writes -> consume -> reset_ephemeral

    let mut state = FullState {
        value: 0,
        items: vec![],
        scratch: None,
        scores: HashMap::new(),
        status: String::new(),
        cache: None,
    };

    // Apply an update that writes to the ephemeral field
    let update = FullStateUpdate {
        value: Some(10),
        items: Some(vec!["step".to_string()]),
        scratch: Some(Some("temp_result".to_string())),
        scores: None,
        status: None,
        cache: None,
    };
    let changed = state.apply(update);
    assert!(
        changed.has_field(FullState::FIELD_SCRATCH),
        "ephemeral field should be marked as changed"
    );
    assert_eq!(state.scratch, Some("temp_result".to_string()));

    // Consume triggered channels (as after_tick does)
    let triggered = vec![FullState::FIELD_SCRATCH];
    consume_triggered_channels(&mut state, &triggered);
    assert_eq!(
        state.scratch,
        Some("temp_result".to_string()),
        "value should remain after consume"
    );

    // Reset ephemeral (as after_tick does)
    state.reset_ephemeral();
    assert_eq!(
        state.scratch, None,
        "value should be cleared after reset_ephemeral"
    );
}

// --- field_count() and field_names() tests ---

#[test]
fn derive_field_count_matches_struct_field_count() {
    // BasicState has 2 fields: count, label
    assert_eq!(<BasicState as State>::field_count(), 2);
}

#[test]
fn derive_field_names_returns_declaration_order() {
    let names = <BasicState as State>::field_names();
    assert_eq!(names, ["count", "label"]);
}

#[test]
fn derive_field_count_for_multi_field_struct() {
    // FinishState has 3 fields: value, result, logs
    assert_eq!(<FinishState as State>::field_count(), 3);
    assert_eq!(
        <FinishState as State>::field_names(),
        ["value", "result", "logs"]
    );
}
