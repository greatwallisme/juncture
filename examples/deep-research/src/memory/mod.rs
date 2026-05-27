//! Memory integration for persistent cross-session research facts.

mod conversation;
mod extractor;
mod store;

// Memory components are exported and available for use
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use conversation::ConversationTracker;
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use extractor::ResearchFactExtractor;
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use store::FactStore;

// Rust guideline compliant 2026-05-27
