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
