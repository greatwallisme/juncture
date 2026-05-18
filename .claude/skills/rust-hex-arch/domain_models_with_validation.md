// WHY newtypes instead of raw String: Compiler catches type mismatches (Email vs AuthorName),
// validation happens once in constructor, function signatures become self-documenting.
// TRADE-OFF: More boilerplate. Worth it for non-trivial domains.

```rust
// domain/models.rs
use crate::domain::errors::ValidationError;

#[derive(Debug, Clone)]
pub struct Author {
    pub id: AuthorId,
    pub name: AuthorName,
    pub email: Email,
}

// CRITICAL: ID derives PartialEq/Eq for test comparisons.
// Name/Email DON'T - same value != same entity (domain semantics, not data equality).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorId(String);

#[derive(Debug, Clone)]
pub struct AuthorName(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl AuthorId {
    // new() never fails - fresh IDs. from_string() validates - prevents using user input
    // where fresh ID expected (or vice versa).
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn from_string(id: String) -> Result<Self, ValidationError> {
        if id.trim().is_empty() {
            return Err(ValidationError::EmptyId);
        }
        Ok(Self(id))
    }

    // Getter instead of Deref - prevents implicit conversions, makes data flow visible
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AuthorName {
    pub fn new(name: String) -> Result<Self, ValidationError> {
        if name.trim().is_empty() {
            return Err(ValidationError::EmptyName);
        }
        if name.len() > 100 {
            return Err(ValidationError::NameTooLong);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Email {
    pub fn new(email: String) -> Result<Self, ValidationError> {
        // Intentionally simple: RFC 5322 validation is extremely complex.
        // For most apps, "@ and ." existence is sufficient.
        // Use `email-validation` crate only if you need full RFC compliance.
        // Trade-off: False positives vs. over-rejecting valid edge cases.
        if !email.contains('@') || !email.contains('.') {
            return Err(ValidationError::InvalidEmail);
        }
        if email.len() > 255 {
            return Err(ValidationError::EmailTooLong);
        }
        Ok(Self(email))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// WHY separate Request/Response models:
// - Request: validated user data, not persisted yet (no ID)
// - Response: DTO for serialization - domain entities stay pure (no Serialize/Deserialize)
// - Add new formats (XML, Protobuf) without touching domain code

#[derive(Debug, Clone)]
pub struct CreateAuthorRequest {
    pub name: AuthorName,
    pub email: Email,
}

impl CreateAuthorRequest {
    pub fn new(name: AuthorName, email: Email) -> Self {
        Self { name, email }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorResponse {
    pub id: String,
    pub name: String,
    pub email: String,
}

// From trait instead of Serialize on entity - decouples domain from JSON/Bincode/etc
impl From<&Author> for AuthorResponse {
    fn from(author: &Author) -> Self {
        Self {
            id: author.id.as_str().to_string(),
            name: author.name.as_str().to_string(),
            email: author.email.as_str().to_string(),
        }
    }
}
```