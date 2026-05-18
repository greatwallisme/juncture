
```rust
// outbound/tests/sqlite_tests.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn setup_test_db() -> Sqlite {
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_str().unwrap();

        // Create in-memory SQLite for tests
        Sqlite::new(":memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_sqlite_create_author() {
        let repo = setup_test_db().await;

        let request = CreateAuthorRequest {
            name: AuthorName::new("Test Author".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        };

        let result = repo.create_author(&request).await;
        assert!(result.is_ok());

        let author = result.unwrap();
        assert_eq!(author.name.as_str(), "Test Author");
        assert_eq!(author.email.as_str(), "test@example.com");
        assert!(!author.id.as_str().is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_find_by_id() {
        let repo = setup_test_db().await;

        // Create author first
        let request = CreateAuthorRequest {
            name: AuthorName::new("Test Author".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        };

        let created = repo.create_author(&request).await.unwrap();

        // Find by ID
        let found = repo.find_by_id(&created.id).await.unwrap();
        assert!(found.is_some());

        let found_author = found.unwrap();
        assert_eq!(found_author.id, created.id);
        assert_eq!(found_author.name.as_str(), "Test Author");
    }

    #[tokio::test]
    async fn test_sqlite_duplicate_email() {
        let repo = setup_test_db().await;

        let request = CreateAuthorRequest {
            name: AuthorName::new("Test Author".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        };

        // Create first author
        repo.create_author(&request).await.unwrap();

        // Try to create second author with same email
        let result = repo.create_author(&request).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            CreateAuthorError::DuplicateAuthor(email) => {
                assert_eq!(email, "test@example.com");
            }
            _ => panic!("Expected DuplicateAuthor error"),
        }
    }
}
```