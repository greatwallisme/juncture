# Critical Anti-Patterns in Hexagonal Architecture

## 1. Leaking Implementation Details in Handlers

```rust
// NEVER: Handler knows about concrete database
pub async fn handler(State(pool): State<SqlitePool>) { }

// ALWAYS: Handler depends on trait abstraction
pub async fn handler(State(service): State<Arc<dyn AuthorService>>) { }
```

**WHY**: Handler coupled to SQLite = cannot swap DB without changing handler. Violates dependency inversion.

## 2. Orchestration in HTTP Handlers

```rust
// NEVER: Business logic scattered in handler
pub async fn handler(State(repo): State<Repo>, State(metrics): State<Metrics>) {
    let author = repo.create().await?;
    metrics.record().await;  // Orchestration belongs in service
}

// ALWAYS: Single service call
pub async fn handler(State(service): State<Arc<Service>>) {
    service.create_author(&request).await?;
}
```

**WHY**: If same business logic needed for CLI/GraphQL/tests, you'd duplicate it. Service = single source of truth.

## 3. Option<T> for Required vs Optional State

```rust
// NEVER: Semantic ambiguity - is this saved or not?
pub struct Author { pub id: Option<AuthorId> }

// ALWAYS: Separate models for different contexts
pub struct CreateAuthorRequest { /* no id */ }
pub struct Author { pub id: AuthorId }
```

**WHY**: Option<T> forces null checks everywhere. Separate models make state explicit at type level.

## 4. Domain Entities Implementing Serialize/Deserialize

```rust
// NEVER: Couples domain to JSON format
#[derive(Serialize, Deserialize)]
pub struct Author { ... }

// ALWAYS: Separate DTOs with From conversions
pub struct Author { ... }
pub struct AuthorResponse { ... }
impl From<&Author> for AuthorResponse { ... }
```

**WHY**: Changing JSON structure shouldn't affect domain. Multiple formats (JSON/Protobuf/XML) become impossible.

## 5. Panic/Unwrap in Domain Code

```rust
// NEVER: Crashes on edge cases
pub fn new(name: String) -> Self {
    Self(name.split_once(' ').unwrap())  // panics if no space
}

// ALWAYS: Return Result
pub fn new(name: String) -> Result<Self, ValidationError> {
    name.split_once(' ')
        .ok_or(ValidationError::InvalidName)
        .map(|(first, last)| Self(first.to_string(), last.to_string()))
}
```

**WHY**: Domain should never crash. Caller decides how to handle errors.

## 6. Repository Returns Database Errors

```rust
// NEVER: DB details leak to service layer
async fn create(&self, req: &Request) -> Result<Author, sqlx::Error>

// ALWAYS: Domain-specific errors
async fn create(&self, req: &Request) -> Result<Author, CreateAuthorError>
```

**WHY**: Service layer shouldn't know about SQLite. Changing DB shouldn't change service code.

## 7. Validation in Services Instead of Constructors

```rust
// NEVER: Invalid states possible
impl Author {
    pub fn new(name: String) -> Self { Self(name) }  // no validation
}
impl AuthorService {
    async fn create(&self, name: String) -> Result<()> {
        if name.is_empty() { return Err(...); }  // too late
    }
}

// ALWAYS: Validation at creation boundary
impl AuthorName {
    pub fn new(name: String) -> Result<Self, ValidationError> {
        if name.is_empty() { return Err(...); }
        Ok(Self(name))
    }
}
```

**WHY**: Once you have an AuthorName, it's guaranteed valid. No defensive checks needed downstream.

## 8. One Giant Port Trait

```rust
// NEVER: Forces every adapter to implement everything
trait Persistence {
    async fn create_author(...);
    async fn create_post(...);
    async fn create_comment(...);
    // ... 100 more methods
}

// ALWAYS: Segregated by entity AND concern
trait AuthorRepository { async fn create_author(...); }
trait PostRepository { async fn create_post(...); }
```

**WHY**: Violates Interface Segregation Principle. Adapters end up with empty impl blocks.

## 9. Direct Database Access in Handlers

```rust
// NEVER: Handler bypasses service layer
pub async fn handler(State(pool): State<Pool>) {
    sqlx::query("INSERT INTO authors...").execute(&pool).await;
}

// ALWAYS: Go through service
pub async fn handler(State(service): State<Arc<dyn AuthorService>>) {
    service.create_author(&request).await?;
}
```

**WHY**: Business rules (validation, metrics, notifications) get bypassed. Security hole.

## 10. Testing Against Real Database in Unit Tests

```rust
// NEVER: Unit tests depend on external infrastructure
#[test]
fn test_create_author() {
    let pool = SqlitePool::connect(...).await;  // slow, fragile
}

// ALWAYS: Mock repositories in unit tests
#[test]
fn test_create_author() {
    let mock_repo = MockAuthorRepository::new();
    // fast, deterministic
}
```

**WHY**: Unit tests should be fast and deterministic. Integration tests cover real DB.
