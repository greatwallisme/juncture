```
src/
├── domain/
│   ├── mod.rs
│   ├── models.rs          # Domain entities (Author, Post, etc.)
│   ├── ports.rs           # Trait definitions (Repository, Service)
│   └── errors.rs          # Domain-specific error types
├── inbound/               # Adapters that call the domain
│   ├── http/              # HTTP handlers (axum, actix-web)
│   └── cli/               # CLI adapters
├── outbound/              # Adapters called by the domain
│   ├── sqlite.rs          # SQLite repository implementation
│   ├── postgres.rs        # Postgres repository implementation
│   └── metrics.rs         # Metrics aggregator
├── lib.rs
└── main.rs                # Bootstrap: wire adapters to ports
```