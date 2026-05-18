// EXPERT NOTE: HTTP handlers are the thinnest layer in hexagonal architecture.
// Their ONLY job is conversion: HTTP -> Domain -> HTTP.
// Handlers should NOT contain business logic, database calls, or orchestration.

```rust
// inbound/http/handlers.rs
use axum::{
    extract::{State, Path, Query},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::domain::{
    models::{AuthorId, AuthorName, Email, UpdateAuthorRequest},
    ports::AuthorService,
    errors::*,
};

// EXPERT NOTE: HTTP request/response models are transport-layer concerns.
// They're DTOs (Data Transfer Objects), not domain models.
// This separation allows domain entities to remain pure and independent of HTTP.

#[derive(Deserialize)]
pub struct CreateAuthorHttpRequest {
    pub name: String,
    pub email: String,
}

#[derive(Deserialize)]
pub struct UpdateAuthorHttpRequest {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct AuthorHttpResponse {
    pub id: String,
    pub name: String,
    pub email: String,
}

#[derive(Serialize)]
pub struct AuthorsListResponse {
    pub authors: Vec<AuthorHttpResponse>,
    pub total: usize,
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_offset")]
    pub offset: usize,
}

fn default_limit() -> usize { 20 }
fn default_offset() -> usize { 0 }

// EXPERT NOTE: TryFrom traits centralize HTTP->Domain conversion logic.
// This keeps handlers focused on flow, not validation details.
// Domain types guarantee validity after conversion.

impl TryFrom<CreateAuthorHttpRequest> for CreateAuthorRequest {
    type Error = ValidationError;

    fn try_from(value: CreateAuthorHttpRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            name: AuthorName::new(value.name)?,
            email: Email::new(value.email)?,
        })
    }
}

impl TryFrom<UpdateAuthorHttpRequest> for UpdateAuthorRequest {
    type Error = ValidationError;

    fn try_from(value: UpdateAuthorHttpRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name.map(AuthorName::new).transpose()?,
            email: value.email.map(Email::new).transpose()?,
        })
    }
}

// EXPERT NOTE: From trait for Domain -> HTTP conversion.
// Alternative: Serialize domain entities directly. Trade-off:
// - DTO approach: Domain independent of serialization, more flexibility
// - Direct serialization: Less boilerplate, domain coupled to format

impl From<&Author> for AuthorHttpResponse {
    fn from(author: &Author) -> Self {
        Self {
            id: author.id.as_str().to_string(),
            name: author.name.as_str().to_string(),
            email: author.email.as_str().to_string(),
        }
    }
}

// EXPERT NOTE: Generic over S: AuthorService enables handler testing with mock services.
// Arc<S> because handlers need shared ownership across async requests.
// State<S> is axum's dependency injection mechanism.

pub async fn create_author<S>(
    State(service): State<Arc<S>>,
    Json(body): Json<CreateAuthorHttpRequest>,
) -> Result<(StatusCode, Json<AuthorHttpResponse>), ApiError>
where
    S: AuthorService,
{
    // Convert HTTP -> Domain
    let request = CreateAuthorRequest::try_from(body)?;

    // Call service (all business logic here)
    let author = service.create_author(&request).await?;

    // Convert Domain -> HTTP
    Ok((
        StatusCode::CREATED,
        Json(AuthorHttpResponse::from(&author))
    ))
}

// EXPERT NOTE: Pattern repeats for all handlers:
// 1. Extract state and request body
// 2. Convert HTTP types to domain types
// 3. Call service method
// 4. Convert domain result to HTTP response
// 5. Error conversions handle mapping to status codes

pub async fn get_author<S>(
    State(service): State<Arc<S>>,
    Path(id): Path<String>,
) -> Result<Json<AuthorHttpResponse>, ApiError>
where
    S: AuthorService,
{
    let author_id = AuthorId::from_string(id)
        .map_err(|_| ApiError::BadRequest("Invalid author ID format".to_string()))?;

    let author = service.get_author(&author_id).await?
        .ok_or(ApiError::NotFound("Author not found".to_string()))?;

    Ok(Json(AuthorHttpResponse::from(&author)))
}

pub async fn update_author<S>(
    State(service): State<Arc<S>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAuthorHttpRequest>,
) -> Result<Json<AuthorHttpResponse>, ApiError>
where
    S: AuthorService,
{
    let author_id = AuthorId::from_string(id)
        .map_err(|_| ApiError::BadRequest("Invalid author ID format".to_string()))?;

    let request = UpdateAuthorRequest::try_from(body)?;
    let author = service.update_author(&author_id, &request).await?;

    Ok(Json(AuthorHttpResponse::from(&author)))
}

pub async fn delete_author<S>(
    State(service): State<Arc<S>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError>
where
    S: AuthorService,
{
    let author_id = AuthorId::from_string(id)
        .map_err(|_| ApiError::BadRequest("Invalid author ID format".to_string()))?;

    service.delete_author(&author_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_authors<S>(
    State(service): State<Arc<S>>,
    Query(params): Query<ListParams>,
) -> Result<Json<AuthorsListResponse>, ApiError>
where
    S: AuthorService,
{
    let authors = service.list_authors(params.limit, params.offset).await?;
    let total = authors.len();

    let response = AuthorsListResponse {
        authors: authors.iter().map(AuthorHttpResponse::from).collect(),
        total,
    };

    Ok(Json(response))
}

// EXPERT NOTE: ApiError is HTTP-specific, not domain errors.
// This separation allows domain errors to evolve independently
// from HTTP status code mappings. New HTTP formats? New error type.
// Business rule changes? Update domain errors, handlers unchanged.

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    ValidationError(String),
    InternalServerError,
}

// EXPERT NOTE: From implementations map domain errors to HTTP errors.
// This is where business errors become status codes.
// Rule of thumb:
// - Client errors (4xx): User can fix the request
// - Server errors (5xx): Bug or infrastructure issue

impl From<CreateAuthorError> for ApiError {
    fn from(err: CreateAuthorError) -> Self {
        match err {
            CreateAuthorError::DuplicateAuthor(email) => {
                ApiError::Conflict(format!("Author with email {} already exists", email))
            }
            CreateAuthorError::ValidationError(e) => {
                ApiError::ValidationError(e.to_string())
            }
            CreateAuthorError::ValidationFailed(msg) => {
                ApiError::ValidationError(msg)
            }
            CreateAuthorError::Unknown(e) => {
                tracing::error!("Internal error creating author: {:?}", e);
                ApiError::InternalServerError
            }
        }
    }
}

impl From<FindAuthorError> for ApiError {
    fn from(err: FindAuthorError) -> Self {
        match err {
            FindAuthorError::NotFound => ApiError::NotFound("Author not found".to_string()),
            FindAuthorError::InvalidId(msg) => ApiError::BadRequest(msg),
            FindAuthorError::Unknown(e) => {
                tracing::error!("Internal error finding author: {:?}", e);
                ApiError::InternalServerError
            }
        }
    }
}

// EXPERT NOTE: IntoResponse for axum integration.
// This determines the actual HTTP response format.
// Log technical errors before conversion - 500s should be actionable in logs.

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::ValidationError(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            ApiError::InternalServerError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
        };

        (status, axum::Json(serde_json::json!({"error": message}))).into_response()
    }
}
```

// EXPERT NOTE: Handler testing strategy:
// 1. Create mock service implementing AuthorService
// 2. Configure mock return values
// 3. Call handler with axum TestClient
// 4. Assert status code and response body
//
// This tests HTTP concerns only (status mapping, JSON conversion).
// Business logic is tested in service tests. Don't duplicate tests.
