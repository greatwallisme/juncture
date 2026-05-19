//! Cross-thread persistent key-value storage for Juncture.
//!
//! This crate provides a flexible, async key-value store implementation with support for:
//!
//! - Thread-safe in-memory storage with `Arc<RwLock<>>`
//! - Namespace-based key organization
//! - Filter-based search queries
//! - Time-to-live (TTL) with background cleanup
//! - Optional vector similarity search (feature-gated)
//! - Batch operations for efficiency
//!
//! # Example
//!
//! ```no_run
//! use juncture_store::{MemoryStore, Store};
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = MemoryStore::new();
//!
//!     // Store a value
//!     store.put(
//!         "checkpoint",
//!         "run_1",
//!         json!({"step": 5, "data": "example"}),
//!         None,
//!     ).await?;
//!
//!     // Retrieve it
//!     let item = store.get("checkpoint", "run_1").await?;
//!     assert!(item.is_some());
//!
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod filter;
pub mod memory;
pub mod trait_;
pub mod types;

pub use error::StoreError;
pub use filter::FilterExpr;
pub use memory::MemoryStore;
pub use trait_::Store;
pub use types::{
    EmbeddingFunc, IndexConfig, Item, SearchItem, SearchQuery, SearchResult, StoreOp, StoreResult,
    TTLConfig,
};

// Rust guideline compliant 2026-05-19
