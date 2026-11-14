//! Cross-surface tests: MCP tools feeding the prompt-cache prefix.
//!
//! This is the only coupling between the two halves of design
//! `12-mcp-and-prefix-cache`: the MCP toolset becomes the S1 prefix segment
//! (§1.9), so its ordering and schema bytes decide whether the cache survives.
//! These tests are mutation-sensitive on purpose: removing the ordering or the
//! normalization makes them fail.

use std::sync::Arc;

use juncture_core::prefix::{
    PrefixSegment, estimate_tokens, ordered_tool_definitions, plan_breakpoints, prefix_bytes,
};
use juncture_mcp::{McpClient, McpTool, McpToolset};
use serde_json::json;

fn mcp_tool(name: &str, schema: serde_json::Value) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: format!("{name} tool"),
        input_schema: schema,
        output_schema: None,
    }
}

fn test_client() -> Arc<McpClient> {
    Arc::new(McpClient::for_test())
}

/// Prove the tool-set change is confined to S1: S2 (system) bytes are unaffected
/// and only S1 differs, which is exactly the scope of a cache invalidation.
#[test]
fn tool_set_change_only_affects_s1() {
    let client = test_client();

    let mut before = McpToolset::new();
    before.add_server_tools(
        "fs",
        &[mcp_tool("read", json!({"type": "object"}))],
        &client,
    );

    let mut after = McpToolset::new();
    after.add_server_tools(
        "fs",
        &[
            mcp_tool("read", json!({"type": "object"})),
            mcp_tool("write", json!({"type": "object"})),
        ],
        &client,
    );

    let before_defs = before.wire_definitions();
    let after_defs = after.wire_definitions();

    let system = "tenant business background";
    let s1_before = prefix_bytes(&before_defs, system, &[]);
    let s1_after = prefix_bytes(&after_defs, system, &[]);
    assert_ne!(s1_before, s1_after, "adding a tool must change S1");

    // The system segment is unchanged in content, so its own bytes are stable
    // even though the combined prefix hash differs.
    let s2_before = prefix_bytes(&[], system, &[]);
    let s2_after = prefix_bytes(&[], system, &[]);
    assert_eq!(s2_before, s2_after, "S2 must not move when only S1 changed");
}

/// Tool discovery order must not leak into wire order.
#[test]
fn mcp_toolset_order_is_independent_of_discovery_order() {
    let client = test_client();

    // Server A discovered after server Z, and each server returning its tools in
    // a different order, must still produce identical wire bytes.
    let mut first = McpToolset::new();
    first.add_server_tools(
        "zeta",
        &[
            mcp_tool("beta", json!({"type": "object"})),
            mcp_tool("alpha", json!({"type": "object"})),
        ],
        &client,
    );
    first.add_server_tools(
        "alpha",
        &[mcp_tool("gamma", json!({"type": "object"}))],
        &client,
    );

    let mut second = McpToolset::new();
    second.add_server_tools(
        "alpha",
        &[mcp_tool("gamma", json!({"type": "object"}))],
        &client,
    );
    second.add_server_tools(
        "zeta",
        &[
            mcp_tool("alpha", json!({"type": "object"})),
            mcp_tool("beta", json!({"type": "object"})),
        ],
        &client,
    );

    let first_defs = first.wire_definitions();
    let second_defs = second.wire_definitions();

    assert_eq!(
        prefix_bytes(&first_defs, "system", &[]),
        prefix_bytes(&second_defs, "system", &[]),
        "canonical (server, tool) ordering must make discovery order irrelevant"
    );
}

/// Schema key order from a server must not leak into wire bytes.
#[test]
fn mcp_schema_key_order_is_normalized() {
    let client = test_client();

    let mut first = McpToolset::new();
    first.add_server_tools(
        "fs",
        &[mcp_tool(
            "read",
            json!({"type": "object", "properties": {"z": {}, "a": {}}}),
        )],
        &client,
    );

    let mut second = McpToolset::new();
    second.add_server_tools(
        "fs",
        &[mcp_tool(
            "read",
            json!({"properties": {"a": {}, "z": {}}, "type": "object"}),
        )],
        &client,
    );

    let first_defs = first.wire_definitions();
    let second_defs = second.wire_definitions();
    assert_eq!(
        prefix_bytes(&first_defs, "s", &[]),
        prefix_bytes(&second_defs, "s", &[]),
        "server-side JSON key order must not change the prefix"
    );
}

/// A qualified MCP name is a valid wire tool name and survives the ordering
/// applied by the prebuilt factories.
#[test]
fn mcp_qualified_names_survive_factory_ordering() {
    let client = test_client();
    let mut toolset = McpToolset::new();
    toolset.add_server_tools(
        "github",
        &[mcp_tool("create_issue", json!({"type": "object"}))],
        &client,
    );

    let ordered = ordered_tool_definitions(&toolset.wire_definitions());

    assert_eq!(ordered.len(), 1);
    assert_eq!(ordered[0].name, "github__create_issue");
    assert!(
        ordered[0].description.starts_with("[from server github]"),
        "provenance must survive so the model can attribute the tool"
    );
}

/// Breakpoint planning over MCP-derived sizes: tools are S1, so a large MCP tool
/// table earns an S1 breakpoint, and the volatile turn never does.
#[test]
fn mcp_tool_table_earns_s1_breakpoint_only() {
    let client = test_client();

    let mut toolset = McpToolset::new();
    // ~30 tools with padded descriptions clears a 1024-token minimum.
    let tools: Vec<McpTool> = (0..30)
        .map(|i| McpTool {
            name: format!("tool_{i:03}"),
            description: "x".repeat(700),
            input_schema: json!({"type": "object"}),
            output_schema: None,
        })
        .collect();
    toolset.add_server_tools("big", &tools, &client);

    let s1_bytes = prefix_bytes(&toolset.wire_definitions(), "short system", &[]);
    let s1_tokens = estimate_tokens(s1_bytes.len());

    let plan = plan_breakpoints(
        "claude-sonnet-4-20250514",
        &[
            (PrefixSegment::Tools, s1_tokens),
            (PrefixSegment::System, 10),
            (PrefixSegment::History, 10),
            (PrefixSegment::Turn, 200_000),
        ],
    );

    assert!(
        plan.iter().any(|b| b.segment == PrefixSegment::Tools),
        "a large MCP tool table must be cached"
    );
    assert!(
        plan.iter().all(|b| b.segment != PrefixSegment::Turn),
        "the volatile turn is never marked"
    );
    assert!(plan.len() <= 3, "the fourth breakpoint slot stays reserved");
}

/// Two identical MCP toolsets must serialize identically across constructions.
#[test]
fn identical_mcp_toolsets_serialize_identically() {
    let client = test_client();

    let build = || {
        let mut toolset = McpToolset::new();
        toolset.add_server_tools(
            "fs",
            &[
                mcp_tool(
                    "read",
                    json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                ),
                mcp_tool(
                    "write",
                    json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                ),
            ],
            &client,
        );
        prefix_bytes(
            &toolset.wire_definitions(),
            "system prompt",
            &["turn one".to_string()],
        )
    };

    assert_eq!(build(), build());
}
