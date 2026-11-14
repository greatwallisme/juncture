//! Transport abstraction for MCP.
//!
//! A transport moves JSON-RPC messages to and from one server. Under the
//! 2026-07-28 stateless protocol it holds no session state, so the trait surface
//! is a single request/response pair plus an optional notification stream
//! (design `12-mcp-and-prefix-cache` §1.3).

#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "stdio")]
pub mod stdio;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpError};

/// A bidirectional MCP message channel to one server.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send one request and await its response.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Transport`] for I/O failures and
    /// [`McpError::Protocol`] when the server returns a JSON-RPC error object.
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;

    /// A stream of server notifications, if this transport supports one.
    ///
    /// `subscriptions/listen` is a single long-lived stream; transports that
    /// cannot provide one return `None`, and the caller simply sees no
    /// out-of-band events.
    fn notifications(&self) -> Option<BoxStream<'static, JsonRpcNotification>> {
        None
    }

    /// Register a tool's input schema so a transport can mirror
    /// `x-mcp-header`-annotated arguments into request headers.
    ///
    /// Only transports that carry per-request headers (Streamable HTTP) act on
    /// this; the default is a no-op.
    fn register_tool_schema(&self, _tool_name: &str, _schema: serde_json::Value) {}

    /// Human-readable transport kind, for diagnostics.
    fn kind(&self) -> &'static str;
}
