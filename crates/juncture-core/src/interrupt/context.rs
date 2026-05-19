use crate::interrupt::InterruptSignal;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// Interrupt context for managing human-in-the-flow interactions
///
/// The context tracks interrupt state across node executions, enabling
/// resumption with human-provided values.
///
/// # Examples
///
/// ```ignore
/// use juncture_core::interrupt::InterruptContext;
/// use serde_json::json;
///
/// let mut context = InterruptContext::new(
///     vec![Some(json!("human_input"))],
///     tokio::sync::mpsc::unbounded_channel(),
/// );
///
/// // Get next interrupt index (increments counter)
/// let index = context.next_index();
/// ```
#[derive(Clone, Debug)]
pub struct InterruptContext {
    /// Resume values indexed by interrupt position
    resume_values: Arc<[Option<serde_json::Value>]>,

    /// Current interrupt index counter
    current_index: Arc<AtomicUsize>,

    /// Channel for sending interrupt signals
    interrupt_tx: mpsc::UnboundedSender<InterruptSignal>,
}

impl InterruptContext {
    /// Create a new interrupt context
    ///
    /// # Arguments
    ///
    /// * `resume_values` - Values to resume interrupts with (indexed by position)
    /// * `interrupt_tx` - Channel for sending interrupt signals
    #[must_use]
    pub fn new(
        resume_values: Vec<Option<serde_json::Value>>,
        interrupt_tx: mpsc::UnboundedSender<InterruptSignal>,
    ) -> Self {
        Self {
            resume_values: resume_values.into_boxed_slice().into(),
            current_index: Arc::new(AtomicUsize::new(0)),
            interrupt_tx,
        }
    }

    /// Get the next interrupt index (atomically increments counter)
    #[must_use]
    pub fn next_index(&self) -> usize {
        self.current_index.fetch_add(1, Ordering::Relaxed)
    }

    /// Get resume value for a given index
    ///
    /// Returns `None` if no resume value exists for this index.
    #[must_use]
    pub fn get_resume_value(&self, index: usize) -> Option<serde_json::Value> {
        self.resume_values
            .get(index)
            .and_then(std::clone::Clone::clone)
    }

    /// Get the current index without incrementing
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.current_index.load(Ordering::Relaxed)
    }

    /// Send an interrupt signal
    ///
    /// # Errors
    ///
    /// Returns an error if the interrupt channel is closed.
    pub fn send_interrupt(
        &self,
        signal: InterruptSignal,
    ) -> Result<(), mpsc::error::SendError<InterruptSignal>> {
        self.interrupt_tx.send(signal)
    }
}

// Rust guideline compliant 2025-01-18
