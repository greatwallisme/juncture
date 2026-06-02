//! HTTP handler for OTLP trace ingest.
//!
//! Accepts OTLP trace data in JSON format at `POST /v1/traces` and
//! converts it to Juncture's internal model for storage.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use tracing::{debug, warn};

use crate::otlp::{OtlpTraceRequest, convert_resource_spans};
use crate::trace_store::TraceStore;

/// Shared state for OTLP handlers.
#[derive(Clone)]
pub(crate) struct OtlpState {
    pub store: Arc<dyn TraceStore>,
}

/// Write converted traces and observations to the store.
async fn persist_traces(
    store: &dyn TraceStore,
    traces: &[(crate::models::Trace, Vec<crate::models::Observation>)],
) -> usize {
    let mut errors = 0;
    for (trace, observations) in traces {
        if let Err(e) = store.upsert_trace(trace).await {
            warn!(trace_id = %trace.id, error = %e, "OTLP ingest: failed to write trace");
            errors += 1;
        }
        for obs in observations {
            if let Err(e) = store.insert_observation(obs).await {
                warn!(obs_id = %obs.id, error = %e, "OTLP ingest: failed to write observation");
                errors += 1;
            }
        }
    }
    errors
}

/// Handle OTLP trace export requests (JSON format).
///
/// Accepts `POST /v1/traces` with OTLP JSON body containing
/// `resourceSpans`. Converts spans to Juncture's `Trace`/`Observation`
/// model and writes them to the store.
pub(crate) async fn handle_otlp_traces(
    State(state): State<OtlpState>,
    Json(req): Json<OtlpTraceRequest>,
) -> impl IntoResponse {
    let span_count: usize = req
        .resource_spans
        .iter()
        .map(|rs| {
            rs.scope_spans
                .iter()
                .map(|ss| ss.spans.len())
                .sum::<usize>()
        })
        .sum();

    debug!(
        resource_count = req.resource_spans.len(),
        span_count, "OTLP ingest received"
    );

    let traces = convert_resource_spans(&req.resource_spans);
    let errors = persist_traces(state.store.as_ref(), &traces).await;
    let trace_count = traces.len();

    if errors > 0 {
        warn!(errors, "OTLP ingest completed with errors");
    } else {
        debug!(trace_count, span_count, "OTLP ingest complete");
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "traces": trace_count,
            "spans": span_count,
            "errors": errors
        })),
    )
}

/// Create the OTLP ingest router.
pub fn create_otlp_router(store: Arc<dyn TraceStore>) -> axum::Router {
    let state = OtlpState { store };
    axum::Router::new()
        .route("/v1/traces", axum::routing::post(handle_otlp_traces))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite_store::SqliteStore;

    #[tokio::test]
    async fn otlp_ingest_basic() {
        let store = Arc::new(SqliteStore::new_memory().await.unwrap());

        let req = OtlpTraceRequest {
            resource_spans: vec![crate::otlp::ResourceSpans {
                resource: Some(crate::otlp::Resource {
                    attributes: Some(vec![crate::otlp::KeyValue {
                        key: "service.name".to_string(),
                        value: Some(crate::otlp::AnyValue {
                            string_value: Some("test".to_string()),
                            int_value: None,
                            double_value: None,
                            bool_value: None,
                        }),
                    }]),
                }),
                scope_spans: vec![crate::otlp::ScopeSpans {
                    scope: Some(crate::otlp::Scope {
                        name: Some("juncture".to_string()),
                    }),
                    spans: vec![crate::otlp::OtlpSpan {
                        trace_id: "0123456789abcdef0123456789abcdef".to_string(),
                        span_id: "0123456789abcdef".to_string(),
                        parent_span_id: None,
                        name: "test-span".to_string(),
                        kind: Some(1),
                        start_time_unix_nano: "1704067200000000000".to_string(),
                        end_time_unix_nano: Some("1704067201000000000".to_string()),
                        attributes: None,
                        status: None,
                    }],
                }],
            }],
        };

        let results = convert_resource_spans(&req.resource_spans);
        assert_eq!(results.len(), 1);

        let (trace, observations) = &results[0];
        store.upsert_trace(trace).await.unwrap();
        for obs in observations {
            store.insert_observation(obs).await.unwrap();
        }

        let loaded = store.get_trace(trace.id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.trace.name, "test-span");
        assert_eq!(loaded.observations.len(), 1);
    }
}
