//! Example 08: Checkpoint and Resume
//!
//! Demonstrates state persistence and resumption:
//! - Using `MemorySaver` for checkpointing
//! - Resuming execution from a checkpoint
//! - Maintaining state across multiple executions
//!
//! Key concepts:
//! - `MemorySaver` for in-memory checkpoint storage
//! - `compile_with_checkpointer()` for persistence
//! - Using `thread_id` to maintain execution continuity

use juncture_checkpoint::MemorySaver;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;
use std::sync::Arc;

/// Workflow state with counter
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CheckpointState {
    /// Execution counter
    count: u32,
    /// Last executed step
    last_step: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<CheckpointState>::new();

    // Add nodes
    graph.add_node_simple(
        "step1",
        NodeFnUpdate(|state: &CheckpointState| {
            let count = state.count;
            async move {
                Ok(CheckpointStateUpdate {
                    count: Some(count + 1),
                    last_step: Some("step1".to_string()),
                })
            }
        }),
    )?;
    graph.add_node_simple(
        "step2",
        NodeFnUpdate(|state: &CheckpointState| {
            let count = state.count;
            async move {
                Ok(CheckpointStateUpdate {
                    count: Some(count + 1),
                    last_step: Some("step2".to_string()),
                })
            }
        }),
    )?;

    // Create a cycle to demonstrate checkpointing
    graph.add_edge("step1", "step2");
    graph.add_edge("step2", "step1");

    graph.set_entry_point("step1");

    // Create a checkpointer
    let checkpointer = MemorySaver::new();
    let compiled = graph.compile_with_checkpointer(Some(Arc::new(checkpointer)))?;

    let mut stdout = std::io::stdout();

    // First execution - run for a few steps
    let config = RunnableConfig::default().with_run_id("example-run-1");

    let initial_state = CheckpointState {
        count: 0,
        last_step: String::new(),
    };

    // Run a few iterations manually
    let mut state = initial_state;
    for i in 0..3 {
        let output = compiled.invoke(state, &config)?;
        state = output.value;
        writeln!(
            stdout,
            "Iteration {}: count={}, last_step={}",
            i, state.count, state.last_step
        )?;
    }

    writeln!(stdout, "\nResuming from checkpoint...")?;

    // Second execution - resume with same thread_id
    // In a real scenario, this would be a separate process/session
    let resume_config = RunnableConfig::default().with_run_id("example-run-1");

    // Start with a fresh state - the checkpointer will restore the actual state
    let fresh_state = CheckpointState {
        count: 0,
        last_step: String::new(),
    };

    let output = compiled.invoke(fresh_state, &resume_config)?;

    writeln!(
        stdout,
        "After resume: count={}, last_step={}",
        output.value.count, output.value.last_step
    )?;
    writeln!(stdout, "Checkpointing allows resuming from saved state")?;

    Ok(())
}

// Rust guideline compliant 2026-05-24
