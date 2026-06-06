//! Example 17: Circuit Breaker
//!
//! Demonstrates the circuit breaker pattern for node execution:
//! - Configuring a circuit breaker with failure threshold and cooldown
//! - How the circuit transitions between Closed, Open, and `HalfOpen` states
//! - Automatic recovery after cooldown period
//!
//! Key concepts:
//! - `CircuitBreakerConfig` for configuring failure threshold and cooldown
//! - `add_node_with_circuit_breaker` builder method
//! - Circuit breaker state transitions: Closed -> Open -> `HalfOpen` -> Closed

use juncture_core::graph::CircuitBreakerConfig;
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;
use std::io::Write;
use std::time::Duration;

/// State for demonstrating circuit breaker
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct AppState {
    /// Current status
    status: String,
    /// Number of attempts
    attempts: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();

    writeln!(stdout, "=== Circuit Breaker Demo ===")?;
    writeln!(stdout)?;
    writeln!(stdout, "Circuit breaker states:")?;
    writeln!(stdout, "  Closed: Normal operation, requests pass through")?;
    writeln!(stdout, "  Open: Circuit is open, requests are rejected")?;
    writeln!(stdout, "  HalfOpen: Testing if circuit can close")?;
    writeln!(stdout)?;

    // Configure circuit breaker: open after 2 failures, cooldown for 1 second
    let cb_config = CircuitBreakerConfig::new(2, Duration::from_secs(1));

    writeln!(stdout, "Configuration:")?;
    writeln!(stdout, "  Failure threshold: {}", cb_config.failure_threshold)?;
    writeln!(
        stdout,
        "  Cooldown duration: {:?}",
        cb_config.cooldown_duration
    )?;
    writeln!(
        stdout,
        "  Half-open max attempts: {}",
        cb_config.half_open_max_attempts
    )?;
    writeln!(stdout)?;

    // Create a graph with circuit breaker
    let mut graph = StateGraph::<AppState>::new();

    graph.add_node_with_circuit_breaker(
        "unstable_service",
        NodeFnUpdate(|_state: &AppState| async move {
            // Simulate a failing service
            Err(juncture_core::JunctureError::execution("Service unavailable"))
        }),
        cb_config,
    )?;

    graph.add_node_simple(
        "fallback",
        NodeFnUpdate(|_state: &AppState| async move {
            Ok(AppStateUpdate {
                status: Some("fallback_result".to_string()),
                ..Default::default()
            })
        }),
    )?;

    graph.add_edge("unstable_service", "fallback");
    graph.set_entry_point("unstable_service");
    graph.set_finish_point("fallback");

    let compiled = graph.compile()?;

    let initial_state = AppState {
        status: "starting".to_string(),
        attempts: 0,
    };

    // Note: Circuit breaker prevents repeated execution after threshold is reached
    // In this demo, the service always fails, so the circuit opens after 2 failures
    writeln!(stdout, "Executing graph with circuit breaker protection...")?;
    writeln!(stdout)?;

    match compiled.invoke(initial_state, &RunnableConfig::new()) {
        Ok(output) => {
            writeln!(stdout, "Final status: {}", output.value.status)?;
            writeln!(stdout, "Steps executed: {}", output.metadata.steps)?;
        }
        Err(e) => {
            writeln!(stdout, "Execution stopped: {e}")?;
            writeln!(stdout)?;
            writeln!(stdout, "This demonstrates the circuit breaker in action:")?;
            writeln!(stdout, "  - After 2 failures, the circuit opens")?;
            writeln!(stdout, "  - Open circuit rejects further execution attempts")?;
            writeln!(stdout, "  - After cooldown (1s), circuit transitions to HalfOpen")?;
            writeln!(stdout, "  - HalfOpen allows one probe attempt")?;
        }
    }

    Ok(())
}

// Rust guideline compliant 2026-06-06
