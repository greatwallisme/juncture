```rust
// domain/services/tests.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use crate::domain::models::*;

    #[derive(Clone)]
    struct MockRepository {
        create_result: Arc<Mutex<Result<Author, CreateAuthorError>>>,
        find_by_id_result: Arc<Mutex<Result<Option<Author>, FindAuthorError>>>,
        find_by_email_result: Arc<Mutex<Result<Option<Author>, FindAuthorError>>>,
    }

    #[async_trait]
    impl AuthorRepository for MockRepository {
        async fn create_author(&self, _request: &CreateAuthorRequest) -> Result<Author, CreateAuthorError> {
            let mut guard = self.create_result.lock().unwrap();
            std::mem::replace(&mut *guard, Err(CreateAuthorError::Unknown(
                anyhow::anyhow!("Mock consumed")
            )))
        }

        async fn find_by_id(&self, _id: &AuthorId) -> Result<Option<Author>, FindAuthorError> {
            let mut guard = self.find_by_id_result.lock().unwrap();
            std::mem::replace(&mut *guard, Ok(None))
        }

        async fn find_by_email(&self, _email: &Email) -> Result<Option<Author>, FindAuthorError> {
            let mut guard = self.find_by_email_result.lock().unwrap();
            std::mem::replace(&mut *guard, Ok(None))
        }

        // Other methods...
        async fn update_author(&self, _id: &AuthorId, _request: &UpdateAuthorRequest) -> Result<Author, UpdateAuthorError> {
            unimplemented!("Not needed for this test")
        }

        async fn delete_author(&self, _id: &AuthorId) -> Result<(), DeleteAuthorError> {
            unimplemented!("Not needed for this test")
        }

        async fn list_authors(&self, _limit: usize, _offset: usize) -> Result<Vec<Author>, ListAuthorsError> {
            unimplemented!("Not needed for this test")
        }
    }

    #[derive(Clone)]
    struct MockMetrics;

    #[async_trait]
    impl AuthorMetrics for MockMetrics {
        async fn record_author_created(&self) {}
        async fn record_author_creation_failed(&self) {}
        async fn record_author_updated(&self) {}
        async fn record_author_deleted(&self) {}
    }

    #[derive(Clone)]
    struct MockNotifier;

    #[async_trait]
    impl AuthorNotifier for MockNotifier {
        async fn send_welcome_email(&self, _author: &Author) {}
        async fn send_deletion_notification(&self, _author: &Author) {}
    }

    #[derive(Clone)]
    struct MockValidator;

    #[async_trait]
    impl AuthorValidator for MockValidator {
        async fn validate_author_creation(&self, _request: &CreateAuthorRequest) -> Result<(), ValidationError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_create_author_success() {
        let author = Author {
            id: AuthorId::from_string("test-id".to_string()).unwrap(),
            name: AuthorName::new("Test Author".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        };

        let repo = MockRepository {
            create_result: Arc::new(Mutex::new(Ok(author.clone()))),
            find_by_id_result: Arc::new(Mutex::new(Ok(Some(author.clone())))),
            find_by_email_result: Arc::new(Mutex::new(Ok(None))),
        };

        let metrics = MockMetrics;
        let notifier = MockNotifier;
        let validator = MockValidator;

        let service = AuthorServiceImpl::new(repo, metrics, notifier, validator);

        let request = CreateAuthorRequest {
            name: AuthorName::new("Test Author".to_string()).unwrap(),
            email: Email::new("test@example.com".to_string()).unwrap(),
        };

        let result = service.create_author(&request).await;
        assert!(result.is_ok());

        let created_author = result.unwrap();
        assert_eq!(created_author.name.as_str(), "Test Author");
        assert_eq!(created_author.email.as_str(), "test@example.com");
    }

    #[tokio::test]
    async fn test_create_author_duplicate_email() {
        let existing_author = Author {
            id: AuthorId::from_string("existing-id".to_string()).unwrap(),
            name: AuthorName::new("Existing Author".to_string()).unwrap(),
            email: Email::new("existing@example.com".to_string()).unwrap(),
        };

        let repo = MockRepository {
            create_result: Arc::new(Mutex::new(Ok(existing_author))),
            find_by_id_result: Arc::new(Mutex::new(Ok(None))),
            find_by_email_result: Arc::new(Mutex::new(Ok(Some(existing_author)))),
        };

        let metrics = MockMetrics;
        let notifier = MockNotifier;
        let validator = MockValidator;

        let service = AuthorServiceImpl::new(repo, metrics, notifier, validator);

        let request = CreateAuthorRequest {
            name: AuthorName::new("New Author".to_string()).unwrap(),
            email: Email::new("existing@example.com".to_string()).unwrap(),
        };

        let result = service.create_author(&request).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            CreateAuthorError::DuplicateAuthor(email) => {
                assert_eq!(email, "existing@example.com");
            }
            _ => panic!("Expected DuplicateAuthor error"),
        }
    }
}
```