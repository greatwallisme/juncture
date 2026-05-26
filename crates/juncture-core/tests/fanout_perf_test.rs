//! Fanout performance reproduction tests.
//!
//! Test 1: Simple Send+worker (no subgraph) -- baseline
//! Test 2: Send+SubgraphNode -- the pattern that causes >5min execution

use std::time::Instant;
use juncture_core::{
    Command, RunnableConfig, StateGraph,
    node::NodeFnCommand,
    command::SendTarget,
    subgraph::{SubgraphConfig, SubgraphPersistence},
};
use juncture_derive::State;
use serde_json::json;
use std::sync::Arc;

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct FanoutState {
    #[reducer(append)]
    results: Vec<String>,
    subjects: Vec<String>,
}

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ChildState {
    #[reducer(append)]
    jokes: Vec<String>,
    subject: String,
}

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ChildInput {
    subject: String,
}

async fn run_fanout_test(num_subjects: usize, time_limit_secs: u64) {
    let mut graph = StateGraph::new();

    graph
        .add_node(
            "router",
            NodeFnCommand(move |state: &FanoutState| {
                let subjects = state.subjects.clone();
                Box::pin(async move {
                    let targets: Vec<SendTarget> = subjects
                        .iter()
                        .map(|s| SendTarget {
                            node: "worker".to_string(),
                            state: json!({
                                "results": [],
                                "subjects": [s],
                            }),
                            timeout: None,
                        })
                        .collect();
                    Ok(Command::update_and_send(
                        FanoutStateUpdate { results: None, subjects: None },
                        targets,
                    ))
                })
            }),
            false,
            None,
            None,
            vec![],
            vec![],
        )
        .expect("add_node should succeed");

    graph
        .add_node_simple(
            "worker",
            juncture_core::node::NodeFnUpdate(|state: &FanoutState| {
                let subject = state.subjects.first().cloned().unwrap_or_default();
                async move {
                    Ok(FanoutStateUpdate {
                        results: Some(vec![format!("processed_{subject}")]),
                        subjects: None,
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph.set_entry_point("router");
    graph.add_edge("router", "worker");
    graph.set_finish_point("worker");

    let compiled = graph.compile().expect("compile should succeed");
    let input = FanoutState {
        results: vec![],
        subjects: (0..num_subjects).map(|i| format!("subject_{i}")).collect(),
    };
    let config = RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    };

    let start = Instant::now();
    let result: juncture_core::GraphOutput<FanoutState, FanoutState> =
        compiled.invoke_async(input, &config).await.expect("fanout should succeed");
    let elapsed = start.elapsed();

    assert!(
        result.value.results.len() >= num_subjects,
        "expected at least {num_subjects} results, got {}",
        result.value.results.len(),
    );
    // Verify each Send target received its own per-target state.
    // Without state_json deserialization, all workers would process subjects[0]="subject_0"
    // producing only "processed_subject_0" entries.
    for i in 0..num_subjects {
        let expected = format!("processed_subject_{i}");
        assert!(
            result.value.results.contains(&expected),
            "missing result '{expected}' -- Send target {i} did not receive independent state. \
             Got results: {:?}",
            result.value.results,
        );
    }
    assert!(
        elapsed.as_secs() < time_limit_secs,
        "Fanout with {num_subjects} subjects took {elapsed:?} -- exceeds {time_limit_secs}s limit",
    );
}

/// Build a child subgraph that takes `ChildInput`, runs one node, returns `ChildState`
fn build_child_subgraph() -> Arc<juncture_core::CompiledGraph<ChildState>> {
    let mut child = StateGraph::new();
    child
        .add_node_simple(
            "generate",
            juncture_core::node::NodeFnUpdate(|state: &ChildState| {
                let subject = state.subject.clone();
                async move {
                    Ok(ChildStateUpdate {
                        jokes: Some(vec![format!("joke_about_{subject}")]),
                        subject: None,
                    })
                }
            }),
        )
        .expect("add_node should succeed");
    child.set_entry_point("generate");
    child.set_finish_point("generate");

    Arc::new(child.compile().expect("child compile should succeed"))
}

async fn run_fanout_subgraph_test(num_subjects: usize, time_limit_secs: u64) {
    let child_graph = build_child_subgraph();

    let mut graph = StateGraph::new();

    graph
        .add_node(
            "router",
            NodeFnCommand(move |state: &FanoutState| {
                let subjects = state.subjects.clone();
                Box::pin(async move {
                    let targets: Vec<SendTarget> = subjects
                        .iter()
                        .map(|s| SendTarget {
                            node: "subgraph".to_string(),
                            state: json!({
                                "results": [],
                                "subjects": [s],
                            }),
                            timeout: None,
                        })
                        .collect();
                    Ok(Command::update_and_send(
                        FanoutStateUpdate { results: None, subjects: None },
                        targets,
                    ))
                })
            }),
            false,
            None,
            None,
            vec![],
            vec![],
        )
        .expect("add_node should succeed");

    graph
        .add_subgraph_with_config(
            "subgraph",
            child_graph,
            |state: &FanoutState| {
                ChildState {
                    jokes: vec![],
                    subject: state.subjects.first().cloned().unwrap_or_default(),
                }
            },
            |child: &ChildState| {
                FanoutStateUpdate {
                    results: Some(child.jokes.clone()),
                    subjects: None,
                }
            },
            SubgraphConfig {
                persistence: SubgraphPersistence::Stateless,
            },
        )
        .expect("add_subgraph_with_config should succeed");

    graph.set_entry_point("router");
    graph.add_edge("router", "subgraph");
    graph.set_finish_point("subgraph");

    let compiled = graph.compile().expect("compile should succeed");
    let input = FanoutState {
        results: vec![],
        subjects: (0..num_subjects).map(|i| format!("subject_{i}")).collect(),
    };
    let config = RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    };

    let start = Instant::now();
    let result: juncture_core::GraphOutput<FanoutState, FanoutState> =
        compiled.invoke_async(input, &config).await.expect("fanout subgraph should succeed");
    let elapsed = start.elapsed();

    assert!(
        result.value.results.len() >= num_subjects,
        "expected at least {num_subjects} results, got {}",
        result.value.results.len(),
    );
    // Verify each Send target received its own state via the subgraph.
    // The subgraph reads state.subject and produces "joke_about_{subject}".
    for i in 0..num_subjects {
        let expected = format!("joke_about_subject_{i}");
        assert!(
            result.value.results.contains(&expected),
            "missing subgraph result '{expected}' -- Send target {i} did not receive independent state. \
             Got results: {:?}",
            result.value.results,
        );
    }
    assert!(
        elapsed.as_secs() < time_limit_secs,
        "Fanout subgraph with {num_subjects} subjects took {elapsed:?} -- exceeds {time_limit_secs}s limit",
    );
}

#[tokio::test]
async fn test_fanout_simple_3_subjects() {
    run_fanout_test(3, 5).await;
}

#[tokio::test]
async fn test_fanout_simple_10_subjects() {
    run_fanout_test(10, 10).await;
}

#[tokio::test]
async fn test_fanout_subgraph_3_subjects() {
    run_fanout_subgraph_test(3, 30).await;
}

#[tokio::test]
async fn test_fanout_subgraph_10_subjects() {
    run_fanout_subgraph_test(10, 120).await;
}
