//! Example 06: Streaming Execution
//!
//! Demonstrates streaming execution of a graph:
//! - Using `stream()` instead of `invoke()`
//! - Processing `StreamEvent` values as they arrive
//! - Different stream modes (Values, Updates, Debug)
//!
//! Key concepts:
//! - Async execution with tokio
//! - `StreamMode` for controlling what data is streamed
//! - `StreamEvent` variants (Values, End, etc.)
//! - Using `futures::StreamExt` to consume the stream

use juncture_core::node::NodeFnUpdate;
use juncture_core::stream::{StreamEvent, StreamMode};
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// Streaming state
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct StreamingState {
    /// Current step number
    step: u32,
    /// Accumulated value
    value: String,
}

/// Step 1 node
async fn step1_node(_state: StreamingState) -> Result<StreamingStateUpdate, JunctureError> {
    Ok(StreamingStateUpdate {
        step: Some(1),
        value: Some("Step 1 completed".to_string()),
    })
}

/// Step 2 node
async fn step2_node(_state: StreamingState) -> Result<StreamingStateUpdate, JunctureError> {
    Ok(StreamingStateUpdate {
        step: Some(2),
        value: Some("Step 2 completed".to_string()),
    })
}

/// Step 3 node
async fn step3_node(_state: StreamingState) -> Result<StreamingStateUpdate, JunctureError> {
    Ok(StreamingStateUpdate {
        step: Some(3),
        value: Some("Step 3 completed".to_string()),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<StreamingState>::new();

    // Add three sequential nodes
    graph.add_node_simple("step1", NodeFnUpdate(step1_node))?;
    graph.add_node_simple("step2", NodeFnUpdate(step2_node))?;
    graph.add_node_simple("step3", NodeFnUpdate(step3_node))?;

    // Connect them in sequence
    graph.add_edge("step1", "step2");
    graph.add_edge("step2", "step3");

    graph.set_entry_point("step1");
    graph.set_finish_point("step3");

    let compiled = graph.compile()?;

    // Initial state
    let initial_state = StreamingState {
        step: 0,
        value: String::new(),
    };

    // Stream execution
    let handle = compiled
        .stream(
            initial_state,
            &RunnableConfig::default(),
            StreamMode::Values,
        )
        .await?;

    // Process stream events
    let mut stream = handle.stream;
    let mut stdout = std::io::stdout();

    while let Some(result) = futures::StreamExt::next(&mut stream).await {
        match result {
            Ok(event) => match event {
                StreamEvent::Values { state, step } => {
                    writeln!(stdout, "Step {}: value={}", step, state.value)?;
                }
                StreamEvent::End { output } => {
                    writeln!(stdout, "Execution complete")?;
                    writeln!(stdout, "Final step: {}", output.step)?;
                }
                _ => {}
            },
            Err(e) => {
                writeln!(stdout, "Error: {e}")?;
                break;
            }
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
