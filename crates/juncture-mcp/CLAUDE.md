# CLAUDE.md -- juncture-mcp

MCP client integration. MCP servers are a *producer of tools*: the crate yields
`Vec<Box<dyn Tool>>` that feeds the existing prebuilt agent factory, so Graph,
Pregel, State, checkpointing, and streaming are untouched (design
`12-mcp-and-prefix-cache` §1.1).

## Structure

```
src/
  lib.rs          -- public API re-exports
  protocol.rs     -- JSON-RPC 2.0 frames + MCP 2026-07-28 results (resultType / inputRequests / requestState)
  client.rs       -- McpClient: server/discover, version negotiation, paged tools/list, tools/call, MRTR resume
  cache.rs        -- ToolListCache: ttlMs expiry + cacheScope public/private bucketing + invalidate_server
  naming.rs       -- qualified_name (server__tool), alias normalization, description provenance
  tool_adapter.rs -- McpToolset / McpToolEntry -> impl Tool; x-mcp-header mirroring; NeedsInput mapping
  transport/
    mod.rs        -- McpTransport trait (request + optional notification stream)
    stdio.rs      -- subprocess stdio transport (stderr pump, kill-on-drop)
    http.rs       -- Streamable HTTP transport (MCP-Protocol-Version / Mcp-Method / Mcp-Name headers, JSON + SSE)
```

## Key Invariants

- `McpToolset` keeps tools in a `Vec` and emits `(server_alias, tool_name)` order.
  The tool table is the first prompt-cache prefix segment, so unordered output
  would silently destroy caching (design 12 §1.9). Use `wire_definitions()` for
  the cache-ready sequence.
- `cacheScope: private` is bucketed by authorization fingerprint. A private page
  with no fingerprint is returned to the caller but never cached; an unspecified
  scope is treated as private.
- `McpToolEntry::invoke` never calls `interrupt!`. A tool body runs inside
  `JoinSet::spawn`, which does not inherit the node's task-local interrupt
  context, and resuming replays the node and would re-call the tool. Interactive
  rounds surface as `ToolError::NeedsInput`; the human branch belongs in the node
  (design 12 §1.5.1–1.5.3).
- `tools/call` results with `isError: true` become `ToolError::ExecutionFailed`
  carrying the tool's text, so the model can self-correct. Protocol errors are
  not model-actionable and are reported as failures.
- MRTR retries use a fresh JSON-RPC id and replay `requestState` verbatim.

## Features

- `stdio` -- subprocess transport (`tokio/process`; native only)
- `http` -- Streamable HTTP transport via `reqwest` with rustls (no system OpenSSL)

## Scope

Implemented: protocol framing, stdio + HTTP transports, `tools/list` caching,
multi-server name disambiguation, `Tool` adapter with MRTR input handling.

Not implemented (design 12 §1.11): MCP server mode, Roots / Sampling / Logging
(deprecated upstream), `resources/*` / `prompts/*`.

## Testing

```bash
cargo test -p juncture-mcp --all-features
cargo test -p juncture-mcp --all-features --test mcp_prefix_integration
```

`tests/mcp_prefix_integration.rs` is the cross-surface suite: it proves a tool-set
change only affects segment S1, that discovery order and server-side JSON key
order cannot change the prefix bytes, and that a large MCP tool table earns an S1
breakpoint while the volatile turn never does.
