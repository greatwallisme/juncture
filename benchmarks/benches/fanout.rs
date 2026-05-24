//! Fanout benchmark: parallel subgraph execution via Send.
//!
//! Port of `LangGraph`'s `bench/fanout_to_subgraph.py`. Measures the performance
//! of dynamic fan-out to multiple parallel subgraph instances.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use juncture_core::command::{Command, SendTarget};
use juncture_core::edge::PathMap;
use juncture_core::node::{NodeFnCommand, NodeFnUpdate};
use juncture_core::send::Send as JunctureSend;
use juncture_core::subgraph::SubgraphConfig;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::sync::Arc;

/// Parent state for the fanout benchmark
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct OverallState {
    /// List of subjects to process in parallel
    subjects: Vec<String>,
    /// Accumulated jokes from all subgraphs (append reducer)
    #[reducer(append)]
    jokes: Vec<String>,
}

/// Subgraph input state (single subject)
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JokeInput {
    /// Subject for joke generation
    subject: String,
}

/// Subgraph output state (jokes list)
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JokeOutput {
    /// Generated jokes
    jokes: Vec<String>,
}

/// Combined subgraph state (input + output)
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct JokeState {
    /// Subject for joke generation
    subject: String,
    /// Generated jokes
    #[reducer(append)]
    jokes: Vec<String>,
}

/// Edit node: modify the subject
async fn edit_node(state: JokeState) -> Result<JokeStateUpdate, JunctureError> {
    Ok(JokeStateUpdate {
        subject: Some(format!("{} - hohoho", state.subject)),
        jokes: None,
    })
}

/// Generate node: create a joke about the subject
async fn generate_node(state: JokeState) -> Result<JokeStateUpdate, JunctureError> {
    Ok(JokeStateUpdate {
        subject: None,
        jokes: Some(vec![format!("Joke about {}", state.subject)]),
    })
}

/// Bump node: append " a" to the first joke
async fn bump_node(state: JokeState) -> Result<JokeStateUpdate, JunctureError> {
    let modified_joke = format!("{} a", state.jokes[0]);
    Ok(JokeStateUpdate {
        subject: None,
        jokes: Some(vec![modified_joke]),
    })
}

/// Router for the bump loop: continue until joke ends with " a" * 10
fn bump_loop_router(state: &JokeState) -> &str {
    if state
        .jokes
        .first()
        .is_some_and(|j| j.ends_with(" aaaaaaaaaa"))
    {
        "__end__"
    } else {
        "bump"
    }
}

/// Create the joke generation subgraph
fn create_joke_subgraph() -> StateGraph<JokeState> {
    let mut graph = StateGraph::new();

    graph
        .add_node_simple("edit", NodeFnUpdate(edit_node))
        .expect("add_node_simple should succeed");
    graph
        .add_node_simple("generate", NodeFnUpdate(generate_node))
        .expect("add_node_simple should succeed");
    graph
        .add_node_simple("bump", NodeFnUpdate(bump_node))
        .expect("add_node_simple should succeed");

    graph.set_entry_point("edit");
    graph.add_edge("edit", "generate");
    graph.add_edge("generate", "bump");

    let path_map = PathMap::from(&[("bump", "bump"), ("__end__", "__end__")]);
    graph.add_conditional_edges("bump", Arc::new(bump_loop_router), path_map);
    graph.set_finish_point("generate");

    graph
}

/// Node that fans out to multiple subgraph instances via Send
async fn continue_to_jokes_node(
    state: OverallState,
) -> Result<Command<OverallState>, JunctureError> {
    let sends: Vec<SendTarget> = state
        .subjects
        .iter()
        .map(|subject| {
            JunctureSend::<JokeInput> {
                node: "generate_joke".to_string(),
                state: JokeInput {
                    subject: subject.clone(),
                },
            }
            .into()
        })
        .collect();

    Ok(Command::send(sends))
}

/// Create the fanout graph with subgraph
fn create_fanout_graph() -> StateGraph<OverallState> {
    let mut graph = StateGraph::new();

    // Create and compile the subgraph
    let subgraph = create_joke_subgraph();
    let compiled_subgraph = Arc::new(subgraph.compile().expect("subgraph compile should succeed"));

    // Add subgraph with explicit state mapping
    graph
        .add_subgraph_with_config(
            "generate_joke",
            compiled_subgraph,
            // input_map: extract JokeState from OverallState's subject
            |parent: &OverallState| JokeState {
                subject: parent.subjects.first().cloned().unwrap_or_default(),
                jokes: vec![],
            },
            // output_map: map JokeState output to OverallState update
            |sub_output: &JokeState| OverallStateUpdate {
                subjects: None,
                jokes: Some(sub_output.jokes.clone()),
            },
            SubgraphConfig::default(),
        )
        .expect("add_subgraph_with_config should succeed");

    // Add the fanout node
    graph
        .add_node_simple("continue_to_jokes", NodeFnCommand(continue_to_jokes_node))
        .expect("add_node_simple should succeed");

    // Set entry point and add edges
    graph.set_entry_point("continue_to_jokes");
    graph.add_edge("continue_to_jokes", "generate_joke");
    graph.add_edge("generate_joke", "__end__");

    graph
}

/// `RunnableConfig` with high recursion limit for deep graphs
fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

/// Create input state with N subjects
fn create_fanout_input(num_subjects: usize) -> OverallState {
    let subjects: Vec<String> = (0..num_subjects).map(|i| format!("subject_{i}")).collect();
    OverallState {
        subjects,
        jokes: vec![],
    }
}

fn benchmark_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout");

    let config = bench_config();

    for &num_subjects in &[10_usize, 100] {
        let graph = create_fanout_graph();
        let compiled = graph.compile().expect("compile should succeed");
        let input = create_fanout_input(num_subjects);

        group.bench_with_input(
            BenchmarkId::new("invoke", num_subjects),
            &num_subjects,
            |b, _| {
                b.iter(|| {
                    let _ = compiled.invoke(input.clone(), &config);
                });
            },
        );
    }

    group.finish();
}

fn benchmark_fanout_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout_checkpoint");

    let config = bench_config();

    for &num_subjects in &[10_usize, 100] {
        let graph = create_fanout_graph();
        let checkpointer = juncture_checkpoint::MemorySaver::new();
        let compiled = graph
            .compile_with_checkpointer(Some(Arc::new(checkpointer)))
            .expect("compile should succeed");
        let input = create_fanout_input(num_subjects);

        group.bench_with_input(
            BenchmarkId::new("invoke", num_subjects),
            &num_subjects,
            |b, _| {
                b.iter(|| {
                    let _ = compiled.invoke(input.clone(), &config);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_fanout, benchmark_fanout_checkpoint);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
