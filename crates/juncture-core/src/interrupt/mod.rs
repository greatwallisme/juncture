//! Human-in-the-Loop (HITL) support
//!
//! This module provides interrupt mechanisms for pausing graph execution
//! to await human input before continuing.

mod context;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
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
#[derive(Clone, Debug)]
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
pub const HIDDEN_TAG: &str = "__hidden__";

/// Generate a deterministic interrupt ID from node name and index
///
/// Uses xxhash for fast, deterministic ID generation based on the
/// node name and index.
///
/// # Arguments
///
/// * `node_name` - The node name
/// * `index` - The interrupt index
///
/// # Returns
///
/// A hexadecimal string representing the 128-bit hash (32 characters)
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt::generate_interrupt_id;
///
/// let id = generate_interrupt_id("my_node", 0);
/// assert!(id.len() == 32); // xxh3 128-bit hash produces 32-character hex string
/// ```
#[must_use]
pub fn generate_interrupt_id(node_name: &str, index: usize) -> String {
    let mut hasher = Xxh3::new();
    node_name.hash(&mut hasher);
    index.hash(&mut hasher);
    // Use finish() twice to get 128-bit result (finish128 is not available in this version)
    let hash1 = hasher.finish();
    let mut hasher2 = Xxh3::new();
    node_name.hash(&mut hasher2);
    index.hash(&mut hasher2);
    hasher2.write_u8(1); // Add differentiator to get different second hash
    let hash2 = hasher2.finish();
    format!("{hash1:016x}{hash2:016x}")
}

/// Check if execution should interrupt based on the current state
///
/// Two-step check:
/// 1. **Version gating**: Only fire if any channel was updated since the last
///    interrupt (comparing `channel_versions` against `versions_seen`).
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
/// * `versions_seen` - Last-seen versions at the time of the previous interrupt
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
    versions_seen: &HashMap<String, Vec<u64>>,
) -> Option<Vec<InterruptSignal>> {
    // Step 1: Version gate -- skip interrupt if no channels updated since last
    let any_updates = channel_versions.iter().any(|(chan, ver)| {
        let max_seen: u64 = versions_seen
            .get(chan)
            .map_or(0, |vers| vers.iter().copied().max().unwrap_or(0));
        ver > &max_seen
    });

    if !any_updates && !versions_seen.is_empty() {
        return None;
    }

    // Step 2: Node name check
    let mut signals = Vec::new();

    for task in pending_tasks {
        let node_name = &task.node_name;

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

// Rust guideline compliant 2026-05-20
