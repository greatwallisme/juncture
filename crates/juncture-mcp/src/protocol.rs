//! JSON-RPC 2.0 framing and MCP 2026-07-28 result types.
//!
//! The 2026-07-28 revision is stateless: there is no handshake and no session
//! id. Each request carries the negotiated protocol version and client identity
//! in `_meta`, and every result carries a `resultType`. See design
//! `12-mcp-and-prefix-cache` §0.3 and §1.4.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// JSON-RPC protocol version string used on the wire.
pub const JSONRPC_VERSION: &str = "2.0";

/// MCP protocol revision this client targets.
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

/// Last revision with the stateful `initialize` handshake.
///
/// Used only on the legacy fallback path, when a server rejects
/// `server/discover` with `-32601`.
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";

/// `_meta` keys reserved by the MCP specification.
pub mod meta_keys {
    /// Negotiated protocol version.
    pub const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
    /// Client identity.
    pub const CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
    /// Client capabilities.
    pub const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
}

/// A JSON-RPC request id.
///
/// Kept as a raw `Value` because the server may echo either a number or a
/// string; correlation must not depend on which form we sent.
pub type RequestId = Value;

/// A JSON-RPC 2.0 request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request id, echoed by the response.
    pub id: RequestId,
    /// Method name, e.g. `tools/list`.
    pub method: String,
    /// Positional/object parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Build a request without a `_meta` block.
    ///
    /// Only the legacy fallback uses this: servers that predate 2026-07-28
    /// reject requests carrying protocol metadata they do not understand.
    #[must_use]
    pub fn without_meta(id: RequestId, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
        }
    }

    /// Build a request, attaching the mandatory `_meta` block.
    ///
    /// `_meta` is mandatory under 2026-07-28. [`Self::without_meta`] exists
    /// because the legacy fallback must be able to omit it.
    #[must_use]
    pub fn with_meta(id: RequestId, method: impl Into<String>, params: Option<Value>) -> Self {
        let mut params = match params {
            Some(Value::Object(map)) => map,
            Some(other) => {
                // Non-object params cannot carry `_meta`; wrap them so the
                // mandatory key is still present.
                let mut map = Map::new();
                map.insert("value".to_string(), other);
                map
            }
            None => Map::new(),
        };

        let mut root = Map::new();
        root.insert(
            meta_keys::PROTOCOL_VERSION.to_string(),
            Value::String(MCP_PROTOCOL_VERSION.to_string()),
        );
        root.insert(
            meta_keys::CLIENT_INFO.to_string(),
            serde_json::json!({"name": "juncture-mcp", "version": env!("CARGO_PKG_VERSION")}),
        );
        root.insert(
            meta_keys::CLIENT_CAPABILITIES.to_string(),
            // This client consumes tools only (design 12 §1.11).
            serde_json::json!({"tools": {}}),
        );
        params.insert("_meta".to_string(), Value::Object(root));

        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params: Some(Value::Object(params)),
        }
    }
}

/// A JSON-RPC 2.0 response envelope.
#[derive(Clone, Debug, Deserialize)]
pub struct JsonRpcResponse {
    /// Echoed request id.
    pub id: RequestId,
    /// Successful payload; mutually exclusive with `error`.
    #[serde(default)]
    pub result: Option<Value>,
    /// Error payload; mutually exclusive with `result`.
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Clone, Debug, Deserialize, thiserror::Error)]
#[error("json-rpc error {code}: {message}")]
pub struct JsonRpcError {
    /// Numeric error code (e.g. `-32601`).
    pub code: i64,
    /// Human-readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(default)]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// MCP header/body mismatch error code.
    pub const HEADER_MISMATCH: i64 = -32020;
    /// JSON-RPC "method not found".
    pub const METHOD_NOT_FOUND: i64 = -32601;

    /// Whether this error reports a header/body mismatch.
    #[must_use]
    pub const fn is_header_mismatch(&self) -> bool {
        self.code == Self::HEADER_MISMATCH
    }
}

/// A JSON-RPC 2.0 notification (no id, no response expected).
#[derive(Clone, Debug, Deserialize)]
pub struct JsonRpcNotification {
    /// Method name, e.g. `notifications/tools/list_changed`.
    pub method: String,
    /// Notification parameters.
    #[serde(default)]
    pub params: Option<Value>,
}

/// Completion state of an MCP result.
///
/// Every result must carry this discriminator under 2026-07-28. Earlier
/// revisions had no such concept; [`McpResult`] treats its absence as
/// [`ResultType::Complete`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultType {
    /// The call finished and `result` is final.
    #[default]
    Complete,
    /// The server needs more input; retry with `inputResponses`.
    InputRequired,
}

impl ResultType {
    /// Whether this result requires another round trip.
    #[must_use]
    pub const fn is_input_required(self) -> bool {
        matches!(self, Self::InputRequired)
    }
}

/// A result envelope whose `resultType` decides how to interpret it.
#[derive(Clone, Debug, Deserialize)]
pub struct McpResult {
    /// Completion state.
    ///
    /// `resultType` is mandatory under 2026-07-28 but absent on every earlier
    /// revision, and the client still speaks to those servers through the
    /// legacy `initialize` path. An absent discriminator means "complete":
    /// there is no pre-2026 notion of an input round, so the payload is final.
    #[serde(default, rename = "resultType")]
    pub result_type: ResultType,
    /// Final payload when `result_type` is `complete`.
    #[serde(default)]
    pub result: Option<Value>,
    /// Requests awaiting client answers when `result_type` is `input_required`.
    ///
    /// A `BTreeMap` so the serialized form is deterministic: this reaches a
    /// tool message and therefore the conversation history, which is a cache
    /// prefix segment (design `12` §1.5.3).
    #[serde(default, rename = "inputRequests")]
    pub input_requests: BTreeMap<String, InputRequest>,
    /// Opaque server state to replay verbatim on the retry.
    #[serde(default, rename = "requestState")]
    pub request_state: Option<String>,
}

/// One request the server needs the client to satisfy.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InputRequest {
    /// Interaction method, e.g. `elicitation/create`.
    pub method: String,
    /// Method-specific parameters.
    #[serde(default)]
    pub params: Value,
}

/// One MCP tool as reported by `tools/list`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpTool {
    /// Tool name, unique only within its server.
    pub name: String,
    /// Description shown to the model.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for the tool input.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
    /// Optional JSON Schema for the tool output.
    #[serde(default, rename = "outputSchema")]
    pub output_schema: Option<Value>,
}

/// Cache scope declared by a list result.
///
/// A `private` list must be bucketed by authorization context; sharing it
/// across tenants would leak data (design `12` §1.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    /// Safe to share across callers.
    Public,
    /// Must be partitioned by authorization context.
    Private,
}

/// `tools/list` result payload.
#[derive(Clone, Debug, Deserialize)]
pub struct ToolsListResult {
    /// Tools on this page.
    pub tools: Vec<McpTool>,
    /// Pagination cursor for the next page, if any.
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
    /// Server-declared time-to-live for this list, in milliseconds.
    ///
    /// Independent of LLM prompt-cache TTLs; the two must never be conflated
    /// (design `12` §2.10).
    #[serde(default, rename = "ttlMs")]
    pub ttl_ms: Option<u64>,
    /// Sharing scope for this list.
    #[serde(default, rename = "cacheScope")]
    pub cache_scope: Option<CacheScope>,
}

/// `tools/call` result payload.
#[derive(Clone, Debug, Deserialize)]
pub struct ToolCallResult {
    /// Content blocks returned by the tool.
    #[serde(default)]
    pub content: Vec<ToolContent>,
    /// Whether the tool reported a failure the model should see and correct.
    ///
    /// Distinct from a protocol error: this must be delivered as a tool message
    /// so the model can self-correct (design `12` §1.10).
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

/// A content block in a `tools/call` result.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    /// Text content.
    #[serde(rename = "text")]
    Text {
        /// The text payload.
        text: String,
    },
    /// Any other content kind, preserved but not interpreted.
    #[serde(other)]
    Other,
}

impl ToolCallResult {
    /// Concatenate all text blocks.
    ///
    /// Non-text blocks are dropped rather than serialized as JSON: a tool
    /// message is model input, and dumping a binary block's JSON envelope is
    /// noise the model cannot use.
    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ToolContent::Text { text } => Some(text.as_str()),
                ToolContent::Other => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Server identity from `server/discover`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerInfo {
    /// Human-readable server name.
    #[serde(default)]
    pub name: String,
    /// Server version string.
    #[serde(default)]
    pub version: String,
}

/// Capabilities advertised by `server/discover`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerCapabilities {
    /// Whether the server exposes tools.
    #[serde(default)]
    pub tools: Option<Value>,
}

/// `server/discover` result payload.
#[derive(Clone, Debug, Deserialize)]
pub struct DiscoverResult {
    /// Protocol revisions the server supports, highest first.
    #[serde(default, rename = "supportedVersions")]
    pub supported_versions: Vec<String>,
    /// Server capabilities.
    #[serde(default)]
    pub capabilities: ServerCapabilities,
    /// Server identity.
    #[serde(default, rename = "serverInfo")]
    pub server_info: ServerInfo,
    /// Cache TTL for this discovery result, in milliseconds.
    #[serde(default, rename = "ttlMs")]
    pub ttl_ms: Option<u64>,
    /// Sharing scope for this discovery result.
    #[serde(default, rename = "cacheScope")]
    pub cache_scope: Option<CacheScope>,
}

/// `initialize` result payload from a pre-2026 server.
///
/// Only the legacy fallback parses this; the modern protocol replaced it with
/// `server/discover`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct InitializeResult {
    /// Protocol revision the server selected.
    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: Option<String>,
    /// Server capabilities.
    #[serde(default)]
    pub capabilities: ServerCapabilities,
    /// Server identity.
    #[serde(default, rename = "serverInfo")]
    pub server_info: ServerInfo,
}

/// Errors produced by the MCP client.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// Transport-level failure (I/O, HTTP).
    #[error("transport error: {0}")]
    Transport(String),

    /// JSON-RPC protocol error.
    #[error("protocol error: {0}")]
    Protocol(#[from] JsonRpcError),

    /// The server does not support any version this client speaks.
    #[error("unsupported protocol version; server supports {supported:?}")]
    UnsupportedVersion {
        /// Versions the server advertised.
        supported: Vec<String>,
    },

    /// Headers and body disagreed (`-32020`).
    #[error("header mismatch")]
    HeaderMismatch,

    /// The server needs input before it can answer.
    #[error("server needs input")]
    NeedsInput {
        /// Requests awaiting answers.
        requests: BTreeMap<String, InputRequest>,
        /// Opaque state to replay verbatim.
        request_state: Option<String>,
    },

    /// The tool reported `isError: true`.
    #[error("tool execution error: {message}")]
    ToolExecution {
        /// Message the model should see.
        message: String,
    },

    /// A payload did not match the expected shape.
    #[error("malformed payload: {0}")]
    Malformed(String),
}

impl From<serde_json::Error> for McpError {
    fn from(error: serde_json::Error) -> Self {
        Self::Malformed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_carries_meta() {
        let request = JsonRpcRequest::with_meta(json!(1), "server/discover", Some(json!({"x": 1})));
        let params = request.params.expect("params present");
        let meta = params.get("_meta").expect("_meta present");
        assert_eq!(meta[meta_keys::PROTOCOL_VERSION], MCP_PROTOCOL_VERSION);
        assert_eq!(meta[meta_keys::CLIENT_INFO]["name"], "juncture-mcp");
        assert_eq!(meta[meta_keys::CLIENT_CAPABILITIES], json!({"tools": {}}));
        assert_eq!(params["x"], 1);
    }

    #[test]
    fn test_request_without_params_still_carries_meta() {
        let request = JsonRpcRequest::with_meta(json!("abc"), "tools/list", None);
        let params = request.params.expect("params present");
        assert!(params.get("_meta").is_some());
    }

    #[test]
    fn test_request_wraps_non_object_params() {
        let request = JsonRpcRequest::with_meta(json!(1), "x", Some(json!([1, 2])));
        let params = request.params.expect("params present");
        assert_eq!(params["value"], json!([1, 2]));
        assert!(
            params.get("_meta").is_some(),
            "mandatory _meta must survive"
        );
    }

    #[test]
    fn test_result_type_parsing() {
        let complete: McpResult =
            serde_json::from_value(json!({"resultType": "complete", "result": {"a": 1}}))
                .expect("parse complete");
        assert!(!complete.result_type.is_input_required());

        let pending: McpResult = serde_json::from_value(json!({
            "resultType": "input_required",
            "inputRequests": {"login": {"method": "elicitation/create", "params": {}}},
            "requestState": "abc"
        }))
        .expect("parse input_required");
        assert!(pending.result_type.is_input_required());
        assert_eq!(pending.input_requests.len(), 1);
        assert_eq!(pending.request_state.as_deref(), Some("abc"));
    }

    #[test]
    fn test_input_requests_serialize_deterministically() {
        let mut requests = BTreeMap::new();
        requests.insert(
            "zeta".to_string(),
            InputRequest {
                method: "elicitation/create".to_string(),
                params: json!({}),
            },
        );
        requests.insert(
            "alpha".to_string(),
            InputRequest {
                method: "sampling/createMessage".to_string(),
                params: json!({}),
            },
        );

        let serialized = serde_json::to_string(&requests).expect("serialize");
        let alpha = serialized.find("alpha").expect("alpha present");
        let zeta = serialized.find("zeta").expect("zeta present");
        assert!(alpha < zeta, "BTreeMap ordering keeps history bytes stable");
    }

    #[test]
    fn test_tools_list_result_parses_paging_and_cache_fields() {
        let result: ToolsListResult = serde_json::from_value(json!({
            "tools": [{"name": "read", "description": "read a file",
                       "inputSchema": {"type": "object"}}],
            "nextCursor": "page-2",
            "ttlMs": 60000,
            "cacheScope": "private"
        }))
        .expect("parse");
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.next_cursor.as_deref(), Some("page-2"));
        assert_eq!(result.ttl_ms, Some(60_000));
        assert_eq!(result.cache_scope, Some(CacheScope::Private));
    }

    #[test]
    fn test_tool_call_result_joins_text_blocks() {
        let result: ToolCallResult = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "first"},
                        {"type": "image", "data": "..."},
                        {"type": "text", "text": "second"}],
            "isError": false
        }))
        .expect("parse");
        assert_eq!(result.text(), "first\nsecond");
        assert!(!result.is_error);
    }

    #[test]
    fn test_tool_call_error_flag() {
        let result: ToolCallResult = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "bad argument"}],
            "isError": true
        }))
        .expect("parse");
        assert!(
            result.is_error,
            "isError must survive so the model can self-correct"
        );
    }

    #[test]
    fn test_header_mismatch_detection() {
        let error = JsonRpcError {
            code: JsonRpcError::HEADER_MISMATCH,
            message: "mismatch".to_string(),
            data: None,
        };
        assert!(error.is_header_mismatch());
    }

    #[test]
    fn test_discover_result_parses_versions() {
        let result: DiscoverResult = serde_json::from_value(json!({
            "supportedVersions": ["2026-07-28", "2025-11-25"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fs", "version": "1.0.0"}
        }))
        .expect("parse");
        assert_eq!(result.supported_versions.len(), 2);
        assert_eq!(result.server_info.name, "fs");
    }

    #[test]
    fn test_jsonrpc_response_error_variant() {
        let response: JsonRpcResponse = serde_json::from_value(json!({
            "id": 1,
            "error": {"code": -32601, "message": "Method not found"}
        }))
        .expect("parse");
        let error = response.error.expect("error present");
        assert_eq!(error.code, JsonRpcError::METHOD_NOT_FOUND);
        assert!(response.result.is_none());
    }
}
