// EXPERT NOTE: Database migrations in hexagonal architecture require careful design.
// Since the repository hides database details, migrations are the only place where
// the database schema and domain models must align.

## Migration Strategy

### Option 1: sqlx Migrate (Recommended for most projects)

```toml
# Cargo.toml
[dependencies]
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio", "migrate"] }
```

```bash
# Create migrations directory
mkdir -p migrations

# Generate new migration
sqlx migrate add create_authors_table
```

```sql
-- migrations/20240101_000001_create_authors_table.up.sql
CREATE TABLE authors (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- migrations/20240101_000001_create_authors_table.down.sql
DROP TABLE authors;
```

```rust
// Run migrations on startup (see repository_adapter_implementation.md)
sqlx::migrate!("./migrations").run(&pool).await?;
```

**EXPERT NOTE**: `sqlx::migrate!` macro is checked at compile time if you use
`sqlx migrate run --check` before building. This catches SQL errors early.

### Option 2: Refinery (Alternative with more features)

```toml
[dependencies]
refinery = { version = "0.8", features = ["sqlite"] }
```

```rust
use refinery::config::Config;

let mut client = Config::new(sqlite_url)
    .run_migration::<RefineryMigrations>()
    .await?;
```

### Option 3: Custom Migration System (For complex needs)

```rust
// outbound/migrations/mod.rs
pub struct Migration {
    version: i64,
    name: &'static str,
    up: fn(&SqlitePool) -> Result<(), Error>,
    down: fn(&SqlitePool) -> Result<(), Error>,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_authors_table",
        up: |pool| {
            sqlx::query("CREATE TABLE authors (...)").execute(pool).await?;
            Ok(())
        },
        down: |pool| {
            sqlx::query("DROP TABLE authors").execute(pool).await?;
            Ok(())
        },
    },
];
```

## Migration Best Practices

### 1. Always Write Down Migrations

```sql
-- GOOD: Reversible migration
CREATE TABLE authors (id TEXT PRIMARY KEY);

-- BAD: Can't be rolled back
DROP TABLE IF EXISTS authors; -- IF EXISTS means down migration can't error properly
```

### 2. Use Transactional Migrations

```sql
-- BEGIN is automatic for single statements in SQLite
-- For multiple statements, wrap explicitly:
BEGIN;
CREATE TABLE authors (...);
CREATE INDEX idx_authors_email ON authors(email);
COMMIT;
```

### 3. Handle Data Migrations Carefully

```sql
-- BAD: Destructive data migration
ALTER TABLE authors ADD COLUMN bio TEXT;
UPDATE authors SET bio = ''; -- Default for existing rows

-- GOOD: Non-breaking, handle NULL explicitly
ALTER TABLE authors ADD COLUMN bio TEXT DEFAULT '';
-- Or make NULL allowed and handle in application logic
ALTER TABLE authors ADD COLUMN bio TEXT;
```

### 4. Index After Data Migration for Performance

```sql
-- Add column
ALTER TABLE posts ADD COLUMN author_id TEXT;

-- Migrate data (may take time)
UPDATE posts SET author_id = (SELECT author_id FROM old_mapping WHERE post_id = posts.id);

-- Then add index (faster than indexing during migration)
CREATE INDEX idx_posts_author_id ON posts(author_id);
```

## Schema Evolution Patterns

### Adding Fields (Non-breaking)

```sql
-- Up
ALTER TABLE authors ADD COLUMN bio TEXT DEFAULT '';

-- Down
-- SQLite doesn't support DROP COLUMN directly before 3.35.0
-- Workaround: recreate table
CREATE TABLE authors_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE
);
INSERT INTO authors_new SELECT id, name, email FROM authors;
DROP TABLE authors;
ALTER TABLE authors_new RENAME TO authors;
```

### Renaming Fields (Breaking)

```sql
-- EXPERT NOTE: Renaming requires careful consideration of running deployments.
-- Strategy: Add new column, deploy code, migrate data, remove old column

-- Step 1: Add new column
ALTER TABLE authors ADD COLUMN full_name TEXT DEFAULT '';

-- Step 2: Deploy code that writes to both old and new columns

-- Step 3: Migrate data
UPDATE authors SET full_name = name;

-- Step 4: Deploy code that reads from new column

-- Step 5: Remove old column (separate migration)
-- See SQLite column removal workaround above
```

### Changing Field Types (Breaking)

```sql
-- Example: TEXT -> INTEGER for status
-- Step 1: Add new column
ALTER TABLE posts ADD COLUMN status_code INTEGER DEFAULT 0;

-- Step 2: Deploy code that maps old enum to new int
-- 0 = draft, 1 = published, etc.

-- Step 3: Migrate data
UPDATE posts SET status_code = CASE status
    WHEN 'draft' THEN 0
    WHEN 'published' THEN 1
    ELSE 0
END;

-- Step 4: Deploy code reading from status_code

-- Step 5: Drop old status column
```

## Zero-Downtime Migrations

For production systems requiring zero downtime:

### Phase 1: Expand (Add new schema)

```sql
ALTER TABLE authors ADD COLUMN display_name TEXT DEFAULT '';
```

Deploy code that writes to both `name` and `display_name`.

### Phase 2: Contract (Migrate data)

```sql
UPDATE authors SET display_name = name;
```

### Phase 3: Switch (Deploy code reading new field)

Deploy code that reads from `display_name`.

### Phase 4: Contract (Remove old field)

```sql
-- Separate migration after confirming all reads use new field
-- See SQLite column removal workaround
```

## Testing Migrations

```rust
#[cfg(test)]
mod tests {
    use super::*;

    async fn test_migration_up_and_down() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Run up migration
        sqlx::query(include_str!("../../migrations/xxx.up.sql"))
            .execute(&pool)
            .await
            .unwrap();

        // Verify schema
        let tables = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(tables.iter().any(|t| t.get::<String, _>("name") == "authors"));

        // Run down migration
        sqlx::query(include_str!("../../migrations/xxx.down.sql"))
            .execute(&pool)
            .await
            .unwrap();

        // Verify cleanup
        let tables = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!tables.iter().any(|t| t.get::<String, _>("name") == "authors"));
    }
}
```

## Common Pitfalls

1. **Not versioning migrations**: Use timestamp or sequential version numbers
2. **Modifying committed migrations**: Always create new migrations, never edit old ones
3. **Forgetting down migrations**: Your rollback strategy depends on them
4. **Data loss in down migrations**: Make sure down migrations preserve data where possible
5. **SQLite limitations**: Remember ALTER TABLE limitations before version 3.35.0
