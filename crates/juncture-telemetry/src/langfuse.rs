//! Langfuse cloud exporter.
//!
//! Sends telemetry data to Langfuse cloud via the REST ingestion API.
//! Runs asynchronously in the background, non-blocking to the hot path.

use tracing::debug;

use crate::models::{Observation, Trace};

/// Configuration for Langfuse cloud export.
#[derive(Clone, Debug)]
pub struct LangfuseConfig {
    /// Langfuse public key.
    pub public_key: String,
    /// Langfuse secret key.
    pub secret_key: String,
    /// Langfuse API base URL (e.g., `https://cloud.langfuse.com`).
    pub base_url: String,
}

/// Exports telemetry data to Langfuse cloud via REST API.
///
/// Uses the `/api/public/ingestion` endpoint with Basic Auth.
/// Supports `trace-create`, `generation-create`, and `span-create` types.
#[derive(Clone, Debug)]
pub struct LangfuseExporter {
    config: LangfuseConfig,
    client: reqwest::Client,
}

impl LangfuseExporter {
    /// Create a new Langfuse exporter.
    #[must_use]
    pub fn new(config: LangfuseConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Export a trace and its observations to Langfuse cloud.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or Langfuse returns errors.
    pub async fn export(
        &self,
        trace: &Trace,
        observations: &[Observation],
    ) -> Result<(), LangfuseExportError> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut batch = Vec::new();
        batch.push(Self::build_trace_item(trace, &now));
        for obs in observations {
            batch.push(Self::build_obs_item(obs, &now));
        }
        self.send_batch(&batch).await
    }

    fn build_trace_item(trace: &Trace, now: &str) -> serde_json::Value {
        serde_json::json!({
            "id": format!("juncture-trace-{}", trace.id),
            "type": "trace-create",
            "timestamp": now,
            "body": {
                "id": trace.id.to_string(),
                "name": trace.name,
                "sessionId": trace.session_id,
                "userId": trace.user_id,
                "tags": trace.tags,
                "metadata": trace.metadata,
                "input": trace.input,
                "output": trace.output,
            }
        })
    }

    fn build_obs_item(obs: &Observation, now: &str) -> serde_json::Value {
        let obs_type = match obs.observation_type {
            crate::models::ObservationType::Generation => "generation-create",
            _ => "span-create",
        };

        let mut body = serde_json::json!({
            "id": obs.id.to_string(),
            "traceId": obs.trace_id.to_string(),
            "name": obs.name,
            "startTime": obs.start_time.to_rfc3339(),
            "endTime": obs.end_time.map(|t| t.to_rfc3339()),
            "input": obs.input,
            "output": obs.output,
            "metadata": obs.metadata,
            "level": obs.level.as_str(),
        });

        if let Some(ref model) = obs.model {
            body["model"] = serde_json::Value::String(model.clone());
        }
        if let Some(ref usage) = obs.usage {
            body["usageDetails"] = serde_json::json!({
                "input": usage.input_tokens,
                "output": usage.output_tokens,
                "total": usage.total_tokens,
            });
        }
        if let Some(cost) = obs.cost {
            body["costDetails"] = serde_json::json!({"total": cost});
        }
        if let Some(parent_id) = obs.parent_observation_id {
            body["parentObservationId"] = serde_json::Value::String(parent_id.to_string());
        }

        serde_json::json!({
            "id": format!("juncture-obs-{}", obs.id),
            "type": obs_type,
            "timestamp": now,
            "body": body,
        })
    }

    async fn send_batch(&self, batch: &[serde_json::Value]) -> Result<(), LangfuseExportError> {
        let resp = self
            .client
            .post(format!("{}/api/public/ingestion", self.config.base_url))
            .basic_auth(&self.config.public_key, Some(&self.config.secret_key))
            .json(&serde_json::json!({"batch": batch}))
            .send()
            .await
            .map_err(|e| LangfuseExportError::Network(e.to_string()))?;

        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| LangfuseExportError::Network(e.to_string()))?;

        if !status.is_success() {
            return Err(LangfuseExportError::Http(status.as_u16(), body.to_string()));
        }

        let error_count = body["errors"].as_array().map_or(0, Vec::len);
        if error_count > 0 {
            let msgs: Vec<String> = body["errors"].as_array().map_or_else(Vec::new, |arr| {
                arr.iter()
                    .filter_map(|e| e["message"].as_str().map(String::from))
                    .collect()
            });
            return Err(LangfuseExportError::Langfuse(msgs.join("; ")));
        }

        debug!("langfuse export: {} items sent", batch.len());
        Ok(())
    }
}

/// Errors from Langfuse cloud export.
#[derive(Debug, thiserror::Error)]
pub enum LangfuseExportError {
    /// Network error.
    #[error("network error: {0}")]
    Network(String),
    /// HTTP error status.
    #[error("HTTP {0}: {1}")]
    Http(u16, String),
    /// Langfuse API returned errors.
    #[error("langfuse errors: {0}")]
    Langfuse(String),
}
