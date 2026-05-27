//! Memory integration for persistent cross-session research facts.

mod conversation;
mod extractor;
mod store;

#[allow(
    unused_imports,
    dead_code,
    reason = "Intentionally deferred - conversation tracking requires orchestrator integration"
)]
pub use conversation::ConversationTracker;
pub use extractor::ResearchFactExtractor;
pub use store::FactStore;

// Rust guideline compliant 2026-05-27
