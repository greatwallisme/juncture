//! Example 23: MCP Transports (stdio and SSE)
//!
//! Demonstrates both transports `juncture-mcp` speaks, end to end:
//! - `stdio`: a subprocess server reached over newline-delimited JSON-RPC
//! - SSE: a Streamable HTTP server whose response is a `text/event-stream`
//!   carrying one JSON-RPC message (design `12-mcp-and-prefix-cache` §1.6)
//!
//! Everything runs locally and needs no API key or external MCP server: the
//! example re-executes its own binary as the stdio child, and serves the HTTP
//! endpoint itself on an ephemeral loopback port.
//!
//! Key concepts:
//! - `StdioTransport` / `HttpTransport` behind the `McpTransport` abstraction
//! - `McpClient::connect` with `server/discover` version negotiation
//! - `McpToolset` producing a deterministic tool table (prompt-cache segment S1)
//! - `ToolNode` executing MCP tools through the ordinary `Tool` pipeline
//!
//! Run: `cargo run -p juncture-simple-example --bin 23_mcp_transports`

use std::convert::Infallible;
use std::io::{BufRead, Write};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use futures::Stream;
use juncture::tools::ToolNode;
use juncture_core::state::messages::{Message, MessagesState, ToolCall};
use juncture_mcp::{
    HttpConfig, HttpTransport, McpClient, McpToolset, StdioConfig, StdioTransport, qualified_name,
};
use serde_json::{Value, json};

/// Server alias used for the stdio transport. It prefixes every wire tool name.
const STDIO_ALIAS: &str = "stdio";
/// Server alias used for the SSE transport.
const SSE_ALIAS: &str = "sse";
/// Argument that turns this binary into the stdio server child.
const STDIO_SERVER_FLAG: &str = "--stdio-server";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The parent spawns this same binary as its stdio server. Handling that
    // before any runtime exists keeps the child a plain blocking JSON-RPC loop,
    // which is what a real stdio MCP server looks like.
    if std::env::args().nth(1).as_deref() == Some(STDIO_SERVER_FLAG) {
        run_stdio_server()?;
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_demo())
}

/// Run the local demonstration: discover tools over both transports, print the
/// canonical tool table, then execute one call per transport through `ToolNode`.
async fn run_demo() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "Example 23: MCP Transports (stdio + SSE)")?;
    writeln!(stdout, "{}", "=".repeat(58))?;

    // --- Transport 1: stdio subprocess -------------------------------------
    let executable = std::env::current_exe()?;
    let stdio_config =
        StdioConfig::new(executable.to_string_lossy().into_owned()).arg(STDIO_SERVER_FLAG);
    let stdio_transport = Arc::new(StdioTransport::spawn(&stdio_config)?);
    let stdio_client = Arc::new(McpClient::connect(stdio_transport, STDIO_ALIAS).await?);
    let stdio_tools = stdio_client.list_all_tools(None).await?;

    writeln!(stdout, "\n[stdio] subprocess transport")?;
    writeln!(
        stdout,
        "  server   : {} {}",
        stdio_client.server_info().server_info.name,
        stdio_client.server_info().server_info.version
    )?;
    writeln!(stdout, "  protocol : {:?}", stdio_client.mode())?;
    writeln!(stdout, "  tools    : {}", stdio_tools.len())?;

    // --- Transport 2: Streamable HTTP answering with SSE -------------------
    // Binding the listener before serving means the port is reachable as soon
    // as the task is spawned; connect() can run without a readiness poll.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = Router::new().route("/mcp", post(handle_mcp));
    // Guard instead of a bare handle: `JoinHandle` detaches on drop, so an
    // early `?` return would leave the server running until the runtime exits.
    let _sse_server = ServerTask(tokio::spawn(
        async move { axum::serve(listener, app).await },
    ));

    let http_config = HttpConfig::new(format!("http://{address}/mcp"));
    let sse_transport = Arc::new(HttpTransport::new(http_config)?);
    let sse_client = Arc::new(McpClient::connect(sse_transport, SSE_ALIAS).await?);
    let sse_tools = sse_client.list_all_tools(None).await?;

    writeln!(stdout, "\n[sse] Streamable HTTP transport")?;
    writeln!(stdout, "  endpoint : http://{address}/mcp")?;
    writeln!(
        stdout,
        "  server   : {} {}",
        sse_client.server_info().server_info.name,
        sse_client.server_info().server_info.version
    )?;
    writeln!(stdout, "  protocol : {:?}", sse_client.mode())?;
    writeln!(stdout, "  tools    : {}", sse_tools.len())?;

    // --- Aggregate both servers into one toolset ---------------------------
    let mut toolset = McpToolset::new();
    toolset.add_server_tools(STDIO_ALIAS, &stdio_tools, &stdio_client);
    toolset.add_server_tools(SSE_ALIAS, &sse_tools, &sse_client);

    // Both servers expose tools named `echo` and `add`; the alias prefix is
    // what keeps them apart. The order below is canonical, not discovery order,
    // because the tool table is the first prompt-cache prefix segment.
    writeln!(
        stdout,
        "\nMCP tools in canonical wire order (cache segment S1):"
    )?;
    for (index, definition) in toolset.wire_definitions().iter().enumerate() {
        writeln!(stdout, "  {}. {}", index + 1, definition.name)?;
    }

    // --- Execute through the ordinary Tool pipeline ------------------------
    let node = ToolNode::<MessagesState>::new(toolset.into_tools());
    let conversation = vec![
        Message::human("Add two numbers over stdio and echo a message over SSE."),
        Message::ai_with_tool_calls(
            "",
            vec![
                ToolCall {
                    id: "call-add".to_string(),
                    name: qualified_name(STDIO_ALIAS, "add"),
                    arguments: json!({"a": 4, "b": 5}),
                },
                ToolCall {
                    id: "call-echo".to_string(),
                    name: qualified_name(SSE_ALIAS, "echo"),
                    arguments: json!({"message": "hello over text/event-stream"}),
                },
            ],
        ),
    ];

    let mut results = node.execute(&conversation).await?;
    // Tools run concurrently, so sort by call id to keep the printed order
    // identical on every run.
    results.sort_by_key(|message| message.tool_call_id.clone());

    writeln!(stdout, "\nToolNode results:")?;
    for message in &results {
        writeln!(
            stdout,
            "  [{}] {}",
            message.tool_call_id.as_deref().unwrap_or("unknown"),
            message.content_text()
        )?;
    }

    // The stdio child is killed when its transport drops at the end of this
    // function, and `_sse_server` aborts the HTTP server on drop.
    writeln!(stdout, "\nBoth transports exercised; shutting down.")?;
    Ok(())
}

/// Guard that aborts the local HTTP server when the demo returns, however it
/// returns.
///
/// A bare `JoinHandle` detaches on drop, so an early `?` would leave the server
/// bound to its port until the runtime exits.
struct ServerTask(tokio::task::JoinHandle<std::io::Result<()>>);

impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Minimal blocking MCP server used as the stdio child process.
///
/// Reads one JSON-RPC request per line and writes one response per line, which
/// is exactly what [`StdioTransport`] does on the other end of the pipe.
fn run_stdio_server() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                // An unparsable line cannot be correlated with a request, so the
                // reply carries a null id.
                writeln!(
                    stdout,
                    "{}",
                    error_envelope(&Value::Null, -32700, &format!("parse error: {error}"))
                )?;
                stdout.flush()?;
                continue;
            }
        };

        // A notification carries no id and expects no reply.
        if request.get("id").is_none() {
            continue;
        }

        writeln!(stdout, "{}", jsonrpc_response(STDIO_ALIAS, &request))?;
        stdout.flush()?;
    }

    Ok(())
}

/// HTTP handler for the SSE transport: answer one JSON-RPC message with a
/// `text/event-stream` whose final frame is the response.
///
/// The leading comment is a keep-alive, and the notification frame before the
/// response is the shape a real server uses; the client must skip both.
#[allow(
    clippy::unused_async,
    reason = "axum's Handler bound for a response returning `impl Stream` is implemented for async fns; the body needs no await of its own"
)]
async fn handle_mcp(
    Json(request): Json<Value>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {"server": SSE_ALIAS}
    });
    let response = jsonrpc_response(SSE_ALIAS, &request);

    let frames = vec![
        Ok(Event::default().comment("keep-alive")),
        Ok(Event::default().data(notification.to_string())),
        Ok(Event::default().data(response)),
    ];
    Sse::new(futures::stream::iter(frames))
}

/// Build a JSON-RPC response line for one request.
fn jsonrpc_response(server: &str, request: &Value) -> String {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    match dispatch(server, method, &params) {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Err((code, message)) => error_envelope(&id, code, &message),
    }
}

/// Build a JSON-RPC error envelope.
fn error_envelope(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// Dispatch one MCP method to its result payload.
///
/// The same function backs both transports, so the two demos differ only in how
/// bytes travel, never in what the server answers.
fn dispatch(server: &str, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "server/discover" => Ok(json!({
            "supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": format!("{server}-mock"), "version": "1.0.0"},
            "ttlMs": 60_000,
            "cacheScope": "public"
        })),
        "tools/list" => Ok(json!({
            "tools": tool_catalog(),
            "ttlMs": 60_000,
            "cacheScope": "public"
        })),
        "tools/call" => call_tool(server, params),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// The tool table both mock servers publish.
///
/// Deliberately identical on both servers: the aggregate toolset must still
/// produce four distinct wire names thanks to the alias prefix.
fn tool_catalog() -> Value {
    json!([
        {
            "name": "echo",
            "description": "Echoes the given message back.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": {"type": "string", "description": "Text to echo"}
                },
                "required": ["message"]
            }
        },
        {
            "name": "add",
            "description": "Adds two integers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "a": {"type": "integer", "description": "First addend"},
                    "b": {"type": "integer", "description": "Second addend"}
                },
                "required": ["a", "b"]
            }
        }
    ])
}

/// Execute one `tools/call` request and wrap the text in an MCP result.
fn call_tool(server: &str, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "missing tool name".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let text = match name {
        "echo" => {
            let message = arguments
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| (-32602, "missing 'message' argument".to_string()))?;
            format!("{server} echoed: {message}")
        }
        "add" => {
            let a = arguments
                .get("a")
                .and_then(Value::as_i64)
                .ok_or_else(|| (-32602, "missing 'a' argument".to_string()))?;
            let b = arguments
                .get("b")
                .and_then(Value::as_i64)
                .ok_or_else(|| (-32602, "missing 'b' argument".to_string()))?;
            let sum = a
                .checked_add(b)
                .ok_or_else(|| (-32602, "integer overflow".to_string()))?;
            format!("{server} computed: {a} + {b} = {sum}")
        }
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };

    Ok(json!({
        "resultType": "complete",
        "result": {
            "content": [{"type": "text", "text": text}],
            "isError": false
        }
    }))
}

// Rust guideline compliant 2026-11-14
