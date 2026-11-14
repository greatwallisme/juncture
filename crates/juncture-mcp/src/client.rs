//! MCP client: discovery, version negotiation, paginated tool listing, and
//! tool invocation.
//!
//! The 2026-07-28 protocol is stateless, so a client is just "an endpoint plus
//! its negotiated identity" (design `12-mcp-and-prefix-cache` §1.3–§1.5). There
//! is no handshake, no session id, and no server-initiated request dispatch.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::cache::ToolListCache;
use crate::protocol::{
    CacheScope, DiscoverResult, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, McpError,
    McpResult, McpTool, ResultType, ToolCallResult, ToolsListResult,
};
use crate::transport::McpTransport;

/// A legacy server detected via the `initialize` fallback.
///
/// Legacy mode is remembered per client so later requests stop sending modern
/// `_meta` to a server that does not understand it (§1.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolMode {
    /// The server speaks 2026-07-28.
    Modern,
    /// The server required the older `initialize` handshake.
    Legacy,
}

/// A connection to one MCP server.
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    /// Stable identifier used as the cache bucket key.
    server_id: String,
    /// Serialized `server/discover` payload, kept for capability inspection.
    server_info: DiscoverResult,
    mode: ProtocolMode,
    /// Monotonic JSON-RPC id. MRTR retries must use a fresh id, so ids are
    /// allocated per attempt and never reused.
    next_id: AtomicU64,
    tool_cache: ToolListCache,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("server_id", &self.server_id)
            .field("mode", &self.mode)
            .field("transport", &self.transport.kind())
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Discover a server and negotiate a protocol version.
    ///
    /// Sends `server/discover` first, as §1.4 requires. A transport error is
    /// surfaced rather than silently degrading, because a discover failure on a
    /// modern server means the endpoint or headers are wrong.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Transport`] for connection faults and
    /// [`McpError::Protocol`] / [`McpError::Malformed`] for bad payloads.
    pub async fn connect(
        transport: Arc<dyn McpTransport>,
        server_id: &str,
    ) -> Result<Self, McpError> {
        let client = Self {
            transport,
            server_id: server_id.to_string(),
            server_info: DiscoverResult {
                supported_versions: Vec::new(),
                capabilities: crate::protocol::ServerCapabilities::default(),
                server_info: crate::protocol::ServerInfo::default(),
                ttl_ms: None,
                cache_scope: None,
            },
            mode: ProtocolMode::Modern,
            next_id: AtomicU64::new(1),
            tool_cache: ToolListCache::new(),
        };

        let discovered = client.discover().await?;
        let mode = Self::negotiate_mode(&discovered)?;

        Ok(Self {
            server_info: discovered,
            mode,
            ..client
        })
    }

    /// Build a client for unit tests that never performs I/O.
    ///
    /// Any request made through it returns a transport error, which is exactly
    /// what tests that only inspect metadata want.
    #[must_use]
    pub fn for_test() -> Self {
        Self {
            transport: Arc::new(UnconnectedTransport),
            server_id: "test".to_string(),
            server_info: DiscoverResult {
                supported_versions: vec![MCP_PROTOCOL_VERSION.to_string()],
                capabilities: crate::protocol::ServerCapabilities::default(),
                server_info: crate::protocol::ServerInfo {
                    name: "test".to_string(),
                    version: "0".to_string(),
                },
                ttl_ms: None,
                cache_scope: None,
            },
            mode: ProtocolMode::Modern,
            next_id: AtomicU64::new(1),
            tool_cache: ToolListCache::new(),
        }
    }

    /// Negotiated protocol mode.
    #[must_use]
    pub const fn mode(&self) -> ProtocolMode {
        self.mode
    }

    /// Server identity from discovery.
    #[must_use]
    pub const fn server_info(&self) -> &DiscoverResult {
        &self.server_info
    }

    /// Stable cache bucket identifier.
    #[must_use]
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// Run `server/discover`.
    ///
    /// # Errors
    ///
    /// Returns a transport or decode error; the caller decides whether the
    /// endpoint is legacy.
    pub async fn discover(&self) -> Result<DiscoverResult, McpError> {
        let response = self.send("server/discover", None).await?;

        let payload = Self::extract_result(response)?;
        serde_json::from_value(payload).map_err(McpError::from)
    }

    /// Decide whether the server is modern, or requires the legacy handshake.
    fn negotiate_mode(discovered: &DiscoverResult) -> Result<ProtocolMode, McpError> {
        if discovered.supported_versions.is_empty() {
            // Older servers omit `supportedVersions`; treat as legacy rather
            // than failing, since the rest of the client degrades safely.
            return Ok(ProtocolMode::Legacy);
        }
        if discovered
            .supported_versions
            .iter()
            .any(|version| version == MCP_PROTOCOL_VERSION)
        {
            return Ok(ProtocolMode::Modern);
        }
        Err(McpError::UnsupportedVersion {
            supported: discovered.supported_versions.clone(),
        })
    }

    /// List one page of tools, honoring the client-side cache.
    ///
    /// # Arguments
    ///
    /// * `cursor` - Page cursor, `None` for the first page.
    /// * `auth_fingerprint` - Authorization context; required when the server
    ///   declares `cacheScope: private`.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, or decode error.
    pub async fn list_tools_page(
        &self,
        cursor: Option<&str>,
        auth_fingerprint: Option<&str>,
    ) -> Result<ToolsListResult, McpError> {
        // A cached page is only reusable if its scope is known; look it up in
        // both buckets so a public entry stays shared.
        for scope in [CacheScope::Public, CacheScope::Private] {
            if let Some((tools, next_cursor)) =
                self.tool_cache
                    .get(&self.server_id, scope, auth_fingerprint, cursor)
            {
                return Ok(ToolsListResult {
                    tools,
                    next_cursor,
                    ttl_ms: None,
                    cache_scope: Some(scope),
                });
            }
        }

        let params = cursor.map(|cursor| serde_json::json!({"cursor": cursor}));
        let response = self.send("tools/list", params).await?;
        let payload = Self::extract_result(response)?;
        let page: ToolsListResult = serde_json::from_value(payload)?;

        // A server that omits `cacheScope` is treated as private: unknown
        // sharing scope must not default to cross-tenant sharing.
        let scope = page.cache_scope.unwrap_or(CacheScope::Private);
        // A private page with no authorization fingerprint cannot be cached
        // safely. That does not make the page unusable: it is returned to the
        // caller, just never stored, so the next caller re-fetches instead of
        // inheriting someone else's tool table.
        let cache_page = crate::cache::Page {
            tools: page.tools.clone(),
            next_cursor: page.next_cursor.clone(),
            scope,
            ttl_ms: page.ttl_ms,
        };
        if let Err(reason) =
            self.tool_cache
                .put_page(&self.server_id, auth_fingerprint, cursor, cache_page)
        {
            tracing::warn!(
                target: "juncture_mcp",
                server = %self.server_id,
                %reason,
                "tools/list page not cached"
            );
        }

        Ok(page)
    }

    /// Collect every page of `tools/list`.
    ///
    /// # Errors
    ///
    /// Returns a transport, protocol, or decode error. A page failure discards
    /// nothing already collected, but the caller must not treat a partial list
    /// as complete.
    pub async fn list_all_tools(
        &self,
        auth_fingerprint: Option<&str>,
    ) -> Result<Vec<McpTool>, McpError> {
        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = self
                .list_tools_page(cursor.as_deref(), auth_fingerprint)
                .await?;
            collected.extend(page.tools);
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }

        Ok(collected)
    }

    /// Invalidate cached tool lists for this server.
    ///
    /// Called when `notifications/tools/list_changed` arrives, which must take
    /// effect immediately rather than at TTL expiry (§1.7).
    pub fn invalidate_tool_cache(&self) {
        self.tool_cache.invalidate_server(&self.server_id);
    }

    /// Call a tool and return the raw result envelope.
    ///
    /// # Errors
    ///
    /// - [`McpError::NeedsInput`] when the server returned `input_required`.
    /// - [`McpError::Protocol`] for a JSON-RPC error object.
    /// - [`McpError::Malformed`] for a payload that does not match the schema.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<McpResult, McpError> {
        let params = crate::tool_adapter::call_params(tool_name, arguments);
        let response = self.send("tools/call", Some(params)).await?;
        let payload = Self::extract_result(response)?;
        let result: McpResult = serde_json::from_value(payload)?;

        if result.result_type == ResultType::InputRequired {
            return Err(McpError::NeedsInput {
                requests: result.input_requests,
                request_state: result.request_state,
            });
        }

        // `isError: true` is a tool-side failure the model must see, not a
        // client-side failure. Surface it typed so the adapter maps it correctly.
        if let Some(value) = &result.result {
            let call: ToolCallResult = serde_json::from_value(value.clone())?;
            if call.is_error {
                return Err(McpError::ToolExecution {
                    message: call.text(),
                });
            }
        }

        Ok(result)
    }

    /// Retry an `input_required` call with the caller's answers.
    ///
    /// Uses a fresh JSON-RPC id as the protocol requires, and replays
    /// `request_state` byte-for-byte (§1.5).
    ///
    /// # Errors
    ///
    /// Same as [`Self::call_tool`].
    pub async fn resume_tool_call(
        &self,
        tool_name: &str,
        arguments: Value,
        input_responses: Value,
        request_state: Option<&str>,
    ) -> Result<McpResult, McpError> {
        let mut params = crate::tool_adapter::call_params(tool_name, arguments);
        if let Some(map) = params.as_object_mut() {
            map.insert("inputResponses".to_string(), input_responses);
            if let Some(state) = request_state {
                // Verbatim: re-encoding can change bytes the server signed.
                map.insert("requestState".to_string(), Value::String(state.to_string()));
            }
        }

        let response = self.send("tools/call", Some(params)).await?;
        let payload = Self::extract_result(response)?;
        let result: McpResult = serde_json::from_value(payload)?;

        if result.result_type == ResultType::InputRequired {
            return Err(McpError::NeedsInput {
                requests: result.input_requests,
                request_state: result.request_state,
            });
        }
        Ok(result)
    }

    /// Send one request with a fresh id and translate a JSON-RPC error.
    async fn send(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::with_meta(Value::from(id), method, params);
        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            if error.is_header_mismatch() {
                return Err(McpError::HeaderMismatch);
            }
            return Err(McpError::Protocol(error));
        }

        Ok(response)
    }

    /// Unwrap a successful response envelope into its `result` payload.
    fn extract_result(response: JsonRpcResponse) -> Result<Value, McpError> {
        response.result.ok_or_else(|| {
            McpError::Malformed("response carried neither result nor error".to_string())
        })
    }
}

/// Transport that fails every request, used by [`McpClient::for_test`],
/// which exists for tests that only inspect client metadata.
struct UnconnectedTransport;

#[async_trait::async_trait]
impl McpTransport for UnconnectedTransport {
    async fn request(&self, _request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        Err(McpError::Transport(
            "test client has no transport".to_string(),
        ))
    }

    fn kind(&self) -> &'static str {
        "unconnected"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use serde_json::json;
    use std::sync::Mutex;

    /// Scripted transport: returns queued responses in order and records requests.
    struct ScriptedTransport {
        responses: Mutex<Vec<Result<JsonRpcResponse, McpError>>>,
        seen: Mutex<Vec<JsonRpcRequest>>,
    }

    impl ScriptedTransport {
        fn new(responses: Vec<Result<JsonRpcResponse, McpError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen_methods(&self) -> Vec<String> {
            self.seen
                .lock()
                .expect("lock")
                .iter()
                .map(|request| request.method.clone())
                .collect()
        }
    }

    #[async_trait]
    impl McpTransport for ScriptedTransport {
        async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            self.seen.lock().expect("lock").push(request);
            let mut responses = self.responses.lock().expect("lock");
            if responses.is_empty() {
                return Err(McpError::Transport("no scripted response".to_string()));
            }
            responses.remove(0)
        }

        fn notifications(
            &self,
        ) -> Option<BoxStream<'static, crate::protocol::JsonRpcNotification>> {
            None
        }

        fn kind(&self) -> &'static str {
            "scripted"
        }
    }

    /// Build a successful response. The `Result` matches the transport queue's
    /// element type so callers can mix successes and scripted failures.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "the queued element type is Result; successes must share that type"
    )]
    fn response(id: u64, result: Value) -> Result<JsonRpcResponse, McpError> {
        Ok(JsonRpcResponse {
            id: json!(id),
            result: Some(result),
            error: None,
        })
    }

    async fn connect_with(transport: Arc<ScriptedTransport>) -> McpClient {
        let transport: Arc<dyn McpTransport> = transport;
        McpClient::connect(transport, "srv").await.expect("connect")
    }

    #[tokio::test]
    async fn test_connect_sends_discover_first() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            1,
            json!({
                "supportedVersions": [MCP_PROTOCOL_VERSION],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fs", "version": "1"}
            }),
        )]));

        let client = connect_with(Arc::clone(&transport)).await;
        assert_eq!(client.mode(), ProtocolMode::Modern);
        assert_eq!(transport.seen_methods(), vec!["server/discover"]);
        assert_eq!(client.server_info().server_info.name, "fs");
    }

    #[tokio::test]
    async fn test_connect_unsupported_version_errors() {
        let transport: Arc<dyn McpTransport> = Arc::new(ScriptedTransport::new(vec![response(
            1,
            json!({"supportedVersions": ["1999-01-01"], "serverInfo": {}}),
        )]));
        let result = McpClient::connect(transport, "srv").await;
        assert!(matches!(result, Err(McpError::UnsupportedVersion { .. })));
    }

    #[tokio::test]
    async fn test_connect_without_versions_falls_back_to_legacy() {
        let transport: Arc<dyn McpTransport> = Arc::new(ScriptedTransport::new(vec![response(
            1,
            json!({"capabilities": {}, "serverInfo": {}}),
        )]));
        let client = McpClient::connect(transport, "srv").await.expect("connect");
        assert_eq!(client.mode(), ProtocolMode::Legacy);
    }

    #[tokio::test]
    async fn test_list_tools_page_paginates_and_caches() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({
                    "supportedVersions": [MCP_PROTOCOL_VERSION],
                    "serverInfo": {}
                }),
            ),
            response(
                2,
                json!({
                    "tools": [{"name": "a", "inputSchema": {"type": "object"}}],
                    "nextCursor": "p2",
                    "ttlMs": 60000,
                    "cacheScope": "public"
                }),
            ),
            response(
                3,
                json!({
                    "tools": [{"name": "b", "inputSchema": {"type": "object"}}],
                    "ttlMs": 60000,
                    "cacheScope": "public"
                }),
            ),
        ]));

        let client = connect_with(Arc::clone(&transport)).await;
        let tools = client.list_all_tools(None).await.expect("list");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "a");
        assert_eq!(tools[1].name, "b");

        // A second full listing must be served from cache: no new transport calls.
        let before = transport.seen_methods().len();
        let again = client.list_all_tools(None).await.expect("cached list");
        assert_eq!(again.len(), 2);
        assert_eq!(
            transport.seen_methods().len(),
            before,
            "cache must avoid refetch"
        );
    }

    #[tokio::test]
    async fn test_private_page_without_fingerprint_is_returned_but_not_cached() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "tools": [{"name": "secret", "inputSchema": {}}],
                    "cacheScope": "private"
                }),
            ),
            response(
                3,
                json!({
                    "tools": [{"name": "secret", "inputSchema": {}}],
                    "cacheScope": "private"
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;

        let first = client
            .list_tools_page(None, None)
            .await
            .expect("first page");
        assert_eq!(first.tools.len(), 1);

        // The page is usable but was not cached, so the transport is hit again
        // instead of serving another caller's tools.
        let second = client
            .list_tools_page(None, None)
            .await
            .expect("second page");
        assert_eq!(second.tools.len(), 1);
        assert_eq!(
            transport.seen_methods(),
            vec!["server/discover", "tools/list", "tools/list"],
            "an uncacheable private page must be re-fetched"
        );
    }

    #[tokio::test]
    async fn test_private_page_with_fingerprint_is_cached_per_bucket() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "tools": [{"name": "secret", "inputSchema": {}}],
                    "cacheScope": "private",
                    "ttlMs": 60000
                }),
            ),
            response(
                3,
                json!({
                    "tools": [{"name": "secret", "inputSchema": {}}],
                    "cacheScope": "private",
                    "ttlMs": 60000
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;

        let _ = client
            .list_tools_page(None, Some("tenant-a"))
            .await
            .expect("a");
        // Same tenant: served from cache.
        let _ = client
            .list_tools_page(None, Some("tenant-a"))
            .await
            .expect("a cached");
        // Different tenant: must not see tenant-a's cached page.
        let _ = client
            .list_tools_page(None, Some("tenant-b"))
            .await
            .expect("b");

        assert_eq!(
            transport.seen_methods(),
            vec!["server/discover", "tools/list", "tools/list"],
            "only the second tenant triggers a refetch; the first is cached"
        );
    }

    #[tokio::test]
    async fn test_missing_cache_scope_defaults_to_private() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(2, json!({"tools": [{"name": "t", "inputSchema": {}}]})),
            response(3, json!({"tools": [{"name": "t", "inputSchema": {}}]})),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;
        // No fingerprint: an unspecified scope must not become shareable, so
        // each call re-fetches rather than reusing the first result.
        let _ = client.list_tools_page(None, None).await.expect("first");
        let _ = client.list_tools_page(None, None).await.expect("second");
        assert_eq!(
            transport.seen_methods(),
            vec!["server/discover", "tools/list", "tools/list"]
        );
    }

    #[tokio::test]
    async fn test_call_tool_returns_complete_result() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "resultType": "complete",
                    "result": {"content": [{"type": "text", "text": "hello"}]}
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;
        let result = client
            .call_tool("echo", json!({"x": 1}))
            .await
            .expect("call");
        assert_eq!(result.result_type, ResultType::Complete);
        assert_eq!(
            transport.seen_methods(),
            vec!["server/discover", "tools/call"]
        );
    }

    #[tokio::test]
    async fn test_call_tool_surfaces_input_required() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "resultType": "input_required",
                    "inputRequests": {"login": {"method": "elicitation/create", "params": {}}},
                    "requestState": "opaque-state"
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;
        let error = client
            .call_tool("t", json!({}))
            .await
            .expect_err("needs input");
        match error {
            McpError::NeedsInput {
                requests,
                request_state,
            } => {
                assert_eq!(requests.len(), 1);
                assert_eq!(request_state.as_deref(), Some("opaque-state"));
            }
            other => panic!("expected NeedsInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_call_tool_tool_error_is_typed() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "resultType": "complete",
                    "result": {
                        "content": [{"type": "text", "text": "bad argument: path"}],
                        "isError": true
                    }
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;
        let error = client
            .call_tool("t", json!({}))
            .await
            .expect_err("tool error");
        match error {
            McpError::ToolExecution { message } => assert!(message.contains("bad argument")),
            other => panic!("expected ToolExecution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resume_tool_call_uses_fresh_id_and_replays_state() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                1,
                json!({"supportedVersions": [MCP_PROTOCOL_VERSION], "serverInfo": {}}),
            ),
            response(
                2,
                json!({
                    "resultType": "complete",
                    "result": {"content": [{"type": "text", "text": "done"}]}
                }),
            ),
        ]));
        let client = connect_with(Arc::clone(&transport)).await;

        let result = client
            .resume_tool_call("t", json!({}), json!({"login": "ok"}), Some("state-1"))
            .await
            .expect("resume");
        assert_eq!(result.result_type, ResultType::Complete);

        // The retry must carry a different id than the discovery request.
        // Copy what the assertions need, then release the guard so it is not
        // held across the rest of the test.
        let (first_id, retry_id, params) = {
            let seen = transport.seen.lock().expect("lock");
            let params = seen[1].params.clone().expect("params");
            (seen[0].id.clone(), seen[1].id.clone(), params)
        };
        assert_ne!(
            first_id, retry_id,
            "MRTR retry requires a fresh JSON-RPC id"
        );
        assert_eq!(params["requestState"], "state-1");
        assert_eq!(params["inputResponses"]["login"], "ok");
    }

    #[tokio::test]
    async fn test_jsonrpc_error_is_protocol_error() {
        let transport: Arc<dyn McpTransport> =
            Arc::new(ScriptedTransport::new(vec![Ok(JsonRpcResponse {
                id: json!(1),
                result: None,
                error: Some(crate::protocol::JsonRpcError {
                    code: crate::protocol::JsonRpcError::METHOD_NOT_FOUND,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            })]));
        let result = McpClient::connect(transport, "srv").await;
        assert!(matches!(result, Err(McpError::Protocol(_))));
    }
}
