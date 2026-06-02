//! Langfuse-compatible REST API handlers.
//!
//! Implements the core Langfuse public API endpoints so that the
//! Langfuse frontend UI can be pointed directly at Juncture's
//! embedded telemetry server.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{Id, Observation, ObservationLevel, TokenUsage, Trace};
use crate::trace_store::{TraceQuery, TraceStore};

/// Shared application state for all API handlers.
#[derive(Clone)]
struct AppState {
    store: Arc<dyn TraceStore>,
}

/// Optional authentication credentials.
/// When set, the server validates Basic Auth headers against these values.
/// When `None`, all requests are accepted (no auth required).
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    /// Langfuse public key (username in Basic Auth).
    pub public_key: Option<String>,
    /// Langfuse secret key (password in Basic Auth).
    pub secret_key: Option<String>,
}

/// Create the axum router with all Langfuse-compatible endpoints,
/// OTLP ingest, and the embedded dashboard UI.
pub fn create_router(store: Arc<dyn TraceStore>) -> Router {
    create_router_with_auth(store, AuthConfig::default())
}

/// Create the axum router with optional Langfuse-compatible authentication.
///
/// When `auth` has both `public_key` and `secret_key` set, the server
/// validates Basic Auth headers on API endpoints. Dashboard and health
/// endpoints remain unauthenticated.
///
/// This allows pointing Langfuse SDK directly at the server:
///
/// ```env
/// LANGFUSE_SECRET_KEY=sk-lf-...
/// LANGFUSE_PUBLIC_KEY=pk-lf-...
/// LANGFUSE_HOST=http://127.0.0.1:8123
/// ```
pub fn create_router_with_auth(store: Arc<dyn TraceStore>, auth: AuthConfig) -> Router {
    let state = AppState {
        store: Arc::clone(&store),
    };

    // OTLP ingest routes (merged into main router)
    let otlp_router = crate::otlp::http::create_otlp_router(store);

    let mut router = Router::new()
        .route("/", get(crate::web::dashboard::serve_dashboard))
        .route("/api/public/ingestion", post(handle_ingestion))
        .route("/api/public/traces", get(handle_query_traces))
        .route("/api/public/traces/{trace_id}", get(handle_get_trace))
        .route("/api/public/sessions", get(handle_query_sessions))
        .route("/api/public/sessions/{session_id}", get(handle_get_session))
        .route("/api/public/stats/daily", get(handle_daily_stats))
        .route("/api/public/stats/models", get(handle_model_stats))
        .route("/api/public/stats/summary", get(handle_summary_stats))
        .route(
            "/api/public/sessions/enriched",
            get(handle_enriched_sessions),
        )
        .with_state(state)
        .merge(otlp_router);

    // Add Basic Auth middleware if credentials are configured
    if let (Some(pk), Some(sk)) = (auth.public_key, auth.secret_key) {
        router = router.layer(axum::middleware::from_fn(move |req, next| {
            let pk = pk.clone();
            let sk = sk.clone();
            async move { auth_middleware(req, next, pk, sk).await }
        }));
    }

    router
}

/// Basic Auth middleware compatible with Langfuse SDK.
///
/// Langfuse SDK sends `Authorization: Basic base64(public_key:secret_key)`.
/// Dashboard and health endpoints are exempt from auth.
async fn auth_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    public_key: String,
    secret_key: String,
) -> axum::response::Response {
    let path = req.uri().path();

    // Skip auth for dashboard, health, and static assets
    if path == "/" || path == "/health" || !path.starts_with("/api/") {
        return next.run(req).await;
    }

    // Extract Basic Auth header
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(encoded) = auth_header.strip_prefix("Basic ") {
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) {
            if let Ok(creds) = String::from_utf8(decoded) {
                let parts: Vec<&str> = creds.splitn(2, ':').collect();
                if parts.len() == 2 && parts[0] == public_key && parts[1] == secret_key {
                    return next.run(req).await;
                }
            }
        }
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "Unauthorized"})),
    )
        .into_response()
}

// ── Ingestion ──────────────────────────────────────────────────

/// Langfuse ingestion request body.
#[derive(Debug, Deserialize)]
struct IngestionRequest {
    batch: Vec<IngestionItem>,
}

/// A single item in an ingestion batch.
#[derive(Debug, Deserialize)]
struct IngestionItem {
    id: Option<String>,
    #[serde(rename = "type")]
    item_type: String,
    body: serde_json::Value,
    #[expect(dead_code, reason = "consumed by serde but not used in processing")]
    timestamp: Option<String>,
}

/// Ingestion response.
#[derive(Debug, Serialize)]
struct IngestionResponse {
    successes: Vec<IngestionSuccess>,
    errors: Vec<IngestionError>,
}

#[derive(Debug, Serialize)]
struct IngestionSuccess {
    id: String,
    status: u16,
}

#[derive(Debug, Serialize)]
struct IngestionError {
    id: String,
    status: u16,
    message: String,
}

/// Handle batch ingestion of traces and observations.
///
/// Accepts the Langfuse ingestion format with a `batch` array of
/// typed items (`trace-create`, `generation-create`, `span-create`, etc.).
async fn handle_ingestion(
    State(state): State<AppState>,
    Json(req): Json<IngestionRequest>,
) -> impl IntoResponse {
    let mut successes = Vec::new();
    let mut errors = Vec::new();

    for item in req.batch {
        let item_id = item
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let result = match item.item_type.as_str() {
            "trace-create" | "trace-update" => handle_trace_ingestion(&state, &item.body).await,
            "generation-create" | "generation-update" => {
                handle_generation_ingestion(&state, &item.body).await
            }
            "span-create" | "span-update" => handle_span_ingestion(&state, &item.body).await,
            "event-create" => handle_event_ingestion(&state, &item.body).await,
            _ => {
                // Unknown type - skip silently
                successes.push(IngestionSuccess {
                    id: item_id,
                    status: 200,
                });
                continue;
            }
        };

        match result {
            Ok(()) => successes.push(IngestionSuccess {
                id: item_id,
                status: 200,
            }),
            Err(e) => errors.push(IngestionError {
                id: item_id,
                status: 500,
                message: e.to_string(),
            }),
        }
    }

    (
        StatusCode::OK,
        Json(IngestionResponse { successes, errors }),
    )
}

async fn handle_trace_ingestion(
    state: &AppState,
    body: &serde_json::Value,
) -> Result<(), crate::trace_store::StoreError> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok())
        .unwrap_or_else(Id::new_v4);

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();

    let mut trace = Trace::new(name);
    trace.id = id;
    trace.session_id = body
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(String::from);
    trace.user_id = body
        .get("userId")
        .and_then(|v| v.as_str())
        .map(String::from);
    trace.tags = body
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    trace.metadata = body
        .get("metadata")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    trace.environment = body
        .get("environment")
        .and_then(|v| v.as_str())
        .map(String::from);
    trace.release = body
        .get("release")
        .and_then(|v| v.as_str())
        .map(String::from);
    trace.input = body.get("input").cloned();
    trace.output = body.get("output").cloned();

    // Parse timestamps if provided
    if let Some(ts) = body.get("timestamp").and_then(|v| v.as_str()) {
        if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
            trace.start_time = dt.with_timezone(&Utc);
        }
    }

    state.store.upsert_trace(&trace).await
}

async fn handle_generation_ingestion(
    state: &AppState,
    body: &serde_json::Value,
) -> Result<(), crate::trace_store::StoreError> {
    let trace_id = body
        .get("traceId")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok())
        .unwrap_or_else(Id::new_v4);

    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok())
        .unwrap_or_else(Id::new_v4);

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("llm_call")
        .to_string();

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let mut obs = Observation::generation(trace_id, name, model);
    obs.id = id;
    obs.parent_observation_id = body
        .get("parentObservationId")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok());

    // Capture input (prompt)
    if let Some(input) = body.get("input") {
        obs.input = Some(input.clone());
    }
    // Langfuse uses "completion" for output
    if let Some(output) = body.get("output").or_else(|| body.get("completion")) {
        obs.output = Some(output.clone());
    }

    // Model parameters
    if let Some(params) = body.get("modelParameters") {
        obs.model_parameters = Some(params.clone());
    }

    // Usage -- supports both Langfuse formats:
    //   "usage": {"inputTokens": N, "outputTokens": N, "totalTokens": N}
    //   "usageDetails": {"input": N, "output": N, "total": N}
    if let Some(usage) = body.get("usage").or_else(|| body.get("usageDetails")) {
        let input = usage
            .get("inputTokens")
            .or_else(|| usage.get("promptTokens"))
            .or_else(|| usage.get("input"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("outputTokens")
            .or_else(|| usage.get("completionTokens"))
            .or_else(|| usage.get("output"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let total = usage
            .get("totalTokens")
            .or_else(|| usage.get("total"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(input + output);
        obs.usage = Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: total,
            cached_tokens: usage
                .get("cachedTokens")
                .or_else(|| usage.get("promptTokensDetails"))
                .and_then(|v| v.get("cachedTokens"))
                .and_then(serde_json::Value::as_u64),
        });
    }

    // Cost -- supports both formats:
    //   "cost": 0.001
    //   "costDetails": {"input": 0.001, "output": 0.002, "total": 0.003}
    if let Some(cost) = body.get("cost").or_else(|| body.get("totalCost")) {
        obs.cost = cost.as_f64();
    } else if let Some(details) = body.get("costDetails") {
        obs.cost = details.get("total").and_then(serde_json::Value::as_f64);
    }

    // Level
    if let Some(level) = body.get("level").and_then(|v| v.as_str()) {
        obs.level = match level {
            "DEBUG" => ObservationLevel::Debug,
            "WARNING" => ObservationLevel::Warning,
            "ERROR" => ObservationLevel::Error,
            _ => ObservationLevel::Default,
        };
    }

    obs.metadata = body
        .get("metadata")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    state.store.insert_observation(&obs).await
}

async fn handle_span_ingestion(
    state: &AppState,
    body: &serde_json::Value,
) -> Result<(), crate::trace_store::StoreError> {
    let trace_id = body
        .get("traceId")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok())
        .unwrap_or_else(Id::new_v4);

    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok())
        .unwrap_or_else(Id::new_v4);

    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("span")
        .to_string();

    let mut obs = Observation::span(trace_id, name);
    obs.id = id;
    obs.parent_observation_id = body
        .get("parentObservationId")
        .and_then(|v| v.as_str())
        .and_then(|s| Id::parse_str(s).ok());
    obs.input = body.get("input").cloned();
    obs.output = body.get("output").cloned();
    obs.metadata = body
        .get("metadata")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    state.store.insert_observation(&obs).await
}

async fn handle_event_ingestion(
    state: &AppState,
    body: &serde_json::Value,
) -> Result<(), crate::trace_store::StoreError> {
    // Events are stored as observations with type Span
    handle_span_ingestion(state, body).await
}

// ── Trace queries ──────────────────────────────────────────────

/// Query parameters for trace listing.
#[derive(Debug, Deserialize)]
struct TraceQueryParams {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "userId")]
    user_id: Option<String>,
    name: Option<String>,
    environment: Option<String>,
    #[serde(rename = "fromTimestamp")]
    from_timestamp: Option<String>,
    #[serde(rename = "toTimestamp")]
    to_timestamp: Option<String>,
    page: Option<u32>,
    #[serde(rename = "pageSize")]
    page_size: Option<u32>,
}

/// Handle trace listing with filters and pagination.
async fn handle_query_traces(
    State(state): State<AppState>,
    Query(params): Query<TraceQueryParams>,
) -> impl IntoResponse {
    let query = TraceQuery {
        session_id: params.session_id,
        user_id: params.user_id,
        name: params.name,
        environment: params.environment,
        tags: Vec::new(),
        from_timestamp: params
            .from_timestamp
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        to_timestamp: params
            .to_timestamp
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        page: params.page,
        page_size: params.page_size,
    };

    match state.store.query_traces(&query).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Handle getting a single trace with its observations.
async fn handle_get_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    let Ok(id) = Id::parse_str(&trace_id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid trace id"})),
        )
            .into_response();
    };

    match state.store.get_trace(id).await {
        Ok(Some(result)) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "trace not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Session queries ────────────────────────────────────────────

/// Query parameters for session listing.
#[derive(Debug, Deserialize)]
struct SessionQueryParams {
    page: Option<u32>,
    #[serde(rename = "pageSize")]
    page_size: Option<u32>,
}

/// Handle session listing with pagination.
async fn handle_query_sessions(
    State(state): State<AppState>,
    Query(params): Query<SessionQueryParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(0);
    let page_size = params.page_size.unwrap_or(50);

    match state.store.query_sessions(page, page_size).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Handle getting a single session.
async fn handle_get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_session(&session_id).await {
        Ok(Some(session)) => (StatusCode::OK, Json(serde_json::json!(session))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "session not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Daily stats ────────────────────────────────────────────────

/// Query parameters for daily statistics.
#[derive(Debug, Deserialize)]
struct DailyStatsParams {
    #[serde(rename = "from")]
    from: Option<String>,
    #[serde(rename = "to")]
    to: Option<String>,
}

/// Handle daily aggregated statistics.
async fn handle_daily_stats(
    State(state): State<AppState>,
    Query(params): Query<DailyStatsParams>,
) -> impl IntoResponse {
    let now = Utc::now();
    let from = params
        .from
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map_or(now - chrono::Duration::days(30), |dt| {
            dt.with_timezone(&Utc)
        });
    let to = params
        .to
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map_or(now, |dt| dt.with_timezone(&Utc));

    match state.store.get_daily_stats(from, to).await {
        Ok(daily_stats) => (StatusCode::OK, Json(serde_json::json!(daily_stats))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Handle per-model aggregated statistics.
async fn handle_model_stats(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.get_model_stats().await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Handle overall summary statistics.
async fn handle_summary_stats(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.get_summary_stats().await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Handle enriched sessions listing.
async fn handle_enriched_sessions(
    State(state): State<AppState>,
    Query(params): Query<SessionQueryParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(0);
    let page_size = params.page_size.unwrap_or(50);

    match state.store.query_enriched_sessions(page, page_size).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ObservationType;
    use crate::sqlite_store::SqliteStore;

    #[tokio::test]
    async fn api_create_router() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let _router = create_router(store);
    }

    #[tokio::test]
    async fn api_ingestion_trace_create() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());

        // Directly test trace ingestion logic
        let body = serde_json::json!({
            "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
            "name": "my-graph",
            "sessionId": "session-1",
            "userId": "user-1",
            "tags": ["production"],
            "metadata": {"key": "value"}
        });

        let cloned = Arc::clone(&store);
        let state = AppState { store: cloned };
        handle_trace_ingestion(&state, &body).await.unwrap();

        let trace_id = Id::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
        let loaded = store.get_trace(trace_id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.trace.name, "my-graph");
        assert_eq!(loaded.trace.session_id.as_deref(), Some("session-1"));
    }

    #[tokio::test]
    async fn api_ingestion_generation_create() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());

        // First create the trace
        let trace_body = serde_json::json!({
            "id": "b1c2d3e4-f5a6-7890-bcde-f12345678901",
            "name": "test"
        });
        let cloned = Arc::clone(&store);
        let state = AppState { store: cloned };
        handle_trace_ingestion(&state, &trace_body).await.unwrap();

        // Then create a generation
        let gen_body = serde_json::json!({
            "id": "c2d3e4f5-a6b7-8901-cdef-123456789012",
            "traceId": "b1c2d3e4-f5a6-7890-bcde-f12345678901",
            "name": "llm_call",
            "model": "claude-sonnet-4-20250514",
            "input": {"messages": [{"role": "user", "content": "hello"}]},
            "output": "hi there",
            "usage": {
                "inputTokens": 10,
                "outputTokens": 5,
                "totalTokens": 15
            },
            "cost": 0.001
        });
        handle_generation_ingestion(&state, &gen_body)
            .await
            .unwrap();

        let trace_id = Id::parse_str("b1c2d3e4-f5a6-7890-bcde-f12345678901").unwrap();
        let loaded = store.get_trace(trace_id).await.unwrap().unwrap();
        assert_eq!(loaded.observations.len(), 1);
        assert_eq!(
            loaded.observations[0].observation_type,
            ObservationType::Generation
        );
    }
}
