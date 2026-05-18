```rust
// domain/errors.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Name cannot be empty")]
    EmptyName,

    #[error("Name cannot exceed 100 characters")]
    NameTooLong,

    #[error("Invalid email format")]
    InvalidEmail,

    #[error("Email cannot exceed 255 characters")]
    EmailTooLong,

    #[error("Id cannot be empty")]
    EmptyId,
}

#[derive(Error, Debug)]
pub enum CreateAuthorError {
    #[error("Author with email {0} already exists")]
    DuplicateAuthor(String),

    #[error("Invalid author data: {0}")]
    ValidationError(#[from] ValidationError),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    // Catch-all for unexpected adapter failures
    #[error("Unexpected error occurred")]
    Unknown(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum FindAuthorError {
    #[error("Author not found")]
    NotFound,

    #[error("Invalid author id: {0}")]
    InvalidId(String),

    #[error("Unexpected error occurred")]
    Unknown(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum UpdateAuthorError {
    #[error("Author not found")]
    NotFound,

    #[error("Invalid author data: {0}")]
    ValidationError(#[from] ValidationError),

    #[error("Author with email {0} already exists")]
    DuplicateEmail(String),

    #[error("Unexpected error occurred")]
    Unknown(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum DeleteAuthorError {
    #[error("Author not found")]
    NotFound,

    #[error("Cannot delete author with existing posts")]
    HasPosts,

    #[error("Unexpected error occurred")]
    Unknown(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum ListAuthorsError {
    #[error("Invalid limit: {0}")]
    InvalidLimit(usize),

    #[error("Invalid offset: {0}")]
    InvalidOffset(usize),

    #[error("Unexpected error occurred")]
    Unknown(#[from] anyhow::Error),
}
```