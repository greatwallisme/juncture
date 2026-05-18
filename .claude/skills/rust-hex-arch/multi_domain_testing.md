// EXPERT NOTE: Multi-domain testing becomes critical when your application
// splits into multiple bounded contexts (Author domain, Post domain, etc.).
// Testing interactions between domains requires different strategies than
// single-domain testing.

## Testing Philosophy for Multiple Domains

### Single-Domain vs Multi-Domain Tests

| Test Type | Scope | Dependencies | Example |
|-----------|-------|--------------|---------|
| Unit | One service | Mocked all dependencies | AuthorService logic |
| Integration | One domain | Real database, mock other domains | AuthorService + real DB |
| Multi-Domain | Multiple domains | Real DBs, real domain interactions | Author deleted cascades to Posts |

### When to Use Each

```
Unit tests (fastest)
    - Business logic in services
    - Error handling paths
    - Edge cases

Integration tests (medium)
    - Repository behavior
    - Database constraints
    - Port/adapter compatibility

Multi-domain tests (slowest)
    - Cross-domain workflows
    - Event-driven interactions
    - Transaction boundaries
```

## Multi-Domain Test Patterns

### Pattern 1: Shared Database, Separate Domains

```rust
// tests/multi_domain_tests.rs
use test_context::TestContext;
use your_app::{
    domain::author::ports::AuthorService,
    domain::post::ports::PostService,
    bootstrap::TestAppState,
};

struct MultiDomainTest {
    author_service: Arc<dyn AuthorService>,
    post_service: Arc<dyn PostService>,
    // Clean up after tests
    _db_guard: DatabaseGuard,
}

impl MultiDomainTest {
    async fn setup() -> Self {
        let state = TestAppState::new().await;
        Self {
            author_service: state.author_service(),
            post_service: state.post_service(),
            _db_guard: state.db_guard(),
        }
    }
}

#[tokio::test]
async fn test_delete_author_with_posts() {
    let test = MultiDomainTest::setup().await;

    // Create author via Author domain
    let author = test.author_service
        .create_author(&CreateAuthorRequest {
            name: AuthorName::new("Test".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        })
        .await
        .unwrap();

    // Create posts via Post domain
    test.post_service
        .create_post(&CreatePostRequest {
            author_id: author.id.clone(),
            title: "Test Post".to_string(),
        })
        .await
        .unwrap();

    // Delete author should fail due to business rule
    let result = test.author_service
        .delete_author(&author.id)
        .await;

    assert!(matches!(result, Err(DeleteAuthorError::HasPosts)));
}
```

### Pattern 2: In-Memory Event Bus

```rust
// EXPERT NOTE: For event-driven inter-domain communication, use a test-specific
// event bus that allows assertions on published events.

use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

struct TestEventBus {
    events: Arc<Mutex<Vec<DomainEvent>>>,
    sender: broadcast::Sender<DomainEvent>,
}

impl TestEventBus {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(100);
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            sender,
        }
    }

    fn publish(&self, event: DomainEvent) {
        self.events.lock().unwrap().push(event.clone());
        let _ = self.sender.send(event);
    }

    fn assert_event_published<F>(&self, predicate: F)
    where
        F: Fn(&DomainEvent) -> bool,
    {
        let events = self.events.lock().unwrap();
        assert!(events.iter().any(predicate), "Expected event not found");
    }
}

#[tokio::test]
async fn test_author_deleted_triggers_post_cleanup() {
    let event_bus = TestEventBus::new();
    let test = MultiDomainTest::with_event_bus(event_bus.clone()).await;

    // Setup: Create author with posts
    let author = setup_author_with_posts(&test).await;

    // Action: Delete author (triggers event)
    test.author_service.delete_author(&author.id).await.unwrap();

    // Assert: Event was published
    event_bus.assert_event_published(|e| {
        matches!(e, DomainEvent::AuthorDeleted { id } if *id == author.id)
    });

    // Assert: Posts were cleaned up (handled by event subscriber)
    let posts = test.post_service.list_by_author(&author.id).await.unwrap();
    assert!(posts.is_empty());
}
```

### Pattern 3: Transaction Boundaries

```rust
// EXPERT NOTE: Testing transaction boundaries ensures data consistency
// across multiple domains. This is especially important for sagas.

#[tokio::test]
async fn test_create_author_with_initial_post_saga() {
    let test = MultiDomainTest::setup().await;

    // This should be atomic: either both succeed or both fail
    let result = test.author_service
        .create_author_with_welcome_post(&CreateAuthorWithPostRequest {
            name: "Test Author".to_string(),
            email: "test@example.com".to_string(),
            post_title: "Welcome!".to_string(),
        })
        .await;

    assert!(result.is_ok());

    // Verify both author and post exist
    let author = test.author_service
        .get_author(&result.unwrap().author_id)
        .await
        .unwrap();
    assert!(author.is_some());

    let posts = test.post_service
        .list_by_author(&author.unwrap().id)
        .await
        .unwrap();
    assert_eq!(posts.len(), 1);
}

// EXPERT NOTE: Test the failure path too - ensure rollback works

#[tokio::test]
async fn test_create_author_with_post_rolls_back_on_post_failure() {
    let test = MultiDomainTest::setup().await;

    // Configure post service to fail
    test.post_service.set_failure_mode(true);

    let result = test.author_service
        .create_author_with_welcome_post(&/* invalid request */)
        .await;

    assert!(result.is_err());

    // Verify author was NOT created (transaction rolled back)
    let authors = test.author_service.list_authors(100, 0).await.unwrap();
    assert!(authors.is_empty());
}
```

## Testing Event-Driven Communication

### Event Definition

```rust
// domain/events.rs
#[derive(Debug, Clone, PartialEq)]
pub enum DomainEvent {
    AuthorCreated { id: AuthorId, email: Email },
    AuthorDeleted { id: AuthorId },
    PostPublished { id: PostId, author_id: AuthorId },
}

#[async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(&self, event: DomainEvent) -> Result<(), anyhow::Error>;
}

#[async_trait]
pub trait EventSubscriber: Send + Sync {
    async fn on_author_deleted(&self, id: AuthorId) -> Result<(), anyhow::Error>;
}
```

### Test Event Publisher

```rust
struct TestEventPublisher {
    events: Arc<Mutex<Vec<DomainEvent>>>,
}

#[async_trait]
impl EventPublisher for TestEventPublisher {
    async fn publish(&self, event: DomainEvent) -> Result<(), anyhow::Error> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

impl TestEventPublisher {
    fn assert_published<F>(&self, predicate: F)
    where
        F: Fn(&DomainEvent) -> bool,
    {
        let events = self.events.lock().unwrap();
        assert!(events.iter().any(predicate), "No matching event found");
    }

    fn assert_not_published<F>(&self, predicate: F)
    where
        F: Fn(&DomainEvent) -> bool,
    {
        let events = self.events.lock().unwrap();
        assert!(!events.iter().any(predicate), "Unexpected event found");
    }

    fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}
```

## Test Organization

### Directory Structure

```
tests/
├── multi_domain/
│   ├── mod.rs
│   ├── author_post_tests.rs     # Author-Post interactions
│   ├── event_tests.rs           # Event-driven communication
│   └── transaction_tests.rs     # Cross-domain transactions
├── helpers/
│   ├── mod.rs
│   ├── test_context.rs          # Shared test setup
│   └── assertions.rs            # Custom assertions
└── fixtures/
    └── test_data.rs             # Test data factories
```

### Shared Test Context

```rust
// tests/helpers/test_context.rs
pub struct TestContext {
    pub db: SqlitePool,
    pub author_service: Arc<dyn AuthorService>,
    pub post_service: Arc<dyn PostService>,
    pub event_bus: TestEventBus,
}

impl TestContext {
    pub async fn new() -> Self {
        let db = SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&db).await;

        let event_bus = TestEventBus::new();
        let author_repo = SqliteAuthorRepository::new(db.clone());
        let post_repo = SqlitePostRepository::new(db.clone());

        let author_service = AuthorServiceImpl::new(
            author_repo,
            event_bus.clone(),
        );

        let post_service = PostServiceImpl::new(
            post_repo,
            event_bus.clone(),
        );

        Self {
            db,
            author_service: Arc::new(author_service),
            post_service: Arc::new(post_service),
            event_bus,
        }
    }

    pub async fn refresh(&self) {
        // Clear all tables between tests
        sqlx::query("DELETE FROM posts").execute(&self.db).await.unwrap();
        sqlx::query("DELETE FROM authors").execute(&self.db).await.unwrap();
        self.event_bus.clear();
    }
}
```

## Performance Considerations

### Running Multi-Domain Tests

```bash
# Run only unit tests (fastest)
cargo test --lib

# Run integration tests
cargo test --test '*_tests'

# Skip multi-domain tests (slowest)
cargo test -- --skip multi_domain

# Run only multi-domain tests when needed
cargo test multi_domain
```

### Parallel Execution

```toml
# Cargo.toml - Disable parallel execution for tests sharing state
[[test]]
name = "multi_domain_tests"
harness = false
```

```rust
// tests/multi_domain_tests.rs
// Run tests sequentially when they share database state
fn main() {
    // Use serial_test crate or custom test runner
    serial_test::serial(|| {
        // Your multi-domain tests here
    });
}
```

## Common Pitfalls

1. **Not cleaning up between tests**: Use `refresh()` or fixtures
2. **Assuming ordering**: Tests should be independent
3. **Hard-coded IDs**: Use factory-generated IDs
4. **Not testing failure paths**: Every saga needs rollback testing
5. **Leaking event state**: Clear event bus between tests
