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
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;
use std::sync::Arc;

/// Score state for conditional routing
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<ScoreState>::new();

    // Add nodes for each path
    graph.add_node_simple(
        "grade",
        NodeFnUpdate(|state: &ScoreState| {
            let score = state.score;
            async move {
                let grade = if score >= 90 {
                    "A".to_string()
                } else if score >= 70 {
                    "B".to_string()
                } else {
                    "C".to_string()
                };

                Ok(ScoreStateUpdate {
                    grade: Some(grade),
                    ..Default::default()
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "excellent",
        NodeFnUpdate(|state: &ScoreState| {
            let grade = state.grade.clone();
            async move {
                Ok(ScoreStateUpdate {
                    grade: Some(format!("{grade} (excellent!)")),
                    ..Default::default()
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "good",
        NodeFnUpdate(|state: &ScoreState| {
            let grade = state.grade.clone();
            async move {
                Ok(ScoreStateUpdate {
                    grade: Some(format!("{grade} (good)")),
                    ..Default::default()
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "retry",
        NodeFnUpdate(|state: &ScoreState| {
            let grade = state.grade.clone();
            async move {
                Ok(ScoreStateUpdate {
                    grade: Some(format!("{grade} (needs improvement)")),
                    ..Default::default()
                })
            }
        }),
    )?;

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

        let output = compiled.invoke(initial_state, &RunnableConfig::new())?;

        writeln!(stdout, "Score {}: Grade = {}", score, output.value.grade)?;
    }

    Ok(())
}

// Rust guideline compliant 2026-05-24
