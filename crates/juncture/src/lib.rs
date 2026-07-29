// Juncture facade crate
//
// This crate re-exports the core functionality and provides
// convenient prelude, builder patterns, and LLM integration.

pub mod llm;
pub mod memory;
pub mod prebuilt;
pub mod tools;

pub mod prelude {
    pub use juncture_core::*;
}

// Re-export core types
pub use juncture_core::*;

// Re-export the functional-API attribute macros (design `03-pregel-engine`
// §13.3) so users can write `use juncture::{entrypoint, task};`.
pub use juncture_derive::{entrypoint, task};

// Rust guideline compliant 2026-07-29
