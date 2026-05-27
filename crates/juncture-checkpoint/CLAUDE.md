# CLAUDE.md -- juncture-checkpoint

Checkpoint persistence for Juncture graph executions. Provides save/restore of complete execution state for time-travel debugging, crash recovery, and HITL workflows.

## Structure

```
src/
  lib.rs       -- crate root, re-exports
  memory.rs    -- MemorySaver implementation
  types.rs     -- DeltaSnapshot, ChannelDelta, TtlConfig, recover_from_deltas()
  serde.rs     -- JsonSerializer, MsgpackSerializer, JsonPlusSerializer, EncryptedSerializer
  cache.rs     -- BaseCache, MemoryCache for checkpoint caching
  error.rs     -- CheckpointError enum
  sqlite.rs    -- SqliteSaver (feature `sqlite`)
  postgres.rs  -- PostgresSaver (feature `postgres`)
```

## Key Types

- `CheckpointSaver` trait is defined in `juncture-core::checkpoint` and re-exported here
- `MemorySaver` stores: `thread_id -> checkpoint_ns -> Vec<CheckpointTuple>` (sorted by created_at DESC)
- Pending writes: `(thread_id, checkpoint_id, checkpoint_ns) -> Vec<PendingWrite>`
- `TtlConfig` controls checkpoint expiration; `MemorySaver::with_ttl_config()` sets TTL, `lazy_cleanup()` garbage-collects expired entries
- `recover_from_deltas()` implements ancestor-walk recovery from delta-only checkpoints
- Serialization backends: `JsonSerializer`, `MsgpackSerializer`, `JsonPlusSerializer`; `detect_format()` auto-detects
- `EncryptedSerializer` (feature-gated `encryption`) using AES-GCM + PBKDF2
- `migrate_checkpoint_schema()` for schema version validation on load (sqlite, postgres)

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
