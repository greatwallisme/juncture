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

        Ok(Self { client, config })
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

        for (name, value) in &self.config.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::Transport(format!("bad header name {name}: {error}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|error| McpError::Transport(format!("bad header value: {error}")))?;
            headers.insert(header_name, header_value);
        }

        Ok(headers)
    }

    /// Parse a response body that may be JSON or an event stream.
    ///
    /// SSE frames that are notifications are skipped; the last `data:` frame
    /// carrying an `id` is the response. Comment lines (leading `:`) are
    /// keep-alives and are ignored.
    fn parse_body(content_type: &str, body: &str) -> Result<JsonRpcResponse, McpError> {
        if content_type.contains("text/event-stream") {
            let mut last: Option<JsonRpcResponse> = None;
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
                if value.get("id").is_none() {
                    continue;
                }
                last = serde_json::from_value(value).ok();
            }
            return last.ok_or_else(|| {
                McpError::Transport("event stream contained no response frame".to_string())
            });
        }

        serde_json::from_str(body)
            .map_err(|error| McpError::Transport(format!("decode json response: {error}")))
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
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

        Self::parse_body(&content_type, &body)
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
        let response = HttpTransport::parse_body("application/json", body).expect("parse");
        assert!(response.result.is_some());
    }

    #[test]
    fn test_parse_sse_body_skips_notifications_and_comments() {
        let body = ": keep-alive\n\
                    event: message\n\
                    data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\
                    \n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"resultType\":\"complete\"}}\n";
        let response = HttpTransport::parse_body("text/event-stream", body).expect("parse");
        assert_eq!(response.id, json!(7));
    }

    #[test]
    fn test_parse_sse_without_response_errors() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n";
        assert!(
            HttpTransport::parse_body("text/event-stream", body).is_err(),
            "an event stream without a response frame has no answer to give"
        );
    }

    #[test]
    fn test_parse_malformed_json_errors() {
        assert!(
            HttpTransport::parse_body("application/json", "not json").is_err(),
            "a non-JSON body must be reported, not guessed at"
        );
    }
}
