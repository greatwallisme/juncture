//! End-to-end integration tests for juncture-telemetry.
//!
//! Tests the complete pipeline: collector → store → API → dashboard.
//! Each test starts a fresh web server on a random port and verifies
//! the full data flow.

#![cfg(feature = "web")]

use std::sync::Arc;

use juncture_telemetry::{
    CaptureConfig, SqliteStore, TelemetryCollector, TokenUsage, TraceStore, web::WebServer,
};

/// Helper to set up a test environment: store + collector + web server.
/// Returns (`base_url`, collector, store, `server_handle`).
/// The `server_handle` must be kept alive for the duration of the test.
async fn setup() -> (
    String,
    TelemetryCollector,
    Arc<SqliteStore>,
    juncture_telemetry::web::WebServerHandle,
) {
    let store = Arc::new(SqliteStore::new_memory().await.unwrap());
    #[expect(
        clippy::clone_on_ref_ptr,
        reason = ".clone() enables unsized coercion Arc<SqliteStore> -> Arc<dyn TraceStore>"
    )]
    let collector = TelemetryCollector::with_capture_config(
        store.clone(),
        CaptureConfig {
            max_prompt_chars: 500,
            max_response_chars: 500,
            capture_full_messages: true,
            capture_tool_io: true,
            sensitive_keys: vec!["api_key".to_string()],
        },
    );
    #[expect(
        clippy::clone_on_ref_ptr,
        reason = ".clone() enables unsized coercion Arc<SqliteStore> -> Arc<dyn TraceStore>"
    )]
    let server = WebServer::new(store.clone(), 0).start().await.unwrap();
    let base_url = server.base_url();

    // Wait until the server is actually accepting connections
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(server.addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    (base_url, collector, store, server)
}

/// Simulate a complete agent execution lifecycle.
///
/// Models a `ReAct` agent that:
/// 1. Receives user question
/// 2. Calls LLM to reason
/// 3. Calls a search tool
/// 4. Calls LLM again with search results
/// 5. Produces final answer
async fn simulate_agent_execution(collector: &TelemetryCollector) -> uuid::Uuid {
    // Track session
    collector
        .track_session("session-42", Some("user-alice".to_string()))
        .await
        .unwrap();

    // Start trace
    let mut trace = collector
        .begin_trace("react_agent", Some("session-42".to_string()))
        .await
        .unwrap();
    trace.user_id = Some("user-alice".to_string());
    trace.tags = vec!["production".to_string(), "v1.2.3".to_string()];
    let trace_id = trace.id;

    // Step 1: First LLM call (reasoning)
    let llm1 = collector.begin_llm_call(
        trace_id,
        None,
        "claude-sonnet-4-20250514",
        Some(&serde_json::json!({
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "What is the capital of France?"}
            ]
        })),
    );
    collector
        .end_llm_call(
            llm1,
            Some("I need to search for this information."),
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

    // Step 2: Tool call (search)
    let tool = collector.begin_tool_call(
        trace_id,
        None,
        "web_search",
        Some(&serde_json::json!({"query": "capital of France"})),
    );
    collector
        .end_tool_call(
            tool,
            Some(serde_json::json!({
                "results": ["Paris is the capital of France."]
            })),
        )
        .await
        .unwrap();

    // Step 3: Second LLM call (with tool result)
    let llm2 = collector.begin_llm_call(
        trace_id,
        None,
        "claude-sonnet-4-20250514",
        Some(&serde_json::json!({
            "messages": [
                {"role": "user", "content": "What is the capital of France?"},
                {"role": "assistant", "content": "I need to search for this information."},
                {"role": "tool", "content": "Paris is the capital of France."}
            ]
        })),
    );
    collector
        .end_llm_call(
            llm2,
            Some("The capital of France is Paris."),
            Some(TokenUsage {
                input_tokens: 80,
                output_tokens: 10,
                total_tokens: 90,
                cached_tokens: None,
            }),
            Some(0.0004),
        )
        .await
        .unwrap();

    // End trace
    collector
        .end_trace(
            trace,
            Some(serde_json::json!({"answer": "Paris"})),
            Some(0.0007),
            Some(155),
        )
        .await
        .unwrap();

    // Flush to store
    collector.flush().await.unwrap();

    trace_id
}

// ── E2E Tests ──────────────────────────────────────────────

#[tokio::test]
async fn e2e_full_agent_lifecycle() {
    let (base_url, collector, store, _server) = setup().await;

    // Simulate agent execution
    let trace_id = simulate_agent_execution(&collector).await;

    // Verify via store directly
    let loaded = store.get_trace(trace_id).await.unwrap();
    assert!(loaded.is_some(), "trace should exist in store");
    let loaded = loaded.unwrap();

    // Verify trace metadata
    assert_eq!(loaded.trace.name, "react_agent");
    assert_eq!(loaded.trace.session_id.as_deref(), Some("session-42"));
    assert_eq!(loaded.trace.user_id.as_deref(), Some("user-alice"));
    assert_eq!(loaded.trace.tags, vec!["production", "v1.2.3"]);
    assert!(loaded.trace.end_time.is_some());
    assert_eq!(loaded.trace.total_cost, Some(0.0007));
    assert_eq!(loaded.trace.total_tokens, Some(155));

    // Verify observations
    assert_eq!(loaded.observations.len(), 3, "should have 3 observations");

    // Find observations by name
    let llm_calls: Vec<_> = loaded
        .observations
        .iter()
        .filter(|o| o.name == "llm_call")
        .collect();
    let tool_calls: Vec<_> = loaded
        .observations
        .iter()
        .filter(|o| o.name == "web_search")
        .collect();

    assert_eq!(llm_calls.len(), 2, "should have 2 LLM calls");
    assert_eq!(tool_calls.len(), 1, "should have 1 tool call");

    // Verify LLM call details
    for llm in &llm_calls {
        assert!(llm.model.is_some());
        assert!(llm.usage.is_some());
        assert!(llm.input.is_some());
        assert!(llm.output.is_some());
    }

    // Verify tool call details
    let tool = tool_calls[0];
    assert!(tool.input.is_some());
    assert!(tool.output.is_some());

    // Verify via API
    let client = reqwest::Client::new();

    // GET /api/public/traces
    let resp = client
        .get(format!("{base_url}/api/public/traces"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["totalCount"], 1);
    assert_eq!(body["data"][0]["name"], "react_agent");

    // GET /api/public/traces/:id
    let resp = client
        .get(format!("{base_url}/api/public/traces/{trace_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["trace"]["name"], "react_agent");
    assert_eq!(body["observations"].as_array().unwrap().len(), 3);

    // GET /api/public/sessions
    let resp = client
        .get(format!("{base_url}/api/public/sessions"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["totalCount"], 1);
    assert_eq!(body["data"][0]["id"], "session-42");

    // Cleanup
    let _ = base_url;
}

#[tokio::test]
async fn e2e_langfuse_ingestion_api() {
    let (base_url, _collector, store, _server) = setup().await;
    let client = reqwest::Client::new();

    // Send Langfuse-format ingestion
    let resp = client
        .post(format!("{base_url}/api/public/ingestion"))
        .json(&serde_json::json!({
            "batch": [
                {
                    "id": "ingest-1",
                    "type": "trace-create",
                    "body": {
                        "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
                        "name": "langfuse-test",
                        "sessionId": "session-lf",
                        "userId": "user-bob",
                        "tags": ["test"],
                        "metadata": {"source": "e2e"}
                    },
                    "timestamp": "2024-01-01T00:00:00Z"
                },
                {
                    "id": "ingest-2",
                    "type": "generation-create",
                    "body": {
                        "id": "b2c3d4e5-f6a7-8901-bcde-f12345678901",
                        "traceId": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
                        "name": "llm_call",
                        "model": "gpt-4",
                        "input": {"prompt": "Hello"},
                        "output": "Hi there!",
                        "usage": {
                            "inputTokens": 10,
                            "outputTokens": 5,
                            "totalTokens": 15
                        },
                        "cost": 0.001
                    },
                    "timestamp": "2024-01-01T00:00:01Z"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["successes"].as_array().unwrap().len(), 2);
    assert_eq!(body["errors"].as_array().unwrap().len(), 0);

    // Verify data was stored
    let trace_id =
        juncture_telemetry::Id::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap();
    let loaded = store.get_trace(trace_id).await.unwrap();
    assert!(loaded.is_some(), "trace from ingestion should exist");
    let loaded = loaded.unwrap();
    assert_eq!(loaded.trace.name, "langfuse-test");
    assert_eq!(loaded.trace.session_id.as_deref(), Some("session-lf"));
    assert_eq!(loaded.trace.user_id.as_deref(), Some("user-bob"));
    assert_eq!(loaded.observations.len(), 1);
    assert_eq!(loaded.observations[0].model.as_deref(), Some("gpt-4"));
}

#[tokio::test]
async fn e2e_otlp_ingest() {
    let (base_url, _collector, store, _server) = setup().await;
    let client = reqwest::Client::new();

    // Send OTLP trace data
    let resp = client
        .post(format!("{base_url}/v1/traces"))
        .json(&serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [{
                        "key": "service.name",
                        "value": {"stringValue": "otlp-test-service"}
                    }]
                },
                "scopeSpans": [{
                    "scope": {"name": "juncture"},
                    "spans": [
                        {
                            "traceId": "0123456789abcdef0123456789abcdef",
                            "spanId": "0123456789abcdef",
                            "name": "graph.invoke",
                            "kind": 1,
                            "startTimeUnixNano": "1704067200000000000",
                            "endTimeUnixNano": "1704067202000000000",
                            "attributes": []
                        },
                        {
                            "traceId": "0123456789abcdef0123456789abcdef",
                            "spanId": "1122334455667788",
                            "parentSpanId": "0123456789abcdef",
                            "name": "llm.call",
                            "kind": 1,
                            "startTimeUnixNano": "1704067200500000000",
                            "endTimeUnixNano": "1704067201500000000",
                            "attributes": [
                                {
                                    "key": "gen_ai.system",
                                    "value": {"stringValue": "anthropic"}
                                },
                                {
                                    "key": "gen_ai.request.model",
                                    "value": {"stringValue": "claude-sonnet-4-20250514"}
                                },
                                {
                                    "key": "gen_ai.usage.input_tokens",
                                    "value": {"intValue": "100"}
                                },
                                {
                                    "key": "gen_ai.usage.output_tokens",
                                    "value": {"intValue": "50"}
                                }
                            ]
                        }
                    ]
                }]
            }]
        }))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["traces"], 1);
    assert_eq!(body["spans"], 2);

    // Verify data was stored
    let trace_id =
        juncture_telemetry::Id::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();
    let loaded = store.get_trace(trace_id).await.unwrap();
    assert!(loaded.is_some(), "OTLP trace should exist");
    let loaded = loaded.unwrap();
    assert_eq!(loaded.trace.name, "graph.invoke");
    assert_eq!(loaded.observations.len(), 2);

    // Verify LLM observation
    let llm_obs = loaded
        .observations
        .iter()
        .find(|o| o.name == "llm.call")
        .unwrap();
    assert_eq!(llm_obs.model.as_deref(), Some("claude-sonnet-4-20250514"));
    assert!(llm_obs.usage.is_some());
    assert_eq!(llm_obs.usage.as_ref().unwrap().input_tokens, 100);
    assert_eq!(llm_obs.usage.as_ref().unwrap().output_tokens, 50);
}

#[tokio::test]
async fn e2e_dashboard_serves_html() {
    let (base_url, _collector, _store, _server) = setup().await;
    let client = reqwest::Client::new();

    let resp = client.get(format!("{base_url}/")).send().await.unwrap();

    assert!(resp.status().is_success());
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"), "should serve HTML");

    let body = resp.text().await.unwrap();
    assert!(body.contains("Juncture"), "dashboard should contain title");
    assert!(
        body.contains("Telemetry"),
        "dashboard should contain subtitle"
    );
    assert!(body.contains("Dashboard"), "dashboard should have nav");
    assert!(body.contains("Traces"), "dashboard should have traces nav");
    assert!(
        body.contains("Sessions"),
        "dashboard should have sessions nav"
    );
    assert!(
        body.contains("/api/public"),
        "dashboard should reference API"
    );
}

#[tokio::test]
async fn e2e_multiple_traces_pagination() {
    let (_base_url, collector, store, _server) = setup().await;

    // Create 5 traces with different sessions
    for i in 0..5 {
        let mut trace = collector
            .begin_trace(format!("graph_{i}"), Some(format!("session-{i}")))
            .await
            .unwrap();
        trace.user_id = Some(format!("user-{i}"));
        collector.end_trace(trace, None, None, None).await.unwrap();
    }
    collector.flush().await.unwrap();

    // Verify all traces exist
    let query = juncture_telemetry::TraceQuery {
        page: Some(0),
        page_size: Some(10),
        ..Default::default()
    };
    let result = store.query_traces(&query).await.unwrap();
    assert_eq!(result.data.len(), 5);
    assert_eq!(result.total_count, 5);

    // Verify pagination
    let query = juncture_telemetry::TraceQuery {
        page: Some(0),
        page_size: Some(2),
        ..Default::default()
    };
    let result = store.query_traces(&query).await.unwrap();
    assert_eq!(result.data.len(), 2);
    assert_eq!(result.total_count, 5);

    // Verify session filter
    let query = juncture_telemetry::TraceQuery {
        session_id: Some("session-2".to_string()),
        ..Default::default()
    };
    let result = store.query_traces(&query).await.unwrap();
    assert_eq!(result.data.len(), 1);
    assert_eq!(result.data[0].name, "graph_2");
}

#[tokio::test]
async fn e2e_cost_aggregation() {
    let (_base_url, collector, store, _server) = setup().await;

    let trace = collector.begin_trace("cost_test", None).await.unwrap();
    let trace_id = trace.id;

    // Add 3 LLM calls with different costs
    for i in 0..3 {
        let obs = collector.begin_llm_call(trace_id, None, "claude-sonnet-4-20250514", None);
        collector
            .end_llm_call(
                obs,
                Some("response"),
                Some(TokenUsage {
                    input_tokens: 100 * (i + 1),
                    output_tokens: 50 * (i + 1),
                    total_tokens: 150 * (i + 1),
                    cached_tokens: None,
                }),
                Some(0.001 * f64::from(u32::try_from(i + 1).unwrap_or(u32::MAX))),
            )
            .await
            .unwrap();
    }

    collector.end_trace(trace, None, None, None).await.unwrap();
    collector.flush().await.unwrap();

    let loaded = store.get_trace(trace_id).await.unwrap().unwrap();
    assert_eq!(loaded.observations.len(), 3);

    // Verify total cost = 0.001 + 0.002 + 0.003 = 0.006
    let total_cost: f64 = loaded.observations.iter().filter_map(|o| o.cost).sum();
    assert!((total_cost - 0.006).abs() < f64::EPSILON);
}
