//! Research agent tools: web search, calculator, file I/O, and memory search.

mod calculator;
mod file_io;
mod memory_search;
mod web_search;

// Tools are exported and available for use
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use calculator::Calculator;
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use file_io::ReadFile;
#[allow(
    unused_imports,
    reason = "Public API - may be used by library consumers"
)]
pub use memory_search::MemorySearch;
pub use web_search::WebSearch;

// Rust guideline compliant 2026-05-27
