//! Telemetry collector - main entry point for the observability engine.
//!
//! The `TelemetryCollector` orchestrates trace/observation lifecycle,
//! delegates writes to the `BatchWriter`, and provides convenience
//! methods for common telemetry operations.

use std::sync::Arc;

use tracing::debug;

use crate::batch_writer::BatchWriter;
use crate::langfuse::{LangfuseConfig, LangfuseExporter};
use crate::models::{CaptureConfig, Id, Observation, Session, TokenUsage, Trace};
use crate::trace_store::{StoreError, TraceStore};

/// Main telemetry collector for Juncture graph execution.
///
/// Creates traces and observations, applies capture configuration,
/// and submits them to the batch writer for async persistence.
///
/// # Examples
///
/// ```ignore
/// use juncture_telemetry::{TelemetryCollector, SqliteStore};
/// use std::sync::Arc;
///
/// let store = Arc::new(SqliteStore::new("telemetry.db").await?);
/// let collector = TelemetryCollector::new(store);
///
/// let trace = collector.begin_trace("my_graph", Some("thread-1"));
/// let obs = collector.begin_llm_call(trace.id, "claude-sonnet-4-20250514");
/// // ... execute LLM call ...
/// collector.end_llm_call(obs.id, Some(response), usage, cost).await;
/// collector.end_trace(trace.id, Some(output), total_cost, total_tokens).await;
/// ```
#[derive(Clone, Debug)]
pub struct TelemetryCollector {
    writer: BatchWriter,
    capture_config: Arc<CaptureConfig>,
}

impl TelemetryCollector {
    /// Create a new collector with default capture configuration.
    #[must_use]
    pub fn new(store: Arc<dyn TraceStore>) -> Self {
        Self {
            writer: BatchWriter::new(store),
            capture_config: Arc::new(CaptureConfig::default()),
        }
    }

    /// Create a new collector with custom capture configuration.
    #[must_use]
    pub fn with_capture_config(store: Arc<dyn TraceStore>, config: CaptureConfig) -> Self {
        Self {
            writer: BatchWriter::new(store),
            capture_config: Arc::new(config),
        }
    }

    /// Create a new collector with Langfuse cloud export enabled.
    ///
    /// When configured, `flush()` and `shutdown()` automatically export
    /// traces and observations to Langfuse cloud alongside local storage.
    #[must_use]
    pub fn with_langfuse(
        store: Arc<dyn TraceStore>,
        config: CaptureConfig,
        langfuse_config: LangfuseConfig,
    ) -> Self {
        let exporter = LangfuseExporter::new(langfuse_config);
        Self {
            writer: BatchWriter::with_config_and_langfuse(store, Some(exporter), 50, 5_000),
            capture_config: Arc::new(config),
        }
    }

    /// Create a collector from pre-built components.
    ///
    /// Used by [`TelemetryConfig`](crate::config::TelemetryConfig) to
    /// construct a collector with a pre-configured batch writer.
    #[must_use]
    pub(crate) fn from_parts(writer: BatchWriter, config: CaptureConfig) -> Self {
        Self {
            writer,
            capture_config: Arc::new(config),
        }
    }

    /// Get the capture configuration.
    #[must_use]
    pub fn capture_config(&self) -> &CaptureConfig {
        &self.capture_config
    }

    // ── Trace lifecycle ──────────────────────────────────────────

    /// Begin a new trace for a graph invocation.
    ///
    /// The trace is immediately submitted to the buffer so that
    /// observations can reference it without FK constraint violations.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn begin_trace(
        &self,
        graph_name: impl Into<String>,
        session_id: Option<String>,
    ) -> Result<Trace, StoreError> {
        let mut trace = Trace::new(graph_name);
        trace.session_id = session_id;
        debug!(trace_id = %trace.id, name = %trace.name, "trace started");
        self.writer.submit_trace(trace.clone()).await?;
        Ok(trace)
    }

    /// End a trace and submit the completed version for async writing.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn end_trace(
        &self,
        mut trace: Trace,
        output: Option<serde_json::Value>,
        total_cost: Option<f64>,
        total_tokens: Option<u64>,
    ) -> Result<(), StoreError> {
        trace.complete(output, total_cost, total_tokens);
        debug!(
            trace_id = %trace.id,
            duration_ms = trace.end_time
                .map_or(0, |e| e.signed_duration_since(trace.start_time).num_milliseconds()),
            "trace ended"
        );
        self.writer.submit_trace(trace).await
    }

    // ── Session management ───────────────────────────────────────

    /// Create or update a session.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn track_session(
        &self,
        thread_id: impl Into<String>,
        user_id: Option<String>,
    ) -> Result<(), StoreError> {
        let mut session = Session::new(thread_id);
        session.user_id = user_id;
        self.writer.submit_session(session).await
    }

    // ── LLM call lifecycle ───────────────────────────────────────

    /// Begin an LLM call observation.
    #[must_use]
    pub fn begin_llm_call(
        &self,
        trace_id: Id,
        parent_id: Option<Id>,
        model: impl Into<String>,
        prompt: Option<&serde_json::Value>,
    ) -> Observation {
        let mut obs = Observation::generation(trace_id, "llm_call", model);
        obs.parent_observation_id = parent_id;
        if self.capture_config.capture_full_messages {
            if let Some(prompt) = prompt {
                let serialized = serde_json::to_string(prompt).unwrap_or_default();
                let truncated = self
                    .capture_config
                    .truncate(&serialized, self.capture_config.max_prompt_chars);
                obs.input = Some(serde_json::Value::String(truncated));
            }
        }
        obs
    }

    /// End an LLM call observation and submit it.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn end_llm_call(
        &self,
        mut obs: Observation,
        response: Option<&str>,
        usage: Option<TokenUsage>,
        cost: Option<f64>,
    ) -> Result<(), StoreError> {
        if let Some(response) = response {
            let truncated = self
                .capture_config
                .truncate(response, self.capture_config.max_response_chars);
            obs.output = Some(serde_json::Value::String(truncated));
        }
        obs.usage = usage;
        obs.cost = cost;
        obs.complete(obs.output.clone());
        self.writer.submit_observation(obs).await
    }

    // ── Tool call lifecycle ──────────────────────────────────────

    /// Begin a tool call observation.
    #[must_use]
    pub fn begin_tool_call(
        &self,
        trace_id: Id,
        parent_id: Option<Id>,
        tool_name: impl Into<String>,
        input: Option<&serde_json::Value>,
    ) -> Observation {
        let mut obs = Observation::tool_call(trace_id, tool_name);
        obs.parent_observation_id = parent_id;
        if self.capture_config.capture_tool_io {
            obs.input = input.cloned();
        }
        obs
    }

    /// End a tool call observation and submit it.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn end_tool_call(
        &self,
        mut obs: Observation,
        output: Option<serde_json::Value>,
    ) -> Result<(), StoreError> {
        if self.capture_config.capture_tool_io {
            obs.output = output;
        }
        obs.complete(obs.output.clone());
        self.writer.submit_observation(obs).await
    }

    // ── Generic span lifecycle ───────────────────────────────────

    /// Begin a generic span observation.
    #[must_use]
    pub fn begin_span(
        &self,
        trace_id: Id,
        parent_id: Option<Id>,
        name: impl Into<String>,
    ) -> Observation {
        let mut obs = Observation::span(trace_id, name);
        obs.parent_observation_id = parent_id;
        obs
    }

    /// End a span observation and submit it.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn end_span(
        &self,
        mut obs: Observation,
        output: Option<serde_json::Value>,
    ) -> Result<(), StoreError> {
        obs.complete(output);
        self.writer.submit_observation(obs).await
    }

    /// Record a failed span and submit it.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if the submission fails.
    pub async fn fail_span(
        &self,
        mut obs: Observation,
        error: impl Into<String>,
    ) -> Result<(), StoreError> {
        obs.fail(error);
        self.writer.submit_observation(obs).await
    }

    // ── Flush / Shutdown ─────────────────────────────────────────

    /// Flush any buffered telemetry items to the store.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if any write fails.
    pub async fn flush(&self) -> Result<(), StoreError> {
        self.writer.flush().await
    }

    /// Shutdown the collector, flushing all remaining items.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Storage` if any write fails.
    pub async fn shutdown(self) -> Result<(), StoreError> {
        self.writer.shutdown().await
    }
}

#[cfg(test)]
#[expect(
    clippy::clone_on_ref_ptr,
    reason = ".clone() needed for unsized coercion Arc<SqliteStore> -> Arc<dyn TraceStore>"
)]
mod tests {
    use super::*;
    use crate::sqlite_store::SqliteStore;

    #[tokio::test]
    async fn collector_trace_lifecycle() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let dyn_store: Arc<dyn TraceStore> = store.clone();
        let collector = TelemetryCollector::new(dyn_store);

        let mut trace = collector
            .begin_trace("test_graph", Some("thread-1".to_string()))
            .await
            .unwrap();
        trace.user_id = Some("user-1".to_string());
        let trace_id = trace.id;

        let obs = collector.begin_span(trace_id, None, "juncture.superstep");
        collector.end_span(obs, None).await.unwrap();

        collector
            .end_trace(
                trace,
                Some(serde_json::json!({"result": "ok"})),
                Some(0.05),
                Some(200),
            )
            .await
            .unwrap();

        collector.flush().await.unwrap();

        let loaded = store.get_trace(trace_id).await.unwrap();
        assert!(loaded.is_some(), "trace should exist");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.observations.len(), 1, "expected 1 observation");
    }

    #[tokio::test]
    async fn collector_llm_call_lifecycle() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let dyn_store: Arc<dyn TraceStore> = store.clone();
        let collector = TelemetryCollector::new(dyn_store);

        let trace = collector.begin_trace("test_graph", None).await.unwrap();
        let trace_id = trace.id;

        let obs = collector.begin_llm_call(
            trace_id,
            None,
            "claude-sonnet-4-20250514",
            Some(&serde_json::json!({"messages": [{"role": "user", "content": "hello"}]})),
        );

        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cached_tokens: None,
        };
        collector
            .end_llm_call(obs, Some("hi there"), Some(usage), Some(0.001))
            .await
            .unwrap();

        collector.end_trace(trace, None, None, None).await.unwrap();
        collector.flush().await.unwrap();

        let loaded = store.get_trace(trace_id).await.unwrap().unwrap();
        assert_eq!(loaded.observations.len(), 1);
        let llm_obs = &loaded.observations[0];
        assert!(llm_obs.input.is_some());
        assert!(llm_obs.output.is_some());
        assert!(llm_obs.usage.is_some());
    }

    #[tokio::test]
    async fn collector_tool_call_lifecycle() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let dyn_store: Arc<dyn TraceStore> = store.clone();
        let collector = TelemetryCollector::new(dyn_store);

        let trace = collector.begin_trace("test_graph", None).await.unwrap();
        let trace_id = trace.id;

        let obs = collector.begin_tool_call(
            trace_id,
            None,
            "search",
            Some(&serde_json::json!({"query": "rust async"})),
        );
        collector
            .end_tool_call(obs, Some(serde_json::json!({"results": ["item1"]})))
            .await
            .unwrap();

        collector.end_trace(trace, None, None, None).await.unwrap();
        collector.flush().await.unwrap();

        let loaded = store.get_trace(trace_id).await.unwrap().unwrap();
        assert_eq!(loaded.observations.len(), 1);
    }

    #[tokio::test]
    async fn collector_capture_truncation() {
        let config = CaptureConfig {
            max_prompt_chars: 20,
            max_response_chars: 20,
            ..Default::default()
        };
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let collector = TelemetryCollector::with_capture_config(store, config);

        let trace = collector.begin_trace("test_graph", None).await.unwrap();
        let long_prompt = serde_json::json!({"content": "a".repeat(1000)});
        let obs = collector.begin_llm_call(trace.id, None, "model", Some(&long_prompt));

        let input_str = obs.input.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(input_str.contains("truncated"));
    }

    #[tokio::test]
    async fn collector_session_tracking() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let dyn_store: Arc<dyn TraceStore> = store.clone();
        let collector = TelemetryCollector::new(dyn_store);

        collector
            .track_session("thread-1", Some("user-1".to_string()))
            .await
            .unwrap();
        collector.flush().await.unwrap();

        let session = store.get_session("thread-1").await.unwrap();
        assert!(session.is_some());
    }

    /// Verify multi-agent tracing: coordinator + researcher + writer agents
    /// with nested LLM calls and tool calls, forming a proper observation tree.
    #[tokio::test]
    async fn collector_multi_agent_tracing() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());
        let dyn_store: Arc<dyn TraceStore> = store.clone();
        let collector = TelemetryCollector::new(dyn_store);

        // Track session
        collector
            .track_session("multi-agent-session", Some("user-1".to_string()))
            .await
            .unwrap();

        // Start trace
        let mut trace = collector
            .begin_trace("research_pipeline", Some("multi-agent-session".to_string()))
            .await
            .unwrap();
        trace.user_id = Some("user-1".to_string());
        trace.tags = vec!["multi-agent".to_string()];
        let trace_id = trace.id;

        // ── Coordinator agent ────────────────────────────────
        let coordinator = collector.begin_span(trace_id, None, "coordinator_agent");

        // Coordinator LLM: decide routing
        let coord_llm = collector.begin_llm_call(
            trace_id,
            Some(coordinator.id),
            "gpt-4o",
            Some(&serde_json::json!({"messages": [
                {"role": "system", "content": "You are a coordinator."},
                {"role": "user", "content": "Research quantum computing"}
            ]})),
        );
        collector
            .end_llm_call(
                coord_llm,
                Some("Delegating to researcher and writer."),
                Some(TokenUsage {
                    input_tokens: 50,
                    output_tokens: 15,
                    total_tokens: 65,
                    cached_tokens: None,
                }),
                Some(0.0003),
            )
            .await
            .unwrap();

        collector.end_span(coordinator, None).await.unwrap();

        // ── Researcher agent ─────────────────────────────────
        let researcher = collector.begin_span(trace_id, None, "researcher_agent");

        // Researcher LLM: analyze query
        let res_llm1 = collector.begin_llm_call(
            trace_id,
            Some(researcher.id),
            "gpt-4o",
            Some(&serde_json::json!({"messages": [
                {"role": "user", "content": "Analyze: quantum computing state"}
            ]})),
        );
        collector
            .end_llm_call(
                res_llm1,
                Some("Key areas: error correction, qubit scaling."),
                Some(TokenUsage {
                    input_tokens: 80,
                    output_tokens: 30,
                    total_tokens: 110,
                    cached_tokens: None,
                }),
                Some(0.0005),
            )
            .await
            .unwrap();

        // Researcher tool: web search
        let res_tool = collector.begin_tool_call(
            trace_id,
            Some(researcher.id),
            "web_search",
            Some(&serde_json::json!({"query": "quantum computing 2025"})),
        );
        collector
            .end_tool_call(
                res_tool,
                Some(serde_json::json!({"results": ["IBM 1000-qubit processor"]})),
            )
            .await
            .unwrap();

        // Researcher LLM: synthesize
        let res_llm2 = collector.begin_llm_call(
            trace_id,
            Some(researcher.id),
            "gpt-4o",
            Some(&serde_json::json!({"messages": [
                {"role": "user", "content": "Synthesize findings"}
            ]})),
        );
        collector
            .end_llm_call(
                res_llm2,
                Some("Quantum computing has made significant progress."),
                Some(TokenUsage {
                    input_tokens: 120,
                    output_tokens: 40,
                    total_tokens: 160,
                    cached_tokens: None,
                }),
                Some(0.0007),
            )
            .await
            .unwrap();

        collector.end_span(researcher, None).await.unwrap();

        // ── Writer agent ─────────────────────────────────────
        let writer = collector.begin_span(trace_id, None, "writer_agent");

        let writer_llm = collector.begin_llm_call(
            trace_id,
            Some(writer.id),
            "gpt-4o",
            Some(&serde_json::json!({"messages": [
                {"role": "user", "content": "Write report based on: Quantum computing has made significant progress."}
            ]})),
        );
        collector
            .end_llm_call(
                writer_llm,
                Some("## Quantum Computing Report\n\nSignificant progress has been made..."),
                Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 80,
                    total_tokens: 180,
                    cached_tokens: None,
                }),
                Some(0.0008),
            )
            .await
            .unwrap();

        collector.end_span(writer, None).await.unwrap();

        // End trace
        collector
            .end_trace(
                trace,
                Some(serde_json::json!({"report": "Quantum computing report..."})),
                Some(0.0023),
                Some(515),
            )
            .await
            .unwrap();

        collector.flush().await.unwrap();

        // ── Verify observation tree ──────────────────────────
        let loaded = store.get_trace(trace_id).await.unwrap().unwrap();
        assert_eq!(
            loaded.observations.len(),
            8,
            "expected 8 observations (3 agents + 4 LLM + 1 tool)"
        );

        // Verify tree structure via parent_observation_id
        let agent_spans: Vec<_> = loaded
            .observations
            .iter()
            .filter(|o| o.parent_observation_id.is_none())
            .collect();
        assert_eq!(agent_spans.len(), 3, "expected 3 top-level agent spans");

        let coordinator_obs = loaded
            .observations
            .iter()
            .find(|o| o.name == "coordinator_agent")
            .unwrap();
        let researcher_obs = loaded
            .observations
            .iter()
            .find(|o| o.name == "researcher_agent")
            .unwrap();
        let writer_obs = loaded
            .observations
            .iter()
            .find(|o| o.name == "writer_agent")
            .unwrap();

        // Coordinator has 1 LLM call
        let coord_children: Vec<_> = loaded
            .observations
            .iter()
            .filter(|o| o.parent_observation_id == Some(coordinator_obs.id))
            .collect();
        assert_eq!(coord_children.len(), 1, "coordinator should have 1 child");
        assert_eq!(coord_children[0].name, "llm_call");

        // Researcher has 3 children: 2 LLM calls + 1 tool call
        let res_children: Vec<_> = loaded
            .observations
            .iter()
            .filter(|o| o.parent_observation_id == Some(researcher_obs.id))
            .collect();
        assert_eq!(res_children.len(), 3, "researcher should have 3 children");

        let res_generations: Vec<_> = res_children
            .iter()
            .filter(|o| o.observation_type == crate::models::ObservationType::Generation)
            .collect();
        let res_tools: Vec<_> = res_children
            .iter()
            .filter(|o| o.observation_type == crate::models::ObservationType::ToolCall)
            .collect();
        assert_eq!(
            res_generations.len(),
            2,
            "researcher should have 2 LLM calls"
        );
        assert_eq!(res_tools.len(), 1, "researcher should have 1 tool call");
        assert_eq!(res_tools[0].name, "web_search");

        // Writer has 1 LLM call
        let writer_children: Vec<_> = loaded
            .observations
            .iter()
            .filter(|o| o.parent_observation_id == Some(writer_obs.id))
            .collect();
        assert_eq!(writer_children.len(), 1, "writer should have 1 child");

        // Verify token usage and cost are recorded
        let total_input: u64 = loaded
            .observations
            .iter()
            .filter_map(|o| o.usage.as_ref())
            .map(|u| u.input_tokens)
            .sum();
        let total_output: u64 = loaded
            .observations
            .iter()
            .filter_map(|o| o.usage.as_ref())
            .map(|u| u.output_tokens)
            .sum();
        assert_eq!(total_input, 350, "total input tokens");
        assert_eq!(total_output, 165, "total output tokens");

        let total_cost: f64 = loaded.observations.iter().filter_map(|o| o.cost).sum();
        assert!(total_cost > 0.0, "total cost should be positive");
    }
}
