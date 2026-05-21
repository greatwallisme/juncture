use juncture_core::subgraph::StateSubset;
use juncture_core::{FieldsChanged, State};
use juncture_derive::State;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
fn derive_generates_field_versions_struct() {
    let versions = BasicStateFieldVersions::default();
    assert_eq!(versions.count, 0);
    assert_eq!(versions.label, 0);
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
