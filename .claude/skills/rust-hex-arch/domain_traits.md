// CRITICAL: Domain defines trait, adapters implement it. NEVER reverse this.
//
// Trait granularity decision:
// - One method per trait? NO - trait bound explosion
// - One giant trait? NO - violates ISP, forces unnecessary dependencies
// - Sweet spot: Group by domain entity AND concern (Repository, Service, Metrics)
//
// Send + Sync + Clone + 'static required for Arc<dyn Trait>:
// - Without Clone: can't pass service to multiple handlers
// - Without Send + Sync: can't share across tokio tasks
// - 'static ensures no borrowed data outlives trait object

```rust
// domain/ports.rs
use async_trait::async_trait;
use crate::domain::{
    models::{Author, CreateAuthorRequest},
    errors::*,
};

// Each method returns domain-specific error - prevents DB details from leaking.
// Handlers see "NotFound" not "sqlite::Error { code: 2067 }".
#[async_trait]
pub trait AuthorRepository: Send + Sync + Clone + 'static {
    async fn create_author(&self, request: &CreateAuthorRequest) -> Result<Author, CreateAuthorError>;
    async fn find_by_id(&self, id: &AuthorId) -> Result<Option<Author>, FindAuthorError>;
    async fn find_by_email(&self, email: &Email) -> Result<Option<Author>, FindAuthorError>;
    async fn update_author(&self, id: &AuthorId, request: &UpdateAuthorRequest) -> Result<Author, UpdateAuthorError>;
    async fn delete_author(&self, id: &AuthorId) -> Result<(), DeleteAuthorError>;
    async fn list_authors(&self, limit: usize, offset: usize) -> Result<Vec<Author>, ListAuthorsError>;
}

// Service port = what handlers depend on. Repository = internal implementation detail.
// Handlers never see database concerns, only business operations.
#[async_trait]
pub trait AuthorService: Send + Sync + Clone + 'static {
    async fn create_author(&self, request: &CreateAuthorRequest) -> Result<Author, CreateAuthorError>;
    async fn get_author(&self, id: &AuthorId) -> Result<Option<Author>, FindAuthorError>;
    async fn update_author(&self, id: &AuthorId, request: &UpdateAuthorRequest) -> Result<Author, UpdateAuthorError>;
    async fn delete_author(&self, id: &AuthorId) -> Result<(), DeleteAuthorError>;
    async fn list_authors(&self, limit: usize, offset: usize) -> Result<Vec<Author>, ListAuthorsError>;
}

// Secondary ports = cross-cutting concerns. Benefits:
// 1. Swap implementations (Prometheus -> StatsD) without touching service code
// 2. Mock in tests (no real metrics server needed)
// 3. Version independently from business logic

#[async_trait]
pub trait AuthorMetrics: Send + Sync + Clone + 'static {
    async fn record_author_created(&self);
    async fn record_author_creation_failed(&self);
    async fn record_author_updated(&self);
    async fn record_author_deleted(&self);
}

#[async_trait]
pub trait AuthorNotifier: Send + Sync + Clone + 'static {
    async fn send_welcome_email(&self, author: &Author);
    async fn send_deletion_notification(&self, author: &Author);
}

#[async_trait]
pub trait AuthorValidator: Send + Sync + Clone + 'static {
    async fn validate_author_creation(&self, request: &CreateAuthorRequest) -> Result<(), ValidationError>;
}

// Port splitting decisions:
// - Different concerns (persistence vs metrics) -> Separate traits
// - Same concern, different entities (Author vs Post) -> Separate traits
// - Same entity, same concern -> One trait
//
// ANTI-PATTERN: One giant "Persistence" trait with Author + Post + Comment methods.
// Forces every adapter to implement everything, even if not needed.
```
