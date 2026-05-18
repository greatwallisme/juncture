// Services orchestrate adapters to implement business logic. They:
// 1. Contain business rules (e.g., "can't delete author with existing posts")
// 2. Coordinate adapters (repository + metrics + notifier)
// 3. NOT contain HTTP/database/protocol concerns
//
// CRITICAL: Service methods are transaction boundaries. If multiple adapters participate
// in a transaction, service must handle it (often via repository).

```rust
// domain/services.rs
use async_trait::async_trait;
use std::sync::Arc;
use crate::domain::{
    models::{Author, CreateAuthorRequest, UpdateAuthorRequest, AuthorId},
    ports::{AuthorService, AuthorRepository, AuthorMetrics, AuthorNotifier, AuthorValidator},
    errors::*,
};

// Generic over dependencies enables easy testing and swapping.
// Trade-off: More complex type signatures. For simple apps, concrete types are fine.
pub struct AuthorServiceImpl<R, M, N, V> {
    repository: R,
    metrics: M,
    notifier: N,
    validator: V,
}

impl<R, M, N, V> AuthorServiceImpl<R, M, N, V>
where
    R: AuthorRepository,
    M: AuthorMetrics,
    N: AuthorNotifier,
    V: AuthorValidator,
{
    pub fn new(repository: R, metrics: M, notifier: N, validator: V) -> Self {
        Self { repository, metrics, notifier, validator }
    }
}

#[async_trait]
impl<R, M, N, V> AuthorService for AuthorServiceImpl<R, M, N, V>
where
    R: AuthorRepository,
    M: AuthorMetrics,
    N: AuthorNotifier,
    V: AuthorValidator,
{
    // Business rule ordering matters:
    // 1. Validate first (fail fast, no DB round-trip)
    // 2. Check business constraints (duplicate email)
    // 3. Persist (last step, only if everything else passes)
    // 4. Side effects (metrics, notifications) - non-blocking if possible

    async fn create_author(&self, request: &CreateAuthorRequest) -> Result<Author, CreateAuthorError> {
        // Step 1: Domain validation
        self.validator.validate_author_creation(request).await
            .map_err(|e| CreateAuthorError::ValidationFailed(e.to_string()))?;

        // Step 2: Business rule - email must be unique
        if let Some(_) = self.repository.find_by_email(&request.email).await? {
            return Err(CreateAuthorError::DuplicateAuthor(request.email.as_str().to_string()));
        }

        // Step 3: Persist
        match self.repository.create_author(request).await {
            Ok(author) => {
                // Step 4: Side effects (fire and forget)
                // CRITICAL: Metrics/notification failures don't roll back creation.
                // For transactional side effects, use saga pattern or outbox pattern.
                self.metrics.record_author_created().await;
                self.notifier.send_welcome_email(&author).await;
                Ok(author)
            }
            Err(e) => {
                self.metrics.record_author_creation_failed().await;
                Err(e)
            }
        }
    }

    // Simple read-through - no business logic, just delegation.
    // Team choice: Some prefer all reads through services for consistency.
    // Others bypass service for pure reads.

    async fn get_author(&self, id: &AuthorId) -> Result<Option<Author>, FindAuthorError> {
        self.repository.find_by_id(id).await
    }

    // Email uniqueness check only if email changed - business logic belongs in service,
    // not repository. This is why we need both layers.

    async fn update_author(&self, id: &AuthorId, request: &UpdateAuthorRequest) -> Result<Author, UpdateAuthorError> {
        let existing_author = self.repository.find_by_id(id).await?
            .ok_or(UpdateAuthorError::NotFound)?;

        // Business rule: Email must remain unique
        if let Some(new_email) = &request.email {
            if *new_email != existing_author.email {
                if let Some(_) = self.repository.find_by_email(new_email).await? {
                    return Err(UpdateAuthorError::DuplicateEmail(new_email.as_str().to_string()));
                }
            }
        }

        let updated_author = self.repository.update_author(id, request).await?;
        self.metrics.record_author_updated().await;
        Ok(updated_author)
    }

    async fn delete_author(&self, id: &AuthorId) -> Result<(), DeleteAuthorError> {
        let author = self.repository.find_by_id(id).await?
            .ok_or(DeleteAuthorError::NotFound)?;

        // Business rule placeholder: Check for related entities before delete.
        // In real apps, query PostRepository. If author has posts:
        // (1) reject deletion, (2) cascade delete, or (3) soft delete.
        // Choice = business requirement, not technical constraint.
        // if self.post_repository.author_has_posts(id).await? {
        //     return Err(DeleteAuthorError::HasPosts);
        // }

        self.repository.delete_author(id).await?;
        self.metrics.record_author_deleted().await;
        self.notifier.send_deletion_notification(&author).await;
        Ok(())
    }

    // Guard against unbounded queries. For large datasets, consider cursor-based pagination.
    async fn list_authors(&self, limit: usize, offset: usize) -> Result<Vec<Author>, ListAuthorsError> {
        if limit > 1000 {
            return Err(ListAuthorsError::InvalidLimit(limit));
        }
        self.repository.list_authors(limit, offset).await
    }
}
```

// Testing without real dependencies:
// 1. Create mock structs implementing each trait
// 2. Configure mock return values per test case
// 3. Assert service called dependencies correctly
// 4. Assert business logic produced expected result
// See unit_testing_services_with_mocks.md for examples.
