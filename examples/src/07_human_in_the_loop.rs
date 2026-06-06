//! Example 07: Human-in-the-Loop (HITL)
//!
//! Demonstrates human-in-the-loop execution with interrupts:
//! - Using `CompileConfig` to set interrupt points
//! - Checking `output.interrupts` after execution
//! - Resuming execution after human intervention
//!
//! Key concepts:
//! - `CompileConfig` with `interrupt_before`/`interrupt_after`
//! - GraphOutput.interrupts for detecting interruptions
//! - HITL workflow for human approval processes

use juncture_core::graph::CompileConfig;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;

/// Approval workflow state
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ApprovalState {
    /// Current step in the workflow
    step: String,
    /// Whether the action is approved
    approved: bool,
    /// Action description
    action: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<ApprovalState>::new();

    // Add nodes
    graph.add_node_simple(
        "propose",
        NodeFnUpdate(|_state: &ApprovalState| async move {
            Ok(ApprovalStateUpdate {
                step: Some("proposed".to_string()),
                action: Some("Deploy to production".to_string()),
                ..Default::default()
            })
        }),
    )?;
    graph.add_node_simple(
        "review",
        NodeFnUpdate(|_state: &ApprovalState| async move {
            Ok(ApprovalStateUpdate {
                step: Some("reviewed".to_string()),
                approved: Some(true),
                ..Default::default()
            })
        }),
    )?;
    graph.add_node_simple(
        "execute",
        NodeFnUpdate(|_state: &ApprovalState| async move {
            Ok(ApprovalStateUpdate {
                step: Some("executed".to_string()),
                ..Default::default()
            })
        }),
    )?;

    // Connect nodes
    graph.add_edge("propose", "review");
    graph.add_edge("review", "execute");

    graph.set_entry_point("propose");
    graph.set_finish_point("execute");

    // Configure HITL - interrupt before the review node
    let config = CompileConfig {
        interrupt_before: vec!["review".to_string()],
        interrupt_after: vec![],
    };

    let compiled = graph.compile_with_config(config)?;

    let initial_state = ApprovalState {
        step: "init".to_string(),
        approved: false,
        action: String::new(),
    };

    // Execute - will stop before review node
    let output = compiled.invoke(initial_state, &RunnableConfig::new())?;

    let mut stdout = std::io::stdout();

    // Check if execution was interrupted
    if output.interrupts.is_empty() {
        writeln!(stdout, "Execution completed without interruption")?;
        writeln!(stdout, "Final step: {}", output.value.step)?;
    } else {
        writeln!(stdout, "Execution interrupted at: {:?}", output.interrupts)?;
        writeln!(stdout, "Current step: {}", output.value.step)?;
        writeln!(stdout, "Proposed action: {}", output.value.action)?;
        writeln!(stdout, "In a real application, this is where you would:")?;
        writeln!(stdout, "  1. Show the action to a human reviewer")?;
        writeln!(stdout, "  2. Wait for approval/rejection")?;
        writeln!(stdout, "  3. Resume execution with the decision")?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
