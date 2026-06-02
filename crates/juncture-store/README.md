# Juncture Store

[![Crates.io](https://img.shields.io/crates/v/juncture-store.svg)](https://crates.io/crates/juncture-store)
[![Documentation](https://docs.rs/juncture-store/badge.svg)](https://docs.rs/juncture-store)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Cross-thread persistent key-value storage for Juncture applications. This crate re-exports the Store types from `juncture-core::store` for convenient access.

## Re-exported Types

| Type | Description |
|------|-------------|
| `Store` | Storage trait |
| `MemoryStore` | In-memory implementation |
| `Item` | Storage item |
| `FilterExpr` | Query filters |
| `SearchQuery` | Search parameters |
| `TTLConfig` | Time-to-live configuration |
| `IndexConfig` | Index configuration |
| `EmbeddingFunc` | Vector embedding function |

## Usage

```rust
use juncture_store::{MemoryStore, Store};

// Create an in-memory store
let store = MemoryStore::new();

// Store items
store.put("key1", serde_json::json!({"value": 42})).await?;

// Retrieve items
let item = store.get("key1").await?;
```

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
