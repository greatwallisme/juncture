//! Example 19: Resource Limits
//!
//! Demonstrates resource limits for graph execution:
//! - Configuring maximum state size
//! - State size enforcement with automatic rollback
//! - Error handling when limits are exceeded
//!
//! Key concepts:
//! - `ResourceLimits` for configuring state size limits
//! - `with_resource_limits` builder method on `RunnableConfig`
//! - Automatic state rollback when size limit is exceeded

use juncture_core::config::ResourceLimits;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// State with a vector that can grow large
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct DataState {
    /// Data buffer that grows with each step
    #[reducer(append)]
    data: Vec<String>,
    /// Current step
    step: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<DataState>::new();

    // Node that adds data to the state
    graph.add_node_simple(
        "add_data",
        NodeFnUpdate(|state: &DataState| {
            let step = state.step;
            async move {
                // Add a large chunk of data each step
                let new_data: Vec<String> = (0..1000)
                    .map(|i| format!("item_{step}_{i}"))
                    .collect();
                Ok(DataStateUpdate {
                    data: Some(new_data),
                    step: Some(step + 1),
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "finish",
        NodeFnUpdate(|_state: &DataState| async move {
            Ok(DataStateUpdate::default())
        }),
    )?;

    graph.add_edge("add_data", "add_data");
    graph.add_edge("add_data", "finish");
    graph.set_entry_point("add_data");
    graph.set_finish_point("finish");

    let compiled = graph.compile()?;

    // Configure resource limits: max 1MB state size
    let limits = ResourceLimits::new().with_max_state_size_bytes(1024 * 1024);
    let config = RunnableConfig::new()
        .with_resource_limits(limits)
        .with_recursion_limit(100);

    let initial_state = DataState::default();

    match compiled.invoke(initial_state, &config) {
        Ok(output) => {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "Steps executed: {}", output.metadata.steps)?;
            writeln!(stdout, "Data items: {}", output.value.data.len())?;
        }
        Err(e) => {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "Execution stopped: {e}")?;
            writeln!(stdout, "This is expected when state size exceeds the limit")?;
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-06-06
