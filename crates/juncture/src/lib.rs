// Juncture facade crate
//
// This crate re-exports the core functionality and provides
// convenient prelude, builder patterns, and LLM integration.

pub mod llm;
pub mod prebuilt;
pub mod tools;

pub mod prelude {
    pub use juncture_core::*;
}

// Re-export core types
pub use juncture_core::*;

// Rust guideline compliant 2026-05-19
