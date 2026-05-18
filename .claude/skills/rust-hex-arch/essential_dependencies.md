# Dependency Decision Framework

## HTTP Framework Decision

| Framework | When to Use | Trade-offs |
|-----------|-------------|------------|
| **axum** | Default choice. Tower ecosystem, type-safe routing, extractors. | More boilerplate for simple handlers |
| **actix-web** | Need actor model,WebSocket, or extensive middleware ecosystem. | Heavier, more complex compile errors |
| **rocket** | Prefer macros, rapid prototyping. | Slower compile, less flexible |

## Database Decision

| Library | When to Use | Trade-offs |
|---------|-------------|------------|
| **sqlx** | Default. Compile-time checked queries with macros, no ORM overhead. | Manual query writing, verbose for complex relations |
| **diesel** | Need schema migration tooling, type-safe query builder. | Slower compile, complex macro errors, async lagging |
| **sea-orm** | Coming from Django/Rails, want dynamic queries. | Runtime errors, less type safety |

**sqlx macro vs runtime**:
- `query!` macro: Compile-time verification, slower compiles, use for stable schemas
- `query` runtime: Faster compiles, runtime errors possible, use for dynamic queries

## Async Decision

| Crate | When to Use |
|-------|-------------|
| **async-trait** | Required for async trait methods - always use for ports |
| **futures** | Need stream combinators, utility functions |

## Error Handling Decision

| Crate | When to Use |
|-------|-------------|
| **thiserror** | Domain errors - implements Display, Error, From:: |
| **anyhow** | Application-level errors (main.rs), quick prototyping |

NEVER use anyhow in domain layer - domain errors must be explicit and typed.

## Testing Decision

| Crate | When to Use |
|-------|-------------|
| **mockall** | Complex mocking needs, automatic Mock struct generation |
| **Manual mocks** | Simple traits, prefer fewer dependencies |

## Minimal Cargo.toml

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
anyhow = "1.0"
thiserror = "1.0"
uuid = { version = "1", features = ["v4"] }
serde = { version = "1.0", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"

# Choose ONE:
axum = "0.7"
tower-http = "0.5"

# Choose database:
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio"] }

[dev-dependencies]
tempfile = "3"
mockall = "0.11"
```
