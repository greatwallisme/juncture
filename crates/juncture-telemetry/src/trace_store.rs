//! Storage trait and query types for telemetry data.
//!
//! Defines the `TraceStore` async trait that abstracts over different
//! storage backends (`SQLite`, `PostgreSQL`, memory). All operations are
//! async to support non-blocking batch writes.

use crate::models::{EnrichedSession, Id, ModelStats, Observation, Session, SummaryStats, Trace};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Error type for trace store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Database connection or query error.
    #[error("storage error: {0}")]
    Storage(String),
    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Record not found.
    #[error("not found: {0}")]
    NotFound(String),
}

/// Query parameters for filtering traces.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceQuery {
    /// Filter by session (thread) identifier.
    pub session_id: Option<String>,
    /// Filter by user identifier.
    pub user_id: Option<String>,
    /// Filter by name (graph name).
    pub name: Option<String>,
    /// Filter by environment.
    pub environment: Option<String>,
    /// Filter by tags (AND logic).
    pub tags: Vec<String>,
    /// Start of time range (inclusive).
    pub from_timestamp: Option<DateTime<Utc>>,
    /// End of time range (inclusive).
    pub to_timestamp: Option<DateTime<Utc>>,
    /// Page number (0-indexed).
    pub page: Option<u32>,
    /// Page size (default 50).
    pub page_size: Option<u32>,
}

/// Paginated response wrapper.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedResponse<T> {
    /// Items in this page.
    pub data: Vec<T>,
    /// Current page number (0-indexed).
    pub page: u32,
    /// Items per page.
    pub page_size: u32,
    /// Total number of items matching the query.
    pub total_count: u64,
}

/// Daily aggregated statistics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStats {
    /// Date (YYYY-MM-DD).
    pub date: String,
    /// Number of traces.
    pub trace_count: u64,
    /// Total observations.
    pub observation_count: u64,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Total cost in USD.
    pub total_cost: f64,
    /// Total duration in milliseconds.
    pub total_duration_ms: u64,
}

/// Async trait for telemetry data storage.
///
/// Implementations must be `Send + Sync` to support concurrent
/// access from the batch writer and web server.
#[async_trait::async_trait]
pub trait TraceStore: Send + Sync + 'static {
    /// Insert or update a trace.
    async fn upsert_trace(&self, trace: &Trace) -> Result<(), StoreError>;

    /// Insert an observation.
    async fn insert_observation(&self, observation: &Observation) -> Result<(), StoreError>;

    /// Insert or update a session.
    async fn upsert_session(&self, session: &Session) -> Result<(), StoreError>;

    /// Get a trace by ID with its observations.
    async fn get_trace(&self, id: Id) -> Result<Option<TraceWithObservations>, StoreError>;

    /// Query traces with filtering and pagination.
    async fn query_traces(
        &self,
        query: &TraceQuery,
    ) -> Result<PaginatedResponse<Trace>, StoreError>;

    /// Get a session by ID.
    async fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError>;

    /// Query sessions with pagination.
    async fn query_sessions(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedResponse<Session>, StoreError>;

    /// Get daily aggregated statistics.
    async fn get_daily_stats(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyStats>, StoreError>;

    /// Get per-model aggregated statistics.
    async fn get_model_stats(&self) -> Result<Vec<ModelStats>, StoreError>;

    /// Get overall summary statistics with latency percentiles.
    async fn get_summary_stats(&self) -> Result<SummaryStats, StoreError>;

    /// Query enriched sessions with aggregated data.
    async fn query_enriched_sessions(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedResponse<EnrichedSession>, StoreError>;

    /// Flush any pending writes. Called before process exit.
    async fn flush(&self) -> Result<(), StoreError>;
}

/// A trace with its associated observations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceWithObservations {
    /// The trace.
    pub trace: Trace,
    /// Observations belonging to this trace, ordered by `start_time`.
    pub observations: Vec<Observation>,
}
