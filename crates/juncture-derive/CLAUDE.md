# CLAUDE.md -- juncture-derive

Proc-macro crate providing `#[derive(State)]`. Has no runtime dependencies beyond `syn`, `quote`, `proc-macro2`.

## What the macro generates

For a struct `Foo` with fields `a: T1, b: T2, ...`:

1. **`FooUpdate`** -- all fields become `Option<T>`, derived with `Default`, `Clone`, `Debug`, `Serialize`, `Deserialize`
2. **`FooFieldVersions`** -- all fields become `u64`, for per-field version tracking
3. **Field index constants** -- `Foo::FIELD_A: usize = 0`, `Foo::FIELD_B: usize = 1`, etc.
4. **`State` trait impl** -- `apply()` dispatches per-field by reducer type, `reset_ephemeral()` clears ephemeral fields, `schema_version()` and `migrate()` for schema evolution

## Container attributes

- `#[state_version(N)]` -- set schema version (default 1)
- `#[migrate_from(N, path::to::func)]` -- migration from version N

## Field attributes

- `#[reducer(replace)]` -- default, one writer per superstep
- `#[reducer(append)]` -- Vec extend
- `#[reducer(ephemeral)]` -- reset to Default after each superstep
- `#[reducer(last_write_wins)]` -- multiple writers, last wins
- `#[reducer(untracked)]` -- not persisted across checkpoints
- `#[reducer(replace_after_finish)]` -- available only after `finish()` call
- `#[reducer(any)]` -- all writers should provide equal values
- `#[reducer(custom = path::to::func)]` -- custom merge function `fn(&mut T, T)`

## Constraints

- Structs must have named fields (no tuple structs or unit structs)
- Maximum 64 fields (u64 bitmask limit)
- Only one derive per struct (standard proc-macro limitation)

## Testing

Changes here affect all downstream crates. Run `cargo test -p juncture-derive && cargo test -p juncture-core` to validate.
