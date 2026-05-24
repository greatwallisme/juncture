//! Example 03: Conditional Routing
//!
//! Demonstrates conditional execution paths based on state:
//! - A router function that determines the next node based on state
//! - Conditional edges that route to different nodes
//! - `PathMap` to define routing options
//!
//! Key concepts:
//! - Router functions that inspect state and return target node names
//! - `add_conditional_edges` for dynamic routing
//! - `PathMap` to define routing mappings

use juncture_core::edge::{PathMap, Router};
use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;
use std::sync::Arc;

/// Score state for conditional routing
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ScoreState {
    /// Numerical score
    score: u32,
    /// Grade assigned based on score
    grade: String,
}

/// Router function - determines path based on score
const fn grade_router(state: &ScoreState) -> &str {
    if state.score >= 90 {
        "excellent"
    } else if state.score >= 70 {
        "good"
    } else {
        "retry"
    }
}

/// Grade node - assigns a grade based on score
async fn grade_node(state: ScoreState) -> Result<ScoreStateUpdate, JunctureError> {
    let grade = if state.score >= 90 {
        "A".to_string()
    } else if state.score >= 70 {
        "B".to_string()
    } else {
        "C".to_string()
    };

    Ok(ScoreStateUpdate {
        grade: Some(grade),
        ..Default::default()
    })
}

/// Excellent handler - processes high scores
async fn excellent_node(state: ScoreState) -> Result<ScoreStateUpdate, JunctureError> {
    Ok(ScoreStateUpdate {
        grade: Some(format!("{} (excellent!)", state.grade)),
        ..Default::default()
    })
}

/// Good handler - processes medium scores
async fn good_node(state: ScoreState) -> Result<ScoreStateUpdate, JunctureError> {
    Ok(ScoreStateUpdate {
        grade: Some(format!("{} (good)", state.grade)),
        ..Default::default()
    })
}

/// Retry handler - suggests improvement for low scores
async fn retry_node(state: ScoreState) -> Result<ScoreStateUpdate, JunctureError> {
    Ok(ScoreStateUpdate {
        grade: Some(format!("{} (needs improvement)", state.grade)),
        ..Default::default()
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<ScoreState>::new();

    // Add nodes for each path
    graph.add_node_simple("grade", NodeFnUpdate(grade_node))?;
    graph.add_node_simple("excellent", NodeFnUpdate(excellent_node))?;
    graph.add_node_simple("good", NodeFnUpdate(good_node))?;
    graph.add_node_simple("retry", NodeFnUpdate(retry_node))?;

    // Add conditional edges from "grade" node
    // The router function will determine which path to take
    graph.add_conditional_edges(
        "grade",
        Arc::new(grade_router) as Arc<dyn Router<ScoreState>>,
        PathMap::from(&[
            ("excellent", "excellent"),
            ("good", "good"),
            ("retry", "retry"),
        ]),
    );

    graph.set_entry_point("grade");

    let compiled = graph.compile()?;

    // Test with different scores
    let mut stdout = std::io::stdout();

    for score in [95, 75, 50] {
        let initial_state = ScoreState {
            score,
            grade: String::new(),
        };

        let output = compiled.invoke(initial_state, &RunnableConfig::default())?;

        writeln!(stdout, "Score {}: Grade = {}", score, output.value.grade)?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
