//! Example 18: Fallback Node
//!
//! Demonstrates graceful degradation using fallback nodes:
//! - Configuring a fallback node for a primary node
//! - Automatic routing to fallback when primary fails
//! - Priority order: fallback > error handler > cancel
//!
//! Key concepts:
//! - `add_node_with_fallback` builder method
//! - Fallback node receives the same state as the failed node
//! - Error propagation when no fallback is configured

use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// State for demonstrating fallback nodes
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ServiceState {
    /// Result from the service
    result: String,
    /// Whether fallback was used
    used_fallback: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();

    writeln!(stdout, "=== Fallback Node Demo ===")?;
    writeln!(stdout)?;
    writeln!(stdout, "Fallback nodes provide graceful degradation:")?;
    writeln!(stdout, "  - When primary node fails, fallback is executed")?;
    writeln!(stdout, "  - Fallback receives the same state as the failed node")?;
    writeln!(stdout, "  - Priority: fallback > error handler > cancel")?;
    writeln!(stdout)?;

    let mut graph = StateGraph::<ServiceState>::new();

    // Primary node that may fail
    graph.add_node_with_fallback(
        "primary_service",
        NodeFnUpdate(|_state: &ServiceState| async move {
            // Simulate service failure
            Err(juncture_core::JunctureError::execution(
                "Primary service unavailable",
            ))
        }),
        "fallback_service",
    )?;

    // Fallback node that provides a default result
    graph.add_node_simple(
        "fallback_service",
        NodeFnUpdate(|_state: &ServiceState| async move {
            Ok(ServiceStateUpdate {
                result: Some("fallback_result".to_string()),
                used_fallback: Some(true),
            })
        }),
    )?;

    // Add edge to make fallback_service reachable
    graph.add_edge("primary_service", "fallback_service");

    // Set entry and finish points
    graph.set_entry_point("primary_service");
    graph.set_finish_point("fallback_service");

    let compiled = graph.compile()?;

    let initial_state = ServiceState {
        result: String::new(),
        used_fallback: false,
    };

    writeln!(stdout, "Executing graph with fallback protection...")?;
    writeln!(stdout)?;

    let output = compiled.invoke(initial_state, &RunnableConfig::new())?;

    writeln!(stdout, "Result: {}", output.value.result)?;
    writeln!(stdout, "Used fallback: {}", output.value.used_fallback)?;
    writeln!(stdout, "Steps executed: {}", output.metadata.steps)?;

    Ok(())
}

// Rust guideline compliant 2026-06-06
