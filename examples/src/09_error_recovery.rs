//! Example 09: Error Recovery Concepts
//!
//! Demonstrates error handling concepts in Juncture graphs:
//! - Proper error propagation from node functions
//! - Using Result types for fallible operations
//! - Error recovery strategies
//!
//! Key concepts:
//! - Returning Result from node functions
//! - Error propagation with `?` operator
//! - Graceful error handling patterns

use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// Processing state
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ProcessingState {
    /// Current operation status
    status: String,
    /// Result value
    result: String,
    /// Retry count
    retries: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<ProcessingState>::new();

    // Add nodes
    graph.add_node_simple(
        "process",
        NodeFnUpdate(|state: &ProcessingState| {
            let retries = state.retries;
            async move {
                // Simulate processing that might fail
                if retries < 2 {
                    return Err(JunctureError::execution("Simulated transient error"));
                }

                Ok(ProcessingStateUpdate {
                    status: Some("completed".to_string()),
                    result: Some("Operation successful!".to_string()),
                    ..Default::default()
                })
            }
        }),
    )?;
    graph.add_node_simple(
        "recovery",
        NodeFnUpdate(|state: &ProcessingState| {
            let retries = state.retries;
            async move {
                Ok(ProcessingStateUpdate {
                    status: Some("recovering".to_string()),
                    retries: Some(retries + 1),
                    result: Some("Attempting recovery...".to_string()),
                })
            }
        }),
    )?;
    graph.add_node_simple(
        "fallback",
        NodeFnUpdate(|_state: &ProcessingState| async move {
            Ok(ProcessingStateUpdate {
                status: Some("fallback".to_string()),
                result: Some("Using fallback result".to_string()),
                ..Default::default()
            })
        }),
    )?;

    // Create a flow: process -> recovery -> process -> fallback
    graph.add_edge("process", "recovery");
    graph.add_edge("recovery", "process");
    graph.add_edge("process", "fallback");

    graph.set_entry_point("process");
    graph.set_finish_point("fallback");

    let compiled = graph.compile()?;

    let initial_state = ProcessingState {
        status: "starting".to_string(),
        result: String::new(),
        retries: 0,
    };

    // Execute with error handling
    match compiled.invoke(initial_state, &RunnableConfig::default()) {
        Ok(output) => {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "Execution completed successfully")?;
            writeln!(stdout, "Final status: {}", output.value.status)?;
            writeln!(stdout, "Final result: {}", output.value.result)?;
            writeln!(stdout, "Total retries: {}", output.value.retries)?;
        }
        Err(e) => {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "Execution failed: {e}")?;
            writeln!(stdout, "\nIn production, you would:")?;
            writeln!(stdout, "  1. Log the error details")?;
            writeln!(
                stdout,
                "  2. Implement retry logic with exponential backoff"
            )?;
            writeln!(stdout, "  3. Add circuit breakers for failing services")?;
            writeln!(stdout, "  4. Use error handler nodes for graceful recovery")?;
        }
    }

    writeln!(
        std::io::stdout(),
        "\nError recovery strategies in Juncture:"
    )?;
    writeln!(
        std::io::stdout(),
        "  - Return Result::Err from nodes to signal failure"
    )?;
    writeln!(
        std::io::stdout(),
        "  - Use conditional edges to route based on error state"
    )?;
    writeln!(
        std::io::stdout(),
        "  - Implement retry logic in node functions"
    )?;
    writeln!(
        std::io::stdout(),
        "  - Add error handler nodes for recovery processing"
    )?;

    Ok(())
}

// Rust guideline compliant 2026-05-24
