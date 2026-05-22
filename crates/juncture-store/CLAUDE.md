# CLAUDE.md -- juncture-store

Thin re-export layer for cross-thread persistent key-value storage.

## Architecture

This crate has **no own implementations**. It re-exports all Store types from
`juncture-core::store` so that downstream consumers can depend on `juncture-store`
directly without pulling in the full `juncture-core` facade.

All types originate from `juncture-core::store`:

| Re-exported type | Source |
|-----------------|--------|
| `Store` | `juncture_core::store::Store` |
| `StoreError` | `juncture_core::store::StoreError` |
| `Item` | `juncture_core::store::Item` |
| `FilterExpr` | `juncture_core::store::FilterExpr` |
| `SearchQuery` | `juncture_core::store::SearchQuery` |
| `SearchResult` | `juncture_core::store::SearchResult` |
| `SearchItem` | `juncture_core::store::SearchItem` |
| `StoreOp` | `juncture_core::store::StoreOp` |
| `StoreResult` | `juncture_core::store::StoreResult` |
| `MemoryStore` | `juncture_core::store::MemoryStore` |
| `TTLConfig` | `juncture_core::store::TTLConfig` |
| `IndexConfig` | `juncture_core::store::IndexConfig` |
| `EmbeddingFunc` | `juncture_core::store::EmbeddingFunc` |

## Dependencies

- `juncture-core` -- the sole dependency, providing all Store types

## Testing

```bash
cargo test -p juncture-store
```

Tests run from `juncture-core::store` module tests. This crate has no own tests
since it contains only re-exports.
