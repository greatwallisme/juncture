// EXPERT NOTE: Repository adapters are where the "rubber meets the road" - they translate
// domain operations into database queries. Critical responsibilities:
// 1. Map domain types <-> database rows (including validation)
// 2. Handle database-specific errors and convert to domain errors
// 3. Manage transactions when needed
// 4. NEVER leak database types to domain (no sqlx::Error in domain)

```rust
// outbound/sqlite.rs
use sqlx::{SqlitePool, Row};
use anyhow::Result;
use std::sync::Arc;
use async_trait::async_trait;
use crate::domain::{
    models::{Author, CreateAuthorRequest, UpdateAuthorRequest, AuthorId, Email, AuthorName},
    ports::AuthorRepository,
    errors::{CreateAuthorError, FindAuthorError, UpdateAuthorError, DeleteAuthorError, ListAuthorsError},
};

// EXPERT NOTE: Newtype wrapper around SqlitePool prevents leaking sqlx types.
// Domain never knows about SqlitePool - it just sees a struct implementing AuthorRepository.

pub struct Sqlite {
    pool: SqlitePool,
}

impl Sqlite {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePool::connect(database_url).await
            .map_err(|e| anyhow::anyhow!("Failed to connect to SQLite: {}", e))?;

        // EXPERT NOTE: Run migrations on startup. In production, you might want
        // separate migration tooling, but this ensures database is up-to-date.
        sqlx::migrate!("./migrations").run(&pool).await
            .map_err(|e| anyhow::anyhow!("Failed to run migrations: {}", e))?;

        Ok(Self { pool })
    }

    // EXPERT NOTE: Private helper using transaction enables atomic multi-step operations.
    // Called with &mut tx instead of &self ensures it's only used within transactions.

    async fn save_author(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        author: &Author,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "INSERT INTO authors (id, name, email) VALUES (?, ?, ?)",
            author.id.as_str(),
            author.name.as_str(),
            author.email.as_str()
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    // EXPERT NOTE: Mapping helper centralizes row-to-domain conversion.
    // Returns DomainError if data in DB is invalid (shouldn't happen with proper validation).
    // This "trust but verify" approach catches data integrity issues early.

    async fn map_row_to_author(row: sqlx::sqlite::SqliteRow) -> Result<Author, anyhow::Error> {
        let id = AuthorId::from_string(row.get::<String, _>("id"))?;
        let name = AuthorName::new(row.get::<String, _>("name"))?;
        let email = Email::new(row.get::<String, _>("email"))?;

        Ok(Author { id, name, email })
    }
}

// EXPERT NOTE: SqlitePool is Clone, but we implement Clone on our wrapper too.
// This allows Arc<Sqlite> to work as expected in the service.

impl Clone for Sqlite {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

#[async_trait]
impl AuthorRepository for Sqlite {
    async fn create_author(
        &self,
        request: &CreateAuthorRequest,
    ) -> Result<Author, CreateAuthorError> {
        // EXPERT NOTE: Transaction ensures rollback on error.
        // For single INSERT, transaction is optional. For multi-step operations,
        // transactions prevent partial updates (data inconsistency).

        let mut tx = self.pool.begin().await
            .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {}", e))?;

        let author = Author {
            id: AuthorId::new(),
            name: request.name.clone(),
            email: request.email.clone(),
        };

        // EXPERT NOTE: Map database-specific errors to domain errors.
        // Unique violation -> DuplicateAuthor (business error)
        // Other errors -> Unknown (technical error, logged, shown as 500)

        if let Err(e) = self.save_author(&mut tx, &author).await {
            if let Some(db_err) = e.as_database_error() {
                if db_err.is_unique_violation() {
                    return Err(CreateAuthorError::DuplicateAuthor(
                        request.email.as_str().to_string()
                    ));
                }
            }
            return Err(anyhow::Error::from(e)
                .context("Failed to save author")
                .into());
        }

        tx.commit().await
            .map_err(|e| anyhow::anyhow!("Failed to commit transaction: {}", e))?;

        Ok(author)
    }

    async fn find_by_id(&self, id: &AuthorId) -> Result<Option<Author>, FindAuthorError> {
        // EXPERT NOTE: Use sqlx::query! macro for compile-time query verification.
        // Trade-off: Slower compiles but catches SQL errors at build time.
        // Alternative: sqlx::query() for faster compiles, runtime errors possible.

        let row = sqlx::query!(
            "SELECT id, name, email FROM authors WHERE id = ?",
            id.as_str()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to find author: {}", e))?;

        match row {
            Some(row) => {
                // EXPERT NOTE: Re-validating data from DB is defensive but optional.
                // If you trust your validation layer, you could use .unwrap() here.
                // Choice depends on how defensive you want to be.

                let author = Author {
                    id: AuthorId::from_string(row.id)
                        .map_err(|e| anyhow::anyhow!("Invalid author id: {}", e))?,
                    name: AuthorName::new(row.name)
                        .map_err(|e| anyhow::anyhow!("Invalid author name: {}", e))?,
                    email: Email::new(row.email)
                        .map_err(|e| anyhow::anyhow!("Invalid author email: {}", e))?,
                };
                Ok(Some(author))
            }
            None => Ok(None),
        }
    }

    // EXPERT NOTE: find_by_email pattern is identical to find_by_id.
    // In production code, consider a generic find_by method to reduce duplication.

    async fn find_by_email(&self, email: &Email) -> Result<Option<Author>, FindAuthorError> {
        let row = sqlx::query!(
            "SELECT id, name, email FROM authors WHERE email = ?",
            email.as_str()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to find author by email: {}", e))?;

        match row {
            Some(row) => {
                let author = Author {
                    id: AuthorId::from_string(row.id)
                        .map_err(|e| anyhow::anyhow!("Invalid author id: {}", e))?,
                    name: AuthorName::new(row.name)
                        .map_err(|e| anyhow::anyhow!("Invalid author name: {}", e))?,
                    email: Email::new(row.email)
                        .map_err(|e| anyhow::anyhow!("Invalid author email: {}", e))?,
                };
                Ok(Some(author))
            }
            None => Ok(None),
        }
    }

    // EXPERT NOTE: Partial update using dynamic query building.
    // Only updates fields that are Some, keeping existing values for None.
    // Alternative: Use COALESCE in SQL for simpler code.

    async fn update_author(
        &self,
        id: &AuthorId,
        request: &UpdateAuthorRequest,
    ) -> Result<Author, UpdateAuthorError> {
        let mut query = sqlx::QueryBuilder::new("UPDATE authors SET ");
        let mut has_updates = false;

        if let Some(name) = &request.name {
            query.push("name = ");
            query.push_bind(name.as_str());
            has_updates = true;
        }

        if let Some(email) = &request.email {
            if has_updates {
                query.push(", ");
            }
            query.push("email = ");
            query.push_bind(email.as_str());
            has_updates = true;
        }

        if !has_updates {
            return self.find_by_id(id).await?
                .ok_or(UpdateAuthorError::NotFound);
        }

        query.push(" WHERE id = ");
        query.push_bind(id.as_str());

        let result = query
            .build()
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to update author: {}", e))?;

        if result.rows_affected() == 0 {
            return Err(UpdateAuthorError::NotFound);
        }

        self.find_by_id(id).await?
            .ok_or(UpdateAuthorError::NotFound)
    }

    async fn delete_author(&self, id: &AuthorId) -> Result<(), DeleteAuthorError> {
        let result = sqlx::query!(
            "DELETE FROM authors WHERE id = ?",
            id.as_str()
        )
        .execute(&self.pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to delete author: {}", e))?;

        if result.rows_affected() == 0 {
            return Err(DeleteAuthorError::NotFound);
        }

        Ok(())
    }

    // EXPERT NOTE: Pagination with limit/offset is simple but has performance issues
    // at high offsets. For large datasets, consider cursor-based pagination or
    // keyset pagination (using indexed columns).

    async fn list_authors(&self, limit: usize, offset: usize) -> Result<Vec<Author>, ListAuthorsError> {
        let rows = sqlx::query!(
            "SELECT id, name, email FROM authors ORDER BY name LIMIT ? OFFSET ?",
            limit as i64,
            offset as i64
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list authors: {}", e))?;

        let mut authors = Vec::new();
        for row in rows {
            let author = Author {
                id: AuthorId::from_string(row.id)
                    .map_err(|e| anyhow::anyhow!("Invalid author id: {}", e))?,
                name: AuthorName::new(row.name)
                    .map_err(|e| anyhow::anyhow!("Invalid author name: {}", e))?,
                email: Email::new(row.email)
                    .map_err(|e| anyhow::anyhow!("Invalid author email: {}", e))?,
            };
            authors.push(author);
        }

        Ok(authors)
    }
}
```

// EXPERT NOTE: When migrating between databases (SQLite -> Postgres):
// 1. Create new adapter (postgres.rs) implementing AuthorRepository
// 2. Update main.rs to use Postgres instead of Sqlite
// 3. Domain code remains unchanged - this is the benefit of ports/adapters
//
// Common gotchas when switching databases:
// - SQLite stores booleans as 0/1, Postgres uses true/false
// - String concatenation differs (|| vs +)
// - Auto-increment vs UUID primary keys
// - Transaction isolation levels vary
