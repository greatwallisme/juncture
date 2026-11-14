//! MCP client integration for Juncture.
//!
//! MCP is a *producer of tools*, not a new execution layer: it yields
//! `Vec<Box<dyn Tool>>` which is handed to the existing prebuilt agent factory,
//! so Graph, Pregel, State, checkpointing, and streaming are untouched (design
//! `12-mcp-and-prefix-cache` §1.1).
//!
//! ```ignore
//! use std::sync::Arc;
//! use juncture_mcp::{McpClient, McpToolset, StdioTransport, StdioConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let transport = Arc::new(StdioTransport::spawn(&StdioConfig::new("my-mcp-server"))?);
//! let client = Arc::new(McpClient::connect(transport, "my-server").await?);
//! let tools = client.list_all_tools(None).await?;
//!
//! let mut toolset = McpToolset::new();
//! toolset.add_server_tools("my-server", &tools, Arc::clone(&client));
//! let _tools = toolset.into_tools(); // feed to create_agent_with_config
//! # Ok(())
//! # }
//! ```
//!
//! # Scope
//!
//! Implemented: protocol framing, stdio and HTTP transports, `tools/list`
//! caching with `ttlMs`/`cacheScope`, multi-server name disambiguation, and the
//! `Tool` adapter including multi-round-trip input handling.
//!
//! Deliberately not implemented (design `12` §1.11): MCP server mode, Roots /
//! Sampling / Logging (deprecated upstream), and `resources/*` / `prompts/*`
//! (this integration targets tools only).

pub mod cache;
pub mod client;
pub mod naming;
pub mod protocol;
pub mod tool_adapter;
pub mod transport;

pub use cache::ToolListCache;
pub use client::{McpClient, ProtocolMode};
pub use naming::{annotated_description, qualified_name, unique_aliases};
pub use protocol::{
    CacheScope, DiscoverResult, InputRequest, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    MCP_PROTOCOL_VERSION, McpError, McpResult, McpTool, ResultType, ServerCapabilities, ServerInfo,
    ToolContent, ToolsListResult,
};
pub use tool_adapter::{
    McpToolEntry, McpToolset, call_params, encode_header_value, header_params, sanitize_arguments,
};
pub use transport::McpTransport;

#[cfg(feature = "stdio")]
pub use transport::stdio::{SharedTransport, StdioConfig, StdioTransport};

#[cfg(feature = "http")]
pub use transport::http::{HttpConfig, HttpTransport};

// Rust guideline compliant 2026-11-14
