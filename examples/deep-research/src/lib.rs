//! Deep research application library.
//!
//! Multi-agent research assistant built with Juncture framework.

pub mod agents;
pub mod config;
pub mod llm;
pub mod memory;
pub mod orchestrator;
pub mod permissions;
pub mod state;
pub mod tools;

// Re-export commonly used types
pub use config::ResearchConfig;
pub use state::{Finding, ResearchState, SubTask, TaskStatus};

// Rust guideline compliant 2026-05-27
