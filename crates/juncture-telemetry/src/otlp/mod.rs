//! OTLP ingest for receiving OpenTelemetry trace data.
//!
//! Provides an HTTP endpoint that accepts OTLP trace data in JSON format
//! and converts it to Juncture's internal `Trace`/`Observation` model.
//! This allows Juncture to act as a lightweight OTLP collector,
//! replacing the need for a separate otel-collector + Jaeger stack.

pub mod http;

use crate::models::{Id, Observation, ObservationLevel, ObservationType, TokenUsage, Trace};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

/// OTLP HTTP trace request (JSON format).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpTraceRequest {
    pub resource_spans: Vec<ResourceSpans>,
}

/// Resource spans container.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceSpans {
    pub resource: Option<Resource>,
    pub scope_spans: Vec<ScopeSpans>,
}

/// OTLP resource with attributes.
#[derive(Debug, Deserialize)]
pub(crate) struct Resource {
    pub attributes: Option<Vec<KeyValue>>,
}

/// Scope spans container.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopeSpans {
    #[expect(
        dead_code,
        reason = "deserialized from OTLP but not used in conversion"
    )]
    pub scope: Option<Scope>,
    pub spans: Vec<OtlpSpan>,
}

/// OTLP scope (instrumentation library).
#[derive(Debug, Deserialize)]
pub(crate) struct Scope {
    #[expect(
        dead_code,
        reason = "deserialized from OTLP but not used in conversion"
    )]
    pub name: Option<String>,
}

/// OTLP span.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OtlpSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    #[expect(
        dead_code,
        reason = "deserialized from OTLP but not used in conversion"
    )]
    pub kind: Option<i32>,
    pub start_time_unix_nano: String,
    pub end_time_unix_nano: Option<String>,
    pub attributes: Option<Vec<KeyValue>>,
    pub status: Option<Status>,
}

/// OTLP key-value attribute.
#[derive(Debug, Deserialize)]
pub(crate) struct KeyValue {
    pub key: String,
    pub value: Option<AnyValue>,
}

/// OTLP any value.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::struct_field_names,
    reason = "OTLP spec uses these field names; renaming would break deserialization"
)]
pub(crate) struct AnyValue {
    pub string_value: Option<String>,
    pub int_value: Option<String>,
    pub double_value: Option<f64>,
    pub bool_value: Option<bool>,
}

/// OTLP span status.
#[derive(Debug, Deserialize)]
pub(crate) struct Status {
    pub code: Option<i32>,
    pub message: Option<String>,
}

/// Convert a hex trace ID (32 chars) to a UUID.
///
/// OTLP uses 128-bit hex strings for trace IDs, which is the same
/// size as a UUID. We parse the hex string into a UUID.
pub(crate) fn hex_to_uuid(hex: &str) -> Id {
    if hex.len() >= 32 {
        // Insert hyphens to make UUID format: 8-4-4-4-12
        let formatted = format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        );
        Id::parse_str(&formatted).unwrap_or_else(|_| Id::new_v4())
    } else {
        Id::new_v4()
    }
}

/// Convert nanosecond timestamp to `DateTime<Utc>`.
pub(crate) fn nano_to_datetime(nano_str: &str) -> DateTime<Utc> {
    let nanos: i64 = nano_str.parse().unwrap_or(0);
    let secs = nanos / 1_000_000_000;
    let sub_nanos = u32::try_from(nanos % 1_000_000_000).unwrap_or(0);
    Utc.timestamp_opt(secs, sub_nanos)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Extract a string attribute value by key.
pub(crate) fn get_string_attr(attrs: &[KeyValue], key: &str) -> Option<String> {
    attrs.iter().find(|a| a.key == key).and_then(|a| {
        a.value.as_ref().and_then(|v| {
            v.string_value
                .clone()
                .or_else(|| v.int_value.clone())
                .or_else(|| v.double_value.map(|d| d.to_string()))
                .or_else(|| v.bool_value.map(|b| b.to_string()))
        })
    })
}

/// Extract an integer attribute value by key.
pub(crate) fn get_int_attr(attrs: &[KeyValue], key: &str) -> Option<u64> {
    attrs
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.int_value.as_ref().and_then(|s| s.parse().ok()))
}

/// Extract a double attribute value by key.
pub(crate) fn get_double_attr(attrs: &[KeyValue], key: &str) -> Option<f64> {
    attrs
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.double_value)
}

/// Convert an OTLP span into a Juncture `Observation`.
///
/// Maps OTLP span attributes to Juncture's observation model,
/// extracting LLM-specific fields (model, tokens, cost) from
/// OpenTelemetry semantic conventions.
pub(crate) fn span_to_observation(span: &OtlpSpan, trace_id: Id) -> Observation {
    let span_uuid = hex_to_uuid(&span.span_id);
    let parent_uuid = span.parent_span_id.as_deref().map(hex_to_uuid);

    let attrs = span.attributes.as_deref().unwrap_or(&[]);

    // Determine observation type from attributes or span kind
    let obs_type = if get_string_attr(attrs, "gen_ai.system").is_some()
        || get_string_attr(attrs, "juncture.llm.model").is_some()
    {
        ObservationType::Generation
    } else if get_string_attr(attrs, "juncture.tool.name").is_some() {
        ObservationType::ToolCall
    } else {
        ObservationType::Span
    };

    let mut obs = Observation {
        id: span_uuid,
        trace_id,
        parent_observation_id: parent_uuid,
        name: span.name.clone(),
        observation_type: obs_type,
        start_time: nano_to_datetime(&span.start_time_unix_nano),
        end_time: span.end_time_unix_nano.as_deref().map(nano_to_datetime),
        input: None,
        output: None,
        metadata: serde_json::Value::Null,
        level: ObservationLevel::Default,
        status_message: None,
        model: get_string_attr(attrs, "gen_ai.request.model")
            .or_else(|| get_string_attr(attrs, "juncture.llm.model")),
        model_parameters: None,
        usage: None,
        cost: None,
    };

    // Extract token usage from OTel semantic conventions
    let input_tokens = get_int_attr(attrs, "gen_ai.usage.input_tokens")
        .or_else(|| get_int_attr(attrs, "juncture.tokens.input"));
    let output_tokens = get_int_attr(attrs, "gen_ai.usage.output_tokens")
        .or_else(|| get_int_attr(attrs, "juncture.tokens.output"));
    let total_tokens = get_int_attr(attrs, "gen_ai.usage.total_tokens")
        .or_else(|| get_int_attr(attrs, "juncture.graph.total_tokens"));

    if input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some() {
        obs.usage = Some(TokenUsage {
            input_tokens: input_tokens.unwrap_or(0),
            output_tokens: output_tokens.unwrap_or(0),
            total_tokens: total_tokens
                .unwrap_or_else(|| input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0)),
            cached_tokens: None,
        });
    }

    // Extract cost
    obs.cost = get_double_attr(attrs, "gen_ai.usage.cost")
        .or_else(|| get_double_attr(attrs, "juncture.cost.usd"));

    // Extract status
    if let Some(status) = &span.status {
        if status.code == Some(2) {
            obs.level = ObservationLevel::Error;
        }
        obs.status_message.clone_from(&status.message);
    }

    obs
}

/// Aggregate token usage and cost from a set of spans.
fn aggregate_usage(spans: &[(&OtlpSpan, String)]) -> (u64, u64, f64) {
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cost = 0.0f64;
    for (span, _) in spans {
        let attrs = span.attributes.as_deref().unwrap_or(&[]);
        total_input += get_int_attr(attrs, "gen_ai.usage.input_tokens")
            .or_else(|| get_int_attr(attrs, "juncture.tokens.input"))
            .unwrap_or(0);
        total_output += get_int_attr(attrs, "gen_ai.usage.output_tokens")
            .or_else(|| get_int_attr(attrs, "juncture.tokens.output"))
            .unwrap_or(0);
        total_cost += get_double_attr(attrs, "gen_ai.usage.cost")
            .or_else(|| get_double_attr(attrs, "juncture.cost.usd"))
            .unwrap_or(0.0);
    }
    (total_input, total_output, total_cost)
}

/// Convert a group of spans sharing a trace ID into a `Trace` + `Observation`s.
fn build_trace_from_spans(
    trace_hex: &str,
    spans: &[(&OtlpSpan, String)],
) -> (Trace, Vec<Observation>) {
    let trace_id = hex_to_uuid(trace_hex);

    let root_name = spans
        .iter()
        .find(|(s, _)| s.parent_span_id.is_none())
        .map_or("otlp-trace", |(s, _)| s.name.as_str());

    let svc_name = spans.first().map_or("", |(_, s)| s.as_str());

    let start_time = spans
        .iter()
        .map(|(s, _)| nano_to_datetime(&s.start_time_unix_nano))
        .min()
        .unwrap_or_else(Utc::now);
    let end_time = spans
        .iter()
        .filter_map(|(s, _)| s.end_time_unix_nano.as_deref().map(nano_to_datetime))
        .max();

    let (total_input, total_output, total_cost) = aggregate_usage(spans);

    let mut trace = Trace::new(root_name);
    trace.id = trace_id;
    trace.start_time = start_time;
    trace.end_time = end_time;
    trace.total_cost = (total_cost > 0.0).then_some(total_cost);
    trace.total_tokens = (total_input + total_output > 0).then_some(total_input + total_output);
    trace.metadata = serde_json::json!({"service.name": svc_name});

    let observations: Vec<Observation> = spans
        .iter()
        .map(|(span, _)| span_to_observation(span, trace_id))
        .collect();

    (trace, observations)
}

/// Convert OTLP resource spans into Juncture `Trace`s and `Observation`s.
///
/// Each unique `traceId` in the OTLP data produces one `Trace`.
/// All spans within that trace become `Observation`s.
pub(crate) fn convert_resource_spans(
    resource_spans: &[ResourceSpans],
) -> Vec<(Trace, Vec<Observation>)> {
    use std::collections::HashMap;

    let mut trace_map: HashMap<String, Vec<(&OtlpSpan, String)>> = HashMap::new();

    for rs in resource_spans {
        let service_name = rs
            .resource
            .as_ref()
            .and_then(|r| r.attributes.as_ref())
            .and_then(|attrs| get_string_attr(attrs, "service.name"))
            .unwrap_or_else(|| "unknown".to_string());

        for ss in &rs.scope_spans {
            for span in &ss.spans {
                trace_map
                    .entry(span.trace_id.clone())
                    .or_default()
                    .push((span, service_name.clone()));
            }
        }
    }

    trace_map
        .iter()
        .map(|(trace_hex, spans)| build_trace_from_spans(trace_hex, spans))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn hex_to_uuid_valid() {
        let hex = "0123456789abcdef0123456789abcdef";
        let uuid = hex_to_uuid(hex);
        assert_eq!(uuid.to_string(), "01234567-89ab-cdef-0123-456789abcdef");
    }

    #[test]
    fn hex_to_uuid_short() {
        let uuid = hex_to_uuid("short");
        assert_ne!(uuid, Id::nil());
    }

    #[test]
    fn nano_to_datetime_test() {
        // 2024-01-01 00:00:00 UTC in nanoseconds
        let nano = "1704067200000000000";
        let dt = nano_to_datetime(nano);
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn get_string_attr_found() {
        let attrs = vec![KeyValue {
            key: "service.name".to_string(),
            value: Some(AnyValue {
                string_value: Some("my-service".to_string()),
                int_value: None,
                double_value: None,
                bool_value: None,
            }),
        }];
        assert_eq!(
            get_string_attr(&attrs, "service.name"),
            Some("my-service".to_string())
        );
        assert_eq!(get_string_attr(&attrs, "missing"), None);
    }

    #[test]
    fn span_to_observation_generation() {
        let span = OtlpSpan {
            trace_id: "0123456789abcdef0123456789abcdef".to_string(),
            span_id: "0123456789abcdef".to_string(),
            parent_span_id: None,
            name: "llm_call".to_string(),
            kind: Some(1),
            start_time_unix_nano: "1704067200000000000".to_string(),
            end_time_unix_nano: Some("1704067201000000000".to_string()),
            attributes: Some(vec![
                KeyValue {
                    key: "gen_ai.system".to_string(),
                    value: Some(AnyValue {
                        string_value: Some("anthropic".to_string()),
                        int_value: None,
                        double_value: None,
                        bool_value: None,
                    }),
                },
                KeyValue {
                    key: "gen_ai.request.model".to_string(),
                    value: Some(AnyValue {
                        string_value: Some("claude-sonnet-4-20250514".to_string()),
                        int_value: None,
                        double_value: None,
                        bool_value: None,
                    }),
                },
                KeyValue {
                    key: "gen_ai.usage.input_tokens".to_string(),
                    value: Some(AnyValue {
                        string_value: None,
                        int_value: Some("100".to_string()),
                        double_value: None,
                        bool_value: None,
                    }),
                },
                KeyValue {
                    key: "gen_ai.usage.output_tokens".to_string(),
                    value: Some(AnyValue {
                        string_value: None,
                        int_value: Some("50".to_string()),
                        double_value: None,
                        bool_value: None,
                    }),
                },
            ]),
            status: None,
        };

        let trace_id = Id::new_v4();
        let obs = span_to_observation(&span, trace_id);
        assert_eq!(obs.observation_type, ObservationType::Generation);
        assert_eq!(obs.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert!(obs.usage.is_some());
        let usage = obs.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }

    #[test]
    fn convert_resource_spans_basic() {
        let request = OtlpTraceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: Some(vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue {
                            string_value: Some("test-service".to_string()),
                            int_value: None,
                            double_value: None,
                            bool_value: None,
                        }),
                    }]),
                }),
                scope_spans: vec![ScopeSpans {
                    scope: Some(Scope {
                        name: Some("juncture".to_string()),
                    }),
                    spans: vec![OtlpSpan {
                        trace_id: "0123456789abcdef0123456789abcdef".to_string(),
                        span_id: "0123456789abcdef".to_string(),
                        parent_span_id: None,
                        name: "graph.invoke".to_string(),
                        kind: Some(1),
                        start_time_unix_nano: "1704067200000000000".to_string(),
                        end_time_unix_nano: Some("1704067201000000000".to_string()),
                        attributes: None,
                        status: None,
                    }],
                }],
            }],
        };

        let results = convert_resource_spans(&request.resource_spans);
        assert_eq!(results.len(), 1);
        let (trace, observations) = &results[0];
        assert_eq!(trace.name, "graph.invoke");
        assert_eq!(observations.len(), 1);
    }
}
