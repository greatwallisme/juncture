# CLAUDE.md -- juncture-store

Cross-thread persistent key-value storage for Juncture. Provides the `Store` trait and in-memory implementation.

## Module Map

| Module | Responsibility |
|--------|---------------|
| `trait_.rs` | `Store` async trait: `get`, `put`, `delete`, `search`, `list_namespaces`, `batch` |
| `memory.rs` | `MemoryStore` -- `Arc<RwLock<HashMap>>` based in-memory implementation |
| `filter.rs` | `FilterExpr` for search queries |
| `types.rs` | `Item`, `SearchItem`, `SearchQuery`, `SearchResult`, `StoreOp`, `StoreResult`, `IndexConfig`, `EmbeddingFunc`, `TTLConfig` |
| `error.rs` | `StoreError` enum |

## Key Design

- Namespace-based key organization: `namespace/key` pairs
- Async trait (`async_trait`) for all operations
- `SearchQuery` supports filter-based search with optional vector similarity (feature-gated)
- `StoreOp` batch operations for efficiency
- TTL support via `TTLConfig` with background cleanup

## Features

- `vector` -- placeholder for vector similarity search (not yet implemented)

## Testing

```bash
cargo test -p juncture-store
```

This crate has no dependency on `juncture-core`. It is optional in the facade crate via the `store` feature flag.
