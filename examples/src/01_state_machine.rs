//! Example 01: Basic State Machine
//!
//! Demonstrates the simplest possible Juncture graph:
//! - A state struct with two fields
//! - Two nodes that sequentially update the state
//! - Linear execution flow: START -> greet -> finish -> END
//!
//! Key concepts:
//! - Using `#[derive(State)]` to generate state/update pairs
//! - Building a graph with `StateGraph`
//! - Compiling and invoking a graph
//! - Accessing final state from `GraphOutput`

use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// Workflow state tracking the current step and a counter
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    /// Current step description
    step: String,
    /// Number of steps executed
    count: u32,
}

/// Greet node - sets step to "greeted" and increments count
async fn greet_node(state: WorkflowState) -> Result<WorkflowStateUpdate, JunctureError> {
    Ok(WorkflowStateUpdate {
        step: Some("greeted".to_string()),
        count: Some(state.count + 1),
    })
}

/// Finish node - sets step to "done" and increments count
async fn finish_node(state: WorkflowState) -> Result<WorkflowStateUpdate, JunctureError> {
    Ok(WorkflowStateUpdate {
        step: Some("done".to_string()),
        count: Some(state.count + 1),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new graph builder
    let mut graph = StateGraph::<WorkflowState>::new();

    // Add nodes to the graph
    graph.add_node_simple("greet", NodeFnUpdate(greet_node))?;
    graph.add_node_simple("finish", NodeFnUpdate(finish_node))?;

    // Define the execution flow: greet -> finish
    graph.add_edge("greet", "finish");

    // Set the entry point (where execution starts)
    graph.set_entry_point("greet");

    // Set the finish point (where execution ends)
    graph.set_finish_point("finish");

    // Compile the graph into an executable form
    let compiled = graph.compile()?;

    // Create initial state
    let initial_state = WorkflowState {
        step: "initialized".to_string(),
        count: 0,
    };

    // Execute the graph (blocking call - creates its own tokio runtime)
    let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

    // Display the final state
    let mut stdout = std::io::stdout();
    writeln!(
        stdout,
        "Final state: step={}, count={}",
        output.value.step, output.value.count
    )?;
    writeln!(stdout, "Steps executed: {}", output.metadata.steps)?;

    Ok(())
}

// Rust guideline compliant 2026-05-24
