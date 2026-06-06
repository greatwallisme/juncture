//! Example 22: Graph Visualization
//!
//! Demonstrates graph visualization capabilities:
//! - Exporting graph as Mermaid diagram
//! - Exporting graph as DOT format
//! - Generating self-contained HTML visualization
//! - Terminal-friendly display
//!
//! Key concepts:
//! - `to_mermaid()` for Mermaid diagram syntax
//! - `to_dot()` for Graphviz DOT format
//! - `to_html()` for interactive HTML visualization
//! - `display()` for terminal output

use juncture_core::node::NodeFnUpdate;
use juncture_core::StateGraph;
use juncture_derive::State;
use std::io::Write;

/// Simple state for visualization demo
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct DemoState {
    step: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<DemoState>::new();

    graph.add_node_simple(
        "start",
        NodeFnUpdate(|_state: &DemoState| async move {
            Ok(DemoStateUpdate {
                step: Some("started".to_string()),
            })
        }),
    )?;

    graph.add_node_simple(
        "process",
        NodeFnUpdate(|_state: &DemoState| async move {
            Ok(DemoStateUpdate {
                step: Some("processed".to_string()),
            })
        }),
    )?;

    graph.add_node_simple(
        "finish",
        NodeFnUpdate(|_state: &DemoState| async move {
            Ok(DemoStateUpdate {
                step: Some("finished".to_string()),
            })
        }),
    )?;

    graph.add_edge("start", "process");
    graph.add_edge("process", "finish");
    graph.set_entry_point("start");
    graph.set_finish_point("finish");

    let compiled = graph.compile()?;

    let mut stdout = std::io::stdout();

    // Terminal display
    writeln!(stdout, "=== Terminal Display ===")?;
    writeln!(stdout, "{}", compiled.display())?;
    writeln!(stdout)?;

    // Mermaid format
    writeln!(stdout, "=== Mermaid Diagram ===")?;
    writeln!(stdout, "```mermaid")?;
    writeln!(stdout, "{}", compiled.to_mermaid())?;
    writeln!(stdout, "```")?;
    writeln!(stdout)?;

    // DOT format
    writeln!(stdout, "=== Graphviz DOT ===")?;
    writeln!(stdout, "{}", compiled.to_dot())?;
    writeln!(stdout)?;

    // HTML visualization
    writeln!(stdout, "=== HTML Visualization ===")?;
    let html = compiled.to_html();
    let html_path = "graph_visualization.html";
    std::fs::write(html_path, &html)?;
    writeln!(stdout, "HTML visualization saved to: {html_path}")?;
    writeln!(stdout, "Open in browser to see interactive graph")?;

    // JSON format
    writeln!(stdout)?;
    writeln!(stdout, "=== JSON Structure ===")?;
    let json = compiled.to_json();
    writeln!(stdout, "{}", serde_json::to_string_pretty(&json)?)?;

    Ok(())
}

// Rust guideline compliant 2026-06-06
