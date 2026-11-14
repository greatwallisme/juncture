//! Adapter turning MCP tools into Juncture [`Tool`] implementations.
//!
//! This is the only integration point between MCP and Juncture (design
//! `12-mcp-and-prefix-cache` §1.1): an MCP server produces
//! `Vec<Box<dyn Tool>>`, which is handed to the existing prebuilt agent factory.
//! Graph, Pregel, and State layers are untouched.
//!
//! # Why `invoke` never interrupts
//!
//! A tool body executes inside `JoinSet::spawn`, which does not inherit the
//! node's task-local interrupt context, so `interrupt!` inside a tool returns a
//! deterministic error. Worse, resuming replays the node and would call the
//! external tool twice. Interactive rounds are therefore surfaced as
//! [`ToolError::NeedsInput`] and resolved by the caller at node level (§1.5.1–1.5.3).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use serde_json::{Map, Value};

use crate::client::McpClient;
use crate::protocol::{InputRequest, McpError, ResultType, ToolCallResult};

/// One tool as exposed to the model, together with its live client.
#[derive(Clone)]
pub struct McpToolEntry {
    /// Disambiguated wire name, e.g. `github__create_issue`.
    pub qualified_name: String,
    /// Server alias that produced this tool.
    pub server: String,
    /// Original tool name within that server.
    pub tool_name: String,
    /// Description shown to the model, with provenance.
    pub description: String,
    /// JSON Schema for the tool input.
    pub input_schema: Value,
    client: Arc<McpClient>,
}

impl std::fmt::Debug for McpToolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolEntry")
            .field("qualified_name", &self.qualified_name)
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl McpToolEntry {
    /// Build an entry from a discovered tool.
    #[must_use]
    pub const fn new(
        qualified_name: String,
        server: String,
        tool_name: String,
        description: String,
        input_schema: Value,
        client: Arc<McpClient>,
    ) -> Self {
        Self {
            qualified_name,
            server,
            tool_name,
            description,
            input_schema,
            client,
        }
    }

    /// Call the tool, resolving protocol-level rounds that need no human.
    ///
    /// # Errors
    ///
    /// - [`ToolError::NeedsInput`] when an input request cannot be satisfied
    ///   automatically. The caller resolves it and retries.
    /// - [`ToolError::ExecutionFailed`] when the tool reported `isError: true`;
    ///   the message is the tool's text, which the model can act on.
    /// - [`ToolError::InvalidInput`] / [`ToolError::ExecutionFailed`] for
    ///   transport and protocol faults.
    pub async fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let payload = self
            .client
            .call_tool(&self.tool_name, arguments)
            .await
            .map_err(|error| Self::map_error(&error))?;

        match payload.result_type {
            ResultType::Complete => {
                let value = payload.result.ok_or_else(|| {
                    ToolError::execution_failed("MCP complete result had no payload".to_string())
                })?;
                Self::interpret_tool_result(&value)
            }
            // `call_tool` converts `input_required` into `McpError::NeedsInput`,
            // so reaching here means the server contradicted itself.
            ResultType::InputRequired => Err(ToolError::execution_failed(
                "MCP returned input_required without an input payload".to_string(),
            )),
        }
    }

    /// Convert an MCP tool payload into the string a tool message will carry.
    fn interpret_tool_result(value: &Value) -> Result<String, ToolError> {
        let parsed: ToolCallResult = serde_json::from_value(value.clone()).map_err(|error| {
            ToolError::execution_failed(format!("malformed tool result: {error}"))
        })?;

        if parsed.is_error {
            // Tool-side errors are model-actionable: return the text so the
            // model can correct the call (design 12 §1.10).
            return Err(ToolError::execution_failed(parsed.text()));
        }

        Ok(parsed.text())
    }

    /// Build a `NeedsInput` error, preserving `request_state` byte-for-byte.
    fn needs_input(
        requests: &BTreeMap<String, InputRequest>,
        request_state: Option<&str>,
    ) -> ToolError {
        let fields = requests
            .iter()
            .map(|(key, request)| (key.clone(), request.method.clone()))
            .collect::<BTreeMap<String, String>>();
        ToolError::needs_input(request_state.map(str::to_string), fields)
    }

    /// Map a client error onto the tool error surface.
    ///
    /// The split mirrors MCP's own distinction: a protocol error is something
    /// the model cannot fix, while a tool-execution error is. Unknown tools and
    /// malformed requests are reported as failures; nothing here is silently
    /// swallowed.
    fn map_error(error: &McpError) -> ToolError {
        match error {
            McpError::NeedsInput {
                requests,
                request_state,
            } => Self::needs_input(requests, request_state.as_deref()),
            McpError::ToolExecution { message } => ToolError::execution_failed(message.clone()),
            McpError::Protocol(json_rpc) => ToolError::execution_failed(format!(
                "MCP protocol error {}: {}",
                json_rpc.code, json_rpc.message
            )),
            McpError::UnsupportedVersion { supported } => ToolError::execution_failed(format!(
                "server supports no common MCP version: {supported:?}"
            )),
            McpError::HeaderMismatch => {
                ToolError::execution_failed("MCP header/body mismatch (-32020)".to_string())
            }
            McpError::Transport(message) | McpError::Malformed(message) => {
                ToolError::execution_failed(message.clone())
            }
        }
    }
}

#[async_trait]
impl Tool for McpToolEntry {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn invoke(&self, input: Value) -> Result<String, ToolError> {
        self.call(input).await
    }
}

/// A collection of MCP tools aggregated from one or more servers.
///
/// Stored as a `Vec` and emitted in a fixed `(server_alias, tool_name)` order so
/// the tool table, which is the first prompt-cache prefix segment, is
/// byte-stable across runs (design `12` §1.9).
#[derive(Debug, Default)]
pub struct McpToolset {
    entries: Vec<McpToolEntry>,
}

impl McpToolset {
    /// Create an empty toolset.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one server's tools.
    ///
    /// `cursor`-based pages are passed one at a time; callers use
    /// [`McpClient::list_all_tools`] to collect a full list first.
    pub fn add_server_tools(
        &mut self,
        server_alias: &str,
        tools: &[crate::protocol::McpTool],
        client: &Arc<McpClient>,
    ) {
        for tool in tools {
            self.entries.push(McpToolEntry::new(
                crate::naming::qualified_name(server_alias, &tool.name),
                server_alias.to_string(),
                tool.name.clone(),
                crate::naming::annotated_description(server_alias, &tool.description),
                // Normalize at the boundary so the S1 prefix never depends on
                // how a server happened to order its schema keys.
                juncture_core::prefix::normalize_json(&tool.input_schema),
                Arc::clone(client),
            ));
        }
    }

    /// Sort entries into the canonical wire order.
    ///
    /// Sorting happens on read so the ordering rule lives in one place and
    /// cannot be forgotten by an insertion path.
    #[must_use]
    pub fn ordered(&self) -> Vec<&McpToolEntry> {
        let mut ordered: Vec<&McpToolEntry> = self.entries.iter().collect();
        ordered.sort_by(|left, right| {
            left.server
                .cmp(&right.server)
                .then_with(|| left.tool_name.cmp(&right.tool_name))
        });
        ordered
    }

    /// Wire tool definitions in canonical order, ready to enter cache segment S1.
    ///
    /// This is the documented seam between MCP and the prompt cache (design
    /// `12` §1.9): ordering and schema normalization are applied by
    /// [`juncture_core::prefix::ordered_tool_definitions`], so the returned
    /// sequence is byte-stable regardless of discovery order or server-side JSON
    /// key order.
    #[must_use]
    pub fn wire_definitions(&self) -> Vec<juncture_core::llm::ToolDefinition> {
        let definitions: Vec<juncture_core::llm::ToolDefinition> = self
            .ordered()
            .into_iter()
            .map(|entry| juncture_core::llm::ToolDefinition {
                name: entry.qualified_name.clone(),
                description: entry.description.clone(),
                parameters: entry.input_schema.clone(),
            })
            .collect();
        juncture_core::prefix::ordered_tool_definitions(&definitions)
    }

    /// Consume the toolset into boxed tools in canonical order.
    #[must_use]
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        let mut entries = self.entries;
        entries.sort_by(|left, right| {
            left.server
                .cmp(&right.server)
                .then_with(|| left.tool_name.cmp(&right.tool_name))
        });
        entries
            .into_iter()
            .map(|entry| Box::new(entry) as Box<dyn Tool>)
            .collect()
    }

    /// Number of tools across all servers.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the toolset is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Extract `x-mcp-header`-annotated parameters from a tool schema.
///
/// Returns `(param_name, header_name)` pairs. Callers mirror those argument
/// values into `Mcp-Param-*` headers when using the HTTP transport (design `12`
/// §1.6). A parameter whose header name is not a valid token is dropped with a
/// warning rather than failing the whole tool list.
#[must_use]
pub fn header_params(input_schema: &Value) -> Vec<(String, String)> {
    let Some(properties) = input_schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut pairs = Vec::new();
    for (name, schema) in properties {
        let Some(header) = schema.get("x-mcp-header").and_then(Value::as_str) else {
            continue;
        };
        if !is_valid_header_token(header) {
            tracing::warn!(
                target: "juncture_mcp",
                param = %name,
                header = %header,
                "dropping x-mcp-header annotation with an invalid header name"
            );
            continue;
        }
        pairs.push((name.clone(), header.to_string()));
    }
    pairs.sort_by(|left, right| left.1.cmp(&right.1));
    pairs
}

/// Whether `name` is a valid HTTP header token.
#[must_use]
pub fn is_valid_header_token(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

/// Encode a header value per MCP's mirroring rule.
///
/// Non-ASCII, control characters, and leading/trailing whitespace are wrapped
/// as `=?base64?{b64}?=`; everything else is sent verbatim.
#[must_use]
pub fn encode_header_value(value: &str) -> String {
    let needs_encoding = value
        .chars()
        .any(|ch| !ch.is_ascii() || ch.is_ascii_control())
        || value.trim() != value;

    if needs_encoding {
        format!("=?base64?{}?=", base64_encode(value.as_bytes()))
    } else {
        value.to_string()
    }
}

/// Minimal standard-alphabet base64 encoder (no padding dependency).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        out.push(ALPHABET[usize::from(b0 >> 2)] as char);
        out.push(ALPHABET[usize::from(((b0 & 0b11) << 4) | (b1 >> 4))] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[usize::from(((b1 & 0b1111) << 2) | (b2 >> 6))] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[usize::from(b2 & 0b11_1111)] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Build `tools/call` arguments, dropping protocol-reserved keys.
///
/// `_meta` belongs to the protocol envelope, not to the tool's arguments; a
/// model-supplied `_meta` would otherwise be forwarded as a tool parameter.
#[must_use]
pub fn sanitize_arguments(arguments: Value) -> Value {
    match arguments {
        Value::Object(mut map) => {
            map.remove("_meta");
            Value::Object(map)
        }
        other => other,
    }
}

/// Build the `tools/call` params object.
#[must_use]
pub fn call_params(tool_name: &str, arguments: Value) -> Value {
    let mut map = Map::new();
    map.insert("name".to_string(), Value::String(tool_name.to_string()));
    map.insert("arguments".to_string(), sanitize_arguments(arguments));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ToolContent;
    use serde_json::json;

    #[test]
    fn test_header_params_extracted_and_sorted() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tenant": {"type": "string", "x-mcp-header": "Mcp-Tenant"},
                "region": {"type": "string", "x-mcp-header": "Mcp-Region"},
                "plain": {"type": "string"}
            }
        });
        let pairs = header_params(&schema);
        assert_eq!(
            pairs,
            vec![
                ("region".to_string(), "Mcp-Region".to_string()),
                ("tenant".to_string(), "Mcp-Tenant".to_string()),
            ]
        );
    }

    #[test]
    fn test_header_params_drops_invalid_header_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "bad": {"type": "string", "x-mcp-header": "Bad Header"}
            }
        });
        assert_eq!(header_params(&schema).len(), 0);
    }

    #[test]
    fn test_header_params_without_properties() {
        assert_eq!(header_params(&json!({"type": "object"})).len(), 0);
    }

    #[test]
    fn test_is_valid_header_token() {
        assert!(is_valid_header_token("Mcp-Tenant"));
        assert!(!is_valid_header_token(""));
        assert!(!is_valid_header_token("has space"));
        assert!(!is_valid_header_token("has:colon"));
    }

    #[test]
    fn test_encode_header_value_plain() {
        assert_eq!(encode_header_value("tenant-a"), "tenant-a");
    }

    #[test]
    fn test_encode_header_value_non_ascii_is_base64() {
        assert_eq!(encode_header_value("租户"), "=?base64?56ef5oi3?=");
    }

    #[test]
    fn test_encode_header_value_whitespace_is_base64() {
        assert_eq!(encode_header_value(" pad "), "=?base64?IHBhZCA=?=");
    }

    #[test]
    fn test_base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_sanitize_arguments_removes_meta() {
        let sanitized = sanitize_arguments(json!({"q": "rust", "_meta": {"x": 1}}));
        assert_eq!(sanitized, json!({"q": "rust"}));
    }

    #[test]
    fn test_call_params_shape() {
        let params = call_params("read_file", json!({"path": "/tmp/x"}));
        assert_eq!(params["name"], "read_file");
        assert_eq!(params["arguments"]["path"], "/tmp/x");
    }

    #[test]
    fn test_toolcallresult_text_ignores_unknown_blocks() {
        let result: ToolCallResult = serde_json::from_value(json!({
            "content": [{"type": "text", "text": "ok"}, {"type": "audio"}]
        }))
        .expect("parse");
        assert_eq!(result.text(), "ok");
    }

    #[test]
    fn test_tool_content_other_variant() {
        let block: ToolContent =
            serde_json::from_value(json!({"type": "resource"})).expect("parse");
        assert!(matches!(block, ToolContent::Other));
    }

    #[test]
    fn test_toolset_ordering_is_canonical() {
        // Construct entries directly: the test targets ordering, not discovery.
        let client = Arc::new(McpClient::for_test());
        let mut toolset = McpToolset::new();
        toolset.add_server_tools("zeta", &[test_tool("beta"), test_tool("alpha")], &client);
        toolset.add_server_tools("alpha", &[test_tool("gamma")], &client);

        let names: Vec<&str> = toolset
            .ordered()
            .iter()
            .map(|e| e.qualified_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["alpha__gamma", "zeta__alpha", "zeta__beta"],
            "sorted by (server, tool)"
        );
    }

    #[test]
    fn test_toolset_normalizes_schema_keys() {
        let client = Arc::new(McpClient::for_test());
        let mut toolset = McpToolset::new();
        toolset.add_server_tools(
            "srv",
            &[crate::protocol::McpTool {
                name: "t".to_string(),
                description: String::new(),
                input_schema: json!({"type": "object", "properties": {"z": {}, "a": {}}}),
                output_schema: None,
            }],
            &client,
        );
        let entry = toolset.ordered()[0];
        assert_eq!(
            serde_json::to_string(&entry.input_schema).expect("serialize"),
            r#"{"properties":{"a":{},"z":{}},"type":"object"}"#
        );
    }

    #[test]
    fn test_toolset_into_tools_preserves_order() {
        let client = Arc::new(McpClient::for_test());
        let mut toolset = McpToolset::new();
        toolset.add_server_tools("b", &[test_tool("two")], &client);
        toolset.add_server_tools("a", &[test_tool("one")], &client);

        let tools = toolset.into_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["a__one", "b__two"]);
    }

    #[test]
    fn test_description_carries_provenance() {
        let client = Arc::new(McpClient::for_test());
        let mut toolset = McpToolset::new();
        toolset.add_server_tools(
            "srv",
            &[crate::protocol::McpTool {
                name: "t".to_string(),
                description: "Does a thing".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
            }],
            &client,
        );
        assert_eq!(
            toolset.ordered()[0].description,
            "[from server srv]\nDoes a thing"
        );
    }

    fn test_tool(name: &str) -> crate::protocol::McpTool {
        crate::protocol::McpTool {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        }
    }
}
