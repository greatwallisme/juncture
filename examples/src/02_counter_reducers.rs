//! Example 02: Counter with Different Reducers
//!
//! Demonstrates the different reducer semantics available in Juncture:
//! - Replace reducer (default) - last writer wins for scalar values
//! - Append reducer - extends collections
//! - Last write wins reducer - explicit last-write-wins semantics
//!
//! Key concepts:
//! - Using `#[reducer(...)]` attribute to customize merge behavior
//! - Understanding how different reducers handle state updates
//! - Multiple nodes updating the same state field

use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::collections::HashMap;
use std::io::Write;

/// Counter state demonstrating different reducer types
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct CounterState {
    /// Default replace reducer - last writer wins
    value: u32,

    /// Append reducer - extends the vector
    #[reducer(append)]
    items: Vec<String>,

    /// Last write wins reducer - explicit semantics
    #[reducer(last_write_wins)]
    status: String,
}

/// Custom merge function for `HashMap` (merge by extending)
#[expect(dead_code, reason = "example showing custom reducer syntax")]
fn merge_scores(
    old: Option<HashMap<String, f32>>,
    new: Option<HashMap<String, f32>>,
) -> HashMap<String, f32> {
    let mut result = old.unwrap_or_default();
    if let Some(new_data) = new {
        result.extend(new_data);
    }
    result
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<CounterState>::new();

    // Add nodes with different update patterns
    graph.add_node_simple(
        "increment",
        NodeFnUpdate(|state: &CounterState| {
            let value = state.value;
            async move {
                Ok(CounterStateUpdate {
                    value: Some(value + 1),
                    items: Some(vec![format!("step_{}", value + 1)]),
                    ..Default::default()
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "set_status",
        NodeFnUpdate(|state: &CounterState| {
            let value = state.value;
            async move {
                Ok(CounterStateUpdate {
                    status: Some(format!("processed_{value}")),
                    ..Default::default()
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "collect",
        NodeFnUpdate(|_state: &CounterState| async move { Ok(CounterStateUpdate::default()) }),
    )?;

    // Create a linear flow
    graph.add_edge("increment", "set_status");
    graph.add_edge("set_status", "increment");
    graph.add_edge("increment", "collect");

    graph.set_entry_point("increment");
    graph.set_finish_point("collect");

    let compiled = graph.compile()?;

    let initial_state = CounterState {
        value: 0,
        items: vec![],
        status: "initialized".to_string(),
    };

    let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

    // Display results
    let mut stdout = std::io::stdout();
    writeln!(stdout, "Final value: {}", output.value.value)?;
    writeln!(stdout, "Items collected: {:?}", output.value.items)?;
    writeln!(stdout, "Final status: {}", output.value.status)?;
    writeln!(stdout, "Steps executed: {}", output.metadata.steps)?;

    Ok(())
}

// Rust guideline compliant 2026-05-24
