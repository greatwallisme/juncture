# Juncture Checkpoint

[![Crates.io](https://img.shields.io/crates/v/juncture-checkpoint.svg)](https://crates.io/crates/juncture-checkpoint)
[![Documentation](https://docs.rs/juncture-checkpoint/badge.svg)](https://docs.rs/juncture-checkpoint)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Checkpoint persistence for Juncture state machine executions. Provides save/restore of complete execution state for time-travel debugging, crash recovery, and HITL workflows.

## Features

- **Multiple Storage Backends**: Memory, SQLite, PostgreSQL
- **Serialization Formats**: JSON, MessagePack, JSON+ (with type info)
- **Encryption**: AES-GCM + PBKDF2 for sensitive checkpoints
- **TTL Support**: Automatic checkpoint expiration
- **Delta Recovery**: Recover from delta-only checkpoints

## Usage

```rust
use juncture_checkpoint::{MemorySaver, SqliteSaver};

// In-memory checkpoint saver (for testing)
let saver = MemorySaver::new();

// SQLite checkpoint saver (for persistence)
let saver = SqliteSaver::new("checkpoints.db").await?;

// Use with graph compilation
let graph = state_graph
    .compile_with_checkpointer(saver);
```

## Storage Backends

| Backend | Feature | Description |
|---------|---------|-------------|
| `MemorySaver` | (default) | In-memory storage, ideal for testing |
| `SqliteSaver` | `sqlite` | SQLite-backed persistence |
| `PostgresSaver` | `postgres` | PostgreSQL-backed persistence |

## Serialization

- `JsonSerializer` - Standard JSON serialization
- `MsgpackSerializer` - Binary MessagePack format
- `JsonPlusSerializer` - JSON with type information
- `EncryptedSerializer` - AES-GCM encryption (feature `encryption`)

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
