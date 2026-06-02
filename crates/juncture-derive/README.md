# Juncture Derive

[![Crates.io](https://img.shields.io/crates/v/juncture-derive.svg)](https://crates.io/crates/juncture-derive)
[![Documentation](https://docs.rs/juncture-derive/badge.svg)](https://docs.rs/juncture-derive)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Proc-macro derive implementation for the Juncture `State` trait. This crate provides `#[derive(State)]` which generates typed State/Update pairs with per-field reducer semantics.

## Generated Code

For a struct like:

```rust
#[derive(State)]
struct MyState {
    #[reducer(append)]
    messages: Vec<String>,
    #[reducer(replace)]
    count: usize,
}
```

The macro generates:

1. **`MyStateUpdate`** - Update struct with all fields as `Option<T>`
2. **Field index constants** - `MyState::FIELD_MESSAGES: usize = 0`, etc.
3. **`State` trait impl** - `apply()` with per-field reducer dispatch

## Reducer Types

| Reducer | Behavior |
|---------|----------|
| `replace` | Default. Single writer per superstep |
| `append` | Vec extend |
| `ephemeral` | Reset to Default after each superstep |
| `last_write_wins` | Multiple writers, last wins |
| `untracked` | Not persisted across checkpoints |
| `replace_after_finish` | Available only after `finish()` call |
| `any` | All writers should provide equal values |
| `custom = path::to::func` | Custom merge function |

## Container Attributes

- `#[state_version(N)]` - Set schema version (default 1)
- `#[migrate_from(N, path::to::func)]` - Migration from version N
- `#[subset_of(ParentState)]` - Generate `StateSubset<ParentState>` impl

## Constraints

- Structs must have named fields (no tuple or unit structs)
- Maximum 64 fields (u64 bitmask limit)
- Only one derive per struct

## License

Licensed under Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.
