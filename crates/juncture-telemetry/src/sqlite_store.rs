//! SQLite-backed implementation of `TraceStore`.
//!
//! Uses `sqlx` with native async support. Schema is auto-created on
//! first open. Suitable for single-process deployments and development.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::debug;

use crate::models::{
    EnrichedSession, Id, ModelStats, Observation, ObservationLevel, ObservationType, Session,
    SummaryStats, TokenUsage, Trace,
};
use crate::trace_store::{
    DailyStats, PaginatedResponse, StoreError, TraceQuery, TraceStore, TraceWithObservations,
};

/// RAII guard that removes a transient `SQLite` database file (and its
/// WAL/SHM companions) when the last `SqliteStore` clone is dropped.
#[derive(Debug)]
struct TransientDbGuard(PathBuf);

impl Drop for TransientDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let wal = PathBuf::from(format!("{}-wal", self.0.display()));
        let shm = PathBuf::from(format!("{}-shm", self.0.display()));
        let _ = std::fs::remove_file(wal);
        let _ = std::fs::remove_file(shm);
    }
}

/// `SQLite` store backed by a `sqlx` connection pool.
///
/// All database operations are async and non-blocking. Stores created
/// via [`SqliteStore::new`] persist to the given file. Stores created
/// via [`SqliteStore::new_memory`] use a transient file that is
/// automatically deleted when the last clone is dropped.
#[derive(Clone, Debug)]
pub struct SqliteStore {
    pool: SqlitePool,
    _transient_guard: Option<Arc<TransientDbGuard>>,
}

impl SqliteStore {
    /// Create a new `SQLite` store at the given file path.
    ///
    /// The database and schema are created if they do not exist.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the database cannot be opened
    /// or the schema cannot be created.
    pub async fn new(path: &str) -> Result<Self, StoreError> {
        let url = format!("sqlite:{path}?mode=rwc");

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .map_err(|e| StoreError::Storage(format!("open db: {e}")))?;

        let store = Self {
            pool,
            _transient_guard: None,
        };
        store.create_schema().await?;
        Ok(store)
    }

    /// Create a transient `SQLite` store backed by a file in the system
    /// temp directory. The file and its WAL/SHM companions are removed
    /// automatically when the last clone is dropped.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the schema cannot be created.
    pub async fn new_memory() -> Result<Self, StoreError> {
        let id = uuid::Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("juncture-telemetry-{id}.db"));
        let path_str = path.to_string_lossy().to_string();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{path_str}?mode=rwc"))
            .await
            .map_err(|e| StoreError::Storage(format!("open db: {e}")))?;

        let store = Self {
            pool,
            _transient_guard: Some(Arc::new(TransientDbGuard(path))),
        };
        store.create_schema().await?;
        Ok(store)
    }

    async fn create_schema(&self) -> Result<(), StoreError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS traces (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                user_id TEXT,
                session_id TEXT,
                tags TEXT NOT NULL DEFAULT '[]',
                metadata TEXT NOT NULL DEFAULT 'null',
                environment TEXT,
                release_version TEXT,
                input TEXT,
                output TEXT,
                start_time TEXT NOT NULL,
                end_time TEXT,
                total_cost REAL,
                total_tokens INTEGER
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("create traces table: {e}")))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("create session index: {e}")))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_traces_user ON traces(user_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("create user index: {e}")))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_traces_start ON traces(start_time)")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("create start index: {e}")))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY,
                trace_id TEXT NOT NULL REFERENCES traces(id),
                parent_observation_id TEXT,
                name TEXT NOT NULL,
                observation_type TEXT NOT NULL,
                start_time TEXT NOT NULL,
                end_time TEXT,
                input TEXT,
                output TEXT,
                metadata TEXT NOT NULL DEFAULT 'null',
                level TEXT NOT NULL DEFAULT 'DEFAULT',
                status_message TEXT,
                model TEXT,
                model_parameters TEXT,
                usage_input_tokens INTEGER,
                usage_output_tokens INTEGER,
                usage_total_tokens INTEGER,
                usage_cached_tokens INTEGER,
                cost REAL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("create observations table: {e}")))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_obs_trace ON observations(trace_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("create obs trace index: {e}")))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_obs_start ON observations(start_time)")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("create obs start index: {e}")))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("create sessions table: {e}")))?;

        debug!("SQLite schema initialized");
        Ok(())
    }
}

#[async_trait::async_trait]
impl TraceStore for SqliteStore {
    async fn upsert_trace(&self, trace: &Trace) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT OR REPLACE INTO traces (
                id, name, user_id, session_id, tags, metadata,
                environment, release_version, input, output,
                start_time, end_time, total_cost, total_tokens
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(trace.id.to_string())
        .bind(&trace.name)
        .bind(&trace.user_id)
        .bind(&trace.session_id)
        .bind(serde_json::to_string(&trace.tags).unwrap_or_else(|_| "[]".to_string()))
        .bind(serde_json::to_string(&trace.metadata).unwrap_or_else(|_| "null".to_string()))
        .bind(&trace.environment)
        .bind(&trace.release)
        .bind(
            trace
                .input
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
        )
        .bind(
            trace
                .output
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
        )
        .bind(trace.start_time.to_rfc3339())
        .bind(trace.end_time.map(|t| t.to_rfc3339()))
        .bind(trace.total_cost)
        .bind(
            trace
                .total_tokens
                .map(|t| i64::try_from(t).unwrap_or(i64::MAX)),
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("insert trace: {e}")))?;
        Ok(())
    }

    async fn insert_observation(&self, observation: &Observation) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT OR REPLACE INTO observations (
                id, trace_id, parent_observation_id, name, observation_type,
                start_time, end_time, input, output, metadata,
                level, status_message, model, model_parameters,
                usage_input_tokens, usage_output_tokens, usage_total_tokens,
                usage_cached_tokens, cost
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?15, ?16, ?17, ?18, ?19)",
        )
        .bind(observation.id.to_string())
        .bind(observation.trace_id.to_string())
        .bind(observation.parent_observation_id.map(|id| id.to_string()))
        .bind(&observation.name)
        .bind(observation.observation_type.as_str())
        .bind(observation.start_time.to_rfc3339())
        .bind(observation.end_time.map(|t| t.to_rfc3339()))
        .bind(
            observation
                .input
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
        )
        .bind(
            observation
                .output
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
        )
        .bind(serde_json::to_string(&observation.metadata).unwrap_or_else(|_| "null".to_string()))
        .bind(observation.level.as_str())
        .bind(&observation.status_message)
        .bind(&observation.model)
        .bind(
            observation
                .model_parameters
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())),
        )
        .bind(
            observation
                .usage
                .as_ref()
                .map(|u| i64::try_from(u.input_tokens).unwrap_or(i64::MAX)),
        )
        .bind(
            observation
                .usage
                .as_ref()
                .map(|u| i64::try_from(u.output_tokens).unwrap_or(i64::MAX)),
        )
        .bind(
            observation
                .usage
                .as_ref()
                .map(|u| i64::try_from(u.total_tokens).unwrap_or(i64::MAX)),
        )
        .bind(observation.usage.as_ref().and_then(|u| {
            u.cached_tokens
                .map(|t| i64::try_from(t).unwrap_or(i64::MAX))
        }))
        .bind(observation.cost)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("insert observation: {e}")))?;
        Ok(())
    }

    async fn upsert_session(&self, session: &Session) -> Result<(), StoreError> {
        sqlx::query("INSERT OR REPLACE INTO sessions (id, user_id, created_at) VALUES (?, ?, ?)")
            .bind(&session.id)
            .bind(&session.user_id)
            .bind(session.created_at.to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("insert session: {e}")))?;
        Ok(())
    }

    async fn get_trace(&self, id: Id) -> Result<Option<TraceWithObservations>, StoreError> {
        let id_str = id.to_string();

        let row = sqlx::query_as::<_, TraceRow>(
            "SELECT id, name, user_id, session_id, tags, metadata,
                    environment, release_version, input, output,
                    start_time, end_time, total_cost, total_tokens
             FROM traces WHERE id = ?",
        )
        .bind(&id_str)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query trace: {e}")))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let trace = row.into_trace();

        let obs_rows = sqlx::query_as::<_, ObservationRow>(
            "SELECT id, trace_id, parent_observation_id, name, observation_type,
                    start_time, end_time, input, output, metadata,
                    level, status_message, model, model_parameters,
                    usage_input_tokens, usage_output_tokens, usage_total_tokens,
                    usage_cached_tokens, cost
             FROM observations WHERE trace_id = ? ORDER BY start_time",
        )
        .bind(&id_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query observations: {e}")))?;

        let observations = obs_rows
            .into_iter()
            .map(ObservationRow::into_observation)
            .collect::<Vec<_>>();

        Ok(Some(TraceWithObservations {
            trace,
            observations,
        }))
    }

    async fn query_traces(
        &self,
        query: &TraceQuery,
    ) -> Result<PaginatedResponse<Trace>, StoreError> {
        let page = query.page.unwrap_or(0);
        let page_size = i64::from(query.page_size.unwrap_or(50).min(500));
        let offset = i64::from(page) * page_size;

        // Build WHERE clause dynamically
        let mut conditions = vec!["1=1".to_string()];
        if query.session_id.is_some() {
            conditions.push("session_id = ?".to_string());
        }
        if query.user_id.is_some() {
            conditions.push("user_id = ?".to_string());
        }
        if query.name.is_some() {
            conditions.push("name LIKE ?".to_string());
        }
        if query.environment.is_some() {
            conditions.push("environment = ?".to_string());
        }
        if query.from_timestamp.is_some() {
            conditions.push("start_time >= ?".to_string());
        }
        if query.to_timestamp.is_some() {
            conditions.push("start_time <= ?".to_string());
        }

        let where_clause = conditions.join(" AND ");

        // Count query
        let count_sql = format!("SELECT COUNT(*) as cnt FROM traces WHERE {where_clause}");
        let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
        if let Some(ref sid) = query.session_id {
            count_q = count_q.bind(sid);
        }
        if let Some(ref uid) = query.user_id {
            count_q = count_q.bind(uid);
        }
        if let Some(ref name) = query.name {
            count_q = count_q.bind(format!("%{name}%"));
        }
        if let Some(ref env) = query.environment {
            count_q = count_q.bind(env);
        }
        if let Some(from) = query.from_timestamp {
            count_q = count_q.bind(from.to_rfc3339());
        }
        if let Some(to) = query.to_timestamp {
            count_q = count_q.bind(to.to_rfc3339());
        }

        let total_count: i64 = count_q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count traces: {e}")))?;

        // Data query
        let data_sql = format!(
            "SELECT id, name, user_id, session_id, tags, metadata,
                    environment, release_version, input, output,
                    start_time, end_time, total_cost, total_tokens
             FROM traces WHERE {where_clause}
             ORDER BY start_time DESC LIMIT ? OFFSET ?"
        );
        let mut data_q = sqlx::query_as::<_, TraceRow>(&data_sql);
        if let Some(ref sid) = query.session_id {
            data_q = data_q.bind(sid);
        }
        if let Some(ref uid) = query.user_id {
            data_q = data_q.bind(uid);
        }
        if let Some(ref name) = query.name {
            data_q = data_q.bind(format!("%{name}%"));
        }
        if let Some(ref env) = query.environment {
            data_q = data_q.bind(env);
        }
        if let Some(from) = query.from_timestamp {
            data_q = data_q.bind(from.to_rfc3339());
        }
        if let Some(to) = query.to_timestamp {
            data_q = data_q.bind(to.to_rfc3339());
        }
        data_q = data_q.bind(page_size);
        data_q = data_q.bind(offset);

        let rows = data_q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("query traces: {e}")))?;

        let traces = rows.into_iter().map(TraceRow::into_trace).collect();

        Ok(PaginatedResponse {
            data: traces,
            page,
            page_size: u32::try_from(page_size).unwrap_or(50),
            total_count: u64::try_from(total_count).unwrap_or(0),
        })
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, user_id, created_at FROM sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query session: {e}")))?;

        Ok(row.map(SessionRow::into_session))
    }

    async fn query_sessions(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedResponse<Session>, StoreError> {
        let page_size = i64::from(page_size.min(500));
        let offset = i64::from(page) * page_size;

        let total_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count sessions: {e}")))?;

        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id, user_id, created_at FROM sessions
             ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query sessions: {e}")))?;

        let sessions = rows.into_iter().map(SessionRow::into_session).collect();

        Ok(PaginatedResponse {
            data: sessions,
            page,
            page_size: u32::try_from(page_size).unwrap_or(50),
            total_count: u64::try_from(total_count).unwrap_or(0),
        })
    }

    async fn get_daily_stats(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<DailyStats>, StoreError> {
        let rows = sqlx::query_as::<_, DailyStatsRow>(
            "SELECT
                DATE(start_time) as date,
                COUNT(*) as trace_count,
                COALESCE(SUM(total_tokens), 0) as total_tokens,
                COALESCE(SUM(total_cost), 0.0) as total_cost
             FROM traces
             WHERE start_time >= ? AND start_time <= ?
             GROUP BY DATE(start_time)
             ORDER BY date",
        )
        .bind(from.to_rfc3339())
        .bind(to.to_rfc3339())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query stats: {e}")))?;

        Ok(rows.into_iter().map(DailyStatsRow::into_stats).collect())
    }

    async fn get_model_stats(&self) -> Result<Vec<ModelStats>, StoreError> {
        let rows = sqlx::query_as::<_, ModelStatsRow>(
            "SELECT model, COUNT(*) as call_count,
                COALESCE(SUM(usage_input_tokens), 0) as input_tokens,
                COALESCE(SUM(usage_output_tokens), 0) as output_tokens,
                COALESCE(SUM(cost), 0.0) as total_cost,
                COALESCE(AVG(CAST((julianday(end_time) - julianday(start_time)) * 86400000 AS REAL)), 0.0) as avg_latency_ms
             FROM observations WHERE model IS NOT NULL
             GROUP BY model ORDER BY total_cost DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query model stats: {e}")))?;

        Ok(rows
            .into_iter()
            .map(ModelStatsRow::into_model_stats)
            .collect())
    }

    async fn get_summary_stats(&self) -> Result<SummaryStats, StoreError> {
        let total_traces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM traces")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count traces: {e}")))?;

        let total_observations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM observations")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count observations: {e}")))?;

        let total_cost: f64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(total_cost), 0.0) FROM traces")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StoreError::Storage(format!("sum cost: {e}")))?;

        let total_tokens: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(total_tokens), 0) FROM traces")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StoreError::Storage(format!("sum tokens: {e}")))?;

        let error_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM observations WHERE level = 'ERROR'")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StoreError::Storage(format!("count errors: {e}")))?;

        let active_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count sessions: {e}")))?;

        // Latency percentiles: get all trace durations, sort, calculate p50/p95/p99
        let durations: Vec<f64> = sqlx::query_scalar(
            "SELECT CAST((julianday(end_time) - julianday(start_time)) * 86400000 AS REAL) as dur
             FROM traces WHERE end_time IS NOT NULL ORDER BY dur",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query latencies: {e}")))?;

        let (p50, p95, p99) = if durations.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            let len = durations.len();
            let p50_idx = len * 50 / 100;
            let p95_idx = len * 95 / 100;
            let p99_idx = (len * 99 / 100).min(len - 1);
            (durations[p50_idx], durations[p95_idx], durations[p99_idx])
        };

        Ok(SummaryStats {
            total_traces: u64::try_from(total_traces).unwrap_or(0),
            total_observations: u64::try_from(total_observations).unwrap_or(0),
            total_cost,
            total_tokens: u64::try_from(total_tokens).unwrap_or(0),
            error_count: u64::try_from(error_count).unwrap_or(0),
            active_sessions: u64::try_from(active_sessions).unwrap_or(0),
            latency_p50_ms: p50,
            latency_p95_ms: p95,
            latency_p99_ms: p99,
        })
    }

    async fn query_enriched_sessions(
        &self,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedResponse<EnrichedSession>, StoreError> {
        let page_size = i64::from(page_size.min(500));
        let offset = i64::from(page) * page_size;

        let total_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Storage(format!("count sessions: {e}")))?;

        let rows = sqlx::query_as::<_, EnrichedSessionRow>(
            "SELECT s.id, s.user_id, s.created_at,
                COUNT(t.id) as trace_count,
                COALESCE(SUM(t.total_cost), 0.0) as total_cost,
                COALESCE(SUM(t.total_tokens), 0) as total_tokens,
                MAX(t.start_time) as last_active
             FROM sessions s LEFT JOIN traces t ON t.session_id = s.id
             GROUP BY s.id ORDER BY s.created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Storage(format!("query enriched sessions: {e}")))?;

        let sessions = rows
            .into_iter()
            .map(EnrichedSessionRow::into_enriched_session)
            .collect();

        Ok(PaginatedResponse {
            data: sessions,
            page,
            page_size: u32::try_from(page_size).unwrap_or(50),
            total_count: u64::try_from(total_count).unwrap_or(0),
        })
    }

    async fn flush(&self) -> Result<(), StoreError> {
        // SQLite with WAL mode auto-flushes. No-op.
        Ok(())
    }
}

// ── sqlx row types ──────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct TraceRow {
    id: String,
    name: String,
    user_id: Option<String>,
    session_id: Option<String>,
    tags: String,
    metadata: String,
    environment: Option<String>,
    release_version: Option<String>,
    input: Option<String>,
    output: Option<String>,
    start_time: String,
    end_time: Option<String>,
    total_cost: Option<f64>,
    total_tokens: Option<i64>,
}

impl TraceRow {
    fn into_trace(self) -> Trace {
        Trace {
            id: Id::parse_str(&self.id).unwrap_or_else(|_| Id::nil()),
            name: self.name,
            user_id: self.user_id,
            session_id: self.session_id,
            tags: serde_json::from_str(&self.tags).unwrap_or_default(),
            metadata: serde_json::from_str(&self.metadata).unwrap_or(serde_json::Value::Null),
            environment: self.environment,
            release: self.release_version,
            input: self.input.and_then(|s| serde_json::from_str(&s).ok()),
            output: self.output.and_then(|s| serde_json::from_str(&s).ok()),
            start_time: DateTime::parse_from_rfc3339(&self.start_time)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            end_time: self.end_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
            total_cost: self.total_cost,
            total_tokens: self.total_tokens.and_then(|t| u64::try_from(t).ok()),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ObservationRow {
    id: String,
    trace_id: String,
    parent_observation_id: Option<String>,
    name: String,
    observation_type: String,
    start_time: String,
    end_time: Option<String>,
    input: Option<String>,
    output: Option<String>,
    metadata: String,
    level: String,
    status_message: Option<String>,
    model: Option<String>,
    model_parameters: Option<String>,
    usage_input_tokens: Option<i64>,
    usage_output_tokens: Option<i64>,
    usage_total_tokens: Option<i64>,
    usage_cached_tokens: Option<i64>,
    cost: Option<f64>,
}

impl ObservationRow {
    fn into_observation(self) -> Observation {
        let observation_type = match self.observation_type.as_str() {
            "GENERATION" => ObservationType::Generation,
            "TOOL_CALL" => ObservationType::ToolCall,
            "RETRIEVAL" => ObservationType::Retrieval,
            _ => ObservationType::Span,
        };

        let level = match self.level.as_str() {
            "DEBUG" => ObservationLevel::Debug,
            "WARNING" => ObservationLevel::Warning,
            "ERROR" => ObservationLevel::Error,
            _ => ObservationLevel::Default,
        };

        let usage = (self.usage_input_tokens.is_some()
            || self.usage_output_tokens.is_some()
            || self.usage_total_tokens.is_some())
        .then(|| TokenUsage {
            input_tokens: self
                .usage_input_tokens
                .and_then(|t| u64::try_from(t).ok())
                .unwrap_or(0),
            output_tokens: self
                .usage_output_tokens
                .and_then(|t| u64::try_from(t).ok())
                .unwrap_or(0),
            total_tokens: self
                .usage_total_tokens
                .and_then(|t| u64::try_from(t).ok())
                .unwrap_or(0),
            cached_tokens: self.usage_cached_tokens.and_then(|t| u64::try_from(t).ok()),
        });

        Observation {
            id: Id::parse_str(&self.id).unwrap_or_else(|_| Id::nil()),
            trace_id: Id::parse_str(&self.trace_id).unwrap_or_else(|_| Id::nil()),
            parent_observation_id: self
                .parent_observation_id
                .and_then(|s| Id::parse_str(&s).ok()),
            name: self.name,
            observation_type,
            start_time: DateTime::parse_from_rfc3339(&self.start_time)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            end_time: self.end_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
            input: self.input.and_then(|s| serde_json::from_str(&s).ok()),
            output: self.output.and_then(|s| serde_json::from_str(&s).ok()),
            metadata: serde_json::from_str(&self.metadata).unwrap_or(serde_json::Value::Null),
            level,
            status_message: self.status_message,
            model: self.model,
            model_parameters: self
                .model_parameters
                .and_then(|s| serde_json::from_str(&s).ok()),
            usage,
            cost: self.cost,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    user_id: Option<String>,
    created_at: String,
}

impl SessionRow {
    fn into_session(self) -> Session {
        Session {
            id: self.id,
            user_id: self.user_id,
            created_at: DateTime::parse_from_rfc3339(&self.created_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
        }
    }
}

#[derive(sqlx::FromRow)]
struct DailyStatsRow {
    date: String,
    trace_count: i64,
    total_tokens: i64,
    total_cost: f64,
}

impl DailyStatsRow {
    fn into_stats(self) -> DailyStats {
        DailyStats {
            date: self.date,
            trace_count: u64::try_from(self.trace_count).unwrap_or(0),
            observation_count: 0,
            total_tokens: u64::try_from(self.total_tokens).unwrap_or(0),
            total_cost: self.total_cost,
            total_duration_ms: 0,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ModelStatsRow {
    model: String,
    call_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    total_cost: f64,
    avg_latency_ms: f64,
}

impl ModelStatsRow {
    fn into_model_stats(self) -> ModelStats {
        ModelStats {
            model: self.model,
            call_count: u64::try_from(self.call_count).unwrap_or(0),
            input_tokens: u64::try_from(self.input_tokens).unwrap_or(0),
            output_tokens: u64::try_from(self.output_tokens).unwrap_or(0),
            total_cost: self.total_cost,
            avg_latency_ms: self.avg_latency_ms,
        }
    }
}

#[derive(sqlx::FromRow)]
struct EnrichedSessionRow {
    id: String,
    user_id: Option<String>,
    created_at: String,
    trace_count: i64,
    total_cost: f64,
    total_tokens: i64,
    last_active: Option<String>,
}

impl EnrichedSessionRow {
    fn into_enriched_session(self) -> EnrichedSession {
        EnrichedSession {
            id: self.id,
            user_id: self.user_id,
            created_at: self.created_at,
            trace_count: u64::try_from(self.trace_count).unwrap_or(0),
            total_cost: self.total_cost,
            total_tokens: u64::try_from(self.total_tokens).unwrap_or(0),
            last_active: self.last_active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_store_create_and_insert_trace() {
        let store = SqliteStore::new_memory().await.unwrap();
        let mut trace = Trace::new("test_graph");
        trace.session_id = Some("thread-1".to_string());
        trace.user_id = Some("user-1".to_string());
        trace.complete(None, Some(0.05), Some(100));

        store.upsert_trace(&trace).await.unwrap();

        let loaded = store.get_trace(trace.id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.trace.name, "test_graph");
        assert_eq!(loaded.trace.session_id.as_deref(), Some("thread-1"));
        assert_eq!(loaded.trace.total_cost, Some(0.05));
    }

    #[tokio::test]
    async fn sqlite_store_insert_observation() {
        let store = SqliteStore::new_memory().await.unwrap();
        let trace = Trace::new("test_graph");
        store.upsert_trace(&trace).await.unwrap();

        let mut obs = Observation::generation(trace.id, "llm_call", "claude-sonnet-4-20250514");
        obs.input = Some(serde_json::json!({"prompt": "hello"}));
        obs.usage = Some(TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cached_tokens: None,
        });
        obs.cost = Some(0.003);
        obs.complete(Some(serde_json::json!({"text": "hi there"})));

        store.insert_observation(&obs).await.unwrap();

        let loaded = store.get_trace(trace.id).await.unwrap().unwrap();
        assert_eq!(loaded.observations.len(), 1);
        assert_eq!(loaded.observations[0].name, "llm_call");
        assert_eq!(
            loaded.observations[0].observation_type,
            ObservationType::Generation
        );
    }

    #[tokio::test]
    async fn sqlite_store_query_traces() {
        let store = SqliteStore::new_memory().await.unwrap();

        for i in 0..5 {
            let mut trace = Trace::new(format!("graph_{i}"));
            trace.session_id = Some("thread-1".to_string());
            store.upsert_trace(&trace).await.unwrap();
        }

        let query = TraceQuery {
            session_id: Some("thread-1".to_string()),
            ..Default::default()
        };
        let result = store.query_traces(&query).await.unwrap();
        assert_eq!(result.data.len(), 5);
        assert_eq!(result.total_count, 5);
    }

    #[tokio::test]
    async fn sqlite_store_sessions() {
        let store = SqliteStore::new_memory().await.unwrap();

        let session = Session::new("thread-1");
        store.upsert_session(&session).await.unwrap();

        let loaded = store.get_session("thread-1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, "thread-1");

        let pages = store.query_sessions(0, 10).await.unwrap();
        assert_eq!(pages.data.len(), 1);
    }

    #[tokio::test]
    async fn sqlite_store_nested_observations() {
        let store = SqliteStore::new_memory().await.unwrap();
        let trace = Trace::new("test_graph");
        store.upsert_trace(&trace).await.unwrap();

        let mut superstep = Observation::span(trace.id, "juncture.superstep");
        superstep.complete(None);
        store.insert_observation(&superstep).await.unwrap();

        let mut node =
            Observation::span(trace.id, "juncture.node.execute").with_parent(superstep.id);
        node.complete(None);
        store.insert_observation(&node).await.unwrap();

        let mut llm = Observation::generation(trace.id, "llm_call", "model").with_parent(node.id);
        llm.complete(None);
        store.insert_observation(&llm).await.unwrap();

        let loaded = store.get_trace(trace.id).await.unwrap().unwrap();
        assert_eq!(loaded.observations.len(), 3);

        let llm_loaded = loaded
            .observations
            .iter()
            .find(|o| o.name == "llm_call")
            .unwrap();
        assert_eq!(llm_loaded.parent_observation_id, Some(node.id));
    }

    #[tokio::test]
    async fn sqlite_store_daily_stats() {
        let store = SqliteStore::new_memory().await.unwrap();

        let mut trace = Trace::new("test");
        trace.total_cost = Some(0.05);
        trace.total_tokens = Some(100);
        store.upsert_trace(&trace).await.unwrap();

        let from = Utc::now() - chrono::Duration::days(1);
        let to = Utc::now() + chrono::Duration::days(1);
        let stats = store.get_daily_stats(from, to).await.unwrap();
        assert!(!stats.is_empty());
    }
}
