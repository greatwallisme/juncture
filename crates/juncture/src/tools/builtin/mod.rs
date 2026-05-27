//! Built-in tools for common agent patterns.
//!
//! This module provides ready-to-use tool implementations that follow
//! established patterns from reference projects (deer-flow, deepagents).
//!
//! # Available Tools
//!
//! - [`ThinkTool`] — Strategic reflection for research agents
//! - [`WebFetchTool`] — Full webpage content fetching (requires `reqwest` feature)

mod think;

#[cfg(feature = "reqwest")]
mod web_fetch;

pub use think::ThinkTool;

#[cfg(feature = "reqwest")]
pub use web_fetch::WebFetchTool;

// Rust guideline compliant 2026-05-27
