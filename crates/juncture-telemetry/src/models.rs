//! Data models for the Langfuse-compatible observability engine.
//!
//! Defines `Trace`, `Observation`, `Session`, and supporting types that
//! form the core telemetry data model. All types are serializable for
//! API responses and storage.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a trace or observation.
pub type Id = Uuid;

/// Trace represents a single graph invocation or request lifecycle.
///
/// A trace is the top-level container that groups all observations
/// (spans, LLM calls, tool calls) generated during one execution.
/// It maps to Langfuse's `Trace` concept and Juncture's `thread_id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trace {
    /// Unique trace identifier.
    pub id: Id,
    /// Human-readable name (typically the graph name).
    pub name: String,
    /// User identifier for per-user cost/quality tracking.
    pub user_id: Option<String>,
    /// Session identifier for multi-turn conversation grouping.
    /// Maps to Juncture's `thread_id`.
    pub session_id: Option<String>,
    /// Flexible string labels for categorization and filtering.
    pub tags: Vec<String>,
    /// Arbitrary key-value metadata.
    pub metadata: serde_json::Value,
    /// Deployment environment (production, staging, development).
    pub environment: Option<String>,
    /// Application release version.
    pub release: Option<String>,
    /// Graph input captured at invocation time.
    pub input: Option<serde_json::Value>,
    /// Graph output captured at completion time.
    pub output: Option<serde_json::Value>,
    /// Trace start timestamp.
    pub start_time: DateTime<Utc>,
    /// Trace end timestamp (set when graph completes).
    pub end_time: Option<DateTime<Utc>>,
    /// Aggregated total cost in USD across all LLM calls.
    pub total_cost: Option<f64>,
    /// Aggregated total tokens consumed.
    pub total_tokens: Option<u64>,
}

impl Trace {
    /// Create a new trace with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            user_id: None,
            session_id: None,
            tags: Vec::new(),
            metadata: serde_json::Value::Null,
            environment: None,
            release: None,
            input: None,
            output: None,
            start_time: Utc::now(),
            end_time: None,
            total_cost: None,
            total_tokens: None,
        }
    }

    /// Mark the trace as completed with optional output and aggregated metrics.
    pub fn complete(
        &mut self,
        output: Option<serde_json::Value>,
        total_cost: Option<f64>,
        total_tokens: Option<u64>,
    ) {
        self.end_time = Some(Utc::now());
        self.output = output;
        self.total_cost = total_cost;
        self.total_tokens = total_tokens;
    }
}

/// Type of observation within a trace.
///
/// Maps to Langfuse's observation types. Each type captures
/// different aspects of agent execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationType {
    /// Generic span covering any timed operation.
    Span,
    /// LLM generation call with prompt/completion data.
    Generation,
    /// Tool invocation with input/output.
    ToolCall,
    /// RAG retrieval step with query and documents.
    Retrieval,
}

impl ObservationType {
    /// Returns the string representation for storage and API use.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Span => "SPAN",
            Self::Generation => "GENERATION",
            Self::ToolCall => "TOOL_CALL",
            Self::Retrieval => "RETRIEVAL",
        }
    }
}

impl std::fmt::Display for ObservationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Severity level for observations.
///
/// Follows Langfuse's level convention for filtering
/// and alerting on error/warning conditions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationLevel {
    /// Detailed diagnostic information.
    Debug,
    /// Normal operational information.
    Default,
    /// Potential issue that does not prevent operation.
    Warning,
    /// Operation failed or produced an error.
    Error,
}

impl ObservationLevel {
    /// Returns the string representation for storage and API use.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Default => "DEFAULT",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

impl std::fmt::Display for ObservationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Token usage statistics for an LLM call.
///
/// Captures input/output/total tokens and optional cached tokens
/// for cost calculation and budget tracking.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Number of input/prompt tokens.
    pub input_tokens: u64,
    /// Number of output/completion tokens.
    pub output_tokens: u64,
    /// Total tokens (input + output).
    pub total_tokens: u64,
    /// Number of cached input tokens (prompt caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

impl From<juncture_core::state::messages::TokenUsage> for TokenUsage {
    fn from(usage: juncture_core::state::messages::TokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: None,
        }
    }
}

/// Observation represents a single unit of work within a trace.
///
/// Observations form a tree structure via `parent_observation_id`.
/// A graph invocation creates observations for each superstep,
/// node execution, LLM call, and tool call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    /// Unique observation identifier.
    pub id: Id,
    /// Parent trace identifier.
    pub trace_id: Id,
    /// Parent observation identifier for nesting (None = top-level).
    pub parent_observation_id: Option<Id>,
    /// Human-readable name (node name, model name, tool name).
    pub name: String,
    /// Type of observation.
    pub observation_type: ObservationType,
    /// Observation start timestamp.
    pub start_time: DateTime<Utc>,
    /// Observation end timestamp.
    pub end_time: Option<DateTime<Utc>>,
    /// Input data (prompt messages, tool arguments, etc.).
    pub input: Option<serde_json::Value>,
    /// Output data (completion text, tool result, etc.).
    pub output: Option<serde_json::Value>,
    /// Arbitrary key-value metadata.
    pub metadata: serde_json::Value,
    /// Severity level.
    pub level: ObservationLevel,
    /// Human-readable status message (error details, etc.).
    pub status_message: Option<String>,
    // Generation-specific fields
    /// LLM model name (e.g., "claude-sonnet-4-20250514").
    pub model: Option<String>,
    /// Model parameters (temperature, `max_tokens`, etc.).
    pub model_parameters: Option<serde_json::Value>,
    /// Token usage statistics.
    pub usage: Option<TokenUsage>,
    /// Cost in USD for this call.
    pub cost: Option<f64>,
}

impl Observation {
    /// Create a new span observation.
    #[must_use]
    pub fn span(trace_id: Id, name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            trace_id,
            parent_observation_id: None,
            name: name.into(),
            observation_type: ObservationType::Span,
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Value::Null,
            level: ObservationLevel::Default,
            status_message: None,
            model: None,
            model_parameters: None,
            usage: None,
            cost: None,
        }
    }

    /// Create a new LLM generation observation.
    #[must_use]
    pub fn generation(trace_id: Id, name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            trace_id,
            parent_observation_id: None,
            name: name.into(),
            observation_type: ObservationType::Generation,
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Value::Null,
            level: ObservationLevel::Default,
            status_message: None,
            model: Some(model.into()),
            model_parameters: None,
            usage: None,
            cost: None,
        }
    }

    /// Create a new tool call observation.
    #[must_use]
    pub fn tool_call(trace_id: Id, name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            trace_id,
            parent_observation_id: None,
            name: name.into(),
            observation_type: ObservationType::ToolCall,
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Value::Null,
            level: ObservationLevel::Default,
            status_message: None,
            model: None,
            model_parameters: None,
            usage: None,
            cost: None,
        }
    }

    /// Set the parent observation for nesting.
    #[must_use]
    pub const fn with_parent(mut self, parent_id: Id) -> Self {
        self.parent_observation_id = Some(parent_id);
        self
    }

    /// Mark the observation as completed with optional output.
    pub fn complete(&mut self, output: Option<serde_json::Value>) {
        self.end_time = Some(Utc::now());
        self.output = output;
    }

    /// Mark the observation as failed with an error message.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.end_time = Some(Utc::now());
        self.level = ObservationLevel::Error;
        self.status_message = Some(message.into());
    }

    /// Duration in milliseconds (if completed).
    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.end_time.map(|end| {
            let duration = end.signed_duration_since(self.start_time);
            u64::try_from(duration.num_milliseconds().max(0)).unwrap_or(0)
        })
    }
}

/// Session groups multiple traces from the same user interaction.
///
/// Maps to Juncture's `thread_id` concept. A session typically
/// represents a multi-turn conversation or workflow.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Session identifier (typically the `thread_id`).
    pub id: String,
    /// User identifier.
    pub user_id: Option<String>,
    /// Session creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl Session {
    /// Create a new session with the given identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            user_id: None,
            created_at: Utc::now(),
        }
    }
}

/// Configuration for LLM prompt/response capture.
///
/// Controls how much data is captured during LLM calls.
/// Allows full capture in development and truncated capture
/// in production for privacy and performance.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    /// Maximum prompt content length in characters.
    /// Content beyond this limit is truncated with a marker.
    pub max_prompt_chars: usize,
    /// Maximum response content length in characters.
    pub max_response_chars: usize,
    /// Whether to capture the full messages array from LLM calls.
    pub capture_full_messages: bool,
    /// Whether to capture tool input/output data.
    pub capture_tool_io: bool,
    /// Sensitive field keys to redact from captured data.
    pub sensitive_keys: Vec<String>,
}

/// Per-model aggregated statistics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    /// Model name (e.g., "claude-sonnet-4-20250514").
    pub model: String,
    /// Number of LLM calls using this model.
    pub call_count: u64,
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens produced.
    pub output_tokens: u64,
    /// Total cost in USD.
    pub total_cost: f64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
}

/// Overall summary statistics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryStats {
    /// Total number of traces.
    pub total_traces: u64,
    /// Total number of observations.
    pub total_observations: u64,
    /// Total cost in USD across all traces.
    pub total_cost: f64,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Number of error-level observations.
    pub error_count: u64,
    /// Number of active (non-completed) sessions.
    pub active_sessions: u64,
    /// Median latency in milliseconds.
    pub latency_p50_ms: f64,
    /// 95th percentile latency in milliseconds.
    pub latency_p95_ms: f64,
    /// 99th percentile latency in milliseconds.
    pub latency_p99_ms: f64,
}

/// Enriched session with aggregated data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichedSession {
    /// Session identifier.
    pub id: String,
    /// User identifier.
    pub user_id: Option<String>,
    /// Session creation timestamp (ISO 8601).
    pub created_at: String,
    /// Number of traces in this session.
    pub trace_count: u64,
    /// Total cost across all traces in this session.
    pub total_cost: f64,
    /// Total tokens across all traces in this session.
    pub total_tokens: u64,
    /// Timestamp of the most recent trace activity.
    pub last_active: Option<String>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            max_prompt_chars: 10_000,
            max_response_chars: 10_000,
            capture_full_messages: true,
            capture_tool_io: true,
            sensitive_keys: vec![
                "authorization".to_string(),
                "api_key".to_string(),
                "api-key".to_string(),
                "password".to_string(),
                "secret".to_string(),
                "token".to_string(),
            ],
        }
    }
}

impl CaptureConfig {
    /// Truncate a string to the configured maximum length.
    /// Returns the original string if within limits, otherwise
    /// truncates and appends a marker.
    #[must_use]
    pub fn truncate(&self, content: &str, max_chars: usize) -> String {
        if content.len() <= max_chars {
            content.to_string()
        } else {
            let truncated: String = content.chars().take(max_chars).collect();
            format!("{truncated}\n... [truncated at {max_chars} chars]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_new_has_id_and_name() {
        let trace = Trace::new("test_graph");
        assert!(!trace.name.is_empty());
        assert!(trace.end_time.is_none());
    }

    #[test]
    fn trace_complete_sets_end_time() {
        let mut trace = Trace::new("test_graph");
        trace.complete(None, Some(0.05), Some(100));
        assert!(trace.end_time.is_some());
        assert_eq!(trace.total_cost, Some(0.05));
        assert_eq!(trace.total_tokens, Some(100));
    }

    #[test]
    fn observation_span_factory() {
        let trace_id = Uuid::new_v4();
        let obs = Observation::span(trace_id, "juncture.node.execute");
        assert_eq!(obs.observation_type, ObservationType::Span);
        assert_eq!(obs.name, "juncture.node.execute");
        assert!(obs.end_time.is_none());
    }

    #[test]
    fn observation_generation_factory() {
        let trace_id = Uuid::new_v4();
        let obs = Observation::generation(trace_id, "llm_call", "claude-sonnet-4-20250514");
        assert_eq!(obs.observation_type, ObservationType::Generation);
        assert_eq!(obs.model.as_deref(), Some("claude-sonnet-4-20250514"));
    }

    #[test]
    fn observation_tool_call_factory() {
        let trace_id = Uuid::new_v4();
        let obs = Observation::tool_call(trace_id, "search");
        assert_eq!(obs.observation_type, ObservationType::ToolCall);
    }

    #[test]
    fn observation_complete_and_fail() {
        let trace_id = Uuid::new_v4();
        let mut obs = Observation::span(trace_id, "test");
        obs.complete(Some(serde_json::json!({"result": "ok"})));
        assert!(obs.end_time.is_some());
        assert_eq!(obs.level, ObservationLevel::Default);

        let mut obs2 = Observation::span(trace_id, "test2");
        obs2.fail("something broke");
        assert!(obs2.end_time.is_some());
        assert_eq!(obs2.level, ObservationLevel::Error);
        assert!(obs2.status_message.is_some());
    }

    #[test]
    fn observation_with_parent() {
        let trace_id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let obs = Observation::span(trace_id, "child").with_parent(parent_id);
        assert_eq!(obs.parent_observation_id, Some(parent_id));
    }

    #[test]
    fn observation_duration_ms() {
        let trace_id = Uuid::new_v4();
        let mut obs = Observation::span(trace_id, "test");
        assert!(obs.duration_ms().is_none());
        obs.complete(None);
        assert!(obs.duration_ms().is_some());
    }

    #[test]
    fn observation_type_display() {
        assert_eq!(ObservationType::Span.to_string(), "SPAN");
        assert_eq!(ObservationType::Generation.to_string(), "GENERATION");
        assert_eq!(ObservationType::ToolCall.to_string(), "TOOL_CALL");
        assert_eq!(ObservationType::Retrieval.to_string(), "RETRIEVAL");
    }

    #[test]
    fn session_new() {
        let session = Session::new("thread-123");
        assert_eq!(session.id, "thread-123");
        assert!(session.user_id.is_none());
    }

    #[test]
    fn capture_config_truncate() {
        let config = CaptureConfig::default();
        let short = "hello";
        assert_eq!(config.truncate(short, 100), "hello");

        let long = "a".repeat(15_000);
        let truncated = config.truncate(&long, 10_000);
        assert!(truncated.len() > 10_000);
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn capture_config_default_sensitive_keys() {
        let config = CaptureConfig::default();
        assert!(config.sensitive_keys.contains(&"authorization".to_string()));
        assert!(config.sensitive_keys.contains(&"api_key".to_string()));
    }

    #[test]
    fn token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage.cached_tokens.is_none());
    }
}
