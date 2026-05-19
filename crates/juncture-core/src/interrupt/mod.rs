//! Human-in-the-Loop (HITL) support
//!
//! This module provides interrupt mechanisms for pausing graph execution
//! to await human input before continuing.

mod context;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use xxhash_rust::xxh3::Xxh3;

pub use context::InterruptContext;

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

/// Generate a deterministic interrupt ID from a payload
///
/// Uses xxhash for fast, deterministic ID generation based on the
/// interrupt payload content.
///
/// # Arguments
///
/// * `payload` - The interrupt payload as JSON value
///
/// # Returns
///
/// A hexadecimal string representing the hash of the payload
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt::generate_interrupt_id;
/// use serde_json::json;
///
/// let payload = json!({"user_input": "continue"});
/// let id = generate_interrupt_id(&payload);
/// assert!(id.len() == 16); // xxh3 produces 16-character hex string
/// ```
#[must_use]
pub fn generate_interrupt_id(payload: &serde_json::Value) -> String {
    let mut hasher = Xxh3::new();
    payload.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:016x}")
}

/// Check if execution should interrupt based on the current state
///
/// Examines pending tasks and compares against configured interrupt
/// triggers to determine if execution should pause.
///
/// # Arguments
///
/// * `pending_tasks` - Tasks scheduled for the next superstep
/// * `interrupt_before` - Nodes that should interrupt before execution
/// * `interrupt_after` - Nodes that should interrupt after execution
///
/// # Returns
///
/// `Some(Vec<InterruptSignal>)` if interruption is needed, `None` otherwise
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt::should_interrupt;
/// use std::collections::HashSet;
///
/// let mut before = HashSet::new();
/// before.insert("human_review".to_string());
///
/// let signals = should_interrupt(&pending_tasks, &before, &HashSet::new());
/// if let Some(signals) = signals {
///     // Handle interrupt
/// }
/// ```
#[allow(
    clippy::implicit_hasher,
    reason = "accepting standard HashSet is fine for this use case"
)]
#[must_use]
pub fn should_interrupt<S: crate::State>(
    pending_tasks: &[crate::PendingTask<S>],
    interrupt_before: &HashSet<String>,
    interrupt_after: &HashSet<String>,
) -> Option<Vec<InterruptSignal>> {
    let mut signals = Vec::new();

    for task in pending_tasks {
        let node_name = &task.node_name;

        // Check if node is in interrupt_before
        if interrupt_before.contains(node_name) {
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(
                    &serde_json::json!({"node": node_name, "trigger": "before"}),
                )),
                payload: serde_json::json!({
                    "node": node_name,
                    "reason": "interrupt_before",
                }),
            });
        }

        // Check if node is in interrupt_after
        if interrupt_after.contains(node_name) {
            signals.push(InterruptSignal {
                index: signals.len(),
                id: Some(generate_interrupt_id(
                    &serde_json::json!({"node": node_name, "trigger": "after"}),
                )),
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
            let mut hasher = Xxh3::new();
            "current_node".hash(&mut hasher);
            index.hash(&mut hasher);
            let hash = hasher.finish();
            format!("{hash:016x}")
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

// Rust guideline compliant 2026-05-19
