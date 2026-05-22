//! Human-in-the-Loop (HITL) support
//!
//! This module provides interrupt mechanisms for pausing graph execution
//! to await human input before continuing.

mod context;

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use xxhash_rust::xxh3::Xxh3;

pub use context::InterruptContext;

// Task-local storage for the interrupt context during node execution.
// This allows the `interrupt!` macro to access the context without
// requiring it to be passed explicitly as a parameter.
tokio::task_local! {
    pub static INTERRUPT_CONTEXT: std::sync::Arc<InterruptContext>;
}

// The interrupt! macro is exported at the crate root via #[macro_export]

/// Signal sent when a node requests interruption
///
/// Contains the interrupt payload and metadata for resumption.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InterruptSignal {
    /// Interrupt index (for position-based resume)
    pub index: usize,

    /// Optional named interrupt ID (for ID-based resume)
    pub id: Option<String>,

    /// Interrupt payload (human-readable context)
    pub payload: serde_json::Value,
}

/// Value provided when resuming from an interrupt
///
/// Supports single values, ID-based resume, and namespace-based resume.
#[derive(Clone, Debug)]
pub enum ResumeValue {
    /// Single value for position-based resume
    Single(serde_json::Value),

    /// Resume with a specific interrupt ID
    /// Key = `interrupt_id`, value = resume value
    ById(std::collections::HashMap<String, serde_json::Value>),

    /// Resume within a specific namespace
    /// Key = namespace (e.g., `node_name:uuid`), value = resume value
    /// Also used for Vec<Value> convenience wrapper (index-based matching)
    ByNamespace(std::collections::HashMap<String, serde_json::Value>),
}

/// Convenience wrapper: Vec<Value> can still be used for index-based matching
#[allow(
    clippy::fallible_impl_from,
    reason = "empty Vec is converted to Null, which is a valid value"
)]
impl From<Vec<serde_json::Value>> for ResumeValue {
    fn from(values: Vec<serde_json::Value>) -> Self {
        // Convert Vec to ByNamespace or Single
        if values.is_empty() {
            Self::Single(serde_json::Value::Null)
        } else if values.len() == 1 {
            Self::Single(values.into_iter().next().unwrap())
        } else {
            // Use index as key for multiple values
            let map: std::collections::HashMap<String, serde_json::Value> = values
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect();
            Self::ByNamespace(map)
        }
    }
}

/// Tag used to mark interrupt signals that should be hidden from external consumers
///
/// Nodes whose names start and end with `__` (e.g. `__route__`) are automatically
/// considered hidden. Hidden nodes are filtered from interrupt checks and stream
/// event emission so internal routing/infrastructure nodes never surface to
/// external consumers.
pub const HIDDEN_TAG: &str = "__hidden__";

/// Check if a node name indicates a hidden (internal) node.
///
/// A node is considered hidden when its name both starts and ends with `__`,
/// following the convention established by `LangGraph`'s `TAG_HIDDEN` mechanism.
///
/// Hidden nodes are filtered from:
/// - `interrupt_before` / `interrupt_after` checks via [`should_interrupt`]
/// - `StreamEvent::Interrupt` emission in the Pregel loop
///
/// # Examples
///
/// ```
/// use juncture_core::interrupt::is_hidden_node;
///
/// assert!(is_hidden_node("__route__"));
/// assert!(is_hidden_node("__internal_router__"));
/// assert!(!is_hidden_node("my_node"));
/// assert!(!is_hidden_node("__incomplete"));
/// assert!(!is_hidden_node("normal__"));
/// ```
#[must_use]
pub fn is_hidden_node(node_name: &str) -> bool {
    node_name.starts_with("__") && node_name.ends_with("__") && node_name.len() > 4
}

/// Generate a deterministic interrupt ID from node name and index
///
/// Uses `xxh3_128` for fast, deterministic 128-bit ID generation based on the
/// node name and index.
///
/// # Arguments
///
/// * `node_name` - The node name
/// * `index` - The interrupt index
///
/// # Returns
///
/// A lowercase hexadecimal string representing the 128-bit hash (32 characters)
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt::generate_interrupt_id;
///
/// let id = generate_interrupt_id("my_node", 0);
/// assert_eq!(id.len(), 32);
/// ```
///
/// # Determinism
///
/// The same `(node_name, index)` pair always produces the same ID within a
/// single process. Cross-process or cross-version reproducibility is **not**
/// guaranteed because the xxh3 internal algorithm may differ across builds
/// (e.g., SIMD variant selection).
#[must_use]
pub fn generate_interrupt_id(node_name: &str, index: usize) -> String {
    let mut hasher = Xxh3::new();
    node_name.hash(&mut hasher);
    index.hash(&mut hasher);
    let hash = hasher.digest128();
    format!("{hash:032x}")
}

/// Check if execution should interrupt based on the current state
///
/// Two-step check:
/// 1. **Version gating**: Only fire if any channel was updated since the last
///    interrupt (comparing `channel_versions` against `versions_seen_for_interrupt`).
/// 2. **Node name check**: Verify that a pending task targets a node listed
///    in `interrupt_before` or `interrupt_after`.
///
/// The version gate prevents infinite interrupt loops after checkpoint restore
/// when no state actually changed.
///
/// # Arguments
///
/// * `pending_tasks` - Tasks scheduled for the next superstep
/// * `interrupt_before` - Nodes that should interrupt before execution
/// * `interrupt_after` - Nodes that should interrupt after execution
/// * `channel_versions` - Current field version map (channel -> version)
/// * `versions_seen_for_interrupt` - Last-seen channel versions at the time of
///   the previous interrupt (flat map: channel -> single version)
///
/// # Returns
///
/// `Some(Vec<InterruptSignal>)` if interruption is needed, `None` otherwise
#[allow(
    clippy::implicit_hasher,
    reason = "accepting standard HashSet is fine for this use case"
)]
#[must_use]
pub fn should_interrupt<S: crate::State>(
    pending_tasks: &[crate::PendingTask<S>],
    interrupt_before: &HashSet<String>,
    interrupt_after: &HashSet<String>,
    channel_versions: &HashMap<String, u64>,
    versions_seen_for_interrupt: &HashMap<String, u64>,
) -> Option<Vec<InterruptSignal>> {
    // Step 1: Version gate -- skip interrupt if no channels updated since last
    let any_updates = channel_versions
        .iter()
        .any(|(chan, ver)| ver > versions_seen_for_interrupt.get(chan).unwrap_or(&0));

    if !any_updates && !versions_seen_for_interrupt.is_empty() {
        return None;
    }

    // Step 2: Node name check (skip hidden/internal nodes)
    let mut signals = Vec::new();

    for task in pending_tasks {
        let node_name = &task.node_name;

        // Hidden nodes (names starting/ending with __) are internal
        // infrastructure and must never surface as interrupts.
        if is_hidden_node(node_name) {
            continue;
        }

        if interrupt_before.contains(node_name) {
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(node_name, signals.len())),
                payload: serde_json::json!({
                    "node": node_name,
                    "reason": "interrupt_before",
                }),
            });
        }

        if interrupt_after.contains(node_name) {
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(node_name, signals.len())),
                payload: serde_json::json!({
                    "node": node_name,
                    "reason": "interrupt_after",
                }),
            });
        }
    }

    if signals.is_empty() {
        None
    } else {
        Some(signals)
    }
}

/// Internal interrupt implementation
///
/// Called by the `interrupt!` macro. Examines the interrupt context to determine
/// whether to resume with a previously provided value or to send an interrupt
/// signal and return an error.
///
/// # Arguments
///
/// * `ctx` - Interrupt context reference
/// * `payload` - Interrupt payload as JSON value
/// * `id` - Optional named interrupt ID
///
/// # Errors
///
/// Returns `JunctureError::interrupted` if this is the first execution
/// (no resume value available). Returns the resume value if resuming.
#[expect(
    clippy::unused_async,
    reason = "async is required by the interrupt! macro's .await expansion"
)]
pub async fn __interrupt_impl(
    ctx: &crate::interrupt::InterruptContext,
    payload: serde_json::Value,
    id: Option<&str>,
) -> Result<serde_json::Value, crate::JunctureError> {
    let index = ctx.next_index();

    let interrupt_id = id.map_or_else(
        || {
            // Use "current_node" as default node name when no explicit ID is provided
            generate_interrupt_id("current_node", index)
        },
        std::string::ToString::to_string,
    );

    if let Some(value) = ctx.get_resume_value(index) {
        return Ok(value);
    }

    ctx.send_interrupt(InterruptSignal {
        index,
        id: Some(interrupt_id),
        payload,
    })
    .map_err(|_err| crate::JunctureError::execution("interrupt channel closed"))?;

    Err(crate::JunctureError::interrupted(index))
}

/// Scratchpad for interrupt handling and transient data storage
///
/// Used by the HITL system to track processed interrupts and store
/// transient data during interrupt handling.
#[derive(Clone, Debug, Default)]
pub struct Scratchpad {
    /// Set of interrupt IDs that have been processed
    processed_interrupts: HashSet<String>,

    /// Transient data storage for interrupt handling
    data: HashMap<String, serde_json::Value>,
}

impl Scratchpad {
    /// Create a new empty scratchpad
    #[must_use]
    pub fn new() -> Self {
        Self {
            processed_interrupts: HashSet::new(),
            data: HashMap::new(),
        }
    }

    /// Check if an interrupt has been processed
    ///
    /// # Arguments
    ///
    /// * `id` - The interrupt ID to check
    ///
    /// # Returns
    ///
    /// `true` if the interrupt has been processed, `false` otherwise
    #[must_use]
    pub fn is_interrupt_processed(&self, id: &str) -> bool {
        self.processed_interrupts.contains(id)
    }

    /// Check if a confirmation-only resume is valid for the given interrupt.
    ///
    /// Returns `true` when the interrupt has already been processed,
    /// meaning the caller can resume without providing an explicit value.
    #[must_use]
    pub fn get_null_resume(&self, interrupt_id: &str) -> bool {
        self.is_interrupt_processed(interrupt_id)
    }

    /// Mark an interrupt as processed
    ///
    /// # Arguments
    ///
    /// * `id` - The interrupt ID to mark as processed
    pub fn mark_interrupt_processed(&mut self, id: &str) {
        self.processed_interrupts.insert(id.to_string());
    }

    /// Get transient data by key
    ///
    /// # Arguments
    ///
    /// * `key` - The data key
    ///
    /// # Returns
    ///
    /// The stored value, if it exists
    #[must_use]
    pub fn get_data(&self, key: &str) -> Option<&serde_json::Value> {
        self.data.get(key)
    }

    /// Set transient data
    ///
    /// # Arguments
    ///
    /// * `key` - The data key
    /// * `value` - The value to store
    pub fn set_data(&mut self, key: String, value: serde_json::Value) {
        self.data.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{HIDDEN_TAG, Scratchpad, is_hidden_node, should_interrupt};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn scratchpad_get_null_resume() {
        let mut pad = Scratchpad::new();
        assert!(!pad.get_null_resume("int-1"));
        pad.mark_interrupt_processed("int-1");
        assert!(pad.get_null_resume("int-1"));
        assert!(!pad.get_null_resume("int-2"));
    }

    // --- is_hidden_node tests ---

    #[test]
    fn hidden_node_double_underscore_prefix_and_suffix() {
        assert!(is_hidden_node("__route__"));
        assert!(is_hidden_node("__internal__"));
        assert!(is_hidden_node("__error_handler__"));
    }

    #[test]
    fn normal_nodes_are_not_hidden() {
        assert!(!is_hidden_node("my_node"));
        assert!(!is_hidden_node("agent"));
        assert!(!is_hidden_node("review"));
    }

    #[test]
    fn partial_underscore_prefix_is_not_hidden() {
        assert!(!is_hidden_node("__incomplete"));
        assert!(!is_hidden_node("__only_start"));
    }

    #[test]
    fn partial_underscore_suffix_is_not_hidden() {
        assert!(!is_hidden_node("only_end__"));
        assert!(!is_hidden_node("incomplete__"));
    }

    #[test]
    fn bare_double_underscore_is_not_hidden() {
        // "____" is only underscores and too short to be a meaningful hidden name
        assert!(!is_hidden_node("____"));
    }

    #[test]
    fn hidden_tag_constant_value() {
        assert_eq!(HIDDEN_TAG, "__hidden__");
    }

    // --- should_interrupt filtering tests ---

    /// Minimal `State` impl for testing `should_interrupt`.
    #[derive(Clone, Debug, serde::Serialize)]
    struct TestState;

    impl crate::State for TestState {
        type Update = TestUpdate;
        fn apply(&mut self, _: Self::Update) -> crate::FieldsChanged {
            crate::FieldsChanged(0)
        }
        fn reset_ephemeral(&mut self) {}
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    struct TestUpdate;

    fn make_task(node_name: &str) -> crate::PendingTask<TestState> {
        crate::PendingTask::pull(uuid::Uuid::new_v4().to_string(), node_name.to_string())
    }

    #[test]
    fn hidden_nodes_filtered_from_interrupt_before() {
        let tasks = vec![
            make_task("agent"),
            make_task("__route__"),
            make_task("review"),
        ];

        let mut interrupt_before = HashSet::new();
        interrupt_before.insert("agent".to_string());
        interrupt_before.insert("__route__".to_string());
        interrupt_before.insert("review".to_string());

        let channel_versions: HashMap<String, u64> =
            std::iter::once(("field_0".to_string(), 1u64)).collect();
        let versions_seen = HashMap::new();

        let result = should_interrupt(
            &tasks,
            &interrupt_before,
            &HashSet::new(),
            &channel_versions,
            &versions_seen,
        );

        let signals = result.expect("should return signals");
        // Only "agent" and "review" should produce signals, "__route__" filtered
        assert_eq!(signals.len(), 2, "hidden node __route__ should be filtered");
        let nodes: Vec<&str> = signals
            .iter()
            .filter_map(|s| s.payload.get("node").and_then(|v| v.as_str()))
            .collect();
        assert!(nodes.contains(&"agent"), "agent should be present");
        assert!(nodes.contains(&"review"), "review should be present");
        assert!(
            !nodes.contains(&"__route__"),
            "__route__ should be filtered"
        );
    }

    #[test]
    fn hidden_nodes_filtered_from_interrupt_after() {
        let tasks = vec![make_task("agent"), make_task("__internal_router__")];

        let mut interrupt_after = HashSet::new();
        interrupt_after.insert("agent".to_string());
        interrupt_after.insert("__internal_router__".to_string());

        let channel_versions: HashMap<String, u64> =
            std::iter::once(("field_0".to_string(), 1u64)).collect();
        let versions_seen = HashMap::new();

        let result = should_interrupt(
            &tasks,
            &HashSet::new(),
            &interrupt_after,
            &channel_versions,
            &versions_seen,
        );

        let signals = result.expect("should return signals");
        assert_eq!(
            signals.len(),
            1,
            "only agent should produce a signal, __internal_router__ filtered"
        );
        let node = signals[0]
            .payload
            .get("node")
            .and_then(|v| v.as_str())
            .expect("should have node");
        assert_eq!(node, "agent");
    }

    #[test]
    fn all_hidden_nodes_produces_no_signals() {
        let tasks = vec![make_task("__route__"), make_task("__handler__")];

        let mut interrupt_before = HashSet::new();
        interrupt_before.insert("__route__".to_string());
        interrupt_before.insert("__handler__".to_string());

        let channel_versions: HashMap<String, u64> =
            std::iter::once(("field_0".to_string(), 1u64)).collect();
        let versions_seen = HashMap::new();

        let result = should_interrupt(
            &tasks,
            &interrupt_before,
            &HashSet::new(),
            &channel_versions,
            &versions_seen,
        );

        assert!(
            result.is_none(),
            "all-hidden-node tasks should produce no interrupt signals"
        );
    }
}

// Rust guideline compliant 2026-05-20
