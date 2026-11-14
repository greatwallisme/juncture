//! Streamable HTTP transport: one POST per JSON-RPC message.
//!
//! Two response encodings are legal and both must be supported: a single
//! `application/json` object, or a `text/event-stream` whose final frame is the
//! response. Cancellation is achieved by closing the event stream; there is no
//! `notifications/cancelled` over HTTP (design `12-mcp-and-prefix-cache` §1.6).

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};

use super::McpTransport;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, McpError};

/// HTTP header carrying the negotiated protocol version.
pub const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
/// HTTP header carrying the JSON-RPC method.
pub const HEADER_METHOD: &str = "Mcp-Method";
/// HTTP header carrying the tool name for `tools/call`.
pub const HEADER_NAME: &str = "Mcp-Name";
/// Prefix for mirrored tool arguments annotated with `x-mcp-header`.
pub const HEADER_ARG_PREFIX: &str = "Mcp-Param-";

/// Configuration for the HTTP transport.
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Fully-qualified endpoint URL.
    pub url: String,
    /// Extra headers applied to every request (authorization, tenant, ...).
    pub headers: Vec<(String, String)>,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl HttpConfig {
    /// Create a config for the given endpoint.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }

    /// Add a header sent with every request.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Streamable HTTP transport.
#[derive(Debug)]
pub struct HttpTransport {
    client: Client,
    config: HttpConfig,
    /// `inputSchema` per wire tool name, for `x-mcp-header` mirroring.
    ///
    /// Populated by [`Self::register_tool_schema`]; a `RwLock` because tools are
    /// registered once at toolset construction and read on every call.
    tool_schemas: std::sync::RwLock<std::collections::HashMap<String, serde_json::Value>>,
}

impl HttpTransport {
    /// Build a transport from configuration.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Transport`] if the HTTP client cannot be constructed
    /// or a configured header is not a valid header name/value.
    pub fn new(config: HttpConfig) -> Result<Self, McpError> {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| McpError::Transport(format!("http client: {error}")))?;

        // Validate header names eagerly so a typo fails at construction rather
        // than on the first request.
        for (name, value) in &config.headers {
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::Transport(format!("bad header name {name}: {error}")))?;
            HeaderValue::from_str(value)
                .map_err(|error| McpError::Transport(format!("bad header value: {error}")))?;
        }

        Ok(Self {
            client,
            config,
            tool_schemas: std::sync::RwLock::new(std::collections::HashMap::new()),
        })
    }

    /// Build the header set for one request.
    fn headers_for(&self, request: &JsonRpcRequest) -> Result<HeaderMap, McpError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_static(MCP_PROTOCOL_VERSION),
        );
        headers.insert(
            HeaderName::from_static("mcp-method"),
            HeaderValue::from_str(&request.method)
                .map_err(|error| McpError::Transport(format!("bad method header: {error}")))?,
        );

        // `tools/call` additionally names the tool.
        if request.method == "tools/call"
            && let Some(name) = request
                .params
                .as_ref()
                .and_then(|params| params.get("name"))
                .and_then(serde_json::Value::as_str)
        {
            let header_name = HeaderName::from_static("mcp-name");
            let value = HeaderValue::from_str(name)
                .map_err(|error| McpError::Transport(format!("bad Mcp-Name header: {error}")))?;
            headers.insert(header_name, value);
        }

        // Mirror `x-mcp-header`-annotated arguments into `Mcp-Param-*` headers.
        // The server declares which parameters it wants as headers in the tool's
        // `inputSchema`, and it rejects a call whose body and headers disagree.
        if let Some(params) = request.params.as_ref() {
            let arguments = params.get("arguments");
            for (param, header) in self.header_params_for(params) {
                let Some(value) = arguments
                    .and_then(|arguments| arguments.get(&param))
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let header_name =
                    HeaderName::from_bytes(format!("{HEADER_ARG_PREFIX}{header}").as_bytes())
                        .map_err(|error| {
                            McpError::Transport(format!(
                                "bad mirrored header name {header}: {error}"
                            ))
                        })?;
                let header_value =
                    HeaderValue::from_str(&crate::tool_adapter::encode_header_value(value))
                        .map_err(|error| {
                            McpError::Transport(format!("bad mirrored header value: {error}"))
                        })?;
                headers.insert(header_name, header_value);
            }
        }

        for (name, value) in &self.config.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::Transport(format!("bad header name {name}: {error}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|error| McpError::Transport(format!("bad header value: {error}")))?;
            headers.insert(header_name, header_value);
        }

        Ok(headers)
    }

    /// Look up the `x-mcp-header` annotations declared for a tool.
    ///
    /// Annotations come from the tool's `inputSchema`, which the transport gets
    /// from [`Self::register_tool_schema`]. Unknown tools have no annotations
    /// and therefore mirror nothing.
    fn header_params_for(&self, params: &serde_json::Value) -> Vec<(String, String)> {
        let Some(tool) = params.get("name").and_then(serde_json::Value::as_str) else {
            return Vec::new();
        };
        let schemas = match self.tool_schemas.read() {
            Ok(schemas) => schemas,
            Err(poisoned) => poisoned.into_inner(),
        };
        schemas
            .get(tool)
            .map(crate::tool_adapter::header_params)
            .unwrap_or_default()
    }

    /// Parse a response body that may be JSON or an event stream.
    ///
    /// SSE frames that are notifications are skipped. Comment lines (leading
    /// `:`) are keep-alives and are ignored.
    ///
    /// When `expected_id` is `Some`, only the frame carrying that id is the
    /// response: a stream may interleave notifications, and a server may emit
    /// more than one id-bearing frame. Taking "the last one" would attribute an
    /// unrelated answer to this request.
    fn parse_body(
        content_type: &str,
        body: &str,
        expected_id: Option<&serde_json::Value>,
    ) -> Result<JsonRpcResponse, McpError> {
        if content_type.contains("text/event-stream") {
            let mut matched: Option<JsonRpcResponse> = None;
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(data.trim()) else {
                    continue;
                };
                let Some(id) = value.get("id") else {
                    continue;
                };
                if expected_id.is_some_and(|expected| expected != id) {
                    continue;
                }
                matched = serde_json::from_value(value).ok();
            }
            return matched.ok_or_else(|| {
                McpError::Transport("event stream contained no response frame".to_string())
            });
        }

        serde_json::from_str(body)
            .map_err(|error| McpError::Transport(format!("decode json response: {error}")))
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    /// Record a tool schema so `x-mcp-header` annotations can be mirrored.
    ///
    /// Without it the transport has no way to know which arguments the server
    /// wants as headers, and a call to such a tool is rejected with a header
    /// mismatch.
    fn register_tool_schema(&self, tool_name: &str, schema: serde_json::Value) {
        let mut schemas = match self.tool_schemas.write() {
            Ok(schemas) => schemas,
            Err(poisoned) => poisoned.into_inner(),
        };
        schemas.insert(tool_name.to_string(), schema);
    }

    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let headers = self.headers_for(&request)?;

        let response = self
            .client
            .post(&self.config.url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|error| McpError::Transport(format!("request failed: {error}")))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();

        let body = response
            .text()
            .await
            .map_err(|error| McpError::Transport(format!("read body: {error}")))?;

        if !status.is_success() {
            // A structured JSON-RPC error is still a protocol answer; only
            // return it when it parses as one.
            if let Ok(parsed) = serde_json::from_str::<JsonRpcResponse>(&body)
                && parsed.error.is_some()
            {
                return Ok(parsed);
            }
            return Err(McpError::Transport(format!(
                "HTTP {}: {body}",
                status.as_u16()
            )));
        }

        Self::parse_body(&content_type, &body, Some(&request.id))
    }

    fn kind(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transport() -> HttpTransport {
        HttpTransport::new(HttpConfig::new("https://example.test/mcp").header("x-tenant", "a"))
            .expect("transport")
    }

    #[test]
    fn test_headers_include_required_keys() {
        let transport = transport();
        let request = JsonRpcRequest::with_meta(json!(1), "tools/list", None);
        let headers = transport.headers_for(&request).expect("headers");

        assert_eq!(headers[HEADER_PROTOCOL_VERSION], MCP_PROTOCOL_VERSION);
        assert_eq!(headers[HEADER_METHOD], "tools/list");
        assert_eq!(headers["x-tenant"], "a");
        assert!(
            headers[ACCEPT]
                .to_str()
                .expect("accept")
                .contains("text/event-stream")
        );
        assert!(
            headers.get(HEADER_NAME).is_none(),
            "only tools/call names a tool"
        );
    }

    #[test]
    fn test_tools_call_sets_name_header() {
        let transport = transport();
        let request = JsonRpcRequest::with_meta(
            json!(1),
            "tools/call",
            Some(json!({"name": "read_file", "arguments": {}})),
        );
        let headers = transport.headers_for(&request).expect("headers");
        assert_eq!(headers[HEADER_NAME], "read_file");
    }

    #[test]
    fn test_x_mcp_header_arguments_are_mirrored() {
        let transport = transport();
        transport.register_tool_schema(
            "read_file",
            json!({
                "type": "object",
                "properties": {
                    "tenant": {"type": "string", "x-mcp-header": "Tenant"},
                    "path": {"type": "string"}
                }
            }),
        );

        let request = JsonRpcRequest::with_meta(
            json!(1),
            "tools/call",
            Some(json!({"name": "read_file",
                        "arguments": {"tenant": "acme", "path": "/x"}})),
        );
        let headers = transport.headers_for(&request).expect("headers");
        assert_eq!(headers["Mcp-Param-Tenant"], "acme");
        assert!(
            headers.get("Mcp-Param-path").is_none(),
            "only annotated parameters become headers"
        );
    }

    #[test]
    fn test_x_mcp_header_value_is_escaped_when_needed() {
        let transport = transport();
        transport.register_tool_schema(
            "read_file",
            json!({
                "type": "object",
                "properties": {"tenant": {"type": "string", "x-mcp-header": "Tenant"}}
            }),
        );
        let request = JsonRpcRequest::with_meta(
            json!(1),
            "tools/call",
            Some(json!({"name": "read_file", "arguments": {"tenant": "租户"}})),
        );
        let headers = transport.headers_for(&request).expect("headers");
        assert_eq!(
            headers["Mcp-Param-Tenant"], "=?base64?56ef5oi3?=",
            "non-ASCII must be base64-wrapped, not sent raw"
        );
    }

    #[test]
    fn test_rejects_invalid_configured_header_name() {
        let result =
            HttpTransport::new(HttpConfig::new("https://example.test").header("bad name", "v"));
        let error = result.expect_err("invalid header name must be rejected");
        assert!(
            matches!(error, McpError::Transport(_)),
            "a malformed header is a transport-configuration fault"
        );
    }

    #[test]
    fn test_rejects_invalid_configured_header_value() {
        let result =
            HttpTransport::new(HttpConfig::new("https://example.test").header("x", "bad\nvalue"));
        let error = result.expect_err("invalid header value must be rejected");
        assert!(
            matches!(error, McpError::Transport(_)),
            "a malformed header is a transport-configuration fault"
        );
    }

    #[test]
    fn test_parse_json_body() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete"}}"#;
        let response = HttpTransport::parse_body("application/json", body, None).expect("parse");
        assert!(response.result.is_some());
    }

    #[test]
    fn test_parse_sse_body_skips_notifications_and_comments() {
        let body = ": keep-alive\n\
                    event: message\n\
                    data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\
                    \n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"resultType\":\"complete\"}}\n";
        let response =
            HttpTransport::parse_body("text/event-stream", body, Some(&json!(7))).expect("parse");
        assert_eq!(response.id, json!(7));
    }

    #[test]
    fn test_parse_sse_without_response_errors() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n";
        assert!(
            HttpTransport::parse_body("text/event-stream", body, Some(&json!(7))).is_err(),
            "an event stream without a response frame has no answer to give"
        );
    }

    #[test]
    fn test_parse_sse_prefers_the_frame_matching_the_request_id() {
        // A stream may carry more than one id-bearing frame; only the one the
        // request asked for is its answer.
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"stale\":true}}\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"fresh\":true}}\n";
        let response =
            HttpTransport::parse_body("text/event-stream", body, Some(&json!(7))).expect("parse");
        assert_eq!(response.id, json!(7));
        assert_eq!(response.result.expect("result")["fresh"], true);
    }

    #[test]
    fn test_parse_malformed_json_errors() {
        assert!(
            HttpTransport::parse_body("application/json", "not json", None).is_err(),
            "a non-JSON body must be reported, not guessed at"
        );
    }
}
