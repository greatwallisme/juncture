//! Standalone profiling runner for all benchmark scenarios.
//!
//! Run with: `cargo run -p juncture-benchmarks --bin profile`
//!
//! Measures wall-clock time, CPU time, and peak RSS for each scenario.
//! Accepts an optional scenario filter:
//! `cargo run -p juncture-benchmarks --bin profile -- sequential`

use std::io::{Write as IoWrite, stderr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use juncture_benchmarks::profiling::{
    ProfileResult, print_profiling_csv, print_profiling_report, save_json,
};
use juncture_checkpoint::MemorySaver;
use juncture_core::checkpoint::CheckpointSaver;
use juncture_core::command::{Command, SendTarget};
use juncture_core::edge::{PathMap, RouteResult, Router};
use juncture_core::node::NodeFnCommand;
use juncture_core::node::NodeFnUpdate;
use juncture_core::state::messages::{MessagesState, MessagesStateUpdate};
use juncture_core::stream::StreamMode;
use juncture_core::subgraph::SubgraphConfig;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

fn bench_config_with_thread(thread_id: &str) -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        thread_id: Some(thread_id.to_string()),
        ..RunnableConfig::new()
    }
}

fn status(msg: &str) {
    let mut lock = stderr().lock();
    writeln!(lock, "{msg}").unwrap_or(());
}

// ---------------------------------------------------------------------------
// Sequential scenario
// ---------------------------------------------------------------------------

fn create_sequential_graph(num_nodes: usize) -> StateGraph<MessagesState> {
    let mut graph = StateGraph::new();
    let names: Vec<String> = (0..num_nodes).map(|i| format!("node_{i}")).collect();
    for name in &names {
        graph
            .add_node_simple(
                name.as_str(),
                NodeFnUpdate(|_state: &MessagesState| async move {
                    Ok(MessagesStateUpdate { messages: None })
                }),
            )
            .expect("add_node_simple should succeed for unique names");
    }
    graph.set_entry_point(names[0].as_str());
    for i in 0..names.len() - 1 {
        graph.add_edge(names[i].as_str(), names[i + 1].as_str());
    }
    graph.set_finish_point(names[num_nodes - 1].as_str());
    graph
}

fn profile_sequential(results: &mut Vec<ProfileResult>) {
    let config = bench_config();
    for &num_nodes in &[10_usize, 100, 1000, 3000] {
        let graph = create_sequential_graph(num_nodes);
        let compiled = graph.compile().expect("compile should succeed");
        let input = MessagesState { messages: vec![] };
        let result = juncture_benchmarks::profiling::profile_execution(
            &format!("sequential_{num_nodes}"),
            num_nodes,
            5,
            || compiled.invoke(input.clone(), &config),
        );
        results.push(result);
    }
}

// ---------------------------------------------------------------------------
// Wide state scenario
// ---------------------------------------------------------------------------

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WideState {
    #[reducer(append)]
    messages: Vec<serde_json::Value>,
    #[reducer(append)]
    trigger_events: Vec<serde_json::Value>,
    #[reducer(last_write_wins)]
    primary_issue_medium: Option<String>,
    autoresponse: Option<std::collections::HashMap<String, serde_json::Value>>,
    issue: Option<std::collections::HashMap<String, serde_json::Value>>,
    relevant_rules: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    memory_docs: Option<Vec<std::collections::HashMap<String, serde_json::Value>>>,
    #[reducer(append)]
    categorizations: Vec<std::collections::HashMap<String, serde_json::Value>>,
    #[reducer(append)]
    responses: Vec<std::collections::HashMap<String, serde_json::Value>>,
    user_info: Option<std::collections::HashMap<String, serde_json::Value>>,
    crm_info: Option<std::collections::HashMap<String, serde_json::Value>>,
    email_thread_id: Option<String>,
    slack_participants: Option<std::collections::HashMap<String, serde_json::Value>>,
    bot_id: Option<String>,
    notified_assignees: Option<std::collections::HashMap<String, serde_json::Value>>,
}

fn create_loop_router(n: usize) -> impl Fn(&WideState) -> &str + Send + Sync + 'static {
    move |state: &WideState| -> &str {
        if state.messages.len() <= n {
            "one"
        } else {
            "__end__"
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "graph construction with 6 nodes and 15+ fields is inherently verbose"
)]
fn create_wide_state_graph(n: usize) -> StateGraph<WideState> {
    let mut graph = StateGraph::new();

    graph
        .add_node_simple(
            "one",
            NodeFnUpdate(|state: &WideState| {
                let _ = state.messages.last();
                async move {
                    Ok(WideStateUpdate {
                        trigger_events: Some(vec![serde_json::json!({"event": "triggered"})]),
                        primary_issue_medium: Some(Some("email".to_string())),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph
        .add_node_simple(
            "two",
            NodeFnUpdate(|state: &WideState| {
                let _ = state.trigger_events.last();
                async move {
                    let mut m = std::collections::HashMap::new();
                    m.insert("enabled".to_string(), serde_json::json!(true));
                    Ok(WideStateUpdate {
                        autoresponse: Some(Some(m)),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph
        .add_node_simple(
            "three",
            NodeFnUpdate(|state: &WideState| {
                let _ = &state.autoresponse;
                async move {
                    Ok(WideStateUpdate {
                        relevant_rules: Some(Some(vec![])),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph
        .add_node_simple(
            "four",
            NodeFnUpdate(|state: &WideState| {
                let _ = state.trigger_events.last();
                async move {
                    Ok(WideStateUpdate {
                        categorizations: Some(vec![]),
                        responses: Some(vec![]),
                        memory_docs: Some(Some(vec![])),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph
        .add_node_simple(
            "five",
            NodeFnUpdate(|state: &WideState| {
                let _ = state.categorizations.last();
                async move {
                    Ok(WideStateUpdate {
                        user_info: Some(Some(std::collections::HashMap::new())),
                        crm_info: Some(Some(std::collections::HashMap::new())),
                        email_thread_id: Some(Some("t".to_string())),
                        slack_participants: Some(Some(std::collections::HashMap::new())),
                        bot_id: Some(Some("b".to_string())),
                        notified_assignees: Some(Some(std::collections::HashMap::new())),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph
        .add_node_simple(
            "six",
            NodeFnUpdate(|state: &WideState| {
                let _ = state.responses.last();
                async move {
                    Ok(WideStateUpdate {
                        messages: Some(vec![serde_json::json!({"message": "completed"})]),
                        ..Default::default()
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph.set_entry_point("one");
    graph.add_edge("one", "two");
    graph.add_edge("two", "three");
    graph.add_edge("two", "four");
    graph.add_edge("three", "five");
    graph.add_edge("four", "five");
    graph.add_edge("five", "six");

    let router = create_loop_router(n);
    graph.add_conditional_edges(
        "six",
        Arc::new(router) as Arc<dyn Router<WideState>>,
        PathMap::from(&[("one", "one"), ("__end__", "__end__")]),
    );
    graph
}

#[allow(
    clippy::too_many_lines,
    reason = "graph construction is inherently verbose with 6 nodes"
)]
fn profile_wide_state(results: &mut Vec<ProfileResult>) {
    let config = bench_config();
    for &iterations in &[300_usize, 600, 1200] {
        let graph = create_wide_state_graph(iterations);
        let compiled = graph.compile().expect("compile should succeed");
        let input = WideState {
            messages: vec![],
            trigger_events: vec![],
            primary_issue_medium: Some("email".to_string()),
            autoresponse: None,
            issue: None,
            relevant_rules: None,
            memory_docs: None,
            categorizations: vec![],
            responses: vec![],
            user_info: None,
            crm_info: None,
            email_thread_id: None,
            slack_participants: None,
            bot_id: None,
            notified_assignees: None,
        };
        let result = juncture_benchmarks::profiling::profile_execution(
            &format!("wide_state_{iterations}"),
            iterations * 6,
            3,
            || compiled.invoke(input.clone(), &config),
        );
        results.push(result);
    }
}

// ---------------------------------------------------------------------------
// Checkpoint scenario
// ---------------------------------------------------------------------------

fn profile_checkpoint(results: &mut Vec<ProfileResult>) {
    let num_nodes = 100_usize;
    let graph = create_sequential_graph(num_nodes);
    let input = MessagesState { messages: vec![] };

    let compiled_no_cp = graph.compile().expect("compile should succeed");
    results.push(juncture_benchmarks::profiling::profile_execution(
        "checkpoint_off_100",
        num_nodes,
        5,
        || compiled_no_cp.invoke(input.clone(), &bench_config_with_thread("profile_no_cp")),
    ));

    let checkpointer: Arc<dyn CheckpointSaver> = Arc::new(MemorySaver::new());
    let compiled_with_cp = graph
        .compile_with_checkpointer(Some(checkpointer))
        .expect("compile should succeed with checkpointer");
    let counter = Arc::new(AtomicUsize::new(0));
    results.push(juncture_benchmarks::profiling::profile_execution(
        "checkpoint_on_100",
        num_nodes,
        5,
        || {
            let iter_id = counter.fetch_add(1, Ordering::Relaxed);
            compiled_with_cp.invoke(
                input.clone(),
                &bench_config_with_thread(&format!("profile_cp_{iter_id}")),
            )
        },
    ));
}

// ---------------------------------------------------------------------------
// Conditional routing scenario
// ---------------------------------------------------------------------------

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct RoutingState {
    value: u32,
    result: String,
}

struct ModuloRouter {
    num_branches: usize,
}

impl ModuloRouter {
    const fn new(num_branches: usize) -> Self {
        Self { num_branches }
    }
}

impl Router<RoutingState> for ModuloRouter {
    fn route(
        &self,
        state: &RoutingState,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<RouteResult, JunctureError>> + Send + '_>,
    > {
        let branch_index =
            state.value % u32::try_from(self.num_branches).expect("num_branches should fit in u32");
        let target = format!("branch_{branch_index}");
        Box::pin(async move { Ok(RouteResult::One(target)) })
    }
}

fn create_conditional_routing_graph(num_branches: usize) -> StateGraph<RoutingState> {
    let mut graph = StateGraph::new();
    graph
        .add_node_simple(
            "route",
            NodeFnUpdate(|state: &RoutingState| {
                let value = state.value;
                async move {
                    Ok(RoutingStateUpdate {
                        value: Some(value),
                        result: None,
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    for i in 0..num_branches {
        let branch_name = format!("branch_{i}");
        let idx = i;
        graph
            .add_node_simple(
                branch_name.as_str(),
                NodeFnUpdate(move |_state: &RoutingState| {
                    Box::pin(async move {
                        Ok(RoutingStateUpdate {
                            value: None,
                            result: Some(format!("branch_{idx}_visited")),
                        })
                    })
                }),
            )
            .expect("add_node_simple should succeed");
    }

    graph
        .add_node_simple(
            "collect",
            NodeFnUpdate(|state: &RoutingState| {
                let result = state.result.clone();
                async move {
                    Ok(RoutingStateUpdate {
                        value: None,
                        result: Some(format!("{result}_complete")),
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph.set_entry_point("route");
    let mut path_map = PathMap::new();
    for i in 0..num_branches {
        path_map.insert(format!("branch_{i}"), format!("branch_{i}"));
    }
    graph.add_conditional_edges("route", Arc::new(ModuloRouter::new(num_branches)), path_map);
    for i in 0..num_branches {
        graph.add_edge(format!("branch_{i}"), "collect");
    }
    graph.set_finish_point("collect");
    graph
}

fn profile_conditional_routing(results: &mut Vec<ProfileResult>) {
    let config = bench_config();
    for &num_branches in &[3_usize, 10, 50] {
        let graph = create_conditional_routing_graph(num_branches);
        let compiled = graph.compile().expect("compile should succeed");
        let input = RoutingState {
            value: 42,
            result: String::new(),
        };
        let result = juncture_benchmarks::profiling::profile_execution(
            &format!("conditional_routing_{num_branches}"),
            3,
            10,
            || compiled.invoke(input.clone(), &config),
        );
        results.push(result);
    }
}

// ---------------------------------------------------------------------------
// Streaming scenario
// ---------------------------------------------------------------------------

fn create_streaming_graph(num_nodes: usize) -> StateGraph<MessagesState> {
    let mut graph = StateGraph::new();
    let names: Vec<String> = (0..num_nodes).map(|i| format!("node_{i}")).collect();
    for name in &names {
        graph
            .add_node_simple(
                name.as_str(),
                NodeFnUpdate(|_state: &MessagesState| async move {
                    Ok(MessagesStateUpdate {
                        messages: Some(vec![]),
                    })
                }),
            )
            .expect("add_node_simple should succeed for unique names");
    }
    graph.set_entry_point(names[0].as_str());
    for i in 0..names.len() - 1 {
        graph.add_edge(names[i].as_str(), names[i + 1].as_str());
    }
    graph.set_finish_point(names[num_nodes - 1].as_str());
    graph
}

fn profile_streaming(results: &mut Vec<ProfileResult>) {
    let config = bench_config();
    // Create runtime once outside the iteration loop to avoid measuring
    // tokio::spawn runtime creation overhead (~1.5ms) in each iteration.
    let rt = tokio::runtime::Runtime::new().expect("runtime creation should succeed");
    for &num_nodes in &[100_usize, 1000, 10000] {
        let graph = create_streaming_graph(num_nodes);
        let compiled = graph.compile().expect("compile should succeed");
        let input = MessagesState { messages: vec![] };
        let result = juncture_benchmarks::profiling::profile_execution(
            &format!("streaming_{num_nodes}"),
            num_nodes,
            3,
            || {
                rt.block_on(async {
                    let handle = compiled
                        .stream(input.clone(), &config, StreamMode::Values)
                        .await
                        .expect("stream should succeed");
                    let mut count = 0_usize;
                    let mut stream = handle.stream;
                    while let Some(item) = stream.next().await {
                        item.expect("stream event should not error");
                        count += 1;
                    }
                    count
                })
            },
        );
        results.push(result);
    }
}

// ---------------------------------------------------------------------------
// Fanout scenario
// ---------------------------------------------------------------------------

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct OverallState {
    subjects: Vec<String>,
    #[reducer(append)]
    jokes: Vec<String>,
}

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct JokeState {
    subject: String,
    #[reducer(append)]
    jokes: Vec<String>,
}

#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct JokeInput {
    subject: String,
}

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

fn create_joke_subgraph() -> StateGraph<JokeState> {
    let mut graph = StateGraph::new();
    graph
        .add_node_simple(
            "edit",
            NodeFnUpdate(|state: &JokeState| {
                let subject = state.subject.clone();
                async move {
                    Ok(JokeStateUpdate {
                        subject: Some(format!("{subject} - hohoho")),
                        jokes: None,
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");
    graph
        .add_node_simple(
            "generate",
            NodeFnUpdate(|state: &JokeState| {
                let subject = state.subject.clone();
                async move {
                    Ok(JokeStateUpdate {
                        subject: None,
                        jokes: Some(vec![format!("Joke about {subject}")]),
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");
    graph
        .add_node_simple(
            "bump",
            NodeFnUpdate(|state: &JokeState| {
                let first_joke = state.jokes.first().cloned().unwrap_or_default();
                async move {
                    Ok(JokeStateUpdate {
                        subject: None,
                        jokes: Some(vec![format!("{first_joke} a")]),
                    })
                }
            }),
        )
        .expect("add_node_simple should succeed");

    graph.set_entry_point("edit");
    graph.add_edge("edit", "generate");
    graph.add_edge("generate", "bump");

    let path_map = PathMap::from(&[("bump", "bump"), ("__end__", "__end__")]);
    graph.add_conditional_edges(
        "bump",
        Arc::new(bump_loop_router) as Arc<dyn Router<JokeState>>,
        path_map,
    );
    graph.set_finish_point("generate");
    graph
}

#[allow(
    clippy::too_many_lines,
    reason = "graph construction with subgraph is inherently verbose"
)]
fn create_fanout_graph() -> StateGraph<OverallState> {
    let subgraph = create_joke_subgraph();
    let compiled_subgraph = Arc::new(subgraph.compile().expect("subgraph compile should succeed"));

    let mut graph = StateGraph::new();
    graph
        .add_subgraph_with_config(
            "generate_joke",
            compiled_subgraph,
            |parent: &OverallState| JokeState {
                subject: parent.subjects.first().cloned().unwrap_or_default(),
                jokes: vec![],
            },
            |sub_output: &JokeState| OverallStateUpdate {
                subjects: None,
                jokes: Some(sub_output.jokes.clone()),
            },
            SubgraphConfig::default(),
        )
        .expect("add_subgraph_with_config should succeed");

    graph
        .add_node_simple(
            "continue_to_jokes",
            NodeFnCommand(|state: &OverallState| {
                let subjects = state.subjects.clone();
                Box::pin(async move {
                    let sends: Vec<SendTarget> = subjects
                        .iter()
                        .map(|s| {
                            juncture_core::send::Send::<JokeInput> {
                                node: "generate_joke".to_string(),
                                state: JokeInput { subject: s.clone() },
                            }
                            .into()
                        })
                        .collect();
                    Ok(Command::send(sends))
                })
            }),
        )
        .expect("add_node_simple should succeed");

    graph.set_entry_point("continue_to_jokes");
    graph.add_edge("continue_to_jokes", "generate_joke");
    graph.add_edge("generate_joke", "__end__");
    graph
}

fn profile_fanout(results: &mut Vec<ProfileResult>) {
    let config = bench_config();
    for &num_subjects in &[10_usize, 100] {
        let graph = create_fanout_graph();
        let compiled = graph.compile().expect("compile should succeed");
        let input = OverallState {
            subjects: (0..num_subjects).map(|i| format!("subject_{i}")).collect(),
            jokes: vec![],
        };
        let result = juncture_benchmarks::profiling::profile_execution(
            &format!("fanout_{num_subjects}"),
            num_subjects * 12,
            3,
            || compiled.invoke(input.clone(), &config),
        );
        results.push(result);
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

type ScenarioRunner = Box<dyn Fn(&mut Vec<ProfileResult>)>;

fn main() {
    let filter = std::env::args().nth(1);
    let mut results = Vec::new();

    let scenarios: Vec<(&str, ScenarioRunner)> = vec![
        ("sequential", Box::new(profile_sequential)),
        ("wide_state", Box::new(profile_wide_state)),
        ("checkpoint", Box::new(profile_checkpoint)),
        ("conditional_routing", Box::new(profile_conditional_routing)),
        ("streaming", Box::new(profile_streaming)),
        ("fanout", Box::new(profile_fanout)),
    ];

    for (name, runner) in scenarios {
        if let Some(ref f) = filter {
            if !name.contains(f.as_str()) {
                continue;
            }
        }
        status(&format!("Profiling: {name}..."));
        runner(&mut results);
    }

    print_profiling_report(&results);

    status("\nCSV output:");
    print_profiling_csv(&results);

    // Save JSON results to benchmarks/ directory for the comparison script.
    // Supports JUNCTURE_BENCH_OUTPUT env var for custom output path.
    let json_path = std::env::var("JUNCTURE_BENCH_OUTPUT").map_or_else(
        |_| {
            let cwd = std::env::current_dir().unwrap_or_default();
            if cwd.file_name().is_some_and(|n| n == "benchmarks") {
                cwd.join("results_rust.json")
            } else {
                cwd.join("benchmarks").join("results_rust.json")
            }
        },
        std::path::PathBuf::from,
    );
    match save_json(&json_path, &results) {
        Ok(()) => status(&format!("Results saved to {}", json_path.display())),
        Err(e) => status(&format!("Failed to save results: {e}")),
    }
}

// Rust guideline compliant 2026-05-24
