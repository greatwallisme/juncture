# CLAUDE.md -- juncture-checkpoint

Checkpoint persistence for Juncture graph executions. Provides save/restore of complete execution state for time-travel debugging, crash recovery, and HITL workflows.

## Module Map

| Module | Responsibility |
|--------|---------------|
| `memory.rs` | `MemorySaver` -- in-memory `CheckpointSaver` using `Arc<RwLock<HashMap>>`. Supports TTL via `with_ttl_config()` and lazy cleanup via `lazy_cleanup()`. For dev/testing. |
| `types.rs` | `DeltaSnapshot`, `ChannelDelta`, `TtlConfig` (with `is_expired()`), `recover_from_deltas()` ancestor-walk recovery. Re-exports checkpoint types from `juncture-core`. |
| `serde.rs` | Serialization backends: `JsonSerializer`, `MsgpackSerializer`, `JsonPlusSerializer`. `detect_format()` auto-detects. `EncryptedSerializer` (feature-gated). |
| `cache.rs` | `BaseCache` and `MemoryCache` for checkpoint caching. |
| `error.rs` | `CheckpointError` enum. |
| `sqlite.rs` | `SqliteSaver` (feature `sqlite`) -- persistent SQLite storage. Includes `migrate_checkpoint_schema()` for schema version validation on load. |
| `postgres.rs` | `PostgresSaver` (feature `postgres`) -- persistent Postgres storage. Same migration logic as sqlite. |

## Key Types

- `CheckpointSaver` trait is defined in `juncture-core::checkpoint` and re-exported here
- `MemorySaver` stores: `thread_id -> checkpoint_ns -> Vec<CheckpointTuple>` (sorted by created_at DESC)
- Pending writes: `(thread_id, checkpoint_id, checkpoint_ns) -> Vec<PendingWrite>`
- `TtlConfig` controls checkpoint expiration; `MemorySaver::with_ttl_config()` sets TTL, `lazy_cleanup()` garbage-collects expired entries
- `recover_from_deltas()` implements ancestor-walk recovery from delta-only checkpoints

## Features

- `sqlite` -- `SqliteSaver` backed by sqlx SQLite
- `postgres` -- `PostgresSaver` backed by sqlx Postgres
- `encryption` -- `EncryptedSerializer` using AES-GCM + PBKDF2

## Testing

```bash
cargo test -p juncture-checkpoint
cargo test -p juncture-checkpoint --features sqlite    # requires SQLite dev libs
cargo test -p juncture-checkpoint --features postgres   # requires Postgres
```
