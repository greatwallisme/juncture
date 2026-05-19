//! Juncture checkpoint persistence
//!
//! This crate provides checkpoint persistence for Juncture state machine executions.
//! It enables time-travel debugging, crash recovery, and human-in-the-loop workflows.
//!
//! # Overview
//!
//! Checkpoint persistence captures the complete state of graph execution at specific points,
//! allowing execution to be paused, resumed, or rolled back to any previous state.
//!
//! # Core Components
//!
//! - [`juncture_core::checkpoint::CheckpointSaver`]: Trait defining checkpoint storage operations
//! - [`MemorySaver`]: In-memory implementation for development/testing
//! - [`juncture_core::checkpoint::Checkpoint`]: Complete execution state snapshot
//! - [`juncture_core::checkpoint::CheckpointMetadata`]: Execution context and provenance
//!
//! # Example
//!
//! ```ignore
//! use juncture_checkpoint::MemorySaver;
//! use juncture_core::{RunnableConfig, checkpoint::CheckpointSaver};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let saver = MemorySaver::new();
//!     let config = RunnableConfig::default().with_thread_id("my-thread");
//!
//!     // Save a checkpoint
//!     let checkpoint = /* create checkpoint */;
//!     let metadata = /* create metadata */;
//!     let updated_config = saver.put(&config, checkpoint, metadata).await?;
//!
//!     // Retrieve latest checkpoint
//!     let tuple = saver.get_tuple(&updated_config).await?.unwrap();
//!
//!     Ok(())
//! }
//! ```

pub mod cache;
pub mod error;
pub mod memory;
pub mod serde;
pub mod types;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "sqlite")]
pub mod sqlite;

// Public re-exports
pub use cache::{BaseCache, MemoryCache};
pub use error::CheckpointError;
pub use memory::MemorySaver;

#[cfg(feature = "postgres")]
pub use postgres::PostgresSaver;

pub use serde::{
    CheckpointSerializer, JsonPlusSerializer, JsonSerializer, MsgpackSerializer,
    SerializationFormat, detect_format,
};

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteSaver;

// Re-export CheckpointSaver from juncture-core for convenience
pub use juncture_core::checkpoint::CheckpointSaver;

#[cfg(feature = "encryption")]
pub use serde::EncryptedSerializer;

pub use types::*;

// Rust guideline compliant 2026-05-19
