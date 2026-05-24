//! Human-in-the-Loop (HITL) support
//!
//! This module provides interrupt mechanisms for pausing graph execution
//! to await human input before continuing.

mod context;

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use xxhash_rust::xxh3::Xxh3;

use chrono::{DateTime, Utc};

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

    /// Timestamp when the interrupt was created
    #[serde(default = "InterruptSignal::current_timestamp")]
    pub timestamp: DateTime<Utc>,
}

impl InterruptSignal {
    /// Returns the current UTC timestamp as the default value for timestamp field
    #[must_use]
    fn current_timestamp() -> DateTime<Utc> {
        Utc::now()
    }
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

/// Record of an interrupt event for audit trail purposes
///
/// Tracks the complete lifecycle of an interrupt from creation to resumption.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InterruptRecord {
    /// Unique identifier for this interrupt
    pub id: String,

    /// Node name where the interrupt occurred
    pub node: String,

    /// Payload associated with the interrupt
    pub payload: serde_json::Value,

    /// Timestamp when the interrupt was created
    pub timestamp: DateTime<Utc>,

    /// Timestamp when the interrupt was resumed (None if still pending)
    pub resumed_at: Option<DateTime<Utc>>,

    /// Value provided when resuming (None if still pending)
    pub resume_value: Option<serde_json::Value>,
}

/// Extract the namespace from an interrupt ID
///
/// Interrupt IDs follow the format `node_name#index` or `namespace:node_name#index`.
/// This function extracts the namespace portion if it exists.
///
/// # Arguments
///
/// * `interrupt_id` - The interrupt ID string to parse
///
/// # Returns
///
/// * `Some(namespace)` - if the ID contains a namespace prefix
/// * `None` - if the ID does not contain a namespace prefix
///
/// # Examples
///
/// ```
/// use juncture_core::interrupt::extract_namespace;
///
/// assert_eq!(extract_namespace("agent:review#0"), Some("agent"));
/// assert_eq!(extract_namespace("node_name#index"), None);
/// assert_eq!(extract_namespace("simple"), None);
/// ```
#[must_use]
pub fn extract_namespace(interrupt_id: &str) -> Option<&str> {
    // Check for namespace:node format
    if let Some(colon_pos) = interrupt_id.find(':') {
        // Ensure there's content before the colon
        if colon_pos > 0 {
            return Some(&interrupt_id[..colon_pos]);
        }
    }
    None
}

/// Validate that resume values cover all pending interrupts
///
/// Ensures that each pending interrupt has a corresponding resume value
/// provided. Returns an error with a list of uncovered interrupt IDs if
/// any are missing.
///
/// # Arguments
///
/// * `pending` - Slice of pending interrupt signals
/// * `resume_values` - Map of interrupt IDs to their resume values
///
/// # Returns
///
/// * `Ok(())` - All pending interrupts have resume values
/// * `Err(Vec<String>)` - List of interrupt IDs without resume values
///
/// # Errors
///
/// Returns an error with a list of interrupt IDs that don't have
/// corresponding resume values if any pending interrupts are uncovered.
///
/// # Examples
///
/// ```
/// use juncture_core::interrupt::{validate_resume_coverage, InterruptSignal};
/// use serde_json::json;
/// use std::collections::HashMap;
/// use chrono::Utc;
///
/// let pending = vec![
///     InterruptSignal {
///         index: 0,
///         id: Some("int-1".to_string()),
///         payload: json!({}),
///         timestamp: Utc::now(),
///     }
/// ];
/// let mut resume_values = HashMap::new();
/// resume_values.insert("int-1".to_string(), json!("value"));
///
/// assert!(validate_resume_coverage(&pending, &resume_values).is_ok());
/// ```
#[expect(
    clippy::implicit_hasher,
    reason = "accepting standard HashMap is fine for this use case"
)]
#[expect(
    clippy::collapsible_if,
    reason = "nested if is more readable for checking conditions in sequence"
)]
pub fn validate_resume_coverage(
    pending: &[InterruptSignal],
    resume_values: &HashMap<String, serde_json::Value>,
) -> Result<(), Vec<String>> {
    let mut uncovered = Vec::new();

    for signal in pending {
        if let Some(ref id) = signal.id {
            if !resume_values.contains_key(id) {
                uncovered.push(id.clone());
            }
        }
    }

    if uncovered.is_empty() {
        Ok(())
    } else {
        Err(uncovered)
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
/// A node is considered hidden when either:
/// 1. Its name both starts and ends with `__` (e.g., `__route__`, `__internal__`)
/// 2. It has the `__hidden__` tag in its tags list
///
/// This follows the convention established by `LangGraph`'s `TAG_HIDDEN` mechanism.
///
/// Hidden nodes are filtered from:
/// - `interrupt_before` / `interrupt_after` checks via [`should_interrupt`]
/// - `StreamEvent::Interrupt` emission in the Pregel loop
///
/// # Arguments
///
/// * `node_name` - The name of the node to check
/// * `tags` - The tags associated with the node
///
/// # Examples
///
/// ```
/// use juncture_core::interrupt::is_hidden_node;
///
/// // Hidden by name pattern
/// assert!(is_hidden_node("__route__", &[]));
/// assert!(is_hidden_node("__internal_router__", &[]));
/// assert!(!is_hidden_node("my_node", &[]));
/// assert!(!is_hidden_node("__incomplete", &[]));
/// assert!(!is_hidden_node("normal__", &[]));
///
/// // Hidden by tag
/// assert!(is_hidden_node("my_node", &vec!["__hidden__".to_string()]));
/// assert!(!is_hidden_node("my_node", &vec!["other_tag".to_string()]));
/// ```
#[must_use]
pub fn is_hidden_node(node_name: &str, tags: &[String]) -> bool {
    let is_hidden_by_name =
        node_name.starts_with("__") && node_name.ends_with("__") && node_name.len() > 4;
    let is_hidden_by_tag = tags.iter().any(|tag| tag == HIDDEN_TAG);
    is_hidden_by_name || is_hidden_by_tag
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
        // PendingTask doesn't have a tags field yet, so pass empty slice
        let tags: &[String] = &[];

        // Hidden nodes (names starting/ending with __ or with __hidden__ tag) are internal
        // infrastructure and must never surface as interrupts.
        if is_hidden_node(node_name, tags) {
            continue;
        }

        if interrupt_before.contains(node_name) {
            let timestamp = Utc::now();
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(node_name, signals.len())),
                payload: serde_json::json!({
                    "node": node_name,
                    "reason": "interrupt_before",
                }),
                timestamp,
            });
        }

        if interrupt_after.contains(node_name) {
            let timestamp = Utc::now();
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(node_name, signals.len())),
                payload: serde_json::json!({
                    "node": node_name,
                    "reason": "interrupt_after",
                }),
                timestamp,
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
        timestamp: Utc::now(),
    })
    .map_err(|_err| crate::JunctureError::execution("interrupt channel closed"))?;

    Err(crate::JunctureError::interrupted(index))
}

/// Scratchpad for interrupt handling and transient data storage
///
/// Used by the HITL system to track processed interrupts, maintain
/// an audit trail, and store transient data during interrupt handling.
#[derive(Clone, Debug, Default)]
pub struct Scratchpad {
    /// Set of interrupt IDs that have been processed
    processed_interrupts: HashSet<String>,

    /// Transient data storage for interrupt handling
    data: HashMap<String, serde_json::Value>,

    /// Audit trail of all interrupts
    interrupt_history: Vec<InterruptRecord>,
}

impl Scratchpad {
    /// Create a new empty scratchpad
    #[must_use]
    pub fn new() -> Self {
        Self {
            processed_interrupts: HashSet::new(),
            data: HashMap::new(),
            interrupt_history: Vec::new(),
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

    /// Record an interrupt event in the audit trail
    ///
    /// Creates a new record with the current timestamp and adds it to
    /// the interrupt history.
    ///
    /// # Arguments
    ///
    /// * `id` - The interrupt ID
    /// * `node` - The node name where the interrupt occurred
    /// * `payload` - The interrupt payload
    pub fn record_interrupt(&mut self, id: String, node: String, payload: serde_json::Value) {
        let record = InterruptRecord {
            id,
            node,
            payload,
            timestamp: Utc::now(),
            resumed_at: None,
            resume_value: None,
        };
        self.interrupt_history.push(record);
    }

    /// Record that an interrupt was resumed
    ///
    /// Finds the interrupt record by ID and updates it with the resume
    /// timestamp and value.
    ///
    /// # Arguments
    ///
    /// * `id` - The interrupt ID to mark as resumed
    /// * `value` - The resume value provided
    pub fn record_resume(&mut self, id: &str, value: serde_json::Value) {
        if let Some(record) = self.interrupt_history.iter_mut().find(|r| r.id == id) {
            record.resumed_at = Some(Utc::now());
            record.resume_value = Some(value);
        }
    }

    /// Get the complete interrupt history
    ///
    /// Returns all interrupt records in chronological order.
    ///
    /// # Returns
    ///
    /// A slice of all interrupt records
    #[must_use]
    pub fn interrupt_history(&self) -> &[InterruptRecord] {
        &self.interrupt_history
    }

    /// Clear transient scratchpad entries
    ///
    /// Removes entries that are not persistent (entries not prefixed with
    /// `null_resume:`). Persistent entries are preserved.
    pub fn clear_transient(&mut self) {
        self.data
            .retain(|key, _value| key.starts_with("null_resume:"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Scratchpad tests ---

    #[test]
    fn scratchpad_get_null_resume() {
        let mut pad = Scratchpad::new();
        assert!(!pad.get_null_resume("int-1"));
        pad.mark_interrupt_processed("int-1");
        assert!(pad.get_null_resume("int-1"));
        assert!(!pad.get_null_resume("int-2"));
    }

    #[test]
    fn scratchpad_record_interrupt() {
        let mut pad = Scratchpad::new();
        pad.record_interrupt(
            "int-1".to_string(),
            "node_a".to_string(),
            serde_json::json!({"reason": "test"}),
        );

        let history = pad.interrupt_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, "int-1");
        assert_eq!(history[0].node, "node_a");
        assert_eq!(history[0].payload["reason"], "test");
        assert!(history[0].resumed_at.is_none());
        assert!(history[0].resume_value.is_none());
    }

    #[test]
    fn scratchpad_record_resume() {
        let mut pad = Scratchpad::new();
        pad.record_interrupt(
            "int-1".to_string(),
            "node_a".to_string(),
            serde_json::json!({}),
        );

        pad.record_resume("int-1", serde_json::json!("approved"));

        let history = pad.interrupt_history();
        assert_eq!(history.len(), 1);
        assert!(history[0].resumed_at.is_some());
        assert_eq!(history[0].resume_value, Some(serde_json::json!("approved")));
    }

    #[test]
    fn scratchpad_interrupt_history_order() {
        let mut pad = Scratchpad::new();

        pad.record_interrupt(
            "int-1".to_string(),
            "node_a".to_string(),
            serde_json::json!({}),
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        pad.record_interrupt(
            "int-2".to_string(),
            "node_b".to_string(),
            serde_json::json!({}),
        );

        let history = pad.interrupt_history();
        assert_eq!(history.len(), 2);
        assert!(history[0].timestamp < history[1].timestamp);
    }

    #[test]
    fn scratchpad_clear_transient() {
        let mut pad = Scratchpad::new();
        pad.set_data("temp_key".to_string(), serde_json::json!("temp"));
        pad.set_data(
            "null_resume:persistent".to_string(),
            serde_json::json!("keep"),
        );

        pad.clear_transient();

        assert!(pad.get_data("temp_key").is_none());
        assert_eq!(
            pad.get_data("null_resume:persistent"),
            Some(&serde_json::json!("keep"))
        );
    }

    #[test]
    fn scratchpad_clear_transient_empty() {
        let mut pad = Scratchpad::new();
        pad.clear_transient();
        assert!(pad.data.is_empty());
    }

    #[test]
    fn scratchpad_record_resume_nonexistent() {
        let mut pad = Scratchpad::new();
        // Recording resume for non-existent interrupt should be safe (no-op)
        pad.record_resume("nonexistent", serde_json::json!("value"));
        assert_eq!(pad.interrupt_history().len(), 0);
    }

    // --- extract_namespace tests ---

    #[test]
    fn extract_namespace_with_namespace() {
        assert_eq!(extract_namespace("agent:review#0"), Some("agent"));
        assert_eq!(extract_namespace("namespace:node#index"), Some("namespace"));
    }

    #[test]
    fn extract_namespace_without_namespace() {
        assert_eq!(extract_namespace("node_name#index"), None);
        assert_eq!(extract_namespace("simple_id"), None);
        assert_eq!(extract_namespace("no_colon"), None);
    }

    #[test]
    fn extract_namespace_empty_namespace() {
        assert_eq!(extract_namespace(":node#index"), None);
        assert_eq!(extract_namespace(":only_colon"), None);
    }

    // --- validate_resume_coverage tests ---

    #[test]
    fn validate_resume_coverage_complete() {
        let pending = vec![InterruptSignal {
            index: 0,
            id: Some("int-1".to_string()),
            payload: serde_json::json!({}),
            timestamp: Utc::now(),
        }];

        let mut resume_values = HashMap::new();
        resume_values.insert("int-1".to_string(), serde_json::json!("value"));

        validate_resume_coverage(&pending, &resume_values).unwrap();
    }

    #[test]
    fn validate_resume_coverage_incomplete() {
        let pending = vec![
            InterruptSignal {
                index: 0,
                id: Some("int-1".to_string()),
                payload: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            InterruptSignal {
                index: 1,
                id: Some("int-2".to_string()),
                payload: serde_json::json!({}),
                timestamp: Utc::now(),
            },
        ];

        let mut resume_values = HashMap::new();
        resume_values.insert("int-1".to_string(), serde_json::json!("value"));

        let result = validate_resume_coverage(&pending, &resume_values);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), vec!["int-2".to_string()]);
    }

    #[test]
    fn validate_resume_coverage_empty_pending() {
        let pending = vec![];
        let resume_values = HashMap::new();

        validate_resume_coverage(&pending, &resume_values).unwrap();
    }

    #[test]
    fn validate_resume_coverage_no_id() {
        let pending = vec![InterruptSignal {
            index: 0,
            id: None,
            payload: serde_json::json!({}),
            timestamp: Utc::now(),
        }];

        let resume_values = HashMap::new();

        // Interrupts without ID are skipped in validation
        validate_resume_coverage(&pending, &resume_values).unwrap();
    }

    #[test]
    fn validate_resume_coverage_multiple_uncovered() {
        let pending = vec![
            InterruptSignal {
                index: 0,
                id: Some("int-1".to_string()),
                payload: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            InterruptSignal {
                index: 1,
                id: Some("int-2".to_string()),
                payload: serde_json::json!({}),
                timestamp: Utc::now(),
            },
            InterruptSignal {
                index: 2,
                id: Some("int-3".to_string()),
                payload: serde_json::json!({}),
                timestamp: Utc::now(),
            },
        ];

        let resume_values = HashMap::new();

        let result = validate_resume_coverage(&pending, &resume_values);
        assert!(result.is_err());
        let uncovered = result.unwrap_err();
        assert_eq!(uncovered.len(), 3);
        assert!(uncovered.contains(&"int-1".to_string()));
        assert!(uncovered.contains(&"int-2".to_string()));
        assert!(uncovered.contains(&"int-3".to_string()));
    }

    // --- is_hidden_node tests ---

    #[test]
    fn hidden_node_double_underscore_prefix_and_suffix() {
        assert!(is_hidden_node("__route__", &[]));
        assert!(is_hidden_node("__internal__", &[]));
        assert!(is_hidden_node("__error_handler__", &[]));
    }

    #[test]
    fn normal_nodes_are_not_hidden() {
        assert!(!is_hidden_node("my_node", &[]));
        assert!(!is_hidden_node("agent", &[]));
        assert!(!is_hidden_node("review", &[]));
    }

    #[test]
    fn partial_underscore_prefix_is_not_hidden() {
        assert!(!is_hidden_node("__incomplete", &[]));
        assert!(!is_hidden_node("__only_start", &[]));
    }

    #[test]
    fn partial_underscore_suffix_is_not_hidden() {
        assert!(!is_hidden_node("only_end__", &[]));
        assert!(!is_hidden_node("incomplete__", &[]));
    }

    #[test]
    fn bare_double_underscore_is_not_hidden() {
        // "____" is only underscores and too short to be a meaningful hidden name
        assert!(!is_hidden_node("____", &[]));
    }

    #[test]
    fn hidden_tag_constant_value() {
        assert_eq!(HIDDEN_TAG, "__hidden__");
    }

    #[test]
    fn hidden_node_by_tag() {
        // Node marked with HIDDEN_TAG should be hidden even with normal name
        assert!(is_hidden_node("my_node", &["__hidden__".to_string()]));
        assert!(is_hidden_node(
            "agent",
            &["__hidden__".to_string(), "other".to_string()]
        ));
    }

    #[test]
    fn hidden_node_by_tag_only_when_exact_match() {
        // Similar tags should not hide the node
        assert!(!is_hidden_node("my_node", &["_hidden_".to_string()]));
        assert!(!is_hidden_node("my_node", &["hidden".to_string()]));
        assert!(!is_hidden_node("my_node", &["__hidden".to_string()]));
        assert!(!is_hidden_node("my_node", &["hidden__".to_string()]));
    }

    #[test]
    fn hidden_node_by_name_or_tag() {
        // Either name pattern OR tag should hide the node
        assert!(is_hidden_node("__internal__", &[])); // Hidden by name
        assert!(is_hidden_node("normal_node", &["__hidden__".to_string()])); // Hidden by tag
        assert!(is_hidden_node("__internal__", &["__hidden__".to_string()])); // Both
    }

    #[test]
    fn normal_node_without_tag_not_hidden() {
        assert!(!is_hidden_node("my_node", &[]));
        assert!(!is_hidden_node("my_node", &["other_tag".to_string()]));
        assert!(!is_hidden_node(
            "my_node",
            &["tag1".to_string(), "tag2".to_string()]
        ));
    }

    // --- should_interrupt filtering tests ---

    /// Minimal `State` impl for testing `should_interrupt`.
    #[derive(Clone, Debug, serde::Serialize)]
    struct TestState;

    impl crate::State for TestState {
        type Update = TestUpdate;
        type FieldVersions = crate::state::FieldVersions;

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
