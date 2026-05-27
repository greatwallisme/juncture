//! Research agent tools: web search, calculator, and file I/O.

mod calculator;
mod file_io;
mod web_search;

pub use calculator::Calculator;
pub use file_io::ReadFile;
pub use web_search::WebSearch;

// Rust guideline compliant 2026-05-27
