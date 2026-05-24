//! Conditional routing benchmark: route based on state value to N branches.
//!
//! Measures the overhead of conditional routing by creating graphs with
//! different numbers of branches (3, 10, 50) and measuring how long it takes
//! to route through them.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use juncture_core::edge::{PathMap, RouteResult, Router};
use juncture_core::node::NodeFnUpdate;
use juncture_core::{JunctureError, RunnableConfig, StateGraph};
use juncture_derive::State;
use std::pin::Pin;
use std::sync::Arc;

/// State for conditional routing benchmark
#[derive(State, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RoutingState {
    /// Value used for routing decision
    value: u32,
    /// Result string set by the branch node
    result: String,
}

/// Router that routes based on value modulo number of branches
struct ModuloRouter {
    num_branches: usize,
}

impl ModuloRouter {
    #[must_use]
    const fn new(num_branches: usize) -> Self {
        Self { num_branches }
    }
}

impl Router<RoutingState> for ModuloRouter {
    fn route(
        &self,
        state: &RoutingState,
    ) -> Pin<Box<dyn futures::Future<Output = Result<RouteResult, JunctureError>> + Send + '_>>
    {
        let branch_index =
            state.value % u32::try_from(self.num_branches).expect("num_branches should fit in u32");
        let target = format!("branch_{branch_index}");
        Box::pin(async move { Ok(RouteResult::One(target)) })
    }
}

/// Branch node that sets result to indicate which branch was visited
fn create_branch_node(
    branch_index: usize,
) -> impl Fn(
    RoutingState,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<RoutingStateUpdate, JunctureError>> + Send>,
> + Send
+ Sync
+ 'static {
    move |_state: RoutingState| {
        Box::pin(async move {
            Ok(RoutingStateUpdate {
                value: None,
                result: Some(format!("branch_{branch_index}_visited")),
            })
        })
    }
}

/// Collect node - receives state from any branch and marks completion
async fn collect_node(state: RoutingState) -> Result<RoutingStateUpdate, JunctureError> {
    Ok(RoutingStateUpdate {
        value: None,
        result: Some(format!("{}_complete", state.result)),
    })
}

/// Build a conditional routing graph with the specified number of branches
///
/// Graph structure:
/// START -> route -> `branch_N` -> collect -> END
///
/// Where route uses a `ModuloRouter` to select which `branch_N` to execute
/// based on state.value % `num_branches`.
fn create_conditional_routing_graph(num_branches: usize) -> StateGraph<RoutingState> {
    let mut graph = StateGraph::new();

    // Add route node that sets the initial value
    graph
        .add_node_simple(
            "route",
            NodeFnUpdate(|state: RoutingState| async move {
                Ok(RoutingStateUpdate {
                    value: Some(state.value),
                    result: None,
                })
            }),
        )
        .expect("add_node_simple should succeed");

    // Add branch nodes
    for i in 0..num_branches {
        let branch_name = format!("branch_{i}");
        let node_fn = create_branch_node(i);
        graph
            .add_node_simple(branch_name.as_str(), NodeFnUpdate(node_fn))
            .expect("add_node_simple should succeed");
    }

    // Add collect node
    graph
        .add_node_simple("collect", NodeFnUpdate(collect_node))
        .expect("add_node_simple should succeed");

    // Set entry point
    graph.set_entry_point("route");

    // Build path map for conditional routing
    let mut path_map = PathMap::new();
    for i in 0..num_branches {
        path_map.insert(format!("branch_{i}"), format!("branch_{i}"));
    }

    // Add conditional edges from route node
    graph.add_conditional_edges("route", Arc::new(ModuloRouter::new(num_branches)), path_map);

    // All branches converge to collect
    for i in 0..num_branches {
        graph.add_edge(format!("branch_{i}"), "collect");
    }

    // Set finish point
    graph.set_finish_point("collect");

    graph
}

/// `RunnableConfig` with reasonable defaults
fn bench_config() -> RunnableConfig {
    RunnableConfig {
        recursion_limit: 20_000_000_000,
        ..RunnableConfig::new()
    }
}

fn benchmark_conditional_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("conditional_routing");

    let config = bench_config();

    for &num_branches in &[3_usize, 10, 50] {
        let graph = create_conditional_routing_graph(num_branches);
        let compiled = graph.compile().expect("compile should succeed");
        let input = RoutingState {
            value: 42,
            result: String::new(),
        };

        group.bench_with_input(
            BenchmarkId::new("invoke", num_branches),
            &num_branches,
            |b, _| {
                b.iter(|| {
                    let _ = compiled.invoke(input.clone(), &config);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, benchmark_conditional_routing);
criterion_main!(benches);

// Rust guideline compliant 2026-05-24
