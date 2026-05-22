//! Cross-thread persistent key-value storage for Juncture.
//!
//! This crate re-exports the Store types from `juncture-core::store` so that
//! consumers can depend on `juncture-store` directly without pulling in the
//! full `juncture-core` facade.
//!
//! All types originate from the authoritative implementation in
//! `juncture-core::store`. There are no duplicate definitions.
//!
//! # Re-exported items
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Store`] | Async key-value store trait |
//! | [`StoreError`] | Error type for store operations |
//! | [`Item`] | Stored item with metadata |
//! | [`FilterExpr`] | Filter expression for search queries |
//! | [`SearchQuery`] | Search query builder |
//! | [`SearchResult`] | Search result set |
//! | [`SearchItem`] | Search result item with optional score |
//! | [`StoreOp`] | Batch operation type |
//! | [`StoreResult`] | Batch operation result |
//! | [`MemoryStore`] | In-memory store implementation |
//! | [`TTLConfig`] | Time-to-live configuration |
//! | [`IndexConfig`] | Vector index configuration |
//! | [`EmbeddingFunc`] | Embedding generation trait |
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

pub use juncture_core::store::{
    EmbeddingFunc, FilterExpr, IndexConfig, Item, MemoryStore, SearchItem, SearchQuery,
    SearchResult, Store, StoreError, StoreOp, StoreResult, TTLConfig,
};

// Rust guideline compliant 2026-05-22
