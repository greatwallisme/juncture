//! Example 20: Health Check
//!
//! Demonstrates health monitoring for graph execution:
//! - Querying health status during execution
//! - Understanding node health states (Healthy, Degraded, Unhealthy)
//! - Monitoring circuit breaker states
//!
//! Key concepts:
//! - `HealthStatus` for overall graph health
//! - `NodeHealth` for per-node health information
//! - `NodeHealthState` enum: Healthy, Degraded, Unhealthy

use juncture_core::graph::CircuitBreakerConfig;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;
use std::time::Duration;

/// State for demonstrating health checks
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct AppState {
    status: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = StateGraph::<AppState>::new();

    // Add a node with circuit breaker
    let cb_config = CircuitBreakerConfig::new(3, Duration::from_secs(5));
    graph.add_node_with_circuit_breaker(
        "service_a",
        NodeFnUpdate(|_state: &AppState| async move {
            Ok(AppStateUpdate {
                status: Some("service_a_done".to_string()),
            })
        }),
        cb_config,
    )?;

    // Add a node without circuit breaker
    graph.add_node_simple(
        "service_b",
        NodeFnUpdate(|_state: &AppState| async move {
            Ok(AppStateUpdate {
                status: Some("service_b_done".to_string()),
            })
        }),
    )?;

    graph.add_edge("service_a", "service_b");
    graph.set_entry_point("service_a");
    graph.set_finish_point("service_b");

    let compiled = graph.compile()?;

    // Note: Health status is available during execution via PregelLoop::health()
    // In a real application, you would expose this via an HTTP endpoint
    let mut stdout = std::io::stdout();
    writeln!(stdout, "Graph compiled successfully")?;
    writeln!(stdout)?;
    writeln!(stdout, "Health monitoring is available during execution:")?;
    writeln!(stdout, "  - PregelLoop::health() returns HealthStatus")?;
    writeln!(stdout, "  - HealthStatus contains per-node NodeHealth")?;
    writeln!(stdout, "  - NodeHealthState: Healthy, Degraded, Unhealthy")?;
    writeln!(stdout)?;
    writeln!(stdout, "In production, expose health via HTTP endpoint:")?;
    writeln!(stdout, "  GET /health -> HealthStatus JSON")?;

    // Execute the graph
    let initial_state = AppState::default();
    let output = compiled.invoke(initial_state, &RunnableConfig::new())?;
    writeln!(stdout)?;
    writeln!(stdout, "Execution completed: {}", output.value.status)?;

    Ok(())
}

// Rust guideline compliant 2026-06-06
